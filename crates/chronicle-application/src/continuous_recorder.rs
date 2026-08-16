//! Shared continuous-recorder runtime around startup, capture, rollover, and drain.
#![allow(clippy::result_large_err)]

/// Parent-aware incremental ETL checkpoint written by the publication
/// transaction and validated at startup.
const INCREMENTAL_ETL_CHECKPOINT_FILE: &str = "incremental-etl-checkpoint-v2.json";

use crate::{
    EpochMetadata, IncrementalWorker, IncrementalWorkerError, IncrementalWorkerPolicy, LagSummary,
    MetadataCode, NormalizedRecorderConfig, QuotaStatus, RecorderCounters, RecorderEpochBoundary,
    RecorderHealth, RecorderLifecycleError, RecorderMetadataV1, RecorderOrchestrationError,
    RecorderOrchestrator, RecorderPoll, RecorderReadiness, RecorderStartup, RecorderStartupError,
    RecordingCaptureMetadata, RecordingMetadata, RecordingStatus, ReservationKind,
    RolloverTransition, RolloverTransitionPhase, load_transition, write_recorder_metadata,
    write_recording_metadata, write_transition_atomic,
};
use chronicle_capture::{CaptureError, CaptureSource};
use chronicle_common::{EpochId, RecordingId, SessionId};
use chronicle_etl::{
    CanonicalDeltaBatchV1, CheckpointFault, CheckpointLifecycle, CheckpointOwner,
    CommittedWalSnapshot, ContinuationDependency, ContinuationLimits, ContinuationState,
    DeltaBatchReference, EPOCH_CONTINUATION_IN_FILE, EPOCH_CONTINUATION_OUT_FILE,
    EPOCH_CONTINUATION_STATE_FILE, ETL_PIPELINE_VERSION, EpochContinuationCheckpoint,
    INCREMENTAL_CHECKPOINT_SCHEMA_VERSION, IncrementalEtlCheckpoint, IncrementalProcessor,
    IncrementalResult, MarkerLineage, SegmentLineage, SourceStatus, publish_delta_then_checkpoint,
    publish_delta_then_checkpoint_with_fault, recover_pending_checkpoint,
    stamp_epoch_operation_provenance,
};
use chronicle_protocol::ProtocolRegistry;
use chronicle_protocol_builtins::registry;
use chronicle_storage::{
    FilesystemRecordingStore, FilesystemSessionStore, RecordingArtifact, RecordingArtifactKind,
    RecordingStore,
};
use chronicle_wal::{GroupCommitWalWriter, WalError};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Debug)]
struct PendingContinuation {
    old_wal_directory: std::path::PathBuf,
    old_epoch_id: EpochId,
    successor_epoch_id: EpochId,
    old_epoch_ordinal: u64,
}

fn recover_pending_continuation(
    wal_directory: &Path,
    current_epoch_id: EpochId,
    continuation_in: Option<&EpochContinuationCheckpoint>,
) -> Result<Option<PendingContinuation>, ContinuousRecorderError> {
    let state_path = wal_directory.join(EPOCH_CONTINUATION_STATE_FILE);
    match fs::metadata(&state_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ContinuousRecorderError::Incremental(error.to_string()));
        }
    }
    let dependency = ContinuationDependency::load(&state_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if dependency.successor_epoch_id != current_epoch_id {
        return Err(ContinuousRecorderError::Incremental(
            "continuation dependency successor mismatch".into(),
        ));
    }
    let Some(predecessor_epoch_id) = dependency.predecessor_epoch_id else {
        return Err(ContinuousRecorderError::Incremental(
            "pending continuation has no predecessor".into(),
        ));
    };
    if let Some(checkpoint) = continuation_in {
        if checkpoint.predecessor_epoch_id != predecessor_epoch_id
            || checkpoint.successor_epoch_id != current_epoch_id
        {
            return Err(ContinuousRecorderError::Incremental(
                "continuation dependency/input lineage mismatch".into(),
            ));
        }
        if dependency.state == ContinuationState::Pending {
            let parent_id = dependency.parent_id.unwrap_or(checkpoint.parent_id);
            ContinuationDependency::ready_from_checkpoint(
                parent_id,
                predecessor_epoch_id,
                checkpoint,
                &ETL_PIPELINE_VERSION.to_string(),
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
            .write_atomic(&state_path)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        }
        return Ok(None);
    }
    if dependency.state == ContinuationState::Ready {
        return Err(ContinuousRecorderError::Incremental(
            "ready continuation has no successor handoff".into(),
        ));
    }
    if dependency.state != ContinuationState::Pending {
        return Ok(None);
    }
    let epochs_directory = wal_directory
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "epochs"))
        .ok_or_else(|| {
            ContinuousRecorderError::Incremental(
                "pending continuation active path is outside epoch catalog".into(),
            )
        })?;
    let root = epochs_directory.parent().ok_or_else(|| {
        ContinuousRecorderError::Incremental("pending continuation catalog root missing".into())
    })?;
    let catalog = crate::EpochCatalog::load(root)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let predecessor = catalog.epoch(predecessor_epoch_id).ok_or_else(|| {
        ContinuousRecorderError::Incremental("pending continuation predecessor missing".into())
    })?;
    Ok(Some(PendingContinuation {
        old_wal_directory: root.join(&predecessor.path),
        old_epoch_id: predecessor_epoch_id,
        successor_epoch_id: current_epoch_id,
        old_epoch_ordinal: predecessor.ordinal,
    }))
}

#[derive(Debug, Error)]
pub enum ContinuousRecorderError {
    #[error(transparent)]
    Startup(#[from] RecorderStartupError),
    #[error(transparent)]
    Orchestration(#[from] RecorderOrchestrationError),
    #[error(transparent)]
    Lifecycle(#[from] RecorderLifecycleError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("recorder metadata persistence failed: {0}")]
    Metadata(String),
    #[error("incremental ETL failed: {0}")]
    Incremental(String),
    #[error(transparent)]
    Quota(#[from] crate::QuotaError),
}

/// Foreground runtime. Owns startup lease and capture source for entire epoch sequence.
pub struct ContinuousRecorderService<S> {
    startup: RecorderStartup,
    recorder: RecorderOrchestrator<S>,
    wal_directory: std::path::PathBuf,
    metadata: RecordingMetadata,
    active_metadata: RecorderMetadataV1,
    state_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    incremental_worker: IncrementalWorker,
    incremental_processor: IncrementalProcessor,
    incremental_registry: ProtocolRegistry,
    incremental_session_id: SessionId,
    incremental_checkpoint: Option<IncrementalEtlCheckpoint>,
    /// Verified predecessor handoff for current epoch ETL. Continuation state
    /// is separate and immutable.
    continuation_in: Option<EpochContinuationCheckpoint>,
    continuation_out: Option<EpochContinuationCheckpoint>,
    pending_continuation: Option<PendingContinuation>,
    next_incremental_millis: u64,
    checkpoint_fault: Option<CheckpointFault>,
    last_wal_bytes: u64,
    epoch_max_bytes: u64,
}

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
            next_incremental_millis: 0,
            checkpoint_fault: None,
            last_wal_bytes,
            epoch_max_bytes: config.epoch.max_bytes,
        })
    }

    /// Finalizes the in-memory incremental reconstruction from verified
    /// checkpoint lineage before standalone ETL verifies the same session.
    pub fn finalize_incremental_session(&mut self) -> Result<(), ContinuousRecorderError> {
        if self.pending_continuation.is_some() {
            // Predecessor continuation is still pending. Its durable state
            // resumes on the next startup, so deferral during shutdown is
            // not a terminal failure.
            return Ok(());
        }
        let recording_id = self.recorder.ingest().recording_id();
        let session_id = self.incremental_session_id;
        let epoch_ordinal = self.recorder.epoch_ordinal();
        self.finalize_incremental_recording(recording_id, session_id, epoch_ordinal)
    }

    fn finalize_incremental_recording(
        &mut self,
        recording_id: RecordingId,
        session_id: SessionId,
        epoch_ordinal: u64,
    ) -> Result<(), ContinuousRecorderError> {
        if self.pending_continuation.is_some() {
            return Err(ContinuousRecorderError::Incremental(
                "successor ETL cannot finalize while predecessor continuation is unresolved".into(),
            ));
        }
        match chronicle_wal::read_committed_snapshot_with_records(
            &self.wal_directory,
            recording_id,
            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
        ) {
            Ok(snapshot) => {
                self.incremental_processor
                    .process_snapshot(
                        CommittedWalSnapshot {
                            marker_sequence: snapshot.snapshot.marker_sequence,
                            marker_digest: snapshot.snapshot.marker_digest,
                            envelopes: &snapshot.envelopes,
                            segment_lineage: &snapshot.snapshot.segment_lineage,
                        },
                        &self.incremental_registry,
                        session_id,
                    )
                    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
            }
            Err(chronicle_wal::WalError::NoPublishedSegments) => return Ok(()),
            Err(error) => return Err(ContinuousRecorderError::Wal(error)),
        }
        let mut output = self
            .incremental_processor
            .finalize(&self.incremental_registry, session_id)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let parent_id = self
            .active_metadata
            .parent_id
            .unwrap_or(self.metadata.recording_id);
        output.session.source_provenance.recording_id = Some(parent_id);
        output.session.source_provenance.epoch_id = Some(EpochId::from_uuid(recording_id.0));
        output.session.source_provenance.epoch_ordinal = Some(epoch_ordinal);
        output.session.source_provenance.continuation_in = self
            .continuation_in
            .as_ref()
            .map(|_| EPOCH_CONTINUATION_IN_FILE.to_owned());
        output.session.source_provenance.continuation_out = self
            .continuation_out
            .as_ref()
            .map(|_| EPOCH_CONTINUATION_OUT_FILE.to_owned());
        stamp_epoch_operation_provenance(
            &mut output.session,
            parent_id,
            EpochId::from_uuid(recording_id.0),
            Some(epoch_ordinal),
            self.incremental_processor
                .last_marker_sequence()
                .map(|sequence| (0, sequence)),
        );
        let checkpoint = self
            .incremental_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checksum.clone());
        let session_bytes = serde_json::to_vec(&output.session)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
            .len() as u64;
        let authorities = self
            .startup
            .quota_authorities()
            .iter()
            .filter(|authority| {
                self.store_root
                    .starts_with(Path::new(&authority.quota().domain))
            })
            .collect::<Vec<_>>();
        if authorities.len() != 1 {
            return Err(crate::QuotaError::InvalidQuota.into());
        }
        let quota = authorities[0];
        let manifest_reservation = session_bytes.saturating_mul(2);
        quota.reserve(ReservationKind::FinalSession, manifest_reservation)?;
        if let Err(error) = quota.reserve(ReservationKind::Manifest, manifest_reservation) {
            let _ = quota.release(ReservationKind::FinalSession, manifest_reservation);
            return Err(error.into());
        }
        let publication = chronicle_etl::finalize_incremental_session(
            &FilesystemSessionStore::new(self.store_root.clone()),
            &output,
            checkpoint,
        );
        let accounting = quota.rebuild_managed_usage();
        let release_final = quota.release(ReservationKind::FinalSession, manifest_reservation);
        let release_manifest = quota.release(ReservationKind::Manifest, manifest_reservation);
        if let Err(publication) = publication {
            return Err(ContinuousRecorderError::Incremental(format!(
                "final session publication failed: {publication}; accounting={accounting:?}; release_final={release_final:?}; release_manifest={release_manifest:?}"
            )));
        }
        accounting?;
        release_final?;
        release_manifest?;
        Ok(())
    }

    /// Inject one publication boundary failure for deterministic restart tests.
    /// Fault is consumed exactly once by the next production publication.
    pub fn set_checkpoint_fault(&mut self, fault: CheckpointFault) {
        self.checkpoint_fault = Some(fault);
    }

    pub fn set_capture_metadata(
        &mut self,
        capture: RecordingCaptureMetadata,
    ) -> Result<(), ContinuousRecorderError> {
        self.metadata.capture = Some(capture);
        write_recording_metadata(&self.wal_directory, &self.metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    pub fn set_parent_id(&mut self, parent_id: RecordingId) -> Result<(), ContinuousRecorderError> {
        if parent_id.as_uuid().is_nil() {
            return Err(ContinuousRecorderError::Metadata(
                "parent recording identity is nil".into(),
            ));
        }
        self.active_metadata.parent_id = Some(parent_id);
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    fn complete_pending_continuation(&mut self) -> Result<(), ContinuousRecorderError> {
        let Some(pending) = self.pending_continuation.clone() else {
            return Ok(());
        };
        let parent_id = self
            .active_metadata
            .parent_id
            .unwrap_or(RecordingId(pending.old_epoch_id.as_uuid()));
        // Export on a scratch processor: a retry after a mid-completion crash
        // must never re-ingest the predecessor WAL into an already-restored
        // assembler, so the live processor stays untouched until Ready is
        // durable.
        let mut processor = IncrementalProcessor::new(chronicle_session::SessionLimits::default());
        // A chained rollover's predecessor is itself a successor: restore its
        // verified handoff so connections idle across that epoch survive the
        // next continuation instead of being silently reset.
        match fs::metadata(pending.old_wal_directory.join(EPOCH_CONTINUATION_IN_FILE)) {
            Ok(_) => {
                let handoff = EpochContinuationCheckpoint::load(
                    pending.old_wal_directory.join(EPOCH_CONTINUATION_IN_FILE),
                )
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
                if handoff.successor_epoch_id != pending.old_epoch_id
                    || handoff.pipeline_version != ETL_PIPELINE_VERSION.to_string()
                {
                    return Err(ContinuousRecorderError::Incremental(
                        "predecessor continuation-in lineage mismatch".into(),
                    ));
                }
                processor
                    .restore_continuation(
                        &handoff,
                        parent_id,
                        handoff.predecessor_epoch_id,
                        pending.old_epoch_id,
                        &ETL_PIPELINE_VERSION.to_string(),
                    )
                    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ContinuousRecorderError::Incremental(error.to_string()));
            }
        }
        let checkpoint = Self::export_continuation(
            parent_id,
            &mut processor,
            &self.incremental_registry,
            self.incremental_session_id,
            &pending.old_wal_directory,
            pending.old_epoch_id,
            pending.successor_epoch_id,
            pending.old_epoch_ordinal,
        )?;
        self.publish_continuation(
            &checkpoint,
            &pending.old_wal_directory,
            &self.wal_directory,
            pending.old_epoch_ordinal,
        )?;
        // Durable Ready precedes any live-processor mutation; later steps are
        // idempotent overwrites, so a crash here retries cleanly.
        ContinuationDependency::ready_from_checkpoint(
            parent_id,
            pending.old_epoch_id,
            &checkpoint,
            &ETL_PIPELINE_VERSION.to_string(),
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
        .write_atomic(self.wal_directory.join(EPOCH_CONTINUATION_STATE_FILE))
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        self.incremental_processor
            .restore_continuation(
                &checkpoint,
                parent_id,
                pending.old_epoch_id,
                pending.successor_epoch_id,
                &ETL_PIPELINE_VERSION.to_string(),
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        self.continuation_in = Some(checkpoint);
        self.active_metadata.continuation_in = Some(EPOCH_CONTINUATION_IN_FILE.to_owned());
        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        self.pending_continuation = None;
        Ok(())
    }

    pub fn poll(&mut self, now_millis: u64) -> Result<RecorderPoll, ContinuousRecorderError> {
        self.refresh_quota_metadata()?;
        let poll = self.recorder.poll(now_millis)?;
        self.sync_active_progress();
        if let Err(error) = self.complete_pending_continuation() {
            eprintln!("continuation completion deferred: {error}");
            self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
            self.active_metadata.health = RecorderHealth::Degraded;
            self.active_metadata.updated_at_unix_seconds = unix_seconds();
            write_recorder_metadata(&self.state_root, &self.active_metadata)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        }
        if self.pending_continuation.is_none() {
            self.poll_incremental(now_millis)?;
        } else {
            self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
            self.active_metadata.health = RecorderHealth::Degraded;
            self.active_metadata.updated_at_unix_seconds = unix_seconds();
            write_recorder_metadata(&self.state_root, &self.active_metadata)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        }
        Ok(poll)
    }

    #[allow(clippy::too_many_lines)]
    fn poll_incremental(&mut self, now_millis: u64) -> Result<(), ContinuousRecorderError> {
        if now_millis < self.next_incremental_millis {
            return Ok(());
        }
        self.next_incremental_millis = now_millis.saturating_add(250);
        let wal_directory = self.wal_directory.clone();
        let recording_id = self.recorder.ingest().recording_id();
        let checkpoint = self.incremental_checkpoint.clone();
        let processor = &mut self.incremental_processor;
        let protocol_registry = &self.incremental_registry;
        let session_id = self.incremental_session_id;
        let previous_processor_state = processor.state_snapshot();
        let mut incremental_result: Option<IncrementalResult> = None;
        let mut marker = None;
        let worker_result = self.incremental_worker.process_once(|| {
            let snapshot = match chronicle_wal::read_committed_snapshot_with_records(
                &wal_directory,
                recording_id,
                chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
            ) {
                Ok(snapshot) => snapshot,
                Err(chronicle_wal::WalError::NoPublishedSegments) => return Ok(0),
                Err(error) => {
                    eprintln!("incremental snapshot failed: {error}");
                    return Err(IncrementalWorkerError::BatchFailed);
                }
            };
            if let Some(checkpoint) = &checkpoint
                && checkpoint.marker.sequence < snapshot.snapshot.marker_sequence
            {
                let marker_digest = snapshot
                    .envelopes
                    .iter()
                    .find(|envelope| envelope.sequence == checkpoint.marker.sequence)
                    .and_then(|envelope| chronicle_wal::encode_envelope(envelope).ok())
                    .map(|bytes| {
                        let digest: [u8; 32] = Sha256::digest(bytes).into();
                        digest_hex(&digest)
                    });
                if marker_digest.as_deref() != Some(checkpoint.marker.marker_digest.as_str()) {
                    eprintln!("incremental checkpoint marker digest mismatch");
                    return Err(IncrementalWorkerError::BatchFailed);
                }
            }
            if let Some(checkpoint) = &checkpoint
                && (checkpoint.epoch_id != EpochId::from_uuid(recording_id.0)
                    || checkpoint.marker.sequence > snapshot.snapshot.marker_sequence
                    || (checkpoint.marker.sequence == snapshot.snapshot.marker_sequence
                        && checkpoint.marker.marker_digest
                            != digest_hex(&snapshot.snapshot.marker_digest)))
            {
                eprintln!("incremental checkpoint epoch or marker sequence mismatch");
                return Err(IncrementalWorkerError::BatchFailed);
            }
            if let Some(checkpoint) = &checkpoint {
                let active_digest = chronicle_wal::verified_segment_prefix_sha256(
                    &wal_directory,
                    recording_id,
                    &snapshot.envelopes,
                    checkpoint.marker.segment_ordinal,
                    checkpoint.marker.sequence,
                )
                .map_err(|error| {
                    eprintln!("incremental checkpoint segment prefix failed: {error}");
                    IncrementalWorkerError::BatchFailed
                })?;
                if digest_hex(&active_digest) != checkpoint.marker.active_segment_digest {
                    eprintln!("incremental checkpoint lineage mismatch");
                    return Err(IncrementalWorkerError::BatchFailed);
                }
            }
            if processor.last_marker_sequence() == Some(snapshot.snapshot.marker_sequence) {
                let digest_matches = processor.marker_digest_matches(&snapshot.snapshot.marker_digest);
                let has_ahead_envelope = snapshot
                    .envelopes
                    .iter()
                    .any(|envelope| envelope.sequence > snapshot.snapshot.marker_sequence);
                if !digest_matches || has_ahead_envelope {
                    eprintln!(
                        "incremental processor marker state mismatch: digest_matches={digest_matches} ahead={has_ahead_envelope} marker={} envelopes={}",
                        snapshot.snapshot.marker_sequence,
                        snapshot.envelopes.len()
                    );
                    return Err(IncrementalWorkerError::BatchFailed);
                }
                return Ok(0);
            }
            let result = processor
                .process_snapshot(
                    CommittedWalSnapshot {
                        marker_sequence: snapshot.snapshot.marker_sequence,
                        marker_digest: snapshot.snapshot.marker_digest,
                        envelopes: &snapshot.envelopes,
                        segment_lineage: &snapshot.snapshot.segment_lineage,
                    },
                    protocol_registry,
                    session_id,
                )
                .map_err(|error| {
                    eprintln!("incremental processor snapshot failed: {error}");
                    IncrementalWorkerError::BatchFailed
                })?;
            let active_segment_digest = chronicle_wal::verified_segment_prefix_sha256(
                &wal_directory,
                recording_id,
                &snapshot.envelopes,
                snapshot.snapshot.marker_segment_ordinal,
                snapshot.snapshot.marker_sequence,
            )
            .map_err(|error| {
                eprintln!("incremental snapshot segment prefix failed: {error}");
                IncrementalWorkerError::BatchFailed
            })?;
            marker = Some((
                snapshot.snapshot.marker_sequence,
                snapshot.snapshot.marker_segment_ordinal,
                snapshot.snapshot.marker_digest,
                active_segment_digest,
            ));
            incremental_result = Some(result);
            Ok(0)
        });
        match worker_result {
            Ok(()) => {
                let mut published = true;
                if let (
                    Some(result),
                    Some((marker_sequence, segment_ordinal, marker_digest, active_segment_digest)),
                ) = (incremental_result, marker)
                    && let Err(error) = self.publish_incremental_result(
                        &result,
                        marker_sequence,
                        segment_ordinal,
                        marker_digest,
                        active_segment_digest,
                    )
                {
                    self.incremental_processor
                        .restore_state(previous_processor_state);
                    published = false;
                    eprintln!("incremental publication failed: {error}");
                }
                if published {
                    if matches!(
                        self.active_metadata.failure,
                        Some(MetadataCode::QuotaPressure | MetadataCode::TerminalQuotaPressure,)
                    ) {
                        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                        if self.active_metadata.failure != Some(MetadataCode::TerminalQuotaPressure)
                        {
                            self.active_metadata.health = RecorderHealth::Degraded;
                        }
                    } else if self.pending_continuation.is_some() {
                        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                        self.active_metadata.health = RecorderHealth::Degraded;
                    } else {
                        self.active_metadata.processing_readiness = RecorderReadiness::Ready;
                        self.active_metadata.health = RecorderHealth::Healthy;
                        self.active_metadata.failure = None;
                    }
                } else {
                    self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                    if self.active_metadata.failure != Some(MetadataCode::TerminalQuotaPressure) {
                        self.active_metadata.health = RecorderHealth::Degraded;
                    }
                    if !matches!(
                        self.active_metadata.failure,
                        Some(MetadataCode::QuotaPressure | MetadataCode::TerminalQuotaPressure,)
                    ) {
                        self.active_metadata.failure = Some(MetadataCode::StorageFailure);
                    }
                }
                self.active_metadata.lag.records = 0;
                self.active_metadata.lag.age_seconds = 0;
                self.active_metadata.lag.bytes = 0;
            }
            Err(error) => {
                eprintln!("incremental worker failed: {error}");
                self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                if self.active_metadata.failure != Some(MetadataCode::TerminalQuotaPressure) {
                    self.active_metadata.health = RecorderHealth::Degraded;
                }
                if !matches!(
                    self.active_metadata.failure,
                    Some(MetadataCode::QuotaPressure | MetadataCode::TerminalQuotaPressure)
                ) {
                    self.active_metadata.failure = Some(MetadataCode::StorageFailure);
                }
            }
        }
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn publish_incremental_result(
        &mut self,
        result: &IncrementalResult,
        marker_sequence: u64,
        segment_ordinal: u64,
        marker_digest: [u8; 32],
        active_segment_digest: [u8; 32],
    ) -> Result<(), ContinuousRecorderError> {
        let recording_id = self.recorder.ingest().recording_id();
        let epoch_ordinal = self
            .active_metadata
            .current_epoch
            .as_ref()
            .map_or(0, |epoch| epoch.ordinal);
        let parent_id = self.active_metadata.parent_id.unwrap_or(recording_id);
        let mut session = result.output.session.clone();
        stamp_epoch_operation_provenance(
            &mut session,
            parent_id,
            EpochId::from_uuid(recording_id.0),
            Some(epoch_ordinal),
            Some((result.first_marker_sequence, result.last_marker_sequence)),
        );
        let operations = session
            .connections
            .iter()
            .flat_map(|connection| connection.operations.iter().cloned())
            .collect();
        let batch = CanonicalDeltaBatchV1::new(
            result.first_marker_sequence,
            result.last_marker_sequence,
            operations,
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let key = format!(
            "deltas/{}/{}.json",
            recording_id.0, result.last_marker_sequence
        );
        let bytes = batch
            .encode()
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let artifact = RecordingArtifact::new(
            key.clone(),
            RecordingArtifactKind::CanonicalDeltaBatch,
            bytes.clone(),
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let mut outputs = self
            .incremental_checkpoint
            .as_ref()
            .map_or_else(Vec::new, |checkpoint| checkpoint.outputs.clone());
        outputs.push(DeltaBatchReference {
            key: key.clone(),
            digest: artifact.checksum,
            first_marker_sequence: result.first_marker_sequence,
            last_marker_sequence: result.last_marker_sequence,
            bytes: bytes.len() as u64,
        });
        let marker_digest_hex = digest_hex(&marker_digest);
        let decoder = self
            .incremental_processor
            .decoder_state(
                self.incremental_session_id,
                recording_id.0,
                epoch_ordinal,
                marker_sequence,
                marker_digest_hex.clone(),
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let successor_epoch_id = EpochId::from_uuid(recording_id.0);
        let continuation = if let Some(handoff) = &self.continuation_in {
            let handoff_bytes = handoff
                .encode()
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
            let handoff_digest: [u8; 32] = Sha256::digest(handoff_bytes).into();
            ContinuationDependency::pending_for_parent(
                Some(parent_id),
                handoff.predecessor_epoch_id,
                successor_epoch_id,
            )
            .transition(
                ContinuationState::Ready,
                Some(digest_hex(&handoff_digest)),
                None,
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
        } else if self.pending_continuation.is_some() {
            ContinuationDependency::load(self.wal_directory.join(EPOCH_CONTINUATION_STATE_FILE))
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
        } else {
            ContinuationDependency {
                parent_id: Some(parent_id),
                state: ContinuationState::Unavailable,
                predecessor_epoch_id: None,
                successor_epoch_id,
                input_digest: None,
                output_digest: None,
                failure_code: Some("no_predecessor".into()),
            }
        };
        let checkpoint = IncrementalEtlCheckpoint {
            version: INCREMENTAL_CHECKPOINT_SCHEMA_VERSION,
            owner: CheckpointOwner::Etl,
            lifecycle: CheckpointLifecycle::Active,
            parent_id,
            epoch_id: successor_epoch_id,
            epoch_ordinal,
            config_digest: self.active_metadata.config_digest.clone(),
            pipeline_version: ETL_PIPELINE_VERSION.to_string(),
            canonical_schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
            marker: MarkerLineage {
                sequence: marker_sequence,
                segment_ordinal,
                marker_digest: marker_digest_hex,
                active_segment_digest: digest_hex(&active_segment_digest),
            },
            segment_lineage: result.segment_lineage.clone(),
            continuation,
            decoder,
            outputs,
            published_operation_keys: self.incremental_processor.published_operation_keys(),
            source_status: SourceStatus::Live,
            checksum: String::new(),
        }
        .with_checksum()
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let store = FilesystemRecordingStore::new(self.store_root.clone());
        let authorities = self
            .startup
            .quota_authorities()
            .iter()
            .filter(|authority| {
                self.store_root
                    .starts_with(Path::new(&authority.quota().domain))
            })
            .collect::<Vec<_>>();
        if authorities.len() != 1 {
            return Err(crate::QuotaError::InvalidQuota.into());
        }
        let quota = authorities[0];
        quota.refresh_from_filesystem()?;
        let checkpoint_bytes = checkpoint
            .encode()
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
            .len() as u64;
        let recording_reservation = bytes.len() as u64;
        let checkpoint_reservation = checkpoint_bytes.saturating_mul(2);
        quota.reserve(ReservationKind::RecordingStore, recording_reservation)?;
        if let Err(error) = quota.reserve(ReservationKind::Checkpoint, checkpoint_reservation) {
            let _ = quota.release(ReservationKind::RecordingStore, recording_reservation);
            return Err(error.into());
        }
        let checkpoint_path = self.wal_directory.join(INCREMENTAL_ETL_CHECKPOINT_FILE);
        let publication = if let Some(fault) = self.checkpoint_fault.take() {
            publish_delta_then_checkpoint_with_fault(
                &store,
                key,
                &batch,
                &checkpoint,
                &checkpoint_path,
                Some(fault),
            )
        } else {
            publish_delta_then_checkpoint(&store, key, &batch, &checkpoint, &checkpoint_path)
        };
        let accounting = quota.rebuild_managed_usage();
        let release_recording =
            quota.release(ReservationKind::RecordingStore, recording_reservation);
        let release_checkpoint = quota.release(ReservationKind::Checkpoint, checkpoint_reservation);
        if let Err(publication) = publication {
            return Err(ContinuousRecorderError::Incremental(format!(
                "publication failed: {publication}; accounting={accounting:?}; release_recording={release_recording:?}; release_checkpoint={release_checkpoint:?}"
            )));
        }
        accounting?;
        release_recording?;
        release_checkpoint?;
        let checkpoint = IncrementalEtlCheckpoint::load(&checkpoint_path)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        self.active_metadata.incremental_checkpoint = Some(crate::IncrementalCheckpointSummaryV1 {
            marker_sequence: checkpoint.marker.sequence,
            checkpoint_digest: checkpoint.checksum.clone(),
            output_count: checkpoint.outputs.len() as u64,
        });
        self.incremental_checkpoint = Some(checkpoint);
        Ok(())
    }

    fn refresh_quota_metadata(&mut self) -> Result<(), ContinuousRecorderError> {
        let current_wal_bytes = crate::recording_physical_wal_bytes(&self.wal_directory)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let wal_growth = current_wal_bytes.saturating_sub(self.last_wal_bytes);
        let mut pressure = false;
        let quota = self
            .startup
            .quota_authorities()
            .iter()
            .map(|authority| {
                if wal_growth > 0
                    && self
                        .wal_directory
                        .starts_with(Path::new(&authority.quota().domain))
                {
                    authority.consume(ReservationKind::Wal, wal_growth)?;
                }
                // Rebuild durable usage on every readiness poll. Refreshing
                // only statvfs free space misses writes made by WAL/store
                // publishers and can admit past quota after restart or
                // concurrent publication.
                let free_bytes = authority.rebuild_managed_usage()?;
                let reserved_bytes = authority.reserved_bytes()?;
                pressure |= free_bytes < authority.quota().minimum_free_bytes
                    || reserved_bytes
                        > free_bytes.saturating_sub(authority.quota().minimum_free_bytes);
                Ok(QuotaStatus {
                    domain: authority.quota().domain.clone(),
                    quota_bytes: authority.quota().quota_bytes,
                    free_bytes,
                    reserved_bytes,
                })
            })
            .collect::<Result<Vec<_>, crate::QuotaError>>()?;
        self.last_wal_bytes = current_wal_bytes;
        let changed = self.active_metadata.quota != quota;
        self.active_metadata.quota = quota;
        let terminal = pressure
            && self.startup.quota_authorities().iter().any(|authority| {
                crate::classify_quota_pressure(
                    authority.available_bytes(),
                    authority.reserved_bytes().unwrap_or(0),
                    authority.quota().quota_bytes,
                    authority.quota().minimum_free_bytes,
                    authority.managed_bytes(),
                ) == crate::QuotaPressureKind::Terminal
            });
        if self.active_metadata.failure == Some(MetadataCode::TerminalQuotaPressure) {
            // Terminal quota pressure latches: required headroom cannot be
            // restored without deleting protected or corrupt evidence, so the
            // recorder stays explicitly failed until an operator intervenes.
            self.recorder.set_admission_enabled(false);
        } else if terminal {
            self.recorder.set_admission_enabled(false);
            self.active_metadata.capture_readiness = RecorderReadiness::NotReady;
            self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
            self.active_metadata.health = RecorderHealth::Failed;
            self.active_metadata.failure = Some(MetadataCode::TerminalQuotaPressure);
        } else if pressure {
            // Disk pressure is recoverable runtime backpressure, not a process
            // failure. Stop admission before the next source poll, preserve
            // committed WAL, and keep status/control-plane polling alive.
            self.recorder.set_admission_enabled(false);
            self.active_metadata.capture_readiness = RecorderReadiness::NotReady;
            self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
            self.active_metadata.health = RecorderHealth::Degraded;
            self.active_metadata.failure = Some(MetadataCode::QuotaPressure);
        } else {
            self.recorder.set_admission_enabled(true);
            if self.active_metadata.failure == Some(MetadataCode::QuotaPressure) {
                self.active_metadata.capture_readiness = RecorderReadiness::Ready;
                self.active_metadata.health = RecorderHealth::Healthy;
                self.active_metadata.failure = None;
            }
        }
        if changed || pressure || self.active_metadata.failure.is_none() {
            write_recorder_metadata(&self.state_root, &self.active_metadata)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        }
        Ok(())
    }

    fn sync_active_progress(&mut self) {
        let counters = self.recorder.ingest().counters();
        self.active_metadata.counters.captured_records = counters.accepted_into_queue.records;
        self.active_metadata.counters.captured_bytes = counters.accepted_into_queue.bytes;
        self.active_metadata.counters.committed_records = counters.committed.records;
        self.active_metadata.counters.committed_bytes = counters.committed.bytes;
        self.active_metadata.counters.quota_rejected_records =
            counters.rejected_due_to_quota.records;
        self.active_metadata.counters.quota_rejected_bytes = counters.rejected_due_to_quota.bytes;
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
    }

    pub fn begin_rollover_transition(
        &self,
        parent_id: RecordingId,
        next_wal_directory: impl AsRef<Path>,
        next_epoch_id: EpochId,
        reservation_bytes: u64,
    ) -> Result<(), ContinuousRecorderError> {
        let next_wal_directory = next_wal_directory.as_ref();
        if reservation_bytes != self.epoch_max_bytes
            || self
                .startup
                .quota_authorities()
                .iter()
                .filter(|authority| {
                    next_wal_directory.starts_with(Path::new(&authority.quota().domain))
                })
                .count()
                != 1
        {
            return Err(ContinuousRecorderError::Metadata(
                "rollover successor quota domain or reservation mismatch".into(),
            ));
        }
        if load_transition(&self.state_root)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?
            .is_some()
        {
            return Err(ContinuousRecorderError::Metadata(
                "rollover transition already exists".into(),
            ));
        }
        let old_epoch_id = EpochId::from_uuid(self.recorder.ingest().recording_id().0);
        let transition = RolloverTransition::new(
            parent_id,
            old_epoch_id,
            next_epoch_id,
            self.recorder.epoch_ordinal(),
            self.recorder.epoch_ordinal().saturating_add(1),
            &self.wal_directory,
            next_wal_directory,
            "pending-boundary",
            "pending-outcome",
            format!("epoch-{next_epoch_id}"),
            reservation_bytes,
        )
        .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        write_transition_atomic(&self.state_root, &transition)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    pub fn reserve_successor_quota(
        &self,
        next_wal_directory: impl AsRef<Path>,
        max_bytes: u64,
    ) -> Result<(), ContinuousRecorderError> {
        let next_wal_directory = next_wal_directory.as_ref();
        if max_bytes != self.epoch_max_bytes {
            return Err(ContinuousRecorderError::Metadata(
                "successor reservation does not match configured epoch limit".into(),
            ));
        }
        let authorities = self
            .startup
            .quota_authorities()
            .iter()
            .filter(|authority| {
                next_wal_directory.starts_with(Path::new(&authority.quota().domain))
            })
            .collect::<Vec<_>>();
        if authorities.len() != 1 {
            return Err(ContinuousRecorderError::Metadata(
                "rollover successor quota domain mismatch".into(),
            ));
        }
        authorities[0].reserve(ReservationKind::Wal, max_bytes)?;
        Ok(())
    }

    pub fn release_successor_quota(
        &self,
        next_wal_directory: impl AsRef<Path>,
        max_bytes: u64,
    ) -> Result<(), ContinuousRecorderError> {
        let next_wal_directory = next_wal_directory.as_ref();
        if max_bytes != self.epoch_max_bytes {
            return Err(ContinuousRecorderError::Metadata(
                "successor reservation does not match configured epoch limit".into(),
            ));
        }
        let authorities = self
            .startup
            .quota_authorities()
            .iter()
            .filter(|authority| {
                next_wal_directory.starts_with(Path::new(&authority.quota().domain))
            })
            .collect::<Vec<_>>();
        if authorities.len() != 1 {
            return Err(ContinuousRecorderError::Metadata(
                "rollover successor quota domain mismatch".into(),
            ));
        }
        authorities[0].release(ReservationKind::Wal, max_bytes)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn export_continuation(
        parent_id: RecordingId,
        processor: &mut IncrementalProcessor,
        registry: &ProtocolRegistry,
        session_id: SessionId,
        old_wal_directory: &Path,
        old_epoch_id: EpochId,
        successor_epoch_id: EpochId,
        old_epoch_ordinal: u64,
    ) -> Result<EpochContinuationCheckpoint, ContinuousRecorderError> {
        let snapshot = chronicle_wal::read_committed_snapshot_with_records(
            old_wal_directory,
            RecordingId(old_epoch_id.as_uuid()),
            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
        )?;
        let marker_sequence = snapshot.snapshot.marker_sequence;
        let marker_digest = snapshot.snapshot.marker_digest;
        let needs_catch_up = processor
            .last_marker_sequence()
            .is_none_or(|sequence| sequence != marker_sequence);
        if needs_catch_up {
            processor
                .process_snapshot(
                    CommittedWalSnapshot {
                        marker_sequence,
                        marker_digest,
                        envelopes: &snapshot.envelopes,
                        segment_lineage: &snapshot.snapshot.segment_lineage,
                    },
                    registry,
                    session_id,
                )
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        } else if !processor.marker_digest_matches(&marker_digest) {
            return Err(ContinuousRecorderError::Incremental(
                "predecessor continuation marker digest mismatch".into(),
            ));
        }
        let marker_digest_hex = digest_hex(&marker_digest);
        let decoder = processor
            .decoder_state(
                session_id,
                old_epoch_id.as_uuid(),
                old_epoch_ordinal,
                marker_sequence,
                marker_digest_hex.clone(),
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let marker = MarkerLineage {
            sequence: marker_sequence,
            segment_ordinal: snapshot.snapshot.marker_segment_ordinal,
            marker_digest: marker_digest_hex,
            active_segment_digest: digest_hex(&chronicle_wal::verified_segment_prefix_sha256(
                old_wal_directory,
                RecordingId(old_epoch_id.as_uuid()),
                &snapshot.envelopes,
                snapshot.snapshot.marker_segment_ordinal,
                marker_sequence,
            )?),
        };
        let segment_lineage = snapshot
            .snapshot
            .segment_lineage
            .iter()
            .map(|segment| SegmentLineage {
                ordinal: segment.ordinal,
                digest: digest_hex(&segment.digest),
                first_sequence: segment.first_sequence,
                last_sequence: segment.last_sequence,
            })
            .collect();
        EpochContinuationCheckpoint::new(
            parent_id,
            old_epoch_id,
            successor_epoch_id,
            marker,
            segment_lineage,
            ETL_PIPELINE_VERSION.to_string(),
            decoder.state,
            ContinuationLimits::default(),
        )
        .and_then(|checkpoint| {
            checkpoint.with_counts([decoder.pending_connection_count, 0, 0, 0, 0, 0, 0])
        })
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))
    }

    fn publish_continuation(
        &self,
        checkpoint: &EpochContinuationCheckpoint,
        old_wal_directory: &Path,
        successor_wal_directory: &Path,
        old_epoch_ordinal: u64,
    ) -> Result<(), ContinuousRecorderError> {
        let bytes = checkpoint
            .encode()
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let key = format!(
            "continuations/{}/{}/{}.json",
            checkpoint.parent_id, checkpoint.predecessor_epoch_id, checkpoint.successor_epoch_id
        );
        let artifact = RecordingArtifact::new(
            key.clone(),
            RecordingArtifactKind::ContinuationCheckpoint,
            bytes,
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
        .with_lineage(
            checkpoint.parent_id,
            checkpoint.predecessor_epoch_id,
            old_epoch_ordinal,
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let store = FilesystemRecordingStore::new(self.store_root.clone());
        let expected = artifact.head();
        store
            .put_if_absent(artifact)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let actual = store
            .head(&key)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        if actual.version != expected.version
            || actual.kind != expected.kind
            || actual.parent_id != expected.parent_id
            || actual.epoch_id != expected.epoch_id
            || actual.epoch_ordinal != expected.epoch_ordinal
            || actual.bytes != expected.bytes
            || actual.checksum != expected.checksum
        {
            return Err(ContinuousRecorderError::Incremental(
                "continuation artifact verification failed".into(),
            ));
        }
        checkpoint
            .write_atomic(old_wal_directory.join(EPOCH_CONTINUATION_OUT_FILE))
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        checkpoint
            .write_atomic(successor_wal_directory.join(EPOCH_CONTINUATION_IN_FILE))
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        Ok(())
    }

    pub fn rollover(
        &mut self,
        now_millis: u64,
        outcome_path: impl AsRef<Path>,
    ) -> Result<RecorderEpochBoundary, ContinuousRecorderError> {
        let boundary = self.recorder.rollover(now_millis, outcome_path)?;
        self.sync_active_progress();
        self.active_metadata.previous_epoch = Some(EpochMetadata {
            ordinal: boundary.old_epoch_ordinal,
            recording_id: boundary.old_recording_id.0,
        });
        self.active_metadata.current_epoch = Some(EpochMetadata {
            ordinal: boundary.new_epoch_ordinal,
            recording_id: boundary.new_recording_id.0,
        });
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(boundary)
    }

    #[allow(clippy::too_many_lines)]
    #[allow(clippy::too_many_lines)]
    pub fn rollover_to(
        &mut self,
        next_wal_directory: impl AsRef<Path>,
        next_writer: GroupCommitWalWriter,
        now_millis: u64,
        outcome_path: impl AsRef<Path>,
    ) -> Result<RecorderEpochBoundary, ContinuousRecorderError> {
        // Capture/WAL rollover is independent of ETL continuation: the
        // successor's continuation dependency is written durably before
        // successor metadata, so a chained rollover while the predecessor
        // continuation is still pending simply re-targets the in-memory
        // pending edge and crash recovery rebuilds it from the active
        // epoch's continuation-state file.
        let next_wal_directory = next_wal_directory.as_ref().to_path_buf();
        let next_epoch_id = EpochId::from_uuid(next_writer.recording_id().0);
        let transition = load_transition(&self.state_root)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?
            .ok_or_else(|| {
                ContinuousRecorderError::Metadata("rollover transition missing".into())
            })?;
        if transition.phase != RolloverTransitionPhase::Prepared
            || transition.new_epoch_id != next_epoch_id
            || transition.new_path != next_wal_directory.to_string_lossy()
        {
            return Err(ContinuousRecorderError::Metadata(
                "rollover transition identity mismatch".into(),
            ));
        }
        let transition = transition
            .with_phase(RolloverTransitionPhase::SuccessorCreated)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        write_transition_atomic(&self.state_root, &transition)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let old_wal_directory = self.wal_directory.clone();
        self.sync_active_progress();
        let boundary = self
            .recorder
            .rollover_to(next_writer, now_millis, &outcome_path)?;
        let boundary_digest = Sha256::digest(
            serde_json::to_vec(&boundary.old_commit)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?,
        );
        let outcome_digest = Sha256::digest(
            fs::read(&outcome_path)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?,
        );
        let transition = transition
            .with_boundary_evidence(
                digest_hex(&boundary_digest.into()),
                digest_hex(&outcome_digest.into()),
            )
            .and_then(|transition| {
                transition.with_phase(RolloverTransitionPhase::BoundaryCommitted)
            })
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        write_transition_atomic(&self.state_root, &transition)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        // Durable pending continuation precedes successor metadata: a crash
        // after the journal but before the state file would otherwise activate
        // a successor whose ETL starts without restored predecessor state.
        let parent_id = self
            .active_metadata
            .parent_id
            .unwrap_or(boundary.old_recording_id);
        ContinuationDependency::pending_for_parent(
            Some(parent_id),
            EpochId::from_uuid(boundary.old_recording_id.0),
            next_epoch_id,
        )
        .write_atomic(next_wal_directory.join(EPOCH_CONTINUATION_STATE_FILE))
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;

        self.metadata.recording_id = boundary.old_recording_id;
        self.metadata
            .last_valid_commit
            .clone_from(&boundary.old_commit);
        write_recording_metadata(&old_wal_directory, &self.metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        let selector = self.metadata.selector.clone();
        let capture = self.metadata.capture.clone();
        self.wal_directory.clone_from(&next_wal_directory);
        self.last_wal_bytes = crate::recording_physical_wal_bytes(&self.wal_directory)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        self.metadata = RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id: boundary.new_recording_id,
            selector,
            status: RecordingStatus::Recording,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: crate::RecordingCounters::default(),
            terminal_wal_loss: None,
            capture,
        };
        write_recording_metadata(&self.wal_directory, &self.metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;

        self.pending_continuation = Some(PendingContinuation {
            old_wal_directory: old_wal_directory.clone(),
            old_epoch_id: EpochId::from_uuid(boundary.old_recording_id.0),
            successor_epoch_id: next_epoch_id,
            old_epoch_ordinal: boundary.old_epoch_ordinal,
        });
        self.incremental_processor =
            IncrementalProcessor::new(chronicle_session::SessionLimits::default());
        self.incremental_session_id = SessionId(boundary.new_recording_id.0);
        self.incremental_checkpoint = None;
        self.continuation_in = None;
        self.continuation_out = None;
        self.next_incremental_millis = 0;
        self.active_metadata.previous_epoch = Some(EpochMetadata {
            ordinal: boundary.old_epoch_ordinal,
            recording_id: boundary.old_recording_id.0,
        });
        self.active_metadata.current_epoch = Some(EpochMetadata {
            ordinal: boundary.new_epoch_ordinal,
            recording_id: boundary.new_recording_id.0,
        });
        self.active_metadata.current_epoch_id = Some(next_epoch_id);
        self.active_metadata.continuation_in = None;
        self.active_metadata.continuation_out = None;
        self.active_metadata.incremental_checkpoint = None;
        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(boundary)
    }

    pub fn complete_rollover_transition(&self) -> Result<(), ContinuousRecorderError> {
        if let Some(transition) = load_transition(&self.state_root)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?
        {
            let transition = transition
                .with_phase(RolloverTransitionPhase::TopologyActivated)
                .and_then(|transition| transition.with_phase(RolloverTransitionPhase::Complete))
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
            write_transition_atomic(&self.state_root, &transition)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
            crate::remove_transition(&self.state_root)
                .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
            return Ok(());
        }
        crate::remove_transition(&self.state_root)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    pub fn release_production_reservations(&self) -> Result<(), ContinuousRecorderError> {
        for authority in self.startup.quota_authorities() {
            for kind in [
                ReservationKind::Wal,
                ReservationKind::Staging,
                ReservationKind::Trash,
            ] {
                authority
                    .release_all(kind)
                    .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
            }
        }
        Ok(())
    }

    pub fn shutdown(
        &mut self,
        reason: crate::ShutdownReason,
        now_millis: u64,
        timeout: Duration,
    ) -> Result<crate::RecordingIngestResult, ContinuousRecorderError> {
        self.startup.begin_draining()?;
        let result = self
            .recorder
            .graceful_shutdown(reason, now_millis, timeout)?;
        self.metadata.last_valid_commit = self.recorder.ingest().commit_boundary();
        result
            .persist_metadata(&self.wal_directory, &mut self.metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        // Final in-process ETL re-acquires the WAL lock; flock(2) treats a
        // second fd for the same file in the same process as a conflicting
        // owner, so release the active epoch lock before publication runs.
        self.recorder.ingest_mut().release_wal_lock();
        if let Err(error) = self.complete_pending_continuation() {
            eprintln!("continuation completion deferred during shutdown: {error}");
        }
        self.sync_active_progress();
        self.active_metadata.lifecycle = crate::RecorderLifecycleState::Stopped;
        self.active_metadata.capture_readiness = RecorderReadiness::NotReady;
        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
        self.active_metadata.health = RecorderHealth::Degraded;
        self.active_metadata.counters.committed_records = result.counters.committed.records;
        self.active_metadata.counters.committed_bytes = result.counters.committed.bytes;
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
        self.active_metadata.shutdown = Some(MetadataCode::None);
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        self.startup.stop()?;
        Ok(result)
    }

    pub fn startup(&self) -> &RecorderStartup {
        &self.startup
    }

    pub fn recorder(&self) -> &RecorderOrchestrator<S> {
        &self.recorder
    }

    /// Read only the durable WAL prefix for incremental ETL. Never repairs or
    /// acquires the recorder-owned WAL lock.
    pub fn committed_snapshot(
        &self,
    ) -> Result<chronicle_wal::CommittedRecordSnapshot, ContinuousRecorderError> {
        Ok(chronicle_wal::read_committed_snapshot_with_records(
            &self.wal_directory,
            self.recorder.ingest().recording_id(),
            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
        )?)
    }
}

#[allow(clippy::too_many_lines)]
fn validate_checkpoint_before_capture(
    config: &NormalizedRecorderConfig,
    wal_directory: &Path,
    recording_id: RecordingId,
    _epoch_ordinal: u64,
) -> Result<(), ContinuousRecorderError> {
    let checkpoint_path = wal_directory.join(INCREMENTAL_ETL_CHECKPOINT_FILE);
    let store = FilesystemRecordingStore::new(config.store_root.clone());
    recover_pending_checkpoint(&store, &checkpoint_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if !checkpoint_path.exists() {
        return Ok(());
    }
    let checkpoint = IncrementalEtlCheckpoint::load(&checkpoint_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let config_digest = config
        .stable_digest()
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint.config_digest != config_digest
        || checkpoint.canonical_schema_version != chronicle_canonical::CANONICAL_SCHEMA_VERSION
        || checkpoint.pipeline_version != ETL_PIPELINE_VERSION.to_string()
    {
        return Err(ContinuousRecorderError::Incremental(format!(
            "checkpoint configuration or pipeline contradiction: config={} expected={} schema={} pipeline={}",
            checkpoint.config_digest,
            config_digest,
            checkpoint.canonical_schema_version,
            checkpoint.pipeline_version,
        )));
    }
    let committed = chronicle_wal::read_committed_snapshot_with_records(
        wal_directory,
        recording_id,
        chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
    )
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let active_segment_digest = chronicle_wal::verified_segment_prefix_sha256(
        wal_directory,
        recording_id,
        &committed.envelopes,
        committed.snapshot.marker_segment_ordinal,
        committed.snapshot.marker_sequence,
    )
    .map(|digest| digest_hex(&digest))
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint.marker.sequence < committed.snapshot.marker_sequence {
        let marker_digest = committed
            .envelopes
            .iter()
            .find(|envelope| envelope.sequence == checkpoint.marker.sequence)
            .and_then(|envelope| chronicle_wal::encode_envelope(envelope).ok())
            .map(|bytes| {
                let digest: [u8; 32] = Sha256::digest(bytes).into();
                digest_hex(&digest)
            });
        if marker_digest.as_deref() != Some(checkpoint.marker.marker_digest.as_str()) {
            return Err(ContinuousRecorderError::Incremental(
                "checkpoint historical marker digest mismatch".into(),
            ));
        }
    }
    let checkpoint_active_digest = chronicle_wal::verified_segment_prefix_sha256(
        wal_directory,
        recording_id,
        &committed.envelopes,
        checkpoint.marker.segment_ordinal,
        checkpoint.marker.sequence,
    )
    .map(|digest| digest_hex(&digest))
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint_active_digest != checkpoint.marker.active_segment_digest {
        return Err(ContinuousRecorderError::Incremental(
            "checkpoint active segment digest mismatch".into(),
        ));
    }
    let marker = MarkerLineage {
        sequence: committed.snapshot.marker_sequence,
        segment_ordinal: committed.snapshot.marker_segment_ordinal,
        marker_digest: digest_hex(&committed.snapshot.marker_digest),
        active_segment_digest,
    };
    let segment_lineage = committed
        .snapshot
        .segment_lineage
        .iter()
        .map(|segment| SegmentLineage {
            ordinal: segment.ordinal,
            digest: digest_hex(&segment.digest),
            first_sequence: segment.first_sequence,
            last_sequence: segment.last_sequence,
        })
        .collect::<Vec<_>>();
    checkpoint
        .validate_against(
            checkpoint.parent_id,
            EpochId::from_uuid(recording_id.0),
            &marker,
            &segment_lineage,
            &ETL_PIPELINE_VERSION.to_string(),
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
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
    Ok(())
}

fn digest_hex(value: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        EpochConfig, EtlConfig, FilesystemDomainConfig, LogLevel, LoggingConfig, RecorderConfigV1,
        RecorderScopeConfig, RetentionMode, SegmentConfig, ShutdownConfig, StoreBackend,
        StoreConfig, process_and_publish_recording_wal,
    };
    use chronicle_capture::FixtureCaptureSource;
    use chronicle_common::{EpochId, RecordingId};
    use chronicle_protocol_builtins::registry;
    use chronicle_wal::{DEFAULT_SEGMENT_BYTES, GroupCommitWalWriter};
    use std::fs;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn config(root: &std::path::Path) -> crate::NormalizedRecorderConfig {
        RecorderConfigV1 {
            version: 1,
            scope: RecorderScopeConfig {
                cgroup_path: root.to_path_buf(),
                cgroup_id: 1,
                shared_scope_acknowledged: true,
            },
            state_root: root.join("state"),
            store_root: root.join("store"),
            domain_lock_root: root.to_path_buf(),
            epoch: EpochConfig {
                max_age_seconds: 3600,
                max_bytes: chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            },
            segment: SegmentConfig {
                max_age_seconds: 300,
                max_bytes: DEFAULT_SEGMENT_BYTES,
            },
            domains: vec![FilesystemDomainConfig {
                root: root.to_path_buf(),
                quota_bytes: 8 * 1024 * 1024 * 1024,
                minimum_free_bytes: 1024 * 1024,
            }],
            retention: Some(RetentionMode::Retain),
            etl: EtlConfig {
                batch_records: 4096,
                max_lag_records: 4096,
                retry_attempts: 1,
            },
            store: StoreConfig {
                backend: StoreBackend::Filesystem,
                max_batch_bytes: 1024,
                max_staging_bytes: 4096,
            },
            shutdown: ShutdownConfig {
                timeout_seconds: 30,
            },
            logging: LoggingConfig {
                level: LogLevel::Info,
            },
        }
        .normalize()
        .unwrap()
    }

    fn pressure_config(root: &std::path::Path) -> crate::NormalizedRecorderConfig {
        let mut config = config(root);
        config.domains[0].quota_bytes = 1_000_000;
        config.domains[0].minimum_free_bytes = 100_000;
        config
    }

    fn start_pressure_service(
        config: &crate::NormalizedRecorderConfig,
    ) -> ContinuousRecorderService<FixtureCaptureSource> {
        let wal = Path::new(&config.domains[0].identity.canonical_root).join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        ContinuousRecorderService::start(
            config,
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap()
    }

    #[test]
    fn quota_pressure_is_terminal_without_cleanup_candidates_and_latches() {
        let root =
            std::env::temp_dir().join(format!("chronicle-quota-terminal-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let config = pressure_config(&root);
        let mut service = start_pressure_service(&config);
        fs::create_dir_all(root.join("store")).unwrap();
        // Durable managed usage alone exceeds the 1 MiB quota ceiling.
        fs::write(root.join("store/fill"), vec![0_u8; 1_050_000]).unwrap();
        service.poll(0).unwrap();
        assert_eq!(
            service.active_metadata.failure,
            Some(crate::MetadataCode::TerminalQuotaPressure)
        );
        assert_eq!(
            service.active_metadata.health,
            crate::RecorderHealth::Failed
        );
        assert_eq!(
            service.active_metadata.capture_readiness,
            crate::RecorderReadiness::NotReady
        );
        assert!(!service.recorder().admission_enabled());
        fs::remove_file(root.join("store/fill")).unwrap();
        service.poll(1).unwrap();
        assert_eq!(
            service.active_metadata.failure,
            Some(crate::MetadataCode::TerminalQuotaPressure)
        );
        assert_eq!(
            service.active_metadata.health,
            crate::RecorderHealth::Failed
        );
        assert!(!service.recorder().admission_enabled());
        fs::remove_dir_all(root).unwrap();
    }

    fn persist_test_startup_metadata(
        config: &crate::NormalizedRecorderConfig,
        quotas: &[crate::QuotaReservationAuthority],
        lifecycle: crate::RecorderLifecycleState,
        recorder_id: uuid::Uuid,
        attempt_id: uuid::Uuid,
    ) -> Result<(), RecorderStartupError> {
        let config_digest = config
            .stable_digest()
            .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
        crate::write_recorder_metadata(
            &config.state_root,
            &crate::RecorderMetadataV1 {
                version: crate::RECORDER_METADATA_SCHEMA_VERSION,
                recorder_id,
                parent_id: None,
                current_epoch_id: None,
                finalized_epoch_count: 0,
                continuation_in: None,
                continuation_out: None,
                retention_state: None,
                last_rollover: None,
                attempt_id,
                config_digest,
                scope: config.scope.clone(),
                boot_clock_identity: "continuous-recorder-test".into(),
                lifecycle,
                capture_readiness: RecorderReadiness::NotReady,
                processing_readiness: RecorderReadiness::Unknown,
                health: RecorderHealth::Degraded,
                current_epoch: None,
                previous_epoch: None,
                active_segment: None,
                commit: None,
                incremental_checkpoint: None,
                lag: crate::LagSummary {
                    records: 0,
                    bytes: 0,
                    age_seconds: 0,
                },
                quota: quotas
                    .iter()
                    .map(|quota| crate::QuotaStatus {
                        domain: quota.quota().domain.clone(),
                        quota_bytes: quota.quota().quota_bytes,
                        free_bytes: quota.available_bytes(),
                        reserved_bytes: quota.reserved_bytes().unwrap(),
                    })
                    .collect(),
                counters: crate::RecorderCounters::default(),
                recovery: Some(crate::RecoverySummary {
                    code: crate::MetadataCode::CrashRecovered,
                    repaired_tail: false,
                }),
                shutdown: None,
                failure: None,
                updated_at_unix_seconds: 0,
                checksum: String::new(),
            },
        )
        .map(|_| ())
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))
    }

    struct StartProbe(Arc<AtomicBool>);

    impl CaptureSource for StartProbe {
        fn start(&mut self) -> Result<(), CaptureError> {
            self.0.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn next_event(&mut self) -> Result<Option<chronicle_capture::CaptureEvent>, CaptureError> {
            Ok(None)
        }
    }

    #[test]
    fn pending_v2_continuation_recovers_predecessor_from_catalog() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-pending-continuation-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("epochs/1")).unwrap();
        let parent_id = RecordingId::new();
        let predecessor = EpochId::new();
        let successor = EpochId::new();
        let mut catalog = crate::EpochCatalog::new(parent_id, predecessor, "epochs/0").unwrap();
        catalog.append_successor(successor, "epochs/1").unwrap();
        catalog.activate_successor(successor).unwrap();
        catalog.write_atomic(&root).unwrap();
        ContinuationDependency::pending_for_parent(Some(parent_id), predecessor, successor)
            .write_atomic(root.join("epochs/1/continuation-state.json"))
            .unwrap();

        let pending = recover_pending_continuation(&root.join("epochs/1"), successor, None)
            .unwrap()
            .unwrap();
        assert_eq!(pending.old_epoch_id, predecessor);
        assert_eq!(pending.successor_epoch_id, successor);
        assert_eq!(pending.old_epoch_ordinal, 0);
        assert_eq!(pending.old_wal_directory, root.join("epochs/0"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v2_rollover_publishes_pending_continuation_before_successor_etl() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-v2-continuation-rollover-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal_root = Path::new(&config.domains[0].identity.canonical_root).join("wal");
        let old_recording_id = RecordingId::new();
        let writer =
            GroupCommitWalWriter::create(&wal_root, old_recording_id, DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config,
            &wal_root,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let parent_id = RecordingId::new();
        service.set_parent_id(parent_id).unwrap();
        for tick in 0..20 {
            service.poll(tick * 250).unwrap();
        }
        assert!(service.recorder().ingest().commit_boundary().is_some());

        let successor = EpochId::new();
        let next = wal_root.join("epochs").join(format!("1-{successor}"));
        fs::create_dir_all(&next).unwrap();
        service
            .reserve_successor_quota(&next, config.epoch.max_bytes)
            .unwrap();
        service
            .begin_rollover_transition(parent_id, &next, successor, config.epoch.max_bytes)
            .unwrap();
        let mut catalog =
            crate::EpochCatalog::new(parent_id, EpochId::from_uuid(old_recording_id.0), ".")
                .unwrap();
        let successor_path = format!("epochs/1-{successor}");
        catalog
            .append_successor(successor, &successor_path)
            .unwrap();
        catalog.write_atomic(&wal_root).unwrap();
        let next_writer = GroupCommitWalWriter::create(
            &next,
            RecordingId::from_uuid(successor.as_uuid()),
            DEFAULT_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let outcome = root.join("epoch-outcome.json");
        fs::write(&outcome, b"{}\n").unwrap();
        service
            .rollover_to(&next, next_writer, 10_000, &outcome)
            .unwrap();
        catalog.activate_successor(successor).unwrap();
        catalog.write_atomic(&wal_root).unwrap();
        service.complete_rollover_transition().unwrap();

        service.poll(10_250).unwrap();
        let dependency =
            ContinuationDependency::load(next.join(EPOCH_CONTINUATION_STATE_FILE)).unwrap();
        assert_eq!(dependency.state, ContinuationState::Ready);
        assert!(next.join(EPOCH_CONTINUATION_IN_FILE).exists());
        assert!(wal_root.join(EPOCH_CONTINUATION_OUT_FILE).exists());
        assert!(service.pending_continuation.is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollover_requires_prepared_transition_before_successor_mutation() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-journal-boundary-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config,
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let next = root.join("next");
        let next_writer =
            GroupCommitWalWriter::create(&next, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let result = service.rollover_to(&next, next_writer, 1, root.join("outcome.json"));
        assert!(
            matches!(result, Err(ContinuousRecorderError::Metadata(message)) if message.contains("transition missing"))
        );
        assert!(
            crate::load_transition(&config.state_root)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn quota_denial_leaves_no_rollover_journal_or_successor() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-quota-denial-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let service = ContinuousRecorderService::start(
            &config,
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        let successor = root.join("successor");
        let result = service.reserve_successor_quota(&successor, u64::MAX);
        assert!(result.is_err());
        assert!(!successor.exists());
        assert!(
            crate::load_transition(&config.state_root)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollover_failure_retains_prepared_evidence_for_restart_recovery() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-failure-boundary-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config,
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        for tick in 0..20 {
            service.poll(tick * 250).unwrap();
        }
        let old_metadata = fs::read(wal.join("recording.json")).unwrap();
        let parent_id = service.active_metadata.parent_id.unwrap();
        let next =
            std::path::PathBuf::from(&config.domains[0].identity.canonical_root).join("next");
        let next_epoch_id = EpochId::new();
        service
            .reserve_successor_quota(&next, config.epoch.max_bytes)
            .unwrap();
        service
            .begin_rollover_transition(parent_id, &next, next_epoch_id, config.epoch.max_bytes)
            .unwrap();
        let next_writer = GroupCommitWalWriter::create(
            &next,
            RecordingId::from_uuid(next_epoch_id.as_uuid()),
            DEFAULT_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let outcome_directory = root.join("outcome-directory");
        fs::create_dir_all(&outcome_directory).unwrap();
        let result = service.rollover_to(&next, next_writer, 1, &outcome_directory);
        assert!(result.is_err());
        assert_eq!(
            crate::load_transition(&config.state_root)
                .unwrap()
                .unwrap()
                .phase,
            crate::RolloverTransitionPhase::SuccessorCreated
        );
        assert!(next.exists());
        assert_eq!(fs::read(wal.join("recording.json")).unwrap(), old_metadata);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_startup_rejects_non_authoritative_epoch_before_capture() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-authoritative-epoch-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let recorder = RecorderOrchestrator::new(
            StartProbe(Arc::clone(&started)),
            crate::RecordingIngest::new(writer),
        );
        let recorder_id = uuid::Uuid::new_v4();
        let attempt_id = uuid::Uuid::new_v4();
        let startup = RecorderStartup::prepare_foundation_with_metadata(
            &config,
            |quotas, lifecycle| {
                persist_test_startup_metadata(&config, quotas, lifecycle, recorder_id, attempt_id)
            },
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
        let result = ContinuousRecorderService::start_with_prepared_startup(
            &config,
            &wal,
            1,
            recorder,
            startup,
            || Ok(()),
            || Ok(()),
        );
        assert!(matches!(
            result,
            Err(ContinuousRecorderError::Startup(
                RecorderStartupError::RecoveryDetail(_)
            ))
        ));
        assert!(!started.load(Ordering::SeqCst));
        let metadata = crate::load_recorder_metadata(&config.state_root).unwrap();
        assert_eq!(metadata.lifecycle, crate::RecorderLifecycleState::Failed);
        assert_eq!(metadata.capture_readiness, RecorderReadiness::NotReady);
        assert_eq!(metadata.processing_readiness, RecorderReadiness::NotReady);
        assert_eq!(metadata.failure, Some(crate::MetadataCode::RecoveryFailure));
        assert_eq!(metadata.recorder_id, recorder_id);
        assert_eq!(metadata.attempt_id, attempt_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_publication_faults_reuse_immutable_output_after_restart_boundary() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-publication-fault-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config(&root),
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        service.set_checkpoint_fault(chronicle_etl::CheckpointFault::BeforeCheckpoint);
        let output_dir = root.join("store/artifacts/deltas");
        let output_files = || {
            fs::read_dir(&output_dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .flat_map(|directory| fs::read_dir(directory.path()).into_iter().flatten())
                .filter_map(Result::ok)
                .filter_map(|entry| {
                    fs::read(entry.path())
                        .ok()
                        .map(|bytes| (entry.path(), bytes))
                })
                .collect::<Vec<_>>()
        };
        let mut orphaned = Vec::new();
        for tick in 0..40 {
            service.poll(tick * 250).unwrap();
            orphaned = output_files();
            if !orphaned.is_empty() && !wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists() {
                break;
            }
        }
        assert!(!orphaned.is_empty());
        assert!(!wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
        let recording_id = service.recorder().ingest().recording_id();
        drop(service);
        let writer = chronicle_wal::prepare_group_commit_reopen(
            &wal,
            recording_id,
            DEFAULT_SEGMENT_BYTES,
            chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            0,
        )
        .unwrap()
        .apply()
        .unwrap()
        .0;
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config(&root),
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        for tick in 40..80 {
            service.poll(tick * 250).unwrap();
        }
        assert!(wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
        let recovered = output_files();
        for (path, bytes) in orphaned {
            assert_eq!(
                recovered
                    .iter()
                    .find(|(candidate, _)| *candidate == path)
                    .map(|(_, value)| value),
                Some(&bytes)
            );
        }
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finalize_deduplicates_repeated_operations_after_restart_boundary() {
        // A restart replays the same capture source into the same epoch, so the
        // assembler holds identical requests twice. Deterministic operation ids
        // collide; finalization must deduplicate like incremental batches do.
        let root =
            std::env::temp_dir().join(format!("chronicle-finalize-dedup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let wal = root.join("wal");
        let writer =
            GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
                .unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config(&root),
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        for tick in 0..12 {
            service.poll(tick * 250).unwrap();
        }
        assert!(wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
        let recording_id = service.recorder().ingest().recording_id();
        drop(service);
        let writer = chronicle_wal::prepare_group_commit_reopen(
            &wal,
            recording_id,
            DEFAULT_SEGMENT_BYTES,
            chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            0,
        )
        .unwrap()
        .apply()
        .unwrap()
        .0;
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config(&root),
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        for tick in 12..24 {
            service.poll(tick * 250).unwrap();
        }
        // Rollover finalizes the epoch whose reconstruction contains the
        // replayed identical requests; deduplication must keep it publishable.
        service
            .rollover(6000, root.join("epoch-outcomes.json"))
            .unwrap();
        service.finalize_incremental_session().unwrap();
        let session = chronicle_storage::FilesystemSessionStore::new(config(&root).store_root)
            .hydrate(SessionId(recording_id.0))
            .unwrap();
        let ids = session
            .connections
            .iter()
            .flat_map(|connection| connection.operations.iter().map(|op| op.id))
            .collect::<Vec<_>>();
        assert_eq!(
            ids.len(),
            ids.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
        session.validate().unwrap();
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn one_shot_checkpoint_persists_manifest_checksum() {
        // Regression: the persisted recording-local checkpoint must carry the
        // real manifest checksum (publication + verification happen inside the
        // ETL transaction before the checkpoint bytes are produced), not the
        // pre-publication placeholder.
        let root = std::env::temp_dir().join(format!(
            "chronicle-one-shot-checkpoint-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let wal = root.join("wal");
        fs::create_dir_all(&wal).unwrap();
        crate::production_wal_from_fixture(
            &wal,
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../fixtures/http/basic-session.json"
            ),
        )
        .unwrap();
        let output = root.join("output");
        let registry = registry().unwrap();
        let published = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
        let checkpoint: crate::RecordingEtlCheckpoint =
            serde_json::from_slice(&fs::read(wal.join("etl-checkpoint.json")).unwrap()).unwrap();
        assert!(
            !checkpoint.manifest_checksum.is_empty(),
            "persisted checkpoint must carry the real manifest checksum"
        );
        let manifest: serde_json::Value = serde_json::from_slice(
            &fs::read(
                output
                    .join("sessions")
                    .join(published.session_id.to_string())
                    .join("manifest.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            checkpoint.manifest_checksum,
            manifest["session_checksum"].as_str().unwrap()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn three_epoch_fixture_has_one_etl_publication_and_replayable_output() {
        let root =
            std::env::temp_dir().join(format!("chronicle-three-epoch-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let wal = root.join("wal");
        let output = root.join("output");
        let recording_id = RecordingId::new();
        let writer =
            GroupCommitWalWriter::create(&wal, recording_id, DEFAULT_SEGMENT_BYTES, 1, 0).unwrap();
        let source = FixtureCaptureSource::from_json(include_bytes!(
            "../../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
        let mut service = ContinuousRecorderService::start(
            &config(&root),
            &wal,
            recorder,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
            || Ok(()),
        )
        .unwrap();
        assert_eq!(
            crate::load_recorder_metadata(config(&root).state_root)
                .unwrap()
                .lifecycle,
            crate::RecorderLifecycleState::Running
        );
        let mut accepted = 0;
        while accepted < 4 {
            if matches!(
                service.poll(accepted * 10).unwrap(),
                RecorderPoll::Accepted(_)
            ) {
                accepted += 1;
                if accepted == 1 || accepted == 2 {
                    service
                        .rollover(accepted * 10 + 1, root.join("epoch-outcomes.json"))
                        .unwrap();
                }
            }
        }
        let active = crate::load_recorder_metadata(config(&root).state_root).unwrap();
        assert_eq!(active.current_epoch.unwrap().ordinal, 2);
        assert_eq!(active.previous_epoch.unwrap().ordinal, 1);
        assert_eq!(active.counters.captured_records, 2);
        assert_eq!(active.counters.committed_records, 0);
        let result = service
            .shutdown(
                crate::ShutdownReason::SourceCompleted,
                100,
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result.status, crate::RecordingStatus::Completed);
        let snapshot = service.committed_snapshot().unwrap();
        assert!(snapshot.snapshot.committed_record_count > 0);
        assert_eq!(
            snapshot.envelopes.last().unwrap().sequence,
            snapshot.snapshot.marker_sequence
        );
        let stopped = crate::load_recorder_metadata(config(&root).state_root).unwrap();
        assert_eq!(stopped.lifecycle, crate::RecorderLifecycleState::Stopped);
        assert_eq!(
            stopped.counters.committed_records,
            result.counters.committed.records
        );
        let incremental_session = SessionId(service.recorder().ingest().recording_id().0);
        service.finalize_incremental_session().unwrap();
        assert!(
            chronicle_storage::FilesystemSessionStore::new(config(&root).store_root)
                .hydrate(incremental_session)
                .is_ok()
        );
        drop(service);
        let registry = registry().unwrap();
        let published = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
        let inspection = crate::inspect_session(&output, published.session_id).unwrap();
        assert!(inspection.integrity_valid);
        assert_eq!(
            inspection.replayability,
            chronicle_replay::Replayability::FullyReplayable
        );
        let repeated = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
        assert_eq!(published.session_id, repeated.session_id);
        assert!(repeated.already_published);
        assert!(root.join("epoch-outcomes.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
