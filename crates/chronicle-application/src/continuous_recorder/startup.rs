use super::{
    CaptureSource, ContinuationState, ContinuousRecorderError, ContinuousRecorderService, Digest,
    EPOCH_CONTINUATION_IN_FILE, ETL_PIPELINE_VERSION, EpochContinuationCheckpoint, EpochId,
    EpochMetadata, FilesystemRecordingStore, INCREMENTAL_ETL_CHECKPOINT_FILE,
    IncrementalEtlCheckpoint, IncrementalProcessor, IncrementalWorker, IncrementalWorkerPolicy,
    LagSummary, MetadataCode, NormalizedRecorderConfig, Path, ProtocolRegistry, QuotaStatus,
    RecorderCounters, RecorderHealth, RecorderMetadataV1, RecorderOrchestrator, RecorderReadiness,
    RecorderStartup, RecorderStartupError, RecordingArtifactKind, RecordingMetadata,
    RecordingStatus, RecordingStore, SessionId, Sha256, digest_hex, fs,
    recover_pending_continuation, registry, unix_seconds, validate_checkpoint_before_capture,
    write_recorder_metadata, write_recording_metadata,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    pub fn start(
        config: &NormalizedRecorderConfig,
        wal_directory: impl AsRef<Path>,
        recorder: RecorderOrchestrator<S>,
        recover_foundation: impl FnOnce(
            &[crate::QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(
            &[crate::QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, ContinuousRecorderError> {
        let mut recorder = recorder;
        let incremental_worker = IncrementalWorker::new(IncrementalWorkerPolicy {
            max_lag_records: config.etl.max_lag_records,
            retry_attempts: config.etl.retry_attempts,
            backoff_millis: 10,
        })
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let incremental_registry =
            registry().map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let expected_recording_id = recorder.ingest().recording_id();
        let expected_epoch_ordinal = recorder.epoch_ordinal();
        let checkpoint_path = wal_directory.as_ref().to_path_buf();
        let startup = RecorderStartup::start_post_foundation(
            config,
            move |quotas| {
                recover_foundation(quotas)?;
                validate_checkpoint_before_capture(
                    config,
                    &checkpoint_path,
                    expected_recording_id,
                    expected_epoch_ordinal,
                )
                .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))
            },
            reserve_quota,
            establish_appendable,
            || {
                recorder
                    .start()
                    .map_err(|_| RecorderStartupError::CaptureAttach)
            },
            resume_etl,
        )?;
        Self::start_after_startup(
            config,
            wal_directory,
            recorder,
            startup,
            incremental_worker,
            incremental_registry,
        )
    }

    pub fn start_with_prepared_startup(
        config: &NormalizedRecorderConfig,
        wal_directory: impl AsRef<Path>,
        authoritative_epoch_ordinal: u64,
        mut recorder: RecorderOrchestrator<S>,
        startup: RecorderStartup,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, ContinuousRecorderError> {
        let recording_id = recorder.ingest().recording_id();
        let epoch_ordinal = recorder.epoch_ordinal();
        let startup = startup.finish(
            establish_appendable,
            || {
                if epoch_ordinal != authoritative_epoch_ordinal {
                    return Err(RecorderStartupError::RecoveryDetail(
                        "recorder epoch does not match authoritative catalog".into(),
                    ));
                }
                validate_checkpoint_before_capture(
                    config,
                    wal_directory.as_ref(),
                    recording_id,
                    authoritative_epoch_ordinal,
                )
                .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
                recorder
                    .start()
                    .map_err(|_| RecorderStartupError::CaptureAttach)
            },
            resume_etl,
        )?;
        let incremental_worker = IncrementalWorker::new(IncrementalWorkerPolicy {
            max_lag_records: config.etl.max_lag_records,
            retry_attempts: config.etl.retry_attempts,
            backoff_millis: 10,
        })
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let incremental_registry =
            registry().map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        Self::start_after_startup(
            config,
            wal_directory,
            recorder,
            startup,
            incremental_worker,
            incremental_registry,
        )
    }

    #[allow(clippy::too_many_lines)]
    fn start_after_startup(
        config: &NormalizedRecorderConfig,
        wal_directory: impl AsRef<Path>,
        recorder: RecorderOrchestrator<S>,
        startup: RecorderStartup,
        mut incremental_worker: IncrementalWorker,
        incremental_registry: ProtocolRegistry,
    ) -> Result<Self, ContinuousRecorderError> {
        incremental_worker.start();
        let starting_epoch_ordinal = recorder.epoch_ordinal();
        let wal_directory = wal_directory.as_ref().to_path_buf();
        let recording_id = recorder.ingest().recording_id();
        let continuation_in = match fs::metadata(wal_directory.join(EPOCH_CONTINUATION_IN_FILE)) {
            Ok(_) => {
                let checkpoint = EpochContinuationCheckpoint::load(
                    wal_directory.join(EPOCH_CONTINUATION_IN_FILE),
                )
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
                if checkpoint.successor_epoch_id != EpochId::from_uuid(recording_id.0)
                    || checkpoint.pipeline_version != ETL_PIPELINE_VERSION.to_string()
                {
                    return Err(ContinuousRecorderError::Incremental(
                        "continuation-in lineage mismatch".into(),
                    ));
                }
                Some(checkpoint)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(ContinuousRecorderError::Incremental(error.to_string())),
        };
        let pending_continuation = recover_pending_continuation(
            &wal_directory,
            EpochId::from_uuid(recording_id.0),
            continuation_in.as_ref(),
        )?;
        // Without a continuation handoff the parent is the persisted recorder
        // parent (the epoch recording id is not the parent; the epoch catalog
        // binds the real parent). The recorder metadata is written by startup
        // foundation before this point.
        let parent_id = continuation_in.as_ref().map_or_else(
            || {
                crate::load_recorder_metadata(&config.state_root)
                    .ok()
                    .and_then(|metadata| metadata.parent_id)
                    .unwrap_or(recording_id)
            },
            |checkpoint| checkpoint.parent_id,
        );
        let last_wal_bytes = crate::recording_physical_wal_bytes(&wal_directory)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let checkpoint_path = wal_directory.join(INCREMENTAL_ETL_CHECKPOINT_FILE);
        let incremental_checkpoint = match fs::metadata(&checkpoint_path) {
            Ok(_) => {
                let checkpoint = IncrementalEtlCheckpoint::load(&checkpoint_path)
                    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
                if checkpoint.parent_id != parent_id
                    || checkpoint.epoch_id != EpochId::from_uuid(recording_id.0)
                    || checkpoint.config_digest
                        != config.stable_digest().map_err(|error| {
                            ContinuousRecorderError::Incremental(error.to_string())
                        })?
                    || checkpoint.pipeline_version != ETL_PIPELINE_VERSION.to_string()
                {
                    return Err(ContinuousRecorderError::Incremental(
                        "incremental checkpoint lineage mismatch".into(),
                    ));
                }
                if checkpoint.continuation.state == ContinuationState::Ready {
                    let handoff = continuation_in.as_ref().ok_or_else(|| {
                        ContinuousRecorderError::Incremental(
                            "ready continuation has no successor handoff".into(),
                        )
                    })?;
                    let digest: [u8; 32] = Sha256::digest(handoff.encode().map_err(|error| {
                        ContinuousRecorderError::Incremental(error.to_string())
                    })?)
                    .into();
                    if checkpoint.continuation.output_digest.as_deref()
                        != Some(digest_hex(&digest).as_str())
                    {
                        return Err(ContinuousRecorderError::Incremental(
                            "ready continuation digest mismatch".into(),
                        ));
                    }
                }
                let store = FilesystemRecordingStore::new(config.store_root.clone());
                for output in &checkpoint.outputs {
                    let head = store
                        .head(&output.key)
                        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
                    if head.key != output.key
                        || head.kind != RecordingArtifactKind::CanonicalDeltaBatch
                        || head.checksum != output.digest
                        || head.bytes != output.bytes
                    {
                        return Err(ContinuousRecorderError::Incremental(
                            "checkpoint output artifact verification failed".into(),
                        ));
                    }
                }
                Some(checkpoint)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ContinuousRecorderError::Incremental(error.to_string()));
            }
        };
        let metadata = RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id,
            selector: Some(crate::RecordingSelectorIdentity {
                canonical_cgroup_path: config.scope.cgroup_path.display().to_string(),
                cgroup_id: config.scope.cgroup_id,
            }),
            status: RecordingStatus::Recording,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: crate::RecordingCounters::default(),
            terminal_wal_loss: None,
            capture: None,
        };
        write_recording_metadata(&wal_directory, &metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let active_metadata = RecorderMetadataV1 {
            version: crate::RECORDER_METADATA_SCHEMA_VERSION,
            recorder_id: recording_id.0,
            parent_id: Some(parent_id),
            current_epoch_id: Some(EpochId::from_uuid(recording_id.0)),
            finalized_epoch_count: 0,
            continuation_in: continuation_in
                .as_ref()
                .map(|_| EPOCH_CONTINUATION_IN_FILE.to_owned()),
            continuation_out: None,
            retention_state: None,
            last_rollover: None,
            attempt_id: uuid::Uuid::new_v4(),
            config_digest: config
                .stable_digest()
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?,
            scope: config.scope.clone(),
            boot_clock_identity: "continuous-recorder".into(),
            lifecycle: crate::RecorderLifecycleState::Running,
            capture_readiness: RecorderReadiness::Ready,
            processing_readiness: RecorderReadiness::Unknown,
            health: RecorderHealth::Degraded,
            current_epoch: Some(EpochMetadata {
                ordinal: starting_epoch_ordinal,
                recording_id: recording_id.0,
            }),
            previous_epoch: pending_continuation.as_ref().map(|pending| EpochMetadata {
                ordinal: pending.old_epoch_ordinal,
                recording_id: pending.old_epoch_id.as_uuid(),
            }),
            active_segment: None,
            commit: None,
            incremental_checkpoint: incremental_checkpoint.as_ref().map(|checkpoint| {
                crate::IncrementalCheckpointSummaryV1 {
                    marker_sequence: checkpoint.marker.sequence,
                    checkpoint_digest: checkpoint.checksum.clone(),
                    output_count: checkpoint.outputs.len() as u64,
                }
            }),
            lag: LagSummary {
                records: 0,
                bytes: 0,
                age_seconds: 0,
            },
            quota: startup
                .quota_authorities()
                .iter()
                .map(|authority| QuotaStatus {
                    domain: authority.quota().domain.clone(),
                    quota_bytes: authority.quota().quota_bytes,
                    free_bytes: authority.available_bytes(),
                    reserved_bytes: authority.reserved_bytes().unwrap_or(0),
                })
                .collect(),
            counters: RecorderCounters::default(),
            recovery: Some(crate::RecoverySummary {
                code: MetadataCode::CleanStart,
                repaired_tail: false,
            }),
            shutdown: None,
            failure: None,
            updated_at_unix_seconds: unix_seconds(),
            checksum: String::new(),
        };
        write_recorder_metadata(&config.state_root, &active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let incremental_session_id = SessionId(recording_id.0);
        let mut incremental_processor =
            IncrementalProcessor::new(chronicle_session::SessionLimits::default());
        if let Some(checkpoint) = &incremental_checkpoint {
            incremental_processor
                .restore_decoder_state(
                    &checkpoint.decoder,
                    incremental_session_id,
                    recording_id.0,
                    checkpoint.epoch_ordinal,
                    checkpoint.marker.sequence,
                    &checkpoint.marker.marker_digest,
                )
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
            incremental_processor.restore_segment_lineage(checkpoint.segment_lineage.clone());
            incremental_processor
                .restore_publication_keys(checkpoint.published_operation_keys.clone())
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        } else if let Some(checkpoint) = &continuation_in {
            incremental_processor
                .restore_continuation(
                    checkpoint,
                    parent_id,
                    checkpoint.predecessor_epoch_id,
                    EpochId::from_uuid(recording_id.0),
                    &ETL_PIPELINE_VERSION.to_string(),
                )
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        }
        Ok(Self {
            startup,
            recorder,
            wal_directory,
            metadata,
            active_metadata,
            state_root: config.state_root.clone(),
            store_root: config.store_root.clone(),
            incremental_worker,
            incremental_processor,
            incremental_registry,
            incremental_session_id,
            incremental_checkpoint,
            continuation_in,
            continuation_out: None,
            pending_continuation,
            etl_batch_records: config.etl.batch_records,
            next_incremental_millis: 0,
            checkpoint_fault: None,
            last_wal_bytes,
            epoch_max_bytes: config.epoch.max_bytes,
        })
    }
}
