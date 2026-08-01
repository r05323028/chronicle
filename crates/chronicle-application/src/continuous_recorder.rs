//! Shared continuous-recorder runtime around startup, capture, rollover, and drain.
#![allow(clippy::result_large_err)]

use crate::{
    EpochMetadata, LagSummary, MetadataCode, NormalizedRecorderConfig, QuotaStatus,
    RecorderCounters, RecorderEpochBoundary, RecorderHealth, RecorderLifecycleError,
    RecorderMetadataV1, RecorderOrchestrationError, RecorderOrchestrator, RecorderPoll,
    RecorderReadiness, RecorderStartup, RecorderStartupError, RecordingCaptureMetadata,
    RecordingMetadata, RecordingStatus, write_recorder_metadata, write_recording_metadata,
};
use chronicle_capture::{CaptureError, CaptureSource};
use chronicle_wal::WalError;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

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
    #[error("recorder metadata persistence failed: {0}")]
    Metadata(String),
}

/// Foreground runtime. Owns startup lease and capture source for entire epoch sequence.
pub struct ContinuousRecorderService<S> {
    startup: RecorderStartup,
    recorder: RecorderOrchestrator<S>,
    wal_directory: std::path::PathBuf,
    metadata: RecordingMetadata,
    active_metadata: RecorderMetadataV1,
    state_root: std::path::PathBuf,
}

impl<S: CaptureSource> ContinuousRecorderService<S> {
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        config: &NormalizedRecorderConfig,
        wal_directory: impl AsRef<Path>,
        recorder: RecorderOrchestrator<S>,
        recover_foundation: impl FnOnce() -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(
            &[crate::QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, ContinuousRecorderError> {
        let mut recorder = recorder;
        let startup = RecorderStartup::start_post_foundation(
            config,
            recover_foundation,
            reserve_quota,
            establish_appendable,
            || {
                recorder
                    .start()
                    .map_err(|_| RecorderStartupError::CaptureAttach)
            },
            resume_etl,
        )?;
        let wal_directory = wal_directory.as_ref().to_path_buf();
        let recording_id = recorder.ingest().recording_id();
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
                ordinal: 0,
                recording_id: recording_id.0,
            }),
            previous_epoch: None,
            active_segment: None,
            commit: None,
            incremental_checkpoint: None,
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
                    free_bytes: authority.quota().free_bytes,
                    reserved_bytes: 0,
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
        Ok(Self {
            startup,
            recorder,
            wal_directory,
            metadata,
            active_metadata,
            state_root: config.state_root.clone(),
        })
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

    pub fn poll(&mut self, now_millis: u64) -> Result<RecorderPoll, ContinuousRecorderError> {
        Ok(self.recorder.poll(now_millis)?)
    }

    fn sync_active_progress(&mut self) {
        let counters = self.recorder.ingest().counters();
        self.active_metadata.counters.captured_records = counters.accepted_into_queue.records;
        self.active_metadata.counters.captured_bytes = counters.accepted_into_queue.bytes;
        self.active_metadata.counters.committed_records = counters.committed.records;
        self.active_metadata.counters.committed_bytes = counters.committed.bytes;
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
    }

    pub fn rollover(
        &mut self,
        now_millis: u64,
        outcome_path: impl AsRef<Path>,
    ) -> Result<RecorderEpochBoundary, ContinuousRecorderError> {
        let boundary = self.recorder.rollover(now_millis, outcome_path)?;
        self.sync_active_progress();
        let recording_id = self.recorder.ingest().recording_id().0;
        self.active_metadata.previous_epoch = Some(EpochMetadata {
            ordinal: boundary.old_epoch_ordinal,
            recording_id,
        });
        self.active_metadata.current_epoch = Some(EpochMetadata {
            ordinal: boundary.new_epoch_ordinal,
            recording_id,
        });
        self.active_metadata.updated_at_unix_seconds = unix_seconds();
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(boundary)
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
    use chronicle_common::RecordingId;
    use chronicle_protocol_builtins::registry;
    use chronicle_wal::{DEFAULT_SEGMENT_BYTES, GroupCommitWalWriter};
    use std::fs;

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
            || Ok(()),
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
        drop(service);
        let registry = registry().unwrap();
        let published = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
        let inspection = crate::inspect_session(&output, published.session_id).unwrap();
        assert!(inspection.integrity_valid);
        assert!(inspection.replayable);
        let repeated = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
        assert_eq!(published.session_id, repeated.session_id);
        assert!(repeated.already_published);
        assert!(root.join("epoch-outcomes.json").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
