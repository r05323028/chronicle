use super::{
    CaptureSource, CheckpointFault, ContinuousRecorderError, ContinuousRecorderService,
    MetadataCode, Path, QuotaStatus, RecorderHealth, RecorderReadiness, RecordingCaptureMetadata,
    RecordingId, ReservationKind, unix_seconds, write_recorder_metadata, write_recording_metadata,
};

impl<S: CaptureSource> ContinuousRecorderService<S> {
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

    pub(super) fn refresh_quota_metadata(&mut self) -> Result<(), ContinuousRecorderError> {
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

    pub(super) fn sync_active_progress(&mut self) {
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
}
