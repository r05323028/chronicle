use super::{
    CaptureSource, CommittedWalSnapshot, ContinuationDependency, ContinuationLimits,
    ContinuousRecorderError, ContinuousRecorderService, Digest, EPOCH_CONTINUATION_IN_FILE,
    EPOCH_CONTINUATION_OUT_FILE, EPOCH_CONTINUATION_STATE_FILE, ETL_PIPELINE_VERSION,
    EpochContinuationCheckpoint, EpochId, EpochMetadata, FilesystemRecordingStore,
    GroupCommitWalWriter, IncrementalProcessor, MarkerLineage, Path, PendingContinuation,
    ProtocolRegistry, RecorderEpochBoundary, RecorderReadiness, RecordingArtifact,
    RecordingArtifactKind, RecordingId, RecordingMetadata, RecordingStatus, RecordingStore,
    ReservationKind, RolloverTransition, RolloverTransitionPhase, SegmentLineage, SessionId,
    Sha256, digest_hex, fs, load_transition, unix_seconds, write_recorder_metadata,
    write_recording_metadata, write_transition_atomic,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
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
    pub(super) fn export_continuation(
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

    pub(super) fn publish_continuation(
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
}
