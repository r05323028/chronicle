use super::{
    CanonicalDeltaBatchV1, CaptureSource, CheckpointLifecycle, CheckpointOwner,
    CommittedWalSnapshot, ContinuationDependency, ContinuationState, ContinuousRecorderError,
    ContinuousRecorderService, DeltaBatchReference, Digest, EPOCH_CONTINUATION_STATE_FILE,
    ETL_PIPELINE_VERSION, EpochId, FilesystemRecordingStore, INCREMENTAL_CHECKPOINT_SCHEMA_VERSION,
    INCREMENTAL_ETL_CHECKPOINT_FILE, IncrementalEtlCheckpoint, IncrementalResult, LagSummary,
    MarkerLineage, MetadataCode, Path, RecorderHealth, RecorderPoll, RecorderReadiness,
    RecordingArtifact, RecordingArtifactKind, ReservationKind, Sha256, SourceStatus, digest_hex,
    publish_delta_then_checkpoint, publish_delta_then_checkpoint_with_fault,
    stamp_epoch_operation_provenance, unix_seconds, write_recorder_metadata,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
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
        let in_backoff = self
            .incremental_worker
            .retry_not_before()
            .is_some_and(|deadline| now_millis < deadline);
        if now_millis < self.next_incremental_millis && !in_backoff {
            return Ok(());
        }
        if !in_backoff {
            self.next_incremental_millis = now_millis.saturating_add(250);
        }
        let recording_id = self.recorder.ingest().recording_id();
        let checkpoint = self.incremental_checkpoint.clone();
        let snapshot = match chronicle_wal::read_committed_snapshot_with_records(
            &self.wal_directory,
            recording_id,
            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
        ) {
            Ok(snapshot) => snapshot,
            Err(chronicle_wal::WalError::NoPublishedSegments) if checkpoint.is_none() => {
                self.incremental_worker.record_success(0);
                self.set_incremental_readiness(LagSummary {
                    records: 0,
                    bytes: 0,
                    age_seconds: 0,
                });
                return self.persist_incremental_metadata();
            }
            Err(error) => {
                self.record_incremental_failure(now_millis);
                eprintln!("incremental snapshot failed: {error}");
                return self.persist_incremental_metadata();
            }
        };

        if let Some(checkpoint) = checkpoint.as_ref()
            && let Err(error) = crate::incremental_runtime::validate_checkpoint(
                &snapshot,
                checkpoint,
                &self.wal_directory,
                recording_id,
            )
        {
            self.record_incremental_failure(now_millis);
            eprintln!("incremental checkpoint validation failed: {error}");
            return self.persist_incremental_metadata();
        }

        let lag_before = match crate::incremental_runtime::lag_summary(
            &snapshot,
            checkpoint.as_ref(),
            now_millis,
        ) {
            Ok(lag) => lag,
            Err(error) => {
                self.record_incremental_failure(now_millis);
                eprintln!("incremental lag calculation failed: {error}");
                return self.persist_incremental_metadata();
            }
        };
        self.active_metadata.lag = lag_before.clone();
        self.incremental_worker.update_lag(lag_before.records);
        if !self
            .incremental_worker
            .begin_attempt(now_millis)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
        {
            self.set_incremental_readiness(lag_before);
            return self.persist_incremental_metadata();
        }

        let planned = match crate::incremental_runtime::plan_next_batch(
            &snapshot,
            checkpoint.as_ref(),
            self.etl_batch_records,
            now_millis,
        ) {
            Ok(planned) => planned,
            Err(error) => {
                self.record_incremental_failure(now_millis);
                eprintln!("incremental batch planning failed: {error}");
                return self.persist_incremental_metadata();
            }
        };
        let Some(planned) = planned else {
            self.incremental_worker.record_success(lag_before.records);
            self.set_incremental_readiness(lag_before);
            return self.persist_incremental_metadata();
        };

        let previous_processor_state = self.incremental_processor.state_snapshot();
        let result = match self.incremental_processor.process_snapshot(
            CommittedWalSnapshot {
                marker_sequence: planned.marker_sequence,
                marker_digest: planned.marker_digest,
                envelopes: &planned.envelopes,
                segment_lineage: &planned.segment_lineage,
            },
            &self.incremental_registry,
            self.incremental_session_id,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.incremental_processor
                    .restore_state(previous_processor_state);
                self.record_incremental_failure(now_millis);
                eprintln!("incremental processor snapshot failed: {error}");
                self.set_incremental_readiness(planned.lag_before);
                return self.persist_incremental_metadata();
            }
        };
        let active_segment_digest = match chronicle_wal::verified_segment_prefix_sha256(
            &self.wal_directory,
            recording_id,
            &snapshot.envelopes,
            planned.marker_segment_ordinal,
            planned.marker_sequence,
        ) {
            Ok(digest) => digest,
            Err(error) => {
                self.incremental_processor
                    .restore_state(previous_processor_state);
                self.record_incremental_failure(now_millis);
                eprintln!("incremental snapshot segment prefix failed: {error}");
                self.set_incremental_readiness(planned.lag_before);
                return self.persist_incremental_metadata();
            }
        };
        if let Err(error) = self.publish_incremental_result(
            &result,
            planned.marker_sequence,
            planned.marker_segment_ordinal,
            planned.marker_digest,
            active_segment_digest,
        ) {
            self.incremental_processor
                .restore_state(previous_processor_state);
            self.record_incremental_failure(now_millis);
            eprintln!("incremental publication failed: {error}");
            self.set_incremental_readiness(planned.lag_before);
            return self.persist_incremental_metadata();
        }

        self.incremental_worker
            .record_success(planned.lag_after.records);
        self.set_incremental_readiness(planned.lag_after);
        self.persist_incremental_metadata()
    }

    fn record_incremental_failure(&mut self, now_millis: u64) {
        let in_backoff = self
            .incremental_worker
            .retry_not_before()
            .is_some_and(|deadline| now_millis < deadline);
        if !in_backoff {
            self.incremental_worker.record_failure(now_millis);
        }
        self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
        if !matches!(
            self.active_metadata.failure,
            Some(MetadataCode::QuotaPressure | MetadataCode::TerminalQuotaPressure)
        ) {
            self.active_metadata.health = RecorderHealth::Degraded;
            self.active_metadata.failure = Some(MetadataCode::StorageFailure);
        }
    }

    fn set_incremental_readiness(&mut self, lag: LagSummary) {
        self.active_metadata.lag = lag;
        let quota_failure = matches!(
            self.active_metadata.failure,
            Some(MetadataCode::QuotaPressure | MetadataCode::TerminalQuotaPressure)
        );
        let ready = self.incremental_worker.processing_ready()
            && self.pending_continuation.is_none()
            && !quota_failure;
        if ready {
            self.active_metadata.processing_readiness = RecorderReadiness::Ready;
            self.active_metadata.health = RecorderHealth::Healthy;
            self.active_metadata.failure = None;
        } else {
            self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
            if self.active_metadata.failure != Some(MetadataCode::TerminalQuotaPressure) {
                self.active_metadata.health = RecorderHealth::Degraded;
            }
        }
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
    }

    fn persist_incremental_metadata(&mut self) -> Result<(), ContinuousRecorderError> {
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
}
