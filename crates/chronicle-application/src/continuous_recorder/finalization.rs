use super::{
    CaptureSource, CommittedWalSnapshot, ContinuousRecorderError, ContinuousRecorderService,
    Duration, EPOCH_CONTINUATION_IN_FILE, EPOCH_CONTINUATION_OUT_FILE, EpochId,
    FilesystemSessionStore, MetadataCode, Path, RecorderHealth, RecorderOrchestrator,
    RecorderReadiness, RecorderStartup, RecordingId, ReservationKind, SessionId,
    stamp_epoch_operation_provenance, unix_seconds, write_recorder_metadata,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
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
