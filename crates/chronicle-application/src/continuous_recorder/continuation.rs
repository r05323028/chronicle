use super::{
    CaptureSource, ContinuationDependency, ContinuousRecorderError, ContinuousRecorderService,
    EPOCH_CONTINUATION_IN_FILE, EPOCH_CONTINUATION_STATE_FILE, ETL_PIPELINE_VERSION,
    EpochContinuationCheckpoint, IncrementalProcessor, RecorderReadiness, RecordingId, fs,
    unix_seconds, write_recorder_metadata,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
    pub(super) fn complete_pending_continuation(&mut self) -> Result<(), ContinuousRecorderError> {
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
        self.next_incremental_millis = 0;
        Ok(())
    }
}
