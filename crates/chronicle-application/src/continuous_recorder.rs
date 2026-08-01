//! Shared continuous-recorder runtime around startup, capture, rollover, and drain.
#![allow(clippy::result_large_err)]

use crate::{
    NormalizedRecorderConfig, RecorderEpochBoundary, RecorderLifecycleError,
    RecorderOrchestrationError, RecorderOrchestrator, RecorderPoll, RecorderStartup,
    RecorderStartupError, RecordingMetadata, RecordingStatus, write_recording_metadata,
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
        let metadata = RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id: recorder.ingest().recording_id(),
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
        Ok(Self {
            startup,
            recorder,
            wal_directory,
            metadata,
        })
    }

    pub fn poll(&mut self, now_millis: u64) -> Result<RecorderPoll, ContinuousRecorderError> {
        Ok(self.recorder.poll(now_millis)?)
    }

    pub fn rollover(
        &mut self,
        now_millis: u64,
        outcome_path: impl AsRef<Path>,
    ) -> Result<RecorderEpochBoundary, ContinuousRecorderError> {
        Ok(self.recorder.rollover(now_millis, outcome_path)?)
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
        self.startup.stop()?;
        Ok(result)
    }

    pub fn startup(&self) -> &RecorderStartup {
        &self.startup
    }

    pub fn recorder(&self) -> &RecorderOrchestrator<S> {
        &self.recorder
    }
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
        let result = service
            .shutdown(
                crate::ShutdownReason::SourceCompleted,
                100,
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result.status, crate::RecordingStatus::Completed);
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
