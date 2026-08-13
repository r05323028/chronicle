//! Application services and configuration. CLI remains an outer adapter.

#[allow(unsafe_code)] // bootstrap credential hardening uses raw descriptor and sigaction syscalls
mod bootstrap;
mod cgroup_selection;
#[allow(unsafe_code)] // bootstrap spawn uses pre_exec descriptor manipulation
mod command_record;
mod command_replay;
mod continuous_recorder;
mod data_dir;
mod doctor;
pub use doctor::{
    DoctorOptions, DoctorProbe, DoctorReport, DoctorStatus, ProductionRecordingBounds,
    ProductionSignalStop, aggregate_doctor_status, doctor_exit_code, doctor_report,
    replay_config_doctor_probe,
};
mod domain_lock;
mod record;
pub use record::{
    AppConfig, ApplicationCommand, CaptureConfig, INGEST_QUEUE_CAPACITY, IngestAdmission,
    PostgresConfig, ProductionRecordingResult, ProductionWalPreflight, ProtocolOverride,
    QueuedWalRecord, RecordByteCount, RecordingBounds, RecordingBuildIdentity,
    RecordingCaptureError, RecordingCaptureErrorCode, RecordingCaptureMetadata,
    RecordingCommitBoundary, RecordingCounters, RecordingHostIdentity, RecordingIngest,
    RecordingIngestFailure, RecordingIngestResult, RecordingMetadata, RecordingMetadataWriteFault,
    RecordingSelectorIdentity, RecordingSelectorScope, RecordingStatus, RecordingStorePreflight,
    RedactionConfig, ReplayConfig, ReplayTiming, S3Config, ShutdownReason, TargetMappingConfig,
    TerminalWalLossPersistence, TerminalWalLossSummary, TerminalWalLossSummaryEntry,
    TerminalWalLossTimeSource, WalConfig, preflight_production_record, preflight_recording_store,
    preflight_wal_destination, validate_production_recording_bounds,
};
#[cfg(test)]
pub(crate) use record::{
    LIFECYCLE_INDEX_FILE, apply_retention_cleanup, compact_lifecycle_index, load_lifecycle_index,
    mark_epoch_cleanup_complete, recover_rollover_transition_for_root, save_lifecycle_index,
};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) use record::{live_capture_metadata, monotonic_millis, preflight_embedded_ebpf};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub use record::{record_continuous_ebpf, record_live_ebpf};
mod epoch_catalog;
mod etl;
pub use etl::{
    PostMarkerUncertainty, PublishedRecordingResult, RECORDING_ETL_CHECKPOINT_VERSION, RecordedWal,
    RecordingEtlCheckpoint, RecordingEtlResult, RecordingRecoveryReconciliation,
    RecoveredRecordingForAppend, RecoveryIssueCode, RecoveryReport, RecoveryTailRepairSummary,
    build_recovery_report, build_recovery_report_for_reopen, decode_recording_metadata,
    decode_recovery_report, load_recording_metadata, mark_recording_forced_termination,
    persist_recovery_report, process_and_publish_recording_wal, process_fixture_wal,
    process_recording_wal, reconcile_recording_metadata, reconcile_recording_metadata_with_scan,
    record_production, recording_physical_wal_bytes, recover_recording_for_append,
    recovery_issue_code, render_recovery_report_json, write_capture_to_wal,
    write_recording_metadata,
};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) use etl::{
    has_published_wal, load_production_ebpf_source, process_and_publish_recording_wal_owned,
};
#[cfg(test)]
pub(crate) use etl::{
    load_recording_etl_checkpoint, metadata_terminal_wal_losses, recording_failure_reason,
    reserve_publication_peak, shutdown_reason_name, write_recording_metadata_inner,
};
pub(crate) use etl::{sha256_string, validate_recording_metadata, write_private_atomic_json};
mod epoch_rollover;
mod incremental_worker;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod listener_discovery;
mod recorder_config;
mod replay_inspect;
pub use chronicle_common::{RecordingId, SessionId, Timestamp, escape_control};
pub use chronicle_protocol::{ProtocolError, TransportErrorCategory};
pub use chronicle_replay::{
    LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayOutcome, Replayability,
    TimingMode,
};
pub use replay_inspect::{
    InspectConnectionSummary, InspectOperationSummary, InspectSessionResult, RecordFixtureResult,
    RecordIssueSummary, ReplayCounts, ReplayOperationSummary, ReplaySessionResult, inspect_session,
    preplan_command_replay_session, production_wal_from_fixture, record_fixture,
    record_fixture_file, render_inspect_human, render_inspect_json, render_json,
    render_replay_human, render_replay_json, replay_command_inputs, replay_context, replay_session,
    replay_session_with_plan, replay_session_with_plan_guard,
};
#[cfg(test)]
pub(crate) use replay_inspect::{MAX_RECORD_ISSUES, replay_context_from};
pub(crate) use replay_inspect::{
    command_replay_plan, hydrate_replay_session, issue_summaries, replay_plan_result,
    replayability_reasons, session_complete,
};
mod recorder_lease;
mod recorder_lifecycle;
mod recorder_metadata;
mod recorder_orchestration;
mod recorder_quota;
mod recorder_startup;
mod recorder_status;
mod recording_catalog;
mod rollover_transition;
mod supervised_scope;

pub use bootstrap::{BOOTSTRAP_FAILURE_EXIT, run_bootstrap};
pub use cgroup_selection::{
    CgroupSelection, CgroupSelectionError, CgroupSelector, PidCgroupSelection,
    preflight_cgroup_selection, preflight_pid_cgroup_selection,
};
pub use command_record::{
    COMMAND_RECORD_ATTACH_DEADLINE, COMMAND_RECORD_SCOPE_PREFIX, ChildStdio, CommandRecordOptions,
    CommandRecordResult, record_command, record_selector, retry_recording,
};
pub use command_replay::{
    COMMAND_REPLAY_SCOPE_PREFIX, CommandReplayOptions, CommandReplayResult, replay_command,
};
pub use continuous_recorder::{ContinuousRecorderError, ContinuousRecorderService};
pub use data_dir::{
    DataDirSource, ResolvedDataDir, data_dir_doctor_probe, ensure_private_data_dir,
    resolve_data_dir,
};
pub use domain_lock::{
    DOMAIN_LOCK_FILE, DomainLockError, DomainLockGuard, acquire_domain_lock, domain_lock_held,
    resolve_domain_lock_path,
};
pub use epoch_catalog::{
    EPOCH_CATALOG_FILE, EpochCatalogEntry, EpochCatalogError, EpochCatalogState, EpochCatalogV1,
};
pub use epoch_rollover::{
    EpochAdmittedObservation, EpochOutcomeError, EpochOutcomeJournalV1, OutcomeRange,
    RolloverAssignment, RolloverFailureReason, write_epoch_outcome_atomic,
};
pub use incremental_worker::{
    IncrementalWorker, IncrementalWorkerCounters, IncrementalWorkerError, IncrementalWorkerPolicy,
    IncrementalWorkerState,
};
pub use recorder_config::{
    EpochConfig, EtlConfig, FilesystemDomainConfig, FilesystemDomainIdentity, LogLevel,
    LoggingConfig, NormalizedRecorderConfig, NormalizedScopeConfig, RECORDER_CONFIG_SCHEMA_VERSION,
    RecorderConfigError, RecorderConfigV1, RecorderScopeConfig, ResolvedFilesystemDomain,
    RetentionMode, SegmentConfig, ShutdownConfig, StoreBackend, StoreConfig,
};
pub use recorder_lease::{RecorderLease, RecorderLeaseError};
pub use recorder_lifecycle::{
    RecorderHealth, RecorderLifecycle, RecorderLifecycleError, RecorderLifecycleState,
    RecorderReadiness, RecorderReadinessState,
};
pub use recorder_metadata::{
    CommitWatermark, EpochMetadata, IncrementalCheckpointSummaryV1, LagSummary, MetadataCode,
    QuotaStatus, RECORDER_METADATA_FILE, RECORDER_METADATA_SCHEMA_VERSION, RecorderCounters,
    RecorderMetadataError, RecorderMetadataV1, RecoverySummary, load_recorder_metadata,
    write_recorder_metadata,
};
pub use recorder_orchestration::{
    AcceptedObservation, ContinuousRecorder, RecorderEpochBoundary, RecorderOrchestrationError,
    RecorderOrchestrator, RecorderPoll,
};
pub use recorder_quota::{
    CapacityProvider, DomainQuota, QUOTA_ENFORCEMENT_ENABLED, QuotaError, QuotaPressureKind,
    QuotaReservation, QuotaReservationAuthority, ReservationKind, StatvfsCapacityProvider,
    classify_quota_pressure,
};
pub use recorder_startup::{RecorderStartup, RecorderStartupError};
pub use recorder_status::{
    RECORDER_STATUS_SCHEMA_VERSION, RecorderStatusError, RecorderStatusState, RecorderStatusV1,
    SegmentCounts, StatusRemediation,
};
pub use recording_catalog::ListedRecording;
pub use recording_catalog::{
    CATALOG_FILE, CATALOG_MAX_BYTES, CATALOG_MAX_ENTRIES, CatalogEntryV1, CatalogV1,
    ChildExitResult, RECORDING_INTENT_FILE, RECORDING_INTENT_MAX_BYTES, RECORDINGS_SUBDIR,
    RecordingCatalogStatus, RecordingIntentV1, catalog_path, claim_recording_name,
    list_recording_ids, list_recordings, load_catalog, load_recording_intent,
    persist_reconciled_catalog, reconcile_catalog, reconcile_entry_status, recording_intent_path,
    recordings_root, resolve_recording, resolve_session, save_catalog, validate_catalog,
    validate_recording_name, write_recording_intent,
};
pub use rollover_transition::{
    ROLLOVER_TRANSITION_FILE, ROLLOVER_TRANSITION_SCHEMA_VERSION, RolloverTransitionError,
    RolloverTransitionPhase, RolloverTransitionV1, load_transition, remove_transition,
    write_transition_atomic,
};
pub use supervised_scope::{
    CleanupOutcome, RealClock, SCOPE_CLEANUP_ABSOLUTE, SCOPE_KILL_WAIT, SCOPE_POLL_INTERVAL,
    SCOPE_TERM_GRACE, ScopeClock, ScopeError, ScopeIdentity, SupervisedScope,
    create_supervised_scope, discover_delegated_root, open_scope, preflight_scope_access,
};

use chronicle_canonical::{
    CommitMarkerProvenance, Completeness, ProvenanceEntry, ProvenanceKind, SourceProvenance,
    SourceStatus,
};
use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureError, CaptureEvent, CaptureEventKind, CaptureSource,
    CaptureSourceSummary, ClockIdentity, FixtureCaptureSource, MonotonicTimestamp, encode_event,
};

use chronicle_etl::{
    ETL_PIPELINE_VERSION, EtlError, EtlIssue, EtlOutput, EtlPipeline,
    assign_recording_ids_with_snapshot,
};
use chronicle_protocol::{ProtocolRegistry, ReplayContext, SecretBytes};
use chronicle_replay::{
    ReplayExecutor, ReplayPlan, ReplayPlanner, ReplayPolicy, ReplayTargetError, TargetMap,
    TargetRule,
};
use chronicle_session::SessionLimits;
use chronicle_storage::{FilesystemSessionStore, PublishSession, StorageError};
use chronicle_wal::{
    COMMIT_MARKER_FRAME_LEN, DEFAULT_MAX_RECORD_BYTES, DEFAULT_MAX_WAL_BYTES,
    DEFAULT_SEGMENT_BYTES, GroupCommitWalWriter, LifecycleEpochEntry, LifecycleIndexV1,
    MAX_SEGMENT_BYTES, MIN_SEGMENT_BYTES, RecordKind, RecordingLock, RecoveryReopenPreview,
    RecoveryReopenReport, RecoveryRepair, RecoveryScan, SEGMENT_HEADER_LEN,
    TERMINAL_WAL_LOSS_SCHEMA_VERSION, TerminalWalLoss, TerminalWalLossAmbiguity,
    TerminalWalLossInterval, TerminalWalLossReason, WalCheckpoint, WalError, WalRecordEnvelope,
    cleanup_finalized_segment_verified, encode_envelope, encode_terminal_wal_loss,
    prepare_group_commit_reopen_from_scan, scan_wal, verified_snapshot_sha256,
};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use chronicle_wal::{prepare_group_commit_reopen, recover_cleanup};
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{CWD, RenameFlags, renameat_with};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::time::{Duration, SystemTime};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use std::time::{Instant, UNIX_EPOCH};
use thiserror::Error;

pub const RECORDING_METADATA_SCHEMA_VERSION: u16 = 1;
pub const RECORDING_COUNTERS_SCHEMA_VERSION: u16 = 1;
pub const RECORDING_CAPTURE_METADATA_SCHEMA_VERSION: u16 = 1;
pub const MAX_RECORDING_CAPTURE_ERRORS: usize = 64;
pub const MAX_RECORDING_CAPABILITIES: usize = 32;
pub const REPLAY_REPORT_VERSION: u16 = 1;
pub const RECOVERY_REPORT_VERSION: u16 = 1;
pub const DEFAULT_RECORDING_DURATION_SECONDS: u64 = 600;
pub const MAX_RECORDING_DURATION_SECONDS: u64 = 3_600;
pub const RECORDING_FINALIZATION_GRACE_MILLIS: u64 = 5_000;
pub const DOCTOR_REPORT_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
    #[error(transparent)]
    Ebpf(#[from] chronicle_capture_ebpf::EbpfCaptureError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Etl(#[from] EtlError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    ReplayTarget(#[from] ReplayTargetError),
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("runtime Authorization credential is missing from environment variable {environment}")]
    MissingReplayCredential { environment: String },
    #[error("runtime Authorization credential has invalid HTTP field-value bytes")]
    InvalidReplayCredential,
    #[error("JSON serialization failed: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("encoded fixture WAL is {bytes} bytes; one-segment limit is {limit} bytes")]
    FixtureWalTooLarge { bytes: u64, limit: u64 },
    #[error("capture flags {0:#x} cannot fit in WAL record flags")]
    CaptureFlagsOutOfRange(u32),
    #[error("WAL directory already exists: {0}")]
    WalDestinationExists(PathBuf),
    #[error("configuration parse failed: {0}")]
    Config(#[from] toml::de::Error),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("recording metadata failed validation: {0}")]
    RecordingMetadataValidation(String),
    #[error("existing published session {session_id} does not match recording output")]
    PublishedRecordingMismatch {
        session_id: chronicle_common::SessionId,
    },
    #[error("recording ETL checkpoint contradicts recovered WAL snapshot")]
    CheckpointContradiction,
    #[error("production recording preflight failed: {0}")]
    ProductionPreflight(&'static str),
    #[error(
        "unsafe data directory path '{0}': must be absolute with no symlink or non-directory components"
    )]
    UnsafeDataDir(PathBuf),
    #[error(
        "data directory resolution is unsupported on this platform; pass --data-dir or set CHRONICLE_DATA_DIR"
    )]
    UnsupportedDataDirResolution,
    #[error("recording catalog invalid: {0}")]
    CatalogInvalid(String),
    #[error("recording catalog has {count} entries; limit is {limit}")]
    CatalogEntryLimit { count: usize, limit: usize },
    #[error("recording catalog path is unsafe: {0}")]
    CatalogUnsafePath(PathBuf),
    #[error("invalid recording name '{0}': {1}")]
    InvalidRecordingName(String, String),
    #[error("recording '{0}' was not found")]
    RecordingNotFound(String),
    #[error("recording name '{0}' is already in use")]
    RecordingNameCollision(String),
    #[error("chronicle data domain is already owned by another process")]
    DomainOwned,
    #[error("domain lock is unsupported on this platform")]
    UnsupportedDomainLock,
    #[error("domain lock path is unsafe")]
    DomainLockUnsafePath,
    #[error("unsupported live supervised scope: {0}")]
    UnsupportedLivePreflight(&'static str),
    #[error("replay target readiness failed: {0}")]
    ReplayReadiness(String),
    #[error("target bootstrap, hardening, or exec failed")]
    TargetLaunchFailed,
    #[error(
        "incompatible domain lock mapping: data directory {data_dir} and lock root {lock_root} resolve different lock paths on the same filesystem; multiple differently locked Chronicle domains on one filesystem are unsupported"
    )]
    IncompatibleDomainLockMapping {
        data_dir: PathBuf,
        lock_root: PathBuf,
    },
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

/// Stable replay-outcome exit-code classification for outer adapters.
pub fn replay_outcome_exit_code(outcome: &ReplayOutcome) -> i32 {
    match outcome {
        ReplayOutcome::Completed | ReplayOutcome::CompletedWithSkips | ReplayOutcome::DryRun => 0,
        ReplayOutcome::StoppedPolicy | ReplayOutcome::StoppedInvalidSession => 4,
        ReplayOutcome::StoppedTransport => 5,
        ReplayOutcome::StoppedVerification => 6,
    }
}

/// Stable application-error exit-code classification for outer adapters.
pub fn application_error_exit_code(error: &ApplicationError) -> i32 {
    match error {
        ApplicationError::InvalidRecordingName(_, _) => 2,
        ApplicationError::UnsupportedLivePreflight(_)
        | ApplicationError::ReplayReadiness(_)
        | ApplicationError::ReplayTarget(_)
        | ApplicationError::Replay(ReplayError::PreflightDenied) => 4,
        ApplicationError::Protocol(ProtocolError::Transport { .. })
        | ApplicationError::Replay(ReplayError::Protocol(ProtocolError::Transport {
            category:
                TransportErrorCategory::Refused
                | TransportErrorCategory::Timeout
                | TransportErrorCategory::Disconnect
                | TransportErrorCategory::Io,
            ..
        })) => 5,
        _ => 3,
    }
}

/// Protocol registry access for outer adapters, so CLI never imports
/// chronicle-protocol-builtins directly.
pub fn protocol_registry() -> Result<ProtocolRegistry, ApplicationError> {
    chronicle_protocol_builtins::registry()
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_capture_events(payload: Vec<u8>) -> Vec<CaptureEvent> {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEventKind, CaptureFlags, ClockIdentity,
            FragmentSequenceEvidence, MonotonicTimestamp, NetworkFamily, PayloadDirection,
            PayloadFragment, RecordingScopeIdentity, SocketEvidence, SocketIdentity, SocketRole,
            TruncationMetadata, TruncationState,
        };
        use chronicle_common::Endpoint;

        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "test-boot".into(),
            },
            nanoseconds: 1,
        };
        let socket = SocketIdentity {
            socket_cookie: 1,
            first_seen: timestamp.clone(),
            network_namespace: None,
        };
        let scope = RecordingScopeIdentity {
            cgroup_id: 1,
            canonical_path: "/test".into(),
            namespace: None,
        };
        let evidence = SocketEvidence {
            timestamp: timestamp.clone(),
            socket: socket.clone(),
            recording_scope: scope.clone(),
            network_family: NetworkFamily::Ipv4,
            local_endpoint: Endpoint::new("192.0.2.10", 41_000),
            remote_endpoint: Endpoint::new("192.0.2.20", 8_080),
            role: SocketRole::Active,
            process: None,
            observed_cgroup_id: 1,
        };
        let captured_length = u32::try_from(payload.len()).unwrap();
        vec![
            CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::SocketConnected(evidence),
            },
            CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                    timestamp: MonotonicTimestamp {
                        nanoseconds: 2,
                        ..timestamp
                    },
                    socket,
                    recording_scope: scope,
                    network_family: NetworkFamily::Ipv4,
                    direction: PayloadDirection::Egress,
                    sequence: FragmentSequenceEvidence {
                        tcp_sequence: 1,
                        continuation_position: 0,
                    },
                    payload,
                    truncation: TruncationMetadata {
                        captured_length,
                        observed_length: Some(captured_length),
                        state: TruncationState::Complete,
                        reason: None,
                    },
                    flags: CaptureFlags::default(),
                }),
            },
        ]
    }

    #[test]
    fn publication_reservations_release_on_success_and_failure() {
        let authority = QuotaReservationAuthority::new(DomainQuota {
            domain: "domain".into(),
            quota_bytes: 1_000,
            minimum_free_bytes: 10,
            free_bytes: 1_000,
        })
        .unwrap();
        {
            let mut reservation = reserve_publication_peak(&authority, 100).unwrap();
            reservation.reserve_checkpoint(100).unwrap();
            assert!(authority.reserved_bytes().unwrap() > 0);
        }
        assert_eq!(authority.reserved_bytes().unwrap(), 0);

        let authority = QuotaReservationAuthority::new(DomainQuota {
            domain: "domain".into(),
            quota_bytes: 100,
            minimum_free_bytes: 10,
            free_bytes: 100,
        })
        .unwrap();
        let mut reservation = reserve_publication_peak(&authority, 20).unwrap();
        assert!(reservation.reserve_checkpoint(100).is_err());
        drop(reservation);
        assert_eq!(authority.reserved_bytes().unwrap(), 0);
    }

    #[test]
    fn production_signal_stop_keeps_first_signal_and_flags_second() {
        let stop = ProductionSignalStop::default();
        assert!(stop.request_interrupt());
        assert_eq!(stop.shutdown_reason(), Some(ShutdownReason::UserInterrupt));
        assert!(!stop.request_termination());
        assert_eq!(stop.shutdown_reason(), Some(ShutdownReason::UserInterrupt));
    }

    #[test]
    fn second_signal_persists_forced_termination_for_recovery() {
        let directory = ingest_directory("forced-termination");
        let mut metadata = recording_metadata(RecordingStatus::Recording);
        metadata.capture = Some(capture_metadata());
        write_recording_metadata(&directory, &metadata).unwrap();

        mark_recording_forced_termination(&directory).unwrap();
        let persisted = load_recording_metadata(&directory).unwrap().unwrap();
        assert_eq!(persisted.status, RecordingStatus::Aborted);
        assert_eq!(
            persisted.shutdown_reason,
            Some(ShutdownReason::ForcedTermination)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn rollover_transition_rolls_back_unpublished_successor() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let state_root = root.join("state");
        let successor = root.join("epochs/1");
        std::fs::create_dir_all(&successor).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let old_id = chronicle_common::RecordingId::new();
        let new_id = chronicle_common::RecordingId::new();
        let mut catalog = EpochCatalogV1::new(EpochCatalogEntry {
            ordinal: 0,
            recording_id: old_id,
            predecessor: None,
            path: ".".into(),
            state: EpochCatalogState::Active,
        })
        .unwrap();
        catalog
            .append(EpochCatalogEntry {
                ordinal: 1,
                recording_id: new_id,
                predecessor: Some(old_id),
                path: "epochs/1".into(),
                state: EpochCatalogState::Prepared,
            })
            .unwrap();
        catalog.write_atomic(&root).unwrap();
        let transition =
            RolloverTransitionV1::new(0, 1, old_id.0, new_id.0, root.join("."), &successor, 1024)
                .unwrap();
        write_transition_atomic(&state_root, &transition).unwrap();

        recover_rollover_transition_for_root(&state_root, &root, Some(&mut catalog), &[], 1024)
            .unwrap();

        assert!(!successor.exists());
        assert_eq!(catalog.epochs.len(), 1);
        assert_eq!(catalog.active().unwrap().recording_id, old_id);
        assert!(load_transition(&state_root).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rollover_transition_adopts_published_successor() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-forward-{}",
            uuid::Uuid::new_v4()
        ));
        let state_root = root.join("state");
        let successor = root.join("epochs/1");
        std::fs::create_dir_all(&successor).unwrap();
        std::fs::create_dir_all(&state_root).unwrap();
        let old_id = chronicle_common::RecordingId::new();
        let new_id = chronicle_common::RecordingId::new();
        let mut catalog = EpochCatalogV1::new(EpochCatalogEntry {
            ordinal: 0,
            recording_id: old_id,
            predecessor: None,
            path: ".".into(),
            state: EpochCatalogState::Active,
        })
        .unwrap();
        catalog
            .append(EpochCatalogEntry {
                ordinal: 1,
                recording_id: new_id,
                predecessor: Some(old_id),
                path: "epochs/1".into(),
                state: EpochCatalogState::Prepared,
            })
            .unwrap();
        catalog.write_atomic(&root).unwrap();
        write_recording_metadata(
            &successor,
            &RecordingMetadata {
                version: RECORDING_METADATA_SCHEMA_VERSION,
                recording_id: new_id,
                selector: None,
                status: RecordingStatus::Recording,
                shutdown_reason: None,
                last_valid_commit: None,
                counters: RecordingCounters::default(),
                terminal_wal_loss: None,
                capture: None,
            },
        )
        .unwrap();
        let transition =
            RolloverTransitionV1::new(0, 1, old_id.0, new_id.0, root.join("."), &successor, 1024)
                .unwrap();
        write_transition_atomic(&state_root, &transition).unwrap();
        catalog.activate_latest().unwrap();
        catalog.write_atomic(&root).unwrap();

        recover_rollover_transition_for_root(&state_root, &root, Some(&mut catalog), &[], 1024)
            .unwrap();

        assert_eq!(catalog.active().unwrap().recording_id, new_id);
        assert_eq!(catalog.epochs[0].state, EpochCatalogState::Finalized);
        assert!(load_transition(&state_root).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn physical_wal_size_counts_nested_segment_files_only() {
        let directory = ingest_directory("physical-size");
        let nested = directory.join("segments/nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(directory.join("segments/first"), [0_u8; 3]).unwrap();
        std::fs::write(nested.join("second"), [0_u8; 5]).unwrap();
        std::fs::write(directory.join("recording.json"), [0_u8; 7]).unwrap();

        assert_eq!(recording_physical_wal_bytes(&directory).unwrap(), 8);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn lifecycle_index_round_trips_and_rejects_corruption() {
        let directory = ingest_directory("lifecycle-index");
        std::fs::create_dir_all(&directory).unwrap();
        let mut index = load_lifecycle_index(&directory).unwrap();
        assert!(index.entries.is_empty());
        mark_epoch_cleanup_complete(&mut index, 3, "a".repeat(64));
        save_lifecycle_index(&directory, &index).unwrap();
        let loaded = load_lifecycle_index(&directory).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert!(loaded.entries[0].cleanup_complete);
        assert_eq!(loaded.entries[0].epoch_ordinal, 3);
        std::fs::write(directory.join(LIFECYCLE_INDEX_FILE), b"not json").unwrap();
        assert!(load_lifecycle_index(&directory).is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn lifecycle_index_compacts_oldest_complete_entries() {
        let mut index = LifecycleIndexV1 {
            version: 1,
            generation: 0,
            transaction_id: "tx".into(),
            source_revision: 1,
            source_digest: "b".repeat(64),
            first_retained_epoch: 0,
            last_compacted_epoch: None,
            entries: (0..4)
                .map(|epoch| LifecycleEpochEntry {
                    epoch_ordinal: epoch,
                    predecessor_epoch: epoch.checked_sub(1),
                    digest: "a".repeat(64),
                    cleanup_complete: true,
                })
                .collect(),
            max_entries: 4,
            max_bytes: 1 << 20,
        };
        assert!(compact_lifecycle_index(&mut index).unwrap());
        assert_eq!(index.entries.len(), 2);
        assert_eq!(index.entries[0].epoch_ordinal, 2);
        assert_eq!(index.generation, 1);
        assert_eq!(index.last_compacted_epoch, Some(1));
        assert_eq!(index.first_retained_epoch, 2);
        assert!(!compact_lifecycle_index(&mut index).unwrap());
    }

    fn retention_config(root: &std::path::Path, min_age_seconds: u64) -> NormalizedRecorderConfig {
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
                max_bytes: chronicle_wal::DEFAULT_SEGMENT_BYTES,
            },
            domains: vec![FilesystemDomainConfig {
                root: root.to_path_buf(),
                quota_bytes: 8 * 1024 * 1024 * 1024,
                minimum_free_bytes: 1024 * 1024,
            }],
            retention: Some(RetentionMode::DeleteAfter {
                min_age_seconds,
                max_retained_bytes: None,
            }),
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

    fn retention_catalog() -> EpochCatalogV1 {
        let old_id = RecordingId::new();
        let new_id = RecordingId::new();
        let catalog = EpochCatalogV1 {
            version: 1,
            epochs: vec![
                EpochCatalogEntry {
                    ordinal: 0,
                    recording_id: old_id,
                    predecessor: None,
                    path: "epochs/0".into(),
                    state: EpochCatalogState::Finalized,
                },
                EpochCatalogEntry {
                    ordinal: 1,
                    recording_id: new_id,
                    predecessor: Some(old_id),
                    path: "epochs/1".into(),
                    state: EpochCatalogState::Active,
                },
            ],
        };
        catalog.validate().unwrap();
        catalog
    }

    #[test]
    fn retention_cleanup_persists_lifecycle_index_and_deletes_proof_eligible_segments() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-retention-index-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = retention_config(&root, 1);
        let root = Path::new(&config.domains[0].identity.canonical_root).to_path_buf();
        let catalog = retention_catalog();
        let segment = root.join("epochs/0/segments/old.chwal");
        std::fs::create_dir_all(segment.parent().unwrap()).unwrap();
        std::fs::write(&segment, vec![0_u8; 64]).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let quotas = recorder_startup::build_quota_authorities(&config).unwrap();
        apply_retention_cleanup(&root, &catalog, &config, &quotas).unwrap();
        assert!(!segment.exists());
        let index = load_lifecycle_index(&root).unwrap();
        assert_eq!(index.entries.len(), 1);
        assert_eq!(index.entries[0].epoch_ordinal, 0);
        assert!(index.entries[0].cleanup_complete);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_segment_cleanup_is_refused_and_evidence_stays_byte_identical() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-retention-corrupt-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let segment = root.join("epochs/0/segments/old.chwal");
        std::fs::create_dir_all(segment.parent().unwrap()).unwrap();
        let evidence = vec![0x5a_u8; 128];
        std::fs::write(&segment, &evidence).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        let intent = root.join("epochs/0/segments/cleanup-intent-0-tampered.json");
        // Tampered proof: expected digest does not match the segment bytes, so
        // deletion is refused and the evidence must remain untouched.
        let result = cleanup_finalized_segment_verified(
            &segment,
            "f".repeat(64).as_str(),
            root.join("retention-trash"),
            &intent,
            "epoch-0-tampered",
        );
        assert_eq!(result, Err(chronicle_wal::RetentionError::DigestMismatch));
        assert_eq!(fs::read(&segment).unwrap(), evidence);
        assert!(!root.join("retention-trash/epoch-0-tampered").exists());
        // A restart retries the same tampered cleanup; deletion is refused
        // again and the evidence remains byte-identical.
        let retry = cleanup_finalized_segment_verified(
            &segment,
            "f".repeat(64).as_str(),
            root.join("retention-trash"),
            &intent,
            "epoch-0-tampered",
        );
        assert_eq!(retry, Err(chronicle_wal::RetentionError::DigestMismatch));
        assert_eq!(fs::read(&segment).unwrap(), evidence);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn protected_lineage_is_preserved_byte_identical_across_restart() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-retention-protected-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let config = retention_config(&root, 3600);
        let root = Path::new(&config.domains[0].identity.canonical_root).to_path_buf();
        let catalog = retention_catalog();
        // Active-epoch segment is protected lineage; young finalized-epoch
        // segment is not yet retention-eligible.
        let active_segment = root.join("epochs/1/segments/live.chwal");
        let young_segment = root.join("epochs/0/segments/young.chwal");
        std::fs::create_dir_all(active_segment.parent().unwrap()).unwrap();
        std::fs::create_dir_all(young_segment.parent().unwrap()).unwrap();
        let active_evidence = vec![0x11_u8; 32];
        let young_evidence = vec![0x22_u8; 32];
        std::fs::write(&active_segment, &active_evidence).unwrap();
        std::fs::write(&young_segment, &young_evidence).unwrap();
        let quotas = recorder_startup::build_quota_authorities(&config).unwrap();
        apply_retention_cleanup(&root, &catalog, &config, &quotas).unwrap();
        // Restart simulation: a second cleanup pass must still refuse deletion
        // and leave every protected byte identical.
        apply_retention_cleanup(&root, &catalog, &config, &quotas).unwrap();
        assert_eq!(fs::read(&active_segment).unwrap(), active_evidence);
        assert_eq!(fs::read(&young_segment).unwrap(), young_evidence);
        assert!(load_lifecycle_index(&root).unwrap().entries.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn ingest_directory(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("chronicle-ingest-{label}-{}", uuid::Uuid::new_v4()))
    }

    fn ingest(directory: &Path) -> RecordingIngest {
        RecordingIngest::new(
            GroupCommitWalWriter::create_with_total_limit(
                directory,
                RecordingId::new(),
                chronicle_wal::MIN_SEGMENT_BYTES,
                chronicle_wal::MIN_SEGMENT_BYTES,
                1,
                0,
            )
            .unwrap(),
        )
    }

    fn queued(payload: Vec<u8>) -> QueuedWalRecord {
        let nanoseconds = u64::from(payload.first().copied().unwrap_or_default());
        queued_at(payload, "fixture-ingest", nanoseconds)
    }

    fn queued_at(payload: Vec<u8>, boot_id: &str, nanoseconds: u64) -> QueuedWalRecord {
        QueuedWalRecord {
            kind: RecordKind::CaptureEvent,
            schema_version: 1,
            flags: 0,
            payload,
            capture_timestamp: Some(MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: boot_id.into(),
                },
                nanoseconds,
            }),
        }
    }

    fn queued_without_timestamp(payload: Vec<u8>) -> QueuedWalRecord {
        QueuedWalRecord {
            kind: RecordKind::CaptureEvent,
            schema_version: 1,
            flags: 0,
            payload,
            capture_timestamp: None,
        }
    }

    #[test]
    fn ingest_empty_queue_after_rollover_keeps_last_durable_commit() {
        let directory = ingest_directory("rollover-empty");
        let mut ingest = ingest(&directory);
        assert_eq!(ingest.admit(queued(vec![1])), Ok(IngestAdmission::Accepted));
        ingest.drain(1).unwrap();
        let boundary = ingest.rollover(2).unwrap().unwrap();
        assert_eq!(ingest.commit_boundary(), Some(boundary.clone()));
        ingest.finish(ShutdownReason::SourceCompleted, 3).unwrap();
        assert_eq!(ingest.commit_boundary(), Some(boundary));
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_empty_queue_finalizes_without_inventing_durability() {
        let directory = ingest_directory("empty");
        let mut ingest = ingest(&directory);
        let result = ingest.finish(ShutdownReason::SourceCompleted, 1).unwrap();
        assert_eq!(result.status, RecordingStatus::Completed);
        assert_eq!(result.shutdown_reason, ShutdownReason::SourceCompleted);
        assert_eq!(result.counters.committed, RecordByteCount::default());
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_final_sync_failure_returns_failed_wal_result_with_uncertain_write() {
        let directory = ingest_directory("sync-failure");
        let mut ingest = ingest(&directory);
        assert_eq!(ingest.admit(queued(vec![1])), Ok(IngestAdmission::Accepted));
        ingest.writer.inject_data_sync_failure_for_test();

        let failure = ingest
            .finish(ShutdownReason::SourceCompleted, 1)
            .unwrap_err();
        assert_eq!(failure.result.status, RecordingStatus::Failed);
        assert_eq!(failure.result.shutdown_reason, ShutdownReason::WalFailure);
        assert_eq!(failure.result.counters.committed.records, 0);
        assert_eq!(failure.result.counters.written_not_committed.records, 1);
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_backpressures_at_fixed_queue_capacity_without_dropping() {
        let directory = ingest_directory("full");
        let mut ingest = ingest(&directory);
        for _ in 0..INGEST_QUEUE_CAPACITY {
            assert_eq!(ingest.admit(queued(vec![1])), Ok(IngestAdmission::Accepted));
        }
        assert_eq!(ingest.admit(queued(vec![2])), Err(queued(vec![2])));
        assert_eq!(ingest.queue_len(), INGEST_QUEUE_CAPACITY);
        assert_eq!(
            ingest.counters().accepted_into_queue.records,
            INGEST_QUEUE_CAPACITY as u64
        );
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_discards_only_wal_limit_suffix_and_wal_limit_wins_duration_tie() {
        let directory = ingest_directory("limit");
        let mut ingest = ingest(&directory);
        let frame_bytes = (chronicle_wal::MIN_SEGMENT_BYTES
            - u64::try_from(chronicle_wal::COMMIT_MARKER_FRAME_LEN).unwrap())
            / 2
            + 1;
        let payload_bytes = usize::try_from(
            frame_bytes - u64::try_from(chronicle_wal::ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ingest.admit(queued(vec![1; payload_bytes])),
            Ok(IngestAdmission::Accepted)
        );
        assert_eq!(
            ingest.admit(queued(vec![2; payload_bytes])),
            Ok(IngestAdmission::Accepted)
        );

        let result = ingest.finish(ShutdownReason::DurationLimit, 1).unwrap();
        assert_eq!(result.shutdown_reason, ShutdownReason::WalSizeLimit);
        // Ordinary ingest counters exclude typed terminal control records.
        assert_eq!(result.counters.committed.records, 1);
        assert_eq!(
            result
                .counters
                .discarded_from_queue_due_to_wal_limit
                .records,
            1
        );
        assert_eq!(
            result.counters.accepted_into_queue.records,
            result.counters.committed.records
                + result
                    .counters
                    .discarded_from_queue_due_to_wal_limit
                    .records
        );
        let terminal = result.terminal_wal_loss.unwrap();
        assert_eq!(terminal.entries.len(), 1);
        assert_eq!(terminal.entries[0].discarded.records, 1);
        assert_eq!(terminal.entries[0].start, terminal.entries[0].end);
        assert_eq!(
            terminal.entries[0].persistence,
            TerminalWalLossPersistence::PersistedWal
        );
        assert_eq!(
            result.counters.rejected_after_stop,
            RecordByteCount::default()
        );
        assert_eq!(
            ingest.admit(queued(vec![3])),
            Ok(IngestAdmission::RejectedAfterStop)
        );
        assert_eq!(ingest.counters().rejected_after_stop.records, 1);
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_emits_one_terminal_loss_per_incompatible_clock() {
        let directory = ingest_directory("limit-clocks");
        let mut ingest = ingest(&directory);
        let frame_bytes = (chronicle_wal::MIN_SEGMENT_BYTES
            - u64::try_from(chronicle_wal::COMMIT_MARKER_FRAME_LEN).unwrap())
            / 2
            + 1;
        let payload_bytes = usize::try_from(
            frame_bytes - u64::try_from(chronicle_wal::ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        for (boot_id, nanoseconds) in [("persisted", 1), ("clock-a", 30), ("clock-b", 10)] {
            assert_eq!(
                ingest.admit(queued_at(vec![1; payload_bytes], boot_id, nanoseconds)),
                Ok(IngestAdmission::Accepted)
            );
        }

        let result = ingest.finish(ShutdownReason::SourceCompleted, 1).unwrap();
        let terminal = result.terminal_wal_loss.unwrap();
        assert_eq!(terminal.discarded.records, 2);
        assert_eq!(terminal.entries.len(), 2);
        assert!(terminal.entries.iter().all(|entry| {
            entry.persistence == TerminalWalLossPersistence::PersistedWal
                && entry.discarded.records == 1
                && entry.start == entry.end
        }));
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_missing_timestamp_uses_only_last_persisted_clock_as_metadata_fallback() {
        let directory = ingest_directory("limit-missing-time");
        let mut ingest = ingest(&directory);
        let frame_bytes = (chronicle_wal::MIN_SEGMENT_BYTES
            - u64::try_from(chronicle_wal::COMMIT_MARKER_FRAME_LEN).unwrap())
            / 2
            + 1;
        let payload_bytes = usize::try_from(
            frame_bytes - u64::try_from(chronicle_wal::ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        ingest
            .admit(queued_at(vec![1; payload_bytes], "persisted", 42))
            .unwrap();
        ingest.drain(10).unwrap();
        ingest
            .admit(queued_without_timestamp(vec![2; payload_bytes]))
            .unwrap();

        let result = ingest.finish(ShutdownReason::SourceCompleted, 11).unwrap();
        let entry = &result.terminal_wal_loss.unwrap().entries[0];
        assert_eq!(
            entry.time_source,
            TerminalWalLossTimeSource::FallbackLastPersisted
        );
        assert_eq!(entry.start.as_ref().unwrap().nanoseconds, 42);
        assert_eq!(entry.end, None);
        assert_eq!(entry.persistence, TerminalWalLossPersistence::MetadataOnly);
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_uses_metadata_only_when_terminal_frame_cannot_fit() {
        let directory = ingest_directory("limit-no-terminal-room");
        let mut ingest = ingest(&directory);
        let frame_bytes = chronicle_wal::MIN_SEGMENT_BYTES
            - u64::try_from(chronicle_wal::SEGMENT_HEADER_LEN).unwrap()
            - u64::try_from(chronicle_wal::COMMIT_MARKER_FRAME_LEN).unwrap();
        let payload_bytes = usize::try_from(
            frame_bytes - u64::try_from(chronicle_wal::ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        assert_eq!(
            ingest.admit(queued_at(vec![1; payload_bytes], "persisted", 1)),
            Ok(IngestAdmission::Accepted)
        );
        assert_eq!(
            ingest.admit(queued_at(vec![2], "discarded", 2)),
            Ok(IngestAdmission::Accepted)
        );

        let result = ingest.finish(ShutdownReason::SourceCompleted, 1).unwrap();
        assert_eq!(result.counters.committed.records, 1);
        let terminal = result.terminal_wal_loss.clone().unwrap();
        assert_eq!(
            terminal.entries[0].persistence,
            TerminalWalLossPersistence::MetadataOnly
        );
        let mut metadata = recording_metadata(RecordingStatus::Recording);
        result.persist_metadata(&directory, &mut metadata).unwrap();
        assert_eq!(
            load_recording_metadata(&directory)
                .unwrap()
                .unwrap()
                .terminal_wal_loss,
            Some(terminal)
        );
        let physical_bytes: u64 = std::fs::read_dir(directory.join("segments"))
            .unwrap()
            .map(|entry| entry.unwrap().metadata().unwrap().len())
            .sum();
        assert_eq!(physical_bytes, chronicle_wal::MIN_SEGMENT_BYTES);
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ingest_terminal_sync_failure_does_not_claim_terminal_persistence() {
        let directory = ingest_directory("limit-terminal-sync-failure");
        let mut ingest = ingest(&directory);
        let frame_bytes = (chronicle_wal::MIN_SEGMENT_BYTES
            - u64::try_from(chronicle_wal::COMMIT_MARKER_FRAME_LEN).unwrap())
            / 2
            + 1;
        let payload_bytes = usize::try_from(
            frame_bytes - u64::try_from(chronicle_wal::ENVELOPE_HEADER_LEN).unwrap(),
        )
        .unwrap();
        ingest
            .admit(queued_at(vec![1; payload_bytes], "persisted", 1))
            .unwrap();
        ingest
            .admit(queued_at(vec![2; payload_bytes], "discarded", 2))
            .unwrap();
        ingest.drain(1).unwrap();
        assert!(ingest.terminal_wal_loss.is_some());
        ingest.writer.inject_data_sync_failure_for_test();

        let failure = ingest
            .finish(ShutdownReason::SourceCompleted, 1)
            .unwrap_err();
        assert_eq!(failure.result.status, RecordingStatus::Failed);
        assert_eq!(failure.result.shutdown_reason, ShutdownReason::WalFailure);
        assert_eq!(
            failure.result.terminal_wal_loss.unwrap().entries[0].persistence,
            TerminalWalLossPersistence::NotPersistedDueToWalFailure
        );
        drop(ingest);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn replay_context_loads_only_redacted_runtime_authorization() {
        let config = ReplayConfig {
            authorization_env: Some("CHRONICLE_TEST_AUTH".into()),
            ..ReplayConfig::default()
        };
        let context =
            replay_context_from(&config, |_| Some("Bearer runtime-secret".into())).unwrap();
        assert_eq!(
            context.credentials["authorization"].expose(),
            b"Bearer runtime-secret"
        );
        assert!(!format!("{context:?}").contains("runtime-secret"));
    }

    #[test]
    fn replay_context_rejects_missing_or_injected_authorization() {
        let config = ReplayConfig {
            authorization_env: Some("CHRONICLE_TEST_AUTH".into()),
            ..ReplayConfig::default()
        };
        assert!(matches!(
            replay_context_from(&config, |_| None),
            Err(ApplicationError::MissingReplayCredential { .. })
        ));
        assert!(matches!(
            replay_context_from(&config, |_| Some("Bearer good\r\nInjected: bad".into())),
            Err(ApplicationError::InvalidReplayCredential)
        ));
    }

    #[tokio::test]
    async fn command_replay_write_denial_never_spawns_target() {
        let root =
            std::env::temp_dir().join(format!("chronicle-replay-preplan-{}", uuid::Uuid::new_v4()));
        let marker = root.join("target-ran");
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/binary-body.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();

        let result = replay_command(
            CommandReplayOptions {
                command: vec![
                    "/bin/sh".into(),
                    "-c".into(),
                    format!("touch '{}'", marker.display()),
                ],
                data_dir: root.clone(),
                session_id: recorded.session_id.to_string(),
                allow_writes: false,
                child_stdout: ChildStdio::Null,
                child_stderr: ChildStdio::Null,
                invoking_uid: rustix::process::geteuid().as_raw(),
                invoking_gid: rustix::process::getegid().as_raw(),
            },
            &ReplayConfig::default(),
        )
        .await
        .unwrap();

        assert_eq!(result.replay.outcome, ReplayOutcome::StoppedPolicy);
        assert!(result.replay.preflight_denied);
        assert!(result.child_exit.is_none());
        assert!(result.target_origin.is_none());
        assert!(!marker.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unsupported_only_command_preplan_is_invalid_session() {
        let root =
            std::env::temp_dir().join(format!("chronicle-replay-invalid-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        let mut session = FilesystemSessionStore::new(&root)
            .hydrate(recorded.session_id)
            .unwrap();
        session.connections[0].operations[0]
            .attributes
            .insert("chronicle.replayable".into(), "false".into());

        let plan = command_replay_plan(&session, false).unwrap();
        let result = replay_plan_result(session.id.to_string(), &plan);
        assert_eq!(result.outcome, ReplayOutcome::StoppedInvalidSession);
        assert!(result.preflight_denied);
        assert_eq!(result.counts.executable, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_fixture_runs_wal_etl_and_atomic_publish() {
        let root = std::env::temp_dir().join(format!("chronicle-record-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        let result = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        assert!(result.issues.is_empty());
        assert!(result.checkpoint.next_sequence > 1);
        let session = FilesystemSessionStore::new(&root)
            .inspect(result.session_id)
            .unwrap();
        assert_eq!(session.id, result.session_id);
        assert_eq!(session.connections.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_reads_published_session_after_fixture_and_wal_removal() {
        let root = std::env::temp_dir().join(format!("chronicle-inspect-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        std::fs::remove_dir_all(root.join("wal")).unwrap();

        let inspected = inspect_session(&root, recorded.session_id).unwrap();
        assert!(inspected.complete);
        assert!(inspected.replayable, "{:?}", inspected.blockers);
        assert_eq!(inspected.replayability, Replayability::FullyReplayable);
        assert_eq!(inspected.executable_operations, 1);
        assert_eq!(inspected.non_executable_operations, 0);
        assert!(inspected.integrity_valid);
        assert!(inspected.blockers.is_empty());
        assert_eq!(inspected.connections[0].operations.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_rendering_is_deterministic_and_excludes_header_values() {
        let root =
            std::env::temp_dir().join(format!("chronicle-inspect-render-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/duplicate-header.json"
        ))
        .unwrap();
        let mut fixture: serde_json::Value = serde_json::from_slice(&fixture).unwrap();
        fixture["events"][0]["payload_hex"] = serde_json::Value::String(
            "474554202f6865616465727320485454502f312e310d0a486f73743a207265636f726465642e696e76616c69640d0a417574686f72697a6174696f6e3a204265617265722073757065722d7365637265740d0a436f6f6b69653a2073657373696f6e3d7365637265740d0a582d5365637265743a20746f702d7365637265740d0a0d0a".into(),
        );
        let mut source =
            FixtureCaptureSource::from_json(&serde_json::to_vec(&fixture).unwrap()).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        let inspected = inspect_session(&root, recorded.session_id).unwrap();

        let human = render_inspect_human(&inspected);
        let json = render_inspect_json(&inspected).unwrap();
        assert_eq!(human, render_inspect_human(&inspected));
        assert_eq!(json, render_inspect_json(&inspected).unwrap());
        for rendered in [&human, &json] {
            for secret in [
                "authorization",
                "super-secret",
                "cookie",
                "session=secret",
                "x-secret",
                "top-secret",
            ] {
                assert!(!rendered.contains(secret));
            }
        }
        assert!(json.starts_with("{\"version\":1,"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_marks_truncated_fixture_non_replayable() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-inspect-truncated-{}",
            uuid::Uuid::new_v4()
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/truncated.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();

        let inspected = inspect_session(&root, recorded.session_id).unwrap();
        assert!(!inspected.complete);
        assert!(!inspected.replayable);
        assert_eq!(inspected.replayability, Replayability::NotReplayable);
        assert_eq!(inspected.executable_operations, 0);
        assert!(!inspected.blockers.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_propagates_missing_and_corrupt_sessions() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-inspect-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(matches!(
            inspect_session(&root, chronicle_common::SessionId::new()),
            Err(ApplicationError::Storage(StorageError::NotFound(_)))
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        std::fs::write(
            root.join("sessions")
                .join(recorded.session_id.to_string())
                .join("manifest.json"),
            b"corrupt",
        )
        .unwrap();
        assert!(matches!(
            inspect_session(&root, recorded.session_id),
            Err(ApplicationError::Storage(StorageError::Validation(_)))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_fixture_marks_truncated_capture_non_replayable() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-record-truncated-{}",
            uuid::Uuid::new_v4()
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/truncated.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        let result = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        assert!(!result.complete);
        assert!(!result.replayability.is_empty());
        let manifest = std::fs::read_to_string(
            root.join("sessions")
                .join(result.session_id.to_string())
                .join("manifest.json"),
        )
        .unwrap();
        assert!(manifest.contains("\"complete\":false"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn issue_summaries_are_bounded_and_omit_messages() {
        let issues: Vec<_> = (0..=MAX_RECORD_ISSUES)
            .map(|sequence| EtlIssue {
                sequence: Some(sequence as u64),
                kind: chronicle_etl::EtlIssueKind::ProtocolDecode,
                message: "secret header value".into(),
            })
            .collect();
        let summaries = issue_summaries(&issues);
        assert_eq!(summaries.len(), MAX_RECORD_ISSUES + 1);
        assert!(
            summaries
                .iter()
                .all(|summary| !summary.code.contains("secret"))
        );
        assert_eq!(summaries.last().unwrap().code, "issue_summary_truncated");
    }

    #[test]
    fn record_fixture_does_not_publish_when_wal_preflight_fails() {
        let root =
            std::env::temp_dir().join(format!("chronicle-record-fail-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        assert!(matches!(
            record_fixture(&mut source, &root, 1),
            Err(ApplicationError::FixtureWalTooLarge { .. })
        ));
        assert!(!root.join("sessions").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capture_source_is_written_and_read_back_through_wal() {
        use chronicle_capture::InMemoryCaptureSource;
        use chronicle_wal::{DEFAULT_MAX_RECORD_BYTES, scan_wal};

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-{}", uuid::Uuid::new_v4()));
        let mut source = InMemoryCaptureSource::new(test_capture_events(b"fixture".to_vec()));
        let recorded = write_capture_to_wal(&mut source, &directory, 2048).unwrap();
        assert_eq!(recorded.record_count, 2);
        assert_eq!(recorded.first_sequence, Some(1));

        let scan = scan_wal(&directory, recorded.recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(scan.committed[0].sequence, 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_identical_capture_is_deduplicated_in_final_session() {
        use chronicle_capture::InMemoryCaptureSource;

        // The same capture events written twice produce identical canonical
        // operations whose deterministic ids collide; the final session must
        // keep one occurrence so publication validation succeeds.
        let root =
            std::env::temp_dir().join(format!("chronicle-app-dedup-{}", uuid::Uuid::new_v4()));
        let wal_directory = root.join("wal");
        // Two distinct capture sequences carrying byte-identical requests:
        // the canonical operations are identical (deterministic ids collide).
        let mut events = test_capture_events(b"GET / HTTP/1.1\r\nHost: example\r\n\r\n".to_vec());
        let mut replay = test_capture_events(b"GET / HTTP/1.1\r\nHost: example\r\n\r\n".to_vec());
        if let CaptureEvent {
            kind: CaptureEventKind::PayloadFragment(fragment),
            ..
        } = &mut replay[1]
        {
            fragment.sequence.tcp_sequence = 2;
        }
        events.extend(replay);
        assert_eq!(events.len(), 4);
        let recorded = write_capture_to_wal(
            &mut InMemoryCaptureSource::new(events),
            &wal_directory,
            8192,
        )
        .unwrap();
        assert_eq!(recorded.record_count, 4);
        let recording_id = recorded.recording_id;
        write_recording_metadata(
            &wal_directory,
            &RecordingMetadata {
                version: RECORDING_METADATA_SCHEMA_VERSION,
                recording_id,
                selector: None,
                status: RecordingStatus::Completed,
                shutdown_reason: Some(ShutdownReason::SourceCompleted),
                last_valid_commit: None,
                counters: RecordingCounters::default(),
                terminal_wal_loss: None,
                capture: None,
            },
        )
        .unwrap();
        reconcile_recording_metadata(&wal_directory, recording_id, None).unwrap();
        let registry = chronicle_protocol_builtins::registry().unwrap();
        let published =
            process_and_publish_recording_wal(&wal_directory, &root, &registry).unwrap();
        let inspection = inspect_session(&root, published.session_id).unwrap();
        let session = FilesystemSessionStore::new(&root)
            .hydrate(published.session_id)
            .unwrap();
        session.validate().unwrap();
        let ids = session
            .connections
            .iter()
            .flat_map(|connection| connection.operations.iter().map(|operation| operation.id))
            .collect::<Vec<_>>();
        assert_eq!(
            ids.len(),
            ids.iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        );
        assert!(!ids.is_empty());
        assert!(inspection.integrity_valid);
        assert!(
            FilesystemSessionStore::new(&root)
                .hydrate(published.session_id)
                .is_ok()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupt_wal_prevents_processing_before_session_publication() {
        use chronicle_capture::InMemoryCaptureSource;
        use chronicle_common::SessionId;
        use chronicle_wal::segment_file_name;

        let root =
            std::env::temp_dir().join(format!("chronicle-app-corrupt-{}", uuid::Uuid::new_v4()));
        let wal_directory = root.join("wal");
        let recorded = write_capture_to_wal(
            &mut InMemoryCaptureSource::new(test_capture_events(
                b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            )),
            &wal_directory,
            2048,
        )
        .unwrap();
        let segment = wal_directory.join("segments").join(segment_file_name(1));
        let mut bytes = std::fs::read(&segment).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&segment, bytes).unwrap();
        assert!(
            process_fixture_wal(
                &wal_directory,
                recorded.recording_id,
                &chronicle_protocol_builtins::registry().unwrap(),
                SessionId::new(),
            )
            .is_err()
        );
        assert!(!root.join("sessions").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_capture_is_rejected_before_wal_directory_creation() {
        use chronicle_capture::InMemoryCaptureSource;

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-limit-{}", uuid::Uuid::new_v4()));
        let mut source = InMemoryCaptureSource::new(test_capture_events(vec![0; 128]));
        let error = write_capture_to_wal(&mut source, &directory, 64).unwrap_err();
        assert!(matches!(error, ApplicationError::FixtureWalTooLarge { .. }));
        assert!(!directory.exists());
    }

    #[test]
    fn fixture_wal_process_returns_checkpoint_after_commit_marker() {
        use chronicle_capture::InMemoryCaptureSource;
        use chronicle_common::SessionId;
        let directory =
            std::env::temp_dir().join(format!("chronicle-app-etl-{}", uuid::Uuid::new_v4()));
        let mut source = InMemoryCaptureSource::new(test_capture_events(b"FAKE request".to_vec()));
        let recorded = write_capture_to_wal(&mut source, &directory, 2048).unwrap();
        let (output, checkpoint) = process_fixture_wal(
            &directory,
            recorded.recording_id,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(checkpoint.next_sequence, 4);
        assert_eq!(output.session.connections.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn recording_metadata(status: RecordingStatus) -> RecordingMetadata {
        RecordingMetadata {
            version: RECORDING_METADATA_SCHEMA_VERSION,
            recording_id: RecordingId::new(),
            selector: Some(RecordingSelectorIdentity {
                canonical_cgroup_path: "/sys/fs/cgroup/workload".into(),
                cgroup_id: 42,
            }),
            status,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: RecordingCounters::default(),
            terminal_wal_loss: None,
            capture: None,
        }
    }

    fn capture_metadata() -> RecordingCaptureMetadata {
        RecordingCaptureMetadata {
            version: RECORDING_CAPTURE_METADATA_SCHEMA_VERSION,
            build: RecordingBuildIdentity {
                chronicle_version: "0.1.0".into(),
                aya_version: "0.14.0".into(),
                aya_ebpf_version: "0.2.1".into(),
                ebpf_object_sha256: "a".repeat(64),
            },
            host: RecordingHostIdentity {
                kernel_release: "6.8.0".into(),
                architecture: "aarch64".into(),
                boot_id: "boot-id".into(),
            },
            scope: RecordingSelectorScope {
                direct_tgid_count: 1,
                descendant_cgroup_count: 0,
                selected_subtree: true,
                shared_scope_acknowledged: false,
            },
            capabilities: ["btf".into(), "cgroup/connect4".into()]
                .into_iter()
                .collect(),
            configured_bounds: RecordingBounds {
                duration_seconds: 600,
                max_wal_bytes: 4 * 1024 * 1024 * 1024,
                segment_bytes: 256 * 1024 * 1024,
                ring_bytes: 8 * 1024 * 1024,
            },
            effective_bounds: RecordingBounds {
                duration_seconds: 600,
                max_wal_bytes: 4 * 1024 * 1024 * 1024,
                segment_bytes: 256 * 1024 * 1024,
                ring_bytes: 8 * 1024 * 1024,
            },
            errors: vec![RecordingCaptureError {
                code: RecordingCaptureErrorCode::Attach,
            }],
        }
    }

    #[test]
    fn production_recording_persists_starting_before_source_and_finalizes_committed_prefix() {
        use chronicle_capture::InMemoryCaptureSource;
        use std::cell::Cell;

        let directory = ingest_directory("production-lifecycle");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        let events = test_capture_events(b"payload".to_vec());
        let stop_checks = Cell::new(0_u8);
        let result = record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            || {
                let persisted = load_recording_metadata(&directory).unwrap().unwrap();
                assert_eq!(persisted.status, RecordingStatus::Starting);
                Ok(InMemoryCaptureSource::new(events))
            },
            || 1,
            || {
                let checks = stop_checks.get();
                stop_checks.set(checks + 1);
                (checks == 1).then_some(ShutdownReason::SourceCompleted)
            },
            || Ok(()),
        )
        .unwrap();

        assert_eq!(result.status, RecordingStatus::Completed);
        assert_eq!(result.shutdown_reason, ShutdownReason::SourceCompleted);
        assert_eq!(result.counters.committed.records, 2);
        assert!(result.last_valid_commit.is_some());
        let persisted = load_recording_metadata(&directory).unwrap().unwrap();
        assert_eq!(persisted.status, RecordingStatus::Completed);
        assert!(persisted.last_valid_commit.is_some());
        assert_eq!(persisted.counters.committed.records, 2);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Lifecycle variants share one recovered WAL fixture.
    fn recording_scoped_etl_preserves_finalized_source_provenance() {
        use chronicle_capture::InMemoryCaptureSource;
        use chronicle_common::SessionId;
        use std::cell::Cell;

        let directory = ingest_directory("recording-etl");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        metadata.recording_id = RecordingId(uuid::Uuid::from_u128(1));
        let events = test_capture_events(b"opaque".to_vec());
        let stop_checks = Cell::new(0_u8);
        let recorded = record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            || Ok(InMemoryCaptureSource::new(events)),
            || 1,
            || {
                let checks = stop_checks.get();
                stop_checks.set(checks + 1);
                (checks == 1).then_some(ShutdownReason::SourceCompleted)
            },
            || Ok(()),
        )
        .unwrap();
        let suffix = chronicle_wal::WalRecordEnvelope::unplaced(
            recorded.recording_id,
            recorded.last_valid_commit.as_ref().unwrap().marker_sequence + 1,
            RecordKind::CaptureEvent,
            1,
            0,
            b"written but not durable".to_vec(),
        );
        OpenOptions::new()
            .append(true)
            .open(
                directory
                    .join("segments")
                    .join(chronicle_wal::segment_file_name(1)),
            )
            .unwrap()
            .write_all(&chronicle_wal::encode_envelope(&suffix).unwrap())
            .unwrap();

        let processed = process_recording_wal(
            &directory,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(
            processed.output.session.source_provenance.status,
            SourceStatus::Completed
        );
        assert_eq!(
            processed.output.session.source_provenance.reason.as_deref(),
            Some("source_completed")
        );
        assert_eq!(processed.ignored_post_commit_records, 1);
        assert!(
            !processed
                .output
                .issues
                .iter()
                .any(|issue| { issue.kind == chronicle_etl::EtlIssueKind::MalformedCaptureEvent })
        );
        assert_eq!(
            processed.output.session.source_provenance.evidence,
            [
                ProvenanceEntry {
                    kind: ProvenanceKind::WalEnvelope,
                    sequence_range: Some((1, 3)),
                    reason: None,
                },
                ProvenanceEntry {
                    kind: ProvenanceKind::Connection,
                    sequence_range: Some((2, 2)),
                    reason: None,
                },
                ProvenanceEntry {
                    kind: ProvenanceKind::CommitMarker,
                    sequence_range: Some((3, 3)),
                    reason: None,
                },
            ]
        );
        let repeated = process_recording_wal(
            &directory,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(repeated.output.session.id, processed.output.session.id);
        assert_eq!(
            repeated.output.session.connections[0].id,
            processed.output.session.connections[0].id
        );
        assert_eq!(
            repeated.output.session.connections[0].operations[0].id,
            processed.output.session.connections[0].operations[0].id
        );
        let root = ingest_directory("recording-publish");
        let first = process_and_publish_recording_wal(
            &directory,
            &root,
            &chronicle_protocol_builtins::registry().unwrap(),
        )
        .unwrap();
        assert!(!first.already_published);
        assert!(!directory.join("output-binding.json").exists());
        let checkpoint_path = directory.join("etl-checkpoint.json");
        let first_checkpoint = load_recording_etl_checkpoint(&directory).unwrap().unwrap();
        assert_eq!(first_checkpoint.session_id, first.session_id);
        assert_eq!(first_checkpoint.commit_marker_sequence, 3);
        assert_eq!(first_checkpoint.status, RecordingStatus::Completed);
        std::fs::remove_file(&checkpoint_path).unwrap();
        let same_root = process_and_publish_recording_wal(
            &directory,
            &root,
            &chronicle_protocol_builtins::registry().unwrap(),
        )
        .unwrap();
        assert_eq!(same_root.session_id, first.session_id);
        assert!(same_root.already_published);
        assert!(checkpoint_path.exists());
        let second_root = ingest_directory("recording-second-root");
        assert!(
            !process_and_publish_recording_wal(
                &directory,
                &second_root,
                &chronicle_protocol_builtins::registry().unwrap(),
            )
            .unwrap()
            .already_published
        );
        assert_ne!(
            load_recording_etl_checkpoint(&directory)
                .unwrap()
                .unwrap()
                .output_root,
            first_checkpoint.output_root
        );
        let mismatch_root = ingest_directory("recording-mismatch-root");
        let mut mismatched = processed.output.session.clone();
        mismatched.source_provenance.reason = Some("mismatch".into());
        FilesystemSessionStore::new(&mismatch_root)
            .publish(PublishSession {
                session: mismatched,
                checkpoint: None,
                issues: issue_summaries(&processed.output.issues)
                    .into_iter()
                    .map(|issue| issue.code)
                    .collect(),
                replayability: replayability_reasons(&processed.output),
                complete: session_complete(&processed.output),
            })
            .unwrap();
        assert!(matches!(
            process_and_publish_recording_wal(
                &directory,
                &mismatch_root,
                &chronicle_protocol_builtins::registry().unwrap(),
            ),
            Err(ApplicationError::PublishedRecordingMismatch { .. })
        ));

        for (status, reason, expected) in [
            (
                RecordingStatus::Failed,
                ShutdownReason::CaptureFailure,
                SourceStatus::Failed,
            ),
            (
                RecordingStatus::Aborted,
                ShutdownReason::ProcessCrashRecovered,
                SourceStatus::Aborted,
            ),
        ] {
            let mut metadata = load_recording_metadata(&directory).unwrap().unwrap();
            metadata.status = status;
            metadata.shutdown_reason = Some(reason);
            write_recording_metadata(&directory, &metadata).unwrap();
            let processed = process_recording_wal(
                &directory,
                &chronicle_protocol_builtins::registry().unwrap(),
                SessionId::new(),
            )
            .unwrap();
            assert_eq!(processed.output.session.source_provenance.status, expected);
            assert_eq!(
                processed.output.session.source_provenance.reason.as_deref(),
                Some(shutdown_reason_name(reason))
            );
        }
        let segment = directory
            .join("segments")
            .join(chronicle_wal::segment_file_name(1));
        let mut bytes = std::fs::read(&segment).unwrap();
        bytes[64] ^= 0xff; // First committed envelope magic; never accepted as a recoverable tail.
        std::fs::write(&segment, bytes).unwrap();
        assert!(matches!(
            process_recording_wal(
                &directory,
                &chronicle_protocol_builtins::registry().unwrap(),
                SessionId::new(),
            ),
            Err(ApplicationError::Wal(_))
        ));
        std::fs::remove_dir_all(directory).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(second_root).unwrap();
        std::fs::remove_dir_all(mismatch_root).unwrap();
    }

    #[test]
    fn metadata_only_terminal_loss_becomes_typed_etl_evidence() {
        let clock = ClockIdentity {
            boot_id: "boot".into(),
        };
        let timestamp = |nanoseconds| MonotonicTimestamp {
            clock: clock.clone(),
            nanoseconds,
        };
        let summary = TerminalWalLossSummary {
            entries: vec![
                TerminalWalLossSummaryEntry {
                    clock: Some(clock.clone()),
                    start: Some(timestamp(10)),
                    end: Some(timestamp(20)),
                    discarded: RecordByteCount {
                        records: 3,
                        bytes: 4,
                    },
                    time_source: TerminalWalLossTimeSource::Observed,
                    persistence: TerminalWalLossPersistence::MetadataOnly,
                },
                TerminalWalLossSummaryEntry {
                    clock: None,
                    start: None,
                    end: None,
                    discarded: RecordByteCount::default(),
                    time_source: TerminalWalLossTimeSource::TimestampUnavailable,
                    persistence: TerminalWalLossPersistence::PersistedWal,
                },
            ],
            discarded: RecordByteCount {
                records: 3,
                bytes: 4,
            },
        };

        let losses = metadata_terminal_wal_losses(Some(&summary));
        assert_eq!(losses.len(), 1);
        assert_eq!(losses[0].discarded_records, 3);
        assert_eq!(losses[0].discarded_payload_bytes, 4);
    }

    #[test]
    fn recording_scoped_etl_recovers_stale_active_metadata() {
        use chronicle_capture::InMemoryCaptureSource;
        use chronicle_common::SessionId;
        use std::cell::Cell;

        let directory = ingest_directory("active-recording-etl");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        metadata.recording_id = RecordingId(uuid::Uuid::from_u128(7));
        let checks = Cell::new(0_u8);
        record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            || {
                Ok(InMemoryCaptureSource::new(test_capture_events(
                    b"opaque".to_vec(),
                )))
            },
            || 1,
            || {
                let count = checks.get();
                checks.set(count + 1);
                (count == 1).then_some(ShutdownReason::SourceCompleted)
            },
            || Ok(()),
        )
        .unwrap();

        let mut stale = load_recording_metadata(&directory).unwrap().unwrap();
        stale.status = RecordingStatus::Recording;
        stale.shutdown_reason = None;
        write_recording_metadata(&directory, &stale).unwrap();

        let processed = process_recording_wal(
            &directory,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(processed.status, RecordingStatus::Aborted);
        assert_eq!(
            processed.output.session.source_provenance.status,
            SourceStatus::Aborted
        );
        let report = decode_recovery_report(
            &std::fs::read(directory.join("etl/recovery-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(report.status, RecordingStatus::Aborted);
        assert_eq!(
            report.shutdown_reason,
            Some(ShutdownReason::ProcessCrashRecovered)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn production_recording_marks_source_failure_failed_after_finalization() {
        use std::sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        };

        struct FailingSource {
            finalized: Arc<AtomicBool>,
        }

        impl CaptureSource for FailingSource {
            fn request_shutdown(&mut self) -> Result<(), CaptureError> {
                Err(CaptureError::FinalLossSample(
                    "injected final sample failure".into(),
                ))
            }

            fn drain(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
                Ok(None)
            }

            fn finalize(&mut self) -> Result<CaptureSourceSummary, CaptureError> {
                self.finalized.store(true, Ordering::SeqCst);
                Ok(CaptureSourceSummary::default())
            }

            fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
                Err(CaptureError::Source("injected source failure".into()))
            }
        }

        let directory = ingest_directory("production-source-failure");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        let finalized = Arc::new(AtomicBool::new(false));
        let result = record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            {
                let finalized = Arc::clone(&finalized);
                move || Ok(FailingSource { finalized })
            },
            || 1,
            || None,
            || Ok(()),
        )
        .unwrap();

        assert!(finalized.load(Ordering::SeqCst));
        assert_eq!(result.status, RecordingStatus::Failed);
        assert_eq!(result.shutdown_reason, ShutdownReason::CaptureFailure);
        assert_eq!(
            load_recording_metadata(&directory).unwrap().unwrap().status,
            RecordingStatus::Failed
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recording_capture_metadata_is_versioned_bounded_and_transitions_live_status() {
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        let core_bytes = serde_json::to_vec(&metadata).unwrap();
        assert_eq!(
            decode_recording_metadata(&core_bytes).unwrap().capture,
            None
        );

        metadata
            .transition_to_recording(capture_metadata())
            .unwrap();
        assert_eq!(metadata.status, RecordingStatus::Recording);
        assert!(metadata.capture.is_some());
        metadata
            .finalize(RecordingStatus::Completed, ShutdownReason::DurationLimit)
            .unwrap();
        assert_eq!(
            metadata.shutdown_reason,
            Some(ShutdownReason::DurationLimit)
        );
        assert_eq!(
            decode_recording_metadata(&serde_json::to_vec(&metadata).unwrap()).unwrap(),
            metadata
        );

        let mut invalid = recording_metadata(RecordingStatus::Starting);
        let mut capture = capture_metadata();
        capture.errors = vec![
            RecordingCaptureError {
                code: RecordingCaptureErrorCode::Source,
            };
            MAX_RECORDING_CAPTURE_ERRORS + 1
        ];
        assert!(matches!(
            invalid.transition_to_recording(capture),
            Err(ApplicationError::RecordingMetadataValidation(_))
        ));
    }

    #[test]
    fn record_production_on_ready_fires_once_after_writer_ready() {
        use chronicle_capture::InMemoryCaptureSource;
        use std::cell::Cell;

        let directory = ingest_directory("production-on-ready");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        let events = test_capture_events(b"ready".to_vec());
        let ready_checks = Cell::new(0_u8);
        let recorded = record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            || Ok(InMemoryCaptureSource::new(events)),
            || 1,
            || Some(ShutdownReason::SourceCompleted),
            || {
                // The WAL writer must already be durable when the hook fires:
                // the metadata must have transitioned to recording and a
                // recording.json must exist.
                let persisted = load_recording_metadata(&directory).unwrap().unwrap();
                assert_eq!(persisted.status, RecordingStatus::Recording);
                let checks = ready_checks.get();
                ready_checks.set(checks + 1);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(ready_checks.get(), 1);
        assert_eq!(recorded.status, RecordingStatus::Completed);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn record_production_on_ready_failure_aborts_as_chronicle_failure() {
        use chronicle_capture::InMemoryCaptureSource;
        use std::cell::Cell;

        let directory = ingest_directory("production-on-ready-failure");
        let mut metadata = recording_metadata(RecordingStatus::Starting);
        let events = test_capture_events(b"never-run".to_vec());
        let ready_checks = Cell::new(0_u8);
        let err = record_production(
            &directory,
            &mut metadata,
            capture_metadata(),
            ProductionRecordingBounds {
                duration_seconds: 60,
                segment_bytes: MIN_SEGMENT_BYTES,
                max_wal_bytes: MIN_SEGMENT_BYTES,
            },
            || Ok(InMemoryCaptureSource::new(events)),
            || 1,
            || Some(ShutdownReason::SourceCompleted),
            || {
                ready_checks.set(ready_checks.get() + 1);
                Err(ApplicationError::ProductionPreflight("readiness pipe died"))
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("readiness pipe died"));
        assert_eq!(ready_checks.get(), 1);
        // The recording is finalized as failed, never Completed.
        let persisted = load_recording_metadata(&directory).unwrap().unwrap();
        assert_eq!(persisted.status, RecordingStatus::Failed);
        assert_eq!(
            persisted.shutdown_reason,
            Some(ShutdownReason::CaptureFailure)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn capture_encoding_errors_do_not_become_wal_failures() {
        assert_eq!(
            recording_failure_reason(&ApplicationError::CaptureFlagsOutOfRange(u32::MAX)),
            ShutdownReason::CaptureFailure
        );
    }

    #[test]
    fn production_recording_bounds_enforce_duration_and_wal_limits() {
        assert_eq!(
            ProductionRecordingBounds::default(),
            ProductionRecordingBounds {
                duration_seconds: DEFAULT_RECORDING_DURATION_SECONDS,
                segment_bytes: DEFAULT_SEGMENT_BYTES,
                max_wal_bytes: DEFAULT_MAX_WAL_BYTES,
            }
        );
        for bounds in [
            ProductionRecordingBounds {
                duration_seconds: 0,
                ..ProductionRecordingBounds::default()
            },
            ProductionRecordingBounds {
                duration_seconds: MAX_RECORDING_DURATION_SECONDS + 1,
                ..ProductionRecordingBounds::default()
            },
            ProductionRecordingBounds {
                segment_bytes: MIN_SEGMENT_BYTES - 1,
                ..ProductionRecordingBounds::default()
            },
            ProductionRecordingBounds {
                max_wal_bytes: MIN_SEGMENT_BYTES - 1,
                ..ProductionRecordingBounds::default()
            },
        ] {
            assert!(matches!(
                validate_production_recording_bounds(bounds),
                Err(ApplicationError::ProductionPreflight(_))
            ));
        }
    }

    #[test]
    fn wal_preflight_is_exclusive_and_reports_free_space_without_metadata() {
        let root =
            std::env::temp_dir().join(format!("chronicle-preflight-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let wal = root.join("wal");
        let preflight =
            preflight_wal_destination(&wal, ProductionRecordingBounds::default()).unwrap();
        assert!(preflight.available_bytes > 0);
        assert!(!wal.exists());
        // An existing dir without WAL artifacts (e.g. only the private intent
        // sidecar) is not a conflict.
        std::fs::create_dir(&wal).unwrap();
        assert!(preflight_wal_destination(&wal, ProductionRecordingBounds::default()).is_ok());
        // An existing WAL (metadata or segments) is a conflict.
        std::fs::write(wal.join("recording.json"), b"{}").unwrap();
        assert!(matches!(
            preflight_wal_destination(&wal, ProductionRecordingBounds::default()),
            Err(ApplicationError::ProductionPreflight(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::too_many_lines)] // Full metadata-lag and mismatch flow stays auditable together.
    fn recovery_reconciles_wal_authority_without_acknowledgement_claim() {
        use chronicle_wal::{
            GroupCommitWalWriter, MIN_SEGMENT_BYTES, WalRecordEnvelope, encode_envelope, scan_wal,
            segment_file_name,
        };

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-reconcile-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let selector = RecordingSelectorIdentity {
            canonical_cgroup_path: "/sys/fs/cgroup/workload".into(),
            cgroup_id: 42,
        };
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let uncertain = WalRecordEnvelope::unplaced(
            recording_id,
            3,
            RecordKind::LossWindow,
            1,
            0,
            b"uncertain".to_vec(),
        );
        OpenOptions::new()
            .append(true)
            .open(wal_directory.join("segments").join(segment_file_name(1)))
            .unwrap()
            .write_all(&encode_envelope(&uncertain).unwrap())
            .unwrap();

        let mut metadata = recording_metadata(RecordingStatus::Starting);
        metadata.recording_id = recording_id;
        metadata.selector = Some(selector.clone());
        metadata.counters.received = RecordByteCount {
            records: 7,
            bytes: 70,
        };
        write_recording_metadata(&wal_directory, &metadata).unwrap();
        let before_read_only_scan = fs::read(wal_directory.join("recording.json")).unwrap();
        let scan = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
        assert_eq!(scan.uncommitted.len(), 1);
        assert_eq!(
            fs::read(wal_directory.join("recording.json")).unwrap(),
            before_read_only_scan
        );

        let reconciled =
            reconcile_recording_metadata(&wal_directory, recording_id, Some(&selector)).unwrap();
        assert!(reconciled.metadata_updated);
        assert_eq!(reconciled.metadata.status, RecordingStatus::Aborted);
        assert_eq!(
            reconciled.metadata.shutdown_reason,
            Some(ShutdownReason::ProcessCrashRecovered)
        );
        assert_eq!(
            reconciled.metadata.counters.received,
            RecordByteCount {
                records: 7,
                bytes: 70
            }
        );
        assert_eq!(
            reconciled.metadata.counters.committed,
            RecordByteCount {
                records: 1,
                bytes: 9
            }
        );
        assert_eq!(
            reconciled.metadata.counters.written_not_committed,
            RecordByteCount {
                records: 1,
                bytes: 9
            }
        );
        assert_eq!(
            reconciled.metadata.last_valid_commit,
            Some(RecordingCommitBoundary {
                marker_sequence: 2,
                durable_through_sequence: 1,
                durable_record_count: 1,
                durable_payload_bytes: 9,
                segment_ordinal: 0,
            })
        );
        assert_eq!(reconciled.scan.uncommitted[0].payload, b"uncertain");

        let bytes = fs::read(wal_directory.join("recording.json")).unwrap();
        let second =
            reconcile_recording_metadata(&wal_directory, recording_id, Some(&selector)).unwrap();
        assert!(!second.metadata_updated);
        assert_eq!(
            fs::read(wal_directory.join("recording.json")).unwrap(),
            bytes
        );

        let wrong_selector = RecordingSelectorIdentity {
            canonical_cgroup_path: "/sys/fs/cgroup/other".into(),
            cgroup_id: 99,
        };
        assert!(matches!(
            reconcile_recording_metadata(&wal_directory, recording_id, Some(&wrong_selector)),
            Err(ApplicationError::RecordingMetadataValidation(_))
        ));
        assert_eq!(
            fs::read(wal_directory.join("recording.json")).unwrap(),
            bytes
        );

        fs::remove_file(wal_directory.join("recording.json")).unwrap();
        let recreated = reconcile_recording_metadata(&wal_directory, recording_id, None).unwrap();
        assert!(recreated.metadata_updated);
        assert_eq!(recreated.metadata.recording_id, recording_id);
        assert_eq!(recreated.metadata.selector, None);
        assert_eq!(recreated.metadata.status, RecordingStatus::Aborted);
        assert_eq!(
            recreated.metadata.shutdown_reason,
            Some(ShutdownReason::ProcessCrashRecovered)
        );
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[test]
    fn recovery_issue_categories_distinguish_frame_marker_and_header_failures() {
        assert_eq!(
            recovery_issue_code(&WalError::EnvelopeChecksum {
                sequence: 1,
                kind: RecordKind::CaptureEvent,
                expected: 1,
                actual: 2,
            }),
            RecoveryIssueCode::FrameCrc
        );
        assert_eq!(
            recovery_issue_code(&WalError::EnvelopeChecksum {
                sequence: 2,
                kind: RecordKind::CommitMarker,
                expected: 1,
                actual: 2,
            }),
            RecoveryIssueCode::MarkerCrc
        );
        assert_eq!(
            recovery_issue_code(&WalError::UnsupportedEnvelopeVersion {
                version: 2,
                kind: RecordKind::CaptureEvent as u16,
            }),
            RecoveryIssueCode::FrameVersion
        );
        assert_eq!(
            recovery_issue_code(&WalError::UnsupportedEnvelopeVersion {
                version: 2,
                kind: RecordKind::CommitMarker as u16,
            }),
            RecoveryIssueCode::MarkerVersion
        );
        assert_eq!(
            recovery_issue_code(&WalError::SegmentHeaderChecksum {
                expected: 1,
                actual: 2,
            }),
            RecoveryIssueCode::HeaderCorruption
        );
    }

    #[cfg(unix)]
    #[test]
    fn recovery_report_is_body_safe_versioned_and_private() {
        use chronicle_wal::{RecoveryRepair, WalRecordEnvelope};
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("chronicle-report-{}", uuid::Uuid::new_v4()));
        let mut metadata = recording_metadata(RecordingStatus::Aborted);
        metadata.shutdown_reason = Some(ShutdownReason::ProcessCrashRecovered);
        metadata.counters.committed = RecordByteCount {
            records: 1,
            bytes: 4,
        };
        metadata.counters.discarded_from_queue_due_to_wal_limit = RecordByteCount {
            records: 2,
            bytes: 20,
        };
        let mut scan = RecoveryScan::default();
        scan.uncommitted.push(WalRecordEnvelope::unplaced(
            metadata.recording_id,
            3,
            RecordKind::CaptureEvent,
            1,
            0,
            b"super-secret-body".to_vec(),
        ));
        let reconciliation = RecordingRecoveryReconciliation {
            metadata,
            scan,
            metadata_updated: true,
        };
        let repair = RecoveryRepair {
            repaired_bytes: 5,
            truncated_to: Some(100),
            scan: RecoveryScan::default(),
        };
        let report = build_recovery_report(
            &reconciliation,
            Some(&repair),
            [RecoveryIssueCode::MarkerDigest],
        )
        .unwrap();
        assert_eq!(
            report.issue_codes,
            vec![
                RecoveryIssueCode::TailRepaired,
                RecoveryIssueCode::MarkerDigest,
                RecoveryIssueCode::WalLimitLoss,
            ]
        );
        let rendered = render_recovery_report_json(&report).unwrap();
        assert!(!rendered.contains("super-secret-body"));
        assert_eq!(report.post_marker.records, 1);
        assert_eq!(report.post_marker.bytes, 17);
        assert_eq!(report.post_marker.first_sequence, Some(3));

        persist_recovery_report(&directory, &report).unwrap();
        let report_directory = directory.join("etl");
        let report_path = report_directory.join("recovery-report.json");
        assert_eq!(
            report_directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            report_path.metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
        let bytes = fs::read(&report_path).unwrap();
        assert_eq!(decode_recovery_report(&bytes).unwrap(), report);
        let mut invalid: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        invalid["version"] = (RECOVERY_REPORT_VERSION + 1).into();
        assert!(matches!(
            decode_recovery_report(&serde_json::to_vec(&invalid).unwrap()),
            Err(ApplicationError::RecordingMetadataValidation(_))
        ));
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_transaction_reports_before_discard_and_hands_off_lock() {
        use chronicle_wal::{
            MIN_SEGMENT_BYTES, WalRecordEnvelope, encode_envelope, segment_file_name,
        };

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-recover-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(segment_file_name(1));
        let uncertain = WalRecordEnvelope::unplaced(
            recording_id,
            3,
            RecordKind::LossWindow,
            1,
            0,
            b"uncertain".to_vec(),
        );
        OpenOptions::new()
            .append(true)
            .open(&segment_path)
            .unwrap()
            .write_all(&encode_envelope(&uncertain).unwrap())
            .unwrap();
        let mut metadata = recording_metadata(RecordingStatus::Recording);
        metadata.recording_id = recording_id;
        write_recording_metadata(&wal_directory, &metadata).unwrap();

        let recovered = recover_recording_for_append(
            &wal_directory,
            recording_id,
            None,
            MIN_SEGMENT_BYTES,
            MIN_SEGMENT_BYTES,
            1,
        )
        .unwrap();
        assert_eq!(recovered.recovery_report.post_marker.records, 1);
        assert!(recovered.recovery_report.repaired_tail.is_some());
        assert_eq!(
            recovered
                .reconciliation
                .metadata
                .counters
                .written_not_committed,
            RecordByteCount::default()
        );
        assert!(matches!(
            RecordingLock::acquire(&wal_directory),
            Err(WalError::RecordingLockHeld)
        ));
        let persisted = decode_recovery_report(
            &fs::read(wal_directory.join("etl/recovery-report.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(persisted, recovered.recovery_report);
        drop(recovered.writer);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn corruption_failure_report_is_specific_body_safe_and_non_destructive() {
        use chronicle_wal::{MIN_SEGMENT_BYTES, SEGMENT_HEADER_LEN, segment_file_name};

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-corrupt-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"secret-body".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(segment_file_name(1));
        let mut corrupted = fs::read(&segment_path).unwrap();
        corrupted[SEGMENT_HEADER_LEN + chronicle_wal::ENVELOPE_HEADER_LEN] ^= 0xff;
        fs::write(&segment_path, &corrupted).unwrap();

        assert!(matches!(
            recover_recording_for_append(
                &wal_directory,
                recording_id,
                None,
                MIN_SEGMENT_BYTES,
                MIN_SEGMENT_BYTES,
                1,
            ),
            Err(ApplicationError::Wal(WalError::EnvelopeChecksum { .. }))
        ));
        assert_eq!(fs::read(&segment_path).unwrap(), corrupted);
        let bytes = fs::read(wal_directory.join("etl/recovery-report.json")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("secret-body"));
        let report = decode_recovery_report(&bytes).unwrap();
        assert_eq!(report.issue_codes, vec![RecoveryIssueCode::FrameCrc]);
        assert_eq!(report.status, RecordingStatus::Failed);
        fs::remove_dir_all(wal_directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recording_metadata_round_trip_versions_and_private_atomic_update() {
        use std::os::unix::fs::PermissionsExt;

        let directory =
            std::env::temp_dir().join(format!("chronicle-metadata-{}", uuid::Uuid::new_v4()));
        let metadata = recording_metadata(RecordingStatus::Starting);
        write_recording_metadata(&directory, &metadata).unwrap();
        assert_eq!(
            load_recording_metadata(&directory).unwrap(),
            Some(metadata.clone())
        );
        assert_eq!(
            directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            directory
                .join("recording.json")
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );

        fs::write(directory.join(".recording.json.tmp-stale"), b"stale").unwrap();
        let mut aborted = metadata.clone();
        aborted.status = RecordingStatus::Aborted;
        aborted.shutdown_reason = Some(ShutdownReason::ProcessCrashRecovered);
        aborted.counters.committed = RecordByteCount {
            records: 2,
            bytes: 10,
        };
        write_recording_metadata(&directory, &aborted).unwrap();
        assert_eq!(load_recording_metadata(&directory).unwrap(), Some(aborted));
        fs::remove_file(directory.join(".recording.json.tmp-stale")).unwrap();

        let bytes = fs::read(directory.join("recording.json")).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        for version in [0, RECORDING_METADATA_SCHEMA_VERSION + 1] {
            let mut invalid = value.clone();
            invalid["version"] = version.into();
            assert!(matches!(
                decode_recording_metadata(&serde_json::to_vec(&invalid).unwrap()),
                Err(ApplicationError::RecordingMetadataValidation(_))
            ));
        }
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recording_metadata_faults_preserve_previous_atomic_value() {
        let directory =
            std::env::temp_dir().join(format!("chronicle-metadata-fault-{}", uuid::Uuid::new_v4()));
        let original = recording_metadata(RecordingStatus::Starting);
        write_recording_metadata(&directory, &original).unwrap();
        let mut replacement = original.clone();
        replacement.status = RecordingStatus::Failed;
        replacement.shutdown_reason = Some(ShutdownReason::WalFailure);

        for fault in [
            RecordingMetadataWriteFault::BeforeFileSync,
            RecordingMetadataWriteFault::BeforeRename,
        ] {
            assert!(write_recording_metadata_inner(&directory, &replacement, Some(fault)).is_err());
            assert_eq!(
                load_recording_metadata(&directory).unwrap(),
                Some(original.clone())
            );
            assert!(fs::read_dir(&directory).unwrap().all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')
            }));
        }

        assert!(
            write_recording_metadata_inner(
                &directory,
                &replacement,
                Some(RecordingMetadataWriteFault::AfterRename)
            )
            .is_err()
        );
        assert_eq!(
            load_recording_metadata(&directory).unwrap(),
            Some(replacement)
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn shared_json_renderer_types_serialization_failures() {
        struct Fails;

        impl Serialize for Fails {
            fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                Err(serde::ser::Error::custom("injected failure"))
            }
        }

        assert!(matches!(
            render_json(&Fails),
            Err(ApplicationError::JsonSerialization(_))
        ));
    }

    fn storage_probe(report: &DoctorReport, name: &str) -> DoctorStatus {
        report
            .probes
            .iter()
            .find(|probe| probe.code == format!("storage.{name}"))
            .unwrap()
            .status
    }

    #[test]
    fn doctor_path_probes_existing_and_missing_directories_without_artifacts() {
        let root = std::env::temp_dir().join(format!("chronicle-doctor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let wal_dir = root.join("wal");
        let output = root.join("output");
        fs::create_dir(&wal_dir).unwrap();
        fs::create_dir(&output).unwrap();

        let report = doctor_report(&DoctorOptions {
            wal_dir: Some(wal_dir.clone()),
            output: Some(output.clone()),
            state_root: None,
            config: None,
        });
        assert!(matches!(
            storage_probe(&report, "wal_dir"),
            DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings
        ));
        assert!(matches!(
            storage_probe(&report, "output"),
            DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings
        ));

        let missing = root.join("new-output");
        let missing_report = doctor_report(&DoctorOptions {
            wal_dir: Some(missing.clone()),
            output: None,
            state_root: None,
            config: None,
        });
        assert!(matches!(
            storage_probe(&missing_report, "wal_dir"),
            DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings
        ));
        assert!(!missing.exists());
        assert_eq!(
            storage_probe(&missing_report, "output"),
            DoctorStatus::NotChecked
        );
        for directory in [&root, &wal_dir, &output] {
            assert!(fs::read_dir(directory).unwrap().all(|entry| {
                let name = entry.unwrap().file_name();
                let name = name.to_string_lossy();
                !name.starts_with(".chronicle-doctor-")
                    && !name.starts_with(".chronicle-doctor-published-")
            }));
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn doctor_path_probes_reject_files_and_missing_parents() {
        let root = std::env::temp_dir().join(format!("chronicle-doctor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let file = root.join("not-a-directory");
        fs::write(&file, b"user data").unwrap();
        let report = doctor_report(&DoctorOptions {
            wal_dir: Some(file.clone()),
            output: Some(root.join("missing-parent").join("output")),
            state_root: None,
            config: None,
        });
        assert_eq!(storage_probe(&report, "wal_dir"), DoctorStatus::Unsupported);
        assert_eq!(storage_probe(&report, "output"), DoctorStatus::Unsupported);
        assert_eq!(fs::read(&file).unwrap(), b"user data");
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn doctor_path_probe_rejects_read_only_directory_when_kernel_enforces_mode() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!("chronicle-doctor-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let read_only = root.join("read-only");
        fs::create_dir(&read_only).unwrap();
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555)).unwrap();
        let report = doctor_report(&DoctorOptions {
            wal_dir: Some(read_only.clone()),
            output: None,
            state_root: None,
            config: None,
        });
        if !matches!(
            storage_probe(&report, "wal_dir"),
            DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings
        ) {
            assert_eq!(storage_probe(&report, "wal_dir"), DoctorStatus::Unsupported);
        }
        fs::set_permissions(&read_only, fs::Permissions::from_mode(0o700)).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn doctor_status_precedence_and_exit_codes_are_stable() {
        let probe = |required, status| DoctorProbe {
            required,
            status,
            code: "test".into(),
            message: "test".into(),
            remediation: "test".into(),
        };
        for (probes, expected, exit) in [
            (
                vec![
                    probe(true, DoctorStatus::NotChecked),
                    probe(true, DoctorStatus::Unsupported),
                ],
                DoctorStatus::Unsupported,
                4,
            ),
            (
                vec![probe(true, DoctorStatus::NotChecked)],
                DoctorStatus::NotChecked,
                4,
            ),
            (
                vec![
                    probe(true, DoctorStatus::Supported),
                    probe(false, DoctorStatus::Unsupported),
                ],
                DoctorStatus::SupportedWithWarnings,
                0,
            ),
            (
                vec![probe(true, DoctorStatus::Supported)],
                DoctorStatus::Supported,
                0,
            ),
        ] {
            let report = DoctorReport::new(probes);
            assert_eq!(report.status, expected);
            assert_eq!(report.exit_code(), exit);
        }
    }

    #[test]
    fn doctor_replay_configuration_failures_are_actionable() {
        let supported = replay_config_doctor_probe(&ReplayConfig::default());
        assert_eq!(supported.status, DoctorStatus::Supported);

        let invalid = replay_config_doctor_probe(&ReplayConfig {
            authorization_env: Some("bad=name".into()),
            ..ReplayConfig::default()
        });
        assert_eq!(invalid.status, DoctorStatus::Unsupported);

        let ignored_grants = replay_config_doctor_probe(&ReplayConfig {
            dry_run: false,
            allow_writes: true,
            ..ReplayConfig::default()
        });
        assert_eq!(ignored_grants.status, DoctorStatus::SupportedWithWarnings);

        let missing_name = format!("CHRONICLE_DOCTOR_MISSING_{}", uuid::Uuid::new_v4().simple());
        let missing = replay_config_doctor_probe(&ReplayConfig {
            authorization_env: Some(missing_name),
            ..ReplayConfig::default()
        });
        assert_eq!(missing.status, DoctorStatus::NotChecked);

        for probe in [supported, invalid, ignored_grants, missing] {
            assert!(!probe.remediation.is_empty());
        }
    }

    #[test]
    fn doctor_report_never_emits_empty_remediation() {
        let report = doctor_report(&DoctorOptions::default());
        assert!(
            report
                .probes
                .iter()
                .all(|probe| !probe.remediation.trim().is_empty())
        );
    }

    #[test]
    fn replay_report_v1_serializes_outcome_and_operation_state() {
        let result = ReplaySessionResult {
            session_id: "session".into(),
            replayability: Replayability::FullyReplayable,
            outcome: ReplayOutcome::DryRun,
            dry_run: true,
            preflight_denied: false,
            transport_failed: false,
            counts: ReplayCounts {
                executable: 1,
                unattempted: 1,
                verification_not_run: 1,
                ..ReplayCounts::default()
            },
            operations: vec![ReplayOperationSummary {
                operation_id: "operation".into(),
                decision: "allowed".into(),
                state: OperationExecutionState::NotAttempted,
                attempted: false,
                verification: "not_run".into(),
                category: "dry_run".into(),
                transport_error: String::new(),
            }],
        };
        let value: serde_json::Value =
            serde_json::from_str(&render_replay_json(&result).unwrap()).unwrap();
        assert_eq!(value["version"], REPLAY_REPORT_VERSION);
        assert_eq!(value["result"]["outcome"], "dry_run");
        assert_eq!(value["result"]["counts"]["verification_not_run"], 1);
        assert_eq!(value["result"]["operations"][0]["state"], "not_attempted");
        assert_eq!(
            result.counts,
            ReplayCounts::from_operations(&result.operations)
        );
    }
}
