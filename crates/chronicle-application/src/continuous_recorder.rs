//! Shared continuous-recorder runtime around startup, capture, rollover, and drain.
#![allow(clippy::result_large_err)]

use crate::{
    EpochMetadata, IncrementalWorker, IncrementalWorkerError, IncrementalWorkerPolicy, LagSummary,
    MetadataCode, NormalizedRecorderConfig, QuotaStatus, RecorderCounters, RecorderEpochBoundary,
    RecorderHealth, RecorderLifecycleError, RecorderMetadataV1, RecorderOrchestrationError,
    RecorderOrchestrator, RecorderPoll, RecorderReadiness, RecorderStartup, RecorderStartupError,
    RecordingCaptureMetadata, RecordingMetadata, RecordingStatus, write_recorder_metadata,
    write_recording_metadata,
};
use chronicle_capture::{CaptureError, CaptureSource};
use chronicle_common::SessionId;
use chronicle_etl::{
    CanonicalDeltaBatchV1, CheckpointLifecycle, CheckpointOwner, CommittedWalSnapshot,
    DecoderReconstructionState, DeltaBatchReference, ETL_PIPELINE_VERSION,
    INCREMENTAL_CHECKPOINT_SCHEMA_VERSION, IncrementalEtlCheckpointV1, IncrementalProcessor,
    IncrementalResult, MarkerLineage, SourceStatus, publish_delta_then_checkpoint, read_checkpoint,
};
use chronicle_protocol::ProtocolRegistry;
use chronicle_protocol_builtins::registry;
use chronicle_storage::{FilesystemRecordingStore, RecordingArtifact, RecordingArtifactKind};
use chronicle_wal::WalError;
use std::fs;
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
    #[error("incremental ETL failed: {0}")]
    Incremental(String),
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
    incremental_checkpoint: Option<IncrementalEtlCheckpointV1>,
    next_incremental_millis: u64,
}

impl<S: CaptureSource> ContinuousRecorderService<S> {
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
        let mut incremental_worker = IncrementalWorker::new(IncrementalWorkerPolicy {
            max_lag_records: config.etl.max_lag_records,
            retry_attempts: config.etl.retry_attempts,
            backoff_millis: 10,
        })
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        incremental_worker.start();
        let incremental_registry =
            registry().map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
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
        let checkpoint_path = wal_directory.join("incremental-etl-checkpoint.json");
        let incremental_checkpoint = match fs::metadata(&checkpoint_path) {
            Ok(_) => Some(
                read_checkpoint(&checkpoint_path)
                    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(ContinuousRecorderError::Incremental(error.to_string()));
            }
        };
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
        let mut incremental_processor =
            IncrementalProcessor::new(chronicle_session::SessionLimits::default());
        if let Some(checkpoint) = &incremental_checkpoint {
            incremental_processor.restore_marker_sequence(checkpoint.marker.sequence);
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
            incremental_session_id: SessionId(recording_id.0),
            incremental_checkpoint,
            next_incremental_millis: 0,
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
        let poll = self.recorder.poll(now_millis)?;
        self.poll_incremental(now_millis)?;
        Ok(poll)
    }

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
        let mut incremental_result: Option<IncrementalResult> = None;
        let mut marker = None;
        let mut previous_marker = None;
        let worker_result = self.incremental_worker.process_once(|| {
            let snapshot = chronicle_wal::read_committed_snapshot_with_records(
                &wal_directory,
                recording_id,
                chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
            )
            .map_err(|_| IncrementalWorkerError::BatchFailed)?;
            if let Some(checkpoint) = &checkpoint
                && (checkpoint.recording_id != recording_id.0
                    || checkpoint.marker.sequence > snapshot.snapshot.marker_sequence
                    || (checkpoint.marker.sequence == snapshot.snapshot.marker_sequence
                        && checkpoint.marker.marker_digest
                            != digest_hex(&snapshot.snapshot.marker_digest)))
            {
                return Err(IncrementalWorkerError::BatchFailed);
            }
            if processor.last_marker_sequence() == Some(snapshot.snapshot.marker_sequence) {
                return Ok(0);
            }
            previous_marker = processor.last_marker_sequence();
            let result = processor
                .process_snapshot(
                    CommittedWalSnapshot {
                        marker_sequence: snapshot.snapshot.marker_sequence,
                        envelopes: &snapshot.envelopes,
                    },
                    protocol_registry,
                    session_id,
                )
                .map_err(|_| IncrementalWorkerError::BatchFailed)?;
            marker = Some((
                snapshot.snapshot.marker_sequence,
                snapshot.snapshot.marker_segment_ordinal,
                snapshot.snapshot.marker_digest,
            ));
            incremental_result = Some(result);
            Ok(0)
        });
        match worker_result {
            Ok(()) => {
                let mut published = true;
                if let (Some(result), Some((marker_sequence, segment_ordinal, marker_digest))) =
                    (incremental_result, marker)
                    && let Err(error) = self.publish_incremental_result(
                        &result,
                        marker_sequence,
                        segment_ordinal,
                        marker_digest,
                    )
                {
                    match previous_marker {
                        Some(marker_sequence) => {
                            self.incremental_processor
                                .restore_marker_sequence(marker_sequence);
                        }
                        None => self.incremental_processor.clear_marker_sequence(),
                    }
                    published = false;
                    let _ = error;
                }
                if published {
                    self.active_metadata.processing_readiness = RecorderReadiness::Ready;
                    self.active_metadata.health = RecorderHealth::Healthy;
                    self.active_metadata.failure = None;
                } else {
                    self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                    self.active_metadata.health = RecorderHealth::Degraded;
                    self.active_metadata.failure = Some(MetadataCode::StorageFailure);
                }
                self.active_metadata.lag.records = 0;
                self.active_metadata.lag.age_seconds = 0;
                self.active_metadata.lag.bytes = 0;
            }
            Err(_error) => {
                self.active_metadata.processing_readiness = RecorderReadiness::NotReady;
                self.active_metadata.health = RecorderHealth::Degraded;
                self.active_metadata.failure = Some(MetadataCode::StorageFailure);
            }
        }
        write_recorder_metadata(&self.state_root, &self.active_metadata)
            .map_err(|error| ContinuousRecorderError::Metadata(error.to_string()))?;
        Ok(())
    }

    fn publish_incremental_result(
        &mut self,
        result: &IncrementalResult,
        marker_sequence: u64,
        segment_ordinal: u64,
        marker_digest: [u8; 32],
    ) -> Result<(), ContinuousRecorderError> {
        let operations = result
            .output
            .session
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
        let recording_id = self.recorder.ingest().recording_id();
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
        let checkpoint = IncrementalEtlCheckpointV1 {
            version: INCREMENTAL_CHECKPOINT_SCHEMA_VERSION,
            owner: CheckpointOwner::Recorder,
            lifecycle: CheckpointLifecycle::Active,
            recording_id: recording_id.0,
            epoch_ordinal: self
                .active_metadata
                .current_epoch
                .as_ref()
                .map_or(0, |epoch| epoch.ordinal),
            config_digest: self.active_metadata.config_digest.clone(),
            pipeline_version: ETL_PIPELINE_VERSION.into(),
            canonical_schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
            marker: MarkerLineage {
                sequence: marker_sequence,
                segment_ordinal,
                marker_digest: digest_hex(&marker_digest),
            },
            segment_lineage: Vec::new(),
            predecessor_digest: self
                .incremental_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checksum.clone()),
            decoder: DecoderReconstructionState {
                connections: Vec::new(),
                serialized_bytes: 0,
            },
            outputs,
            source_status: SourceStatus::Live,
            checksum: String::new(),
        };
        let store = FilesystemRecordingStore::new(self.store_root.clone());
        publish_delta_then_checkpoint(
            &store,
            key,
            &batch,
            &checkpoint,
            self.wal_directory.join("incremental-etl-checkpoint.json"),
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        let checkpoint =
            read_checkpoint(self.wal_directory.join("incremental-etl-checkpoint.json"))
                .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        self.active_metadata.incremental_checkpoint = Some(crate::IncrementalCheckpointSummaryV1 {
            marker_sequence: checkpoint.marker.sequence,
            checkpoint_digest: checkpoint.checksum.clone(),
            output_count: checkpoint.outputs.len() as u64,
        });
        self.incremental_checkpoint = Some(checkpoint);
        Ok(())
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

fn digest_hex(value: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
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
