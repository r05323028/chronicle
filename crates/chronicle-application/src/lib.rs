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
#[cfg(target_os = "linux")]
pub use record::record_continuous_ebpf;
#[cfg(all(test, target_os = "linux"))]
pub(crate) use record::recover_rollover_transition_v2_for_root;
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
    preflight_wal_destination, preflight_wal_destination_for_lifetime,
    validate_epoch_recording_bounds, validate_production_recording_bounds,
};
#[cfg(test)]
pub(crate) use record::{
    LIFECYCLE_INDEX_FILE, apply_retention_cleanup, compact_lifecycle_index, load_lifecycle_index,
    mark_epoch_cleanup_complete, save_lifecycle_index,
};
#[cfg(target_os = "linux")]
pub(crate) use record::{
    preflight_embedded_ebpf, public_continuous_config, record_continuous_ebpf_with_source,
};
mod epoch_catalog;
mod etl;
pub use etl::{
    PostMarkerUncertainty, PublishedRecordingResult, RECORDING_ETL_CHECKPOINT_VERSION, RecordedWal,
    RecordingEtlCheckpoint, RecordingEtlResult, RecordingRecoveryReconciliation,
    RecoveredRecordingForAppend, RecoveryIssueCode, RecoveryReport, RecoveryTailRepairSummary,
    build_recovery_report, build_recovery_report_for_reopen, decode_recording_metadata,
    decode_recovery_report, load_recording_metadata, mark_recording_forced_termination,
    persist_recovery_report, process_and_publish_recording_wal,
    process_and_publish_recording_wal_with_parent, process_fixture_wal, process_recording_wal,
    process_recording_wal_with_parent, reconcile_recording_metadata,
    reconcile_recording_metadata_with_scan, recording_physical_wal_bytes,
    recover_recording_for_append, recovery_issue_code, render_recovery_report_json,
    resolve_epoch_parent_context, write_capture_to_wal, write_recording_metadata,
};
#[cfg(target_os = "linux")]
pub(crate) use etl::{
    has_published_wal, load_production_ebpf_source, process_and_publish_recording_wal_owned,
};
pub(crate) use etl::{
    load_recording_etl_checkpoint, sha256_string, validate_recording_metadata,
    write_private_atomic_json,
};
#[cfg(test)]
pub(crate) use etl::{reserve_publication_peak, write_recording_metadata_inner};
mod epoch_rollover;
mod incremental_runtime;
mod incremental_worker;
#[cfg(any(target_os = "linux", test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod listener_discovery;
mod recorder_config;
mod replay_inspect;
pub use chronicle_common::{
    EpochId, EpochIdParseError, RecordingId, SessionId, Timestamp, escape_control,
};
pub use replay_inspect::{
    InspectConnectionSummary, InspectOperationSummary, InspectSessionResult, OperationStatus,
    ParentReplayEpochPlan, ParentReplayPlan, RecordFixtureResult, RecordIssueSummary, ReplayCounts,
    ReplayOperationSummary, ReplayRequest, ReplaySessionResult, ReplayStatus, ReplayabilityStatus,
    build_parent_replay_plan, inspect_session, preplan_command_replay_session,
    production_wal_from_fixture, record_fixture, record_fixture_file, render_inspect_human,
    render_inspect_json, render_json, render_replay_human, render_replay_json,
    replay_command_inputs, replay_context, replay_parent_session_with_plan, replay_session,
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
mod recording_run;
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
    EPOCH_CATALOG_FILE, EPOCH_CATALOG_SCHEMA_VERSION, EpochAuthorityError, EpochCatalog,
    EpochCatalogEntry, EpochCatalogError, EpochCatalogState, EpochCatalogSummary,
    EpochEvidenceAuthority, EpochRecoveryDecision, EpochRecoveryEvidence, reconcile_epoch_evidence,
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
    list_parent_recording_views, list_recording_ids, list_recordings, load_catalog,
    load_recording_intent, persist_reconciled_catalog, reconcile_catalog, reconcile_entry_status,
    recording_intent_path, recordings_root, resolve_recording, resolve_session, save_catalog,
    validate_catalog, validate_recording_name, write_recording_intent,
};
pub use recording_run::{
    CaptureMode, ContinuousRecordingCoordinator, CoordinatorAction, EpochBounds,
    ParentRecordingViewV2, RECORDING_RUN_FILE, RECORDING_RUN_SCHEMA_VERSION,
    RecordingDurationError, RecordingLifetime, RecordingRunV2, RunEpochSummaryV2,
    RunLifecycleState, RunPersistenceError, RunStopController, RunStopPolicy, RunStopReason,
    SegmentBounds, TargetOwnership, parse_recording_duration, sort_parent_recording_views,
};
pub use rollover_transition::{
    ROLLOVER_TRANSITION_FILE, ROLLOVER_TRANSITION_SCHEMA_VERSION, RolloverTransition,
    RolloverTransitionError, RolloverTransitionPhase, load_transition, remove_transition,
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
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureError, CaptureEventKind, CaptureSource,
    CaptureSourceSummary, ClockIdentity, FixtureCaptureSource, MonotonicTimestamp, encode_event,
};

use chronicle_etl::{
    ETL_PIPELINE_VERSION, EpochContinuationCheckpoint, EtlError, EtlIssue, EtlOutput, EtlPipeline,
    assign_recording_ids_with_snapshot, stamp_epoch_operation_provenance,
};
use chronicle_protocol::{
    ProtocolError, ProtocolRegistry, ReplayContext, SecretBytes, TransportErrorCategory,
};
use chronicle_replay::{
    LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayExecutor, ReplayOutcome,
    ReplayPlan, ReplayPlanner, ReplayPolicy, ReplayTargetError, Replayability, TargetMap,
    TargetRule, TimingMode,
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
    prepare_group_commit_reopen_from_scan, verified_snapshot_sha256,
};
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
use std::time::{Instant, UNIX_EPOCH};
use thiserror::Error;

pub const RECORDING_METADATA_SCHEMA_VERSION: u16 = 1;
pub const RECORDING_COUNTERS_SCHEMA_VERSION: u16 = 1;
pub const RECORDING_CAPTURE_METADATA_SCHEMA_VERSION: u16 = 1;
pub const MAX_RECORDING_CAPTURE_ERRORS: usize = 64;
pub const MAX_RECORDING_CAPABILITIES: usize = 32;
pub const REPLAY_REPORT_VERSION: u16 = 1;
pub const RECOVERY_REPORT_VERSION: u16 = 1;
pub(crate) const DEFAULT_RECORDING_DURATION_SECONDS: u64 = 600;
pub(crate) const MAX_RECORDING_DURATION_SECONDS: u64 = 3_600;
pub const RECORDING_FINALIZATION_GRACE_MILLIS: u64 = 5_000;
pub const DOCTOR_REPORT_VERSION: u16 = 1;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[cfg(target_os = "linux")]
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

/// Stable replay-result exit-code classification for outer adapters.
/// Operates on the application-owned result contract so adapters never touch
/// the replay-owned outcome taxonomy.
pub fn replay_result_exit_code(result: &ReplaySessionResult) -> i32 {
    match result.outcome {
        ReplayStatus::Completed | ReplayStatus::CompletedWithSkips | ReplayStatus::DryRun => 0,
        ReplayStatus::StoppedPolicy | ReplayStatus::StoppedInvalidSession => 4,
        ReplayStatus::StoppedTransport => 5,
        ReplayStatus::StoppedVerification => 6,
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
mod tests;
