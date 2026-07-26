//! Application services and configuration. CLI remains an outer adapter.

use chronicle_capture::{CaptureError, CaptureSource, FixtureCaptureSource, encode_event};
use chronicle_common::RecordingId;
use chronicle_etl::{EtlError, EtlIssue, EtlOutput, EtlPipeline};
use chronicle_protocol::{ProtocolError, ProtocolRegistry, ReplayContext, SecretBytes};
use chronicle_replay::{
    LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayExecutor, ReplayOutcome,
    ReplayPlan, ReplayPlanner, ReplayPolicy, ReplayTargetError, TargetMap, TimingMode,
};
use chronicle_session::SessionLimits;
use chronicle_storage::{FilesystemSessionStore, PublishSession, StorageError};
use chronicle_wal::{
    DEFAULT_MAX_RECORD_BYTES, GroupCommitWalWriter, RecordKind, RecordingLock,
    RecoveryReopenPreview, RecoveryReopenReport, RecoveryRepair, RecoveryScan, SegmentedWalWriter,
    WalCheckpoint, WalError, WalReader, WalRecord, WalWriter, encode_record,
    prepare_group_commit_reopen_from_scan,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const RECORDING_METADATA_SCHEMA_V1: u16 = 1;
pub const RECORDING_COUNTERS_SCHEMA_V1: u16 = 1;
pub const REPLAY_REPORT_VERSION: u16 = 2;
pub const RECOVERY_REPORT_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    Starting,
    Recording,
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    UserInterrupt,
    TerminationSignal,
    SourceCompleted,
    DurationLimit,
    WalSizeLimit,
    CaptureFailure,
    WalFailure,
    ProcessCrashRecovered,
    ForcedTermination,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSelectorIdentity {
    pub canonical_cgroup_path: String,
    pub cgroup_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordByteCount {
    pub records: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCounters {
    pub received: RecordByteCount,
    pub accepted_into_queue: RecordByteCount,
    pub written_not_committed: RecordByteCount,
    pub committed: RecordByteCount,
    pub discarded_from_queue_due_to_wal_limit: RecordByteCount,
    pub kernel_or_backend_dropped: RecordByteCount,
    pub rejected_after_stop: RecordByteCount,
    pub etl_checkpointed: RecordByteCount,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCommitBoundary {
    pub marker_sequence: u64,
    pub durable_through_sequence: u64,
    pub durable_record_count: u64,
    pub durable_payload_bytes: u64,
    pub segment_ordinal: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMetadataV1 {
    pub version: u16,
    pub recording_id: RecordingId,
    pub selector: Option<RecordingSelectorIdentity>,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub counters: RecordingCounters,
}

#[derive(Deserialize)]
struct RecordingMetadataDiscriminator {
    version: u16,
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingMetadataWriteFault {
    BeforeFileSync,
    BeforeRename,
    AfterRename,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub wal: WalConfig,
    pub capture: CaptureConfig,
    pub postgres: Option<PostgresConfig>,
    pub s3: Option<S3Config>,
    pub protocol_overrides: Vec<ProtocolOverride>,
    pub redaction: RedactionConfig,
    pub replay: ReplayConfig,
}

impl AppConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WalConfig {
    pub directory: PathBuf,
    pub segment_size_bytes: u64,
    pub disk_limit_bytes: u64,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("./chronicle-wal"),
            segment_size_bytes: 64 * 1024 * 1024,
            disk_limit_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub process_ids: Vec<u32>,
    pub network_namespaces: Vec<u64>,
    pub ports: Vec<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub connection_url_env: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: Option<String>,
    pub bucket: String,
    pub region: Option<String>,
    pub path_style: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtocolOverride {
    pub port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RedactionConfig {
    pub redact_headers: Vec<String>,
    pub drop_payloads: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // Independent safety gates remain visible in config.
pub struct ReplayConfig {
    pub target_mappings: Vec<TargetMappingConfig>,
    pub dry_run: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub allow_publication: bool,
    pub authorization_env: Option<String>,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            target_mappings: Vec::new(),
            dry_run: true,
            allow_reads: false,
            allow_writes: false,
            allow_publication: false,
            authorization_env: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetMappingConfig {
    pub protocol: Option<String>,
    pub recorded_host: Option<String>,
    pub recorded_port: Option<u16>,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Clone, Debug)]
pub enum ApplicationCommand {
    Record,
    Etl,
    Replay {
        session_id: String,
        timing: ReplayTiming,
    },
    Inspect {
        session_id: String,
    },
    Doctor,
}

#[derive(Clone, Copy, Debug)]
pub enum ReplayTiming {
    Preserve,
    Asap,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Capture(#[from] CaptureError),
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
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

pub fn decode_recording_metadata(bytes: &[u8]) -> Result<RecordingMetadataV1, ApplicationError> {
    let discriminator: RecordingMetadataDiscriminator = serde_json::from_slice(bytes)
        .map_err(|error| ApplicationError::RecordingMetadataValidation(error.to_string()))?;
    if discriminator.version != RECORDING_METADATA_SCHEMA_V1 {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recording metadata version {}",
            discriminator.version
        )));
    }
    let metadata: RecordingMetadataV1 = serde_json::from_slice(bytes)
        .map_err(|error| ApplicationError::RecordingMetadataValidation(error.to_string()))?;
    validate_recording_metadata(&metadata)?;
    Ok(metadata)
}

pub fn load_recording_metadata(
    wal_directory: impl AsRef<Path>,
) -> Result<Option<RecordingMetadataV1>, ApplicationError> {
    match fs::read(wal_directory.as_ref().join("recording.json")) {
        Ok(bytes) => decode_recording_metadata(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_recording_metadata(
    wal_directory: impl AsRef<Path>,
    metadata: &RecordingMetadataV1,
) -> Result<(), ApplicationError> {
    write_recording_metadata_inner(wal_directory.as_ref(), metadata, None)
}

fn validate_recording_metadata(metadata: &RecordingMetadataV1) -> Result<(), ApplicationError> {
    if metadata.version != RECORDING_METADATA_SCHEMA_V1 {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recording metadata version {}",
            metadata.version
        )));
    }
    let terminal = matches!(
        metadata.status,
        RecordingStatus::Completed | RecordingStatus::Failed | RecordingStatus::Aborted
    );
    if terminal != metadata.shutdown_reason.is_some() {
        return Err(ApplicationError::RecordingMetadataValidation(
            "terminal status and shutdown reason must be present together".into(),
        ));
    }
    Ok(())
}

fn write_recording_metadata_inner(
    wal_directory: &Path,
    metadata: &RecordingMetadataV1,
    fault: Option<RecordingMetadataWriteFault>,
) -> Result<(), ApplicationError> {
    validate_recording_metadata(metadata)?;
    write_private_atomic_json(wal_directory, "recording.json", metadata, fault)
}

fn write_private_atomic_json<T: Serialize + ?Sized>(
    directory: &Path,
    file_name: &str,
    value: &T,
    #[cfg_attr(not(test), allow(unused_variables))] fault: Option<RecordingMetadataWriteFault>,
) -> Result<(), ApplicationError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    return Err(ApplicationError::RecordingMetadataValidation(
        "private atomic JSON persistence is unsupported on this platform".into(),
    ));

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        fs::create_dir_all(directory)?;
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
        let final_path = directory.join(file_name);
        let temp_path = directory.join(format!(".{file_name}.tmp-{}", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(value)?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut file = options.open(&temp_path)?;
        let mut published = false;
        let result = (|| -> Result<(), ApplicationError> {
            file.write_all(&bytes)?;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::BeforeFileSync) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            file.sync_all()?;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::BeforeRename) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            fs::rename(&temp_path, &final_path)?;
            published = true;
            #[cfg(test)]
            if fault == Some(RecordingMetadataWriteFault::AfterRename) {
                return Err(std::io::Error::other("injected atomic JSON failure").into());
            }
            File::open(directory)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() && !published {
            let _ = fs::remove_file(temp_path);
        }
        result
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingRecoveryReconciliation {
    pub metadata: RecordingMetadataV1,
    pub scan: RecoveryScan,
    pub metadata_updated: bool,
}

pub fn reconcile_recording_metadata(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
) -> Result<RecordingRecoveryReconciliation, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let scan = lock.scan_v2(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    reconcile_recording_metadata_with_scan(
        &lock,
        wal_directory,
        recording_id,
        expected_selector,
        scan,
    )
}

pub fn reconcile_recording_metadata_with_scan(
    _lock: &RecordingLock,
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
    scan: RecoveryScan,
) -> Result<RecordingRecoveryReconciliation, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let original = load_recording_metadata(wal_directory)?;
    let mut metadata = original.clone().unwrap_or_else(|| RecordingMetadataV1 {
        version: RECORDING_METADATA_SCHEMA_V1,
        recording_id,
        selector: expected_selector.cloned(),
        status: RecordingStatus::Aborted,
        shutdown_reason: Some(ShutdownReason::ProcessCrashRecovered),
        last_valid_commit: None,
        counters: RecordingCounters::default(),
    });
    if metadata.recording_id != recording_id {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording ID does not match WAL segment identity".into(),
        ));
    }
    if let Some(expected) = expected_selector
        && metadata.selector.as_ref() != Some(expected)
    {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording selector does not match expected immutable selector".into(),
        ));
    }
    if matches!(
        metadata.status,
        RecordingStatus::Starting | RecordingStatus::Recording
    ) {
        metadata.status = RecordingStatus::Aborted;
        metadata.shutdown_reason = Some(ShutdownReason::ProcessCrashRecovered);
    }
    metadata.last_valid_commit = match (
        scan.authority.marker_sequence,
        scan.authority.durable_through_sequence,
        scan.authority.segment_ordinal,
    ) {
        (Some(marker_sequence), Some(durable_through_sequence), Some(segment_ordinal)) => {
            Some(RecordingCommitBoundary {
                marker_sequence,
                durable_through_sequence,
                durable_record_count: scan.authority.durable_record_count,
                durable_payload_bytes: scan.authority.durable_payload_bytes,
                segment_ordinal,
            })
        }
        _ => None,
    };
    metadata.counters.committed = RecordByteCount {
        records: scan.authority.durable_record_count,
        bytes: scan.authority.durable_payload_bytes,
    };
    metadata.counters.written_not_committed = RecordByteCount {
        records: u64::try_from(scan.uncommitted.len()).map_err(|_| {
            ApplicationError::RecordingMetadataValidation(
                "uncommitted record count exceeds metadata range".into(),
            )
        })?,
        bytes: scan.uncommitted.iter().try_fold(0_u64, |total, envelope| {
            total
                .checked_add(u64::try_from(envelope.payload.len()).map_err(|_| {
                    ApplicationError::RecordingMetadataValidation(
                        "uncommitted payload bytes exceed metadata range".into(),
                    )
                })?)
                .ok_or_else(|| {
                    ApplicationError::RecordingMetadataValidation(
                        "uncommitted payload bytes exceed metadata range".into(),
                    )
                })
        })?,
    };
    let metadata_updated = original.as_ref() != Some(&metadata);
    if metadata_updated {
        write_recording_metadata(wal_directory, &metadata)?;
    }
    Ok(RecordingRecoveryReconciliation {
        metadata,
        scan,
        metadata_updated,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryIssueCode {
    TailRepaired,
    PartialTail,
    FrameCrc,
    FrameVersion,
    HeaderCorruption,
    MarkerCrc,
    MarkerVersion,
    MarkerReference,
    MarkerDigest,
    Sequence,
    Identity,
    WalLimitLoss,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryTailRepairSummary {
    pub repaired_bytes: u64,
    pub truncated_to: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostMarkerUncertainty {
    pub records: u64,
    pub bytes: u64,
    pub first_sequence: Option<u64>,
    pub last_sequence: Option<u64>,
    pub partial_tail_offset: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReportV1 {
    pub version: u16,
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub repaired_tail: Option<RecoveryTailRepairSummary>,
    pub removed_segments: usize,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub committed: RecordByteCount,
    pub post_marker: PostMarkerUncertainty,
    pub wal_limit_loss: RecordByteCount,
    pub issue_codes: Vec<RecoveryIssueCode>,
}

pub fn recovery_issue_code(error: &WalError) -> RecoveryIssueCode {
    match error {
        WalError::EnvelopeChecksum {
            kind: RecordKind::CommitMarker,
            ..
        } => RecoveryIssueCode::MarkerCrc,
        WalError::EnvelopeChecksum { .. } | WalError::Checksum { .. } => {
            RecoveryIssueCode::FrameCrc
        }
        WalError::SegmentHeaderChecksum { .. }
        | WalError::InvalidSegmentMagic
        | WalError::InvalidSegmentHeaderLength(_)
        | WalError::NonzeroSegmentHeaderReserved
        | WalError::TruncatedSegmentHeader { .. }
        | WalError::UnsupportedSegmentHeaderVersion(_) => RecoveryIssueCode::HeaderCorruption,
        WalError::UnsupportedCommitMarkerVersion(_) => RecoveryIssueCode::MarkerVersion,
        WalError::UnsupportedEnvelopeVersion { kind, .. }
            if *kind == RecordKind::CommitMarker as u16 =>
        {
            RecoveryIssueCode::MarkerVersion
        }
        WalError::UnsupportedEnvelopeVersion { .. } | WalError::UnsupportedVersion(_) => {
            RecoveryIssueCode::FrameVersion
        }
        WalError::CommitDigestMismatch => RecoveryIssueCode::MarkerDigest,
        WalError::CommitBoundaryMismatch { .. }
        | WalError::CommitDurableThroughMismatch { .. }
        | WalError::CommitRecordCountMismatch { .. }
        | WalError::CommitPayloadBytesMismatch { .. }
        | WalError::CommitBatchContainsMarker { .. }
        | WalError::EmptyCommitBatch => RecoveryIssueCode::MarkerReference,
        WalError::UnexpectedSequence { .. }
        | WalError::OutOfOrder { .. }
        | WalError::RecoverySegmentFirstSequenceMismatch { .. } => RecoveryIssueCode::Sequence,
        WalError::CommitRecordingIdMismatch
        | WalError::CommitSegmentMismatch
        | WalError::SegmentIdentityMismatch(_)
        | WalError::EnvelopeRecordingIdMismatch => RecoveryIssueCode::Identity,
        _ => RecoveryIssueCode::Other,
    }
}

pub fn build_recovery_report(
    reconciliation: &RecordingRecoveryReconciliation,
    repair: Option<&RecoveryRepair>,
    additional_issues: impl IntoIterator<Item = RecoveryIssueCode>,
) -> Result<RecoveryReportV1, ApplicationError> {
    let uncommitted_bytes =
        reconciliation
            .scan
            .uncommitted
            .iter()
            .try_fold(0_u64, |total, envelope| {
                total
                    .checked_add(u64::try_from(envelope.payload.len()).map_err(|_| {
                        ApplicationError::RecordingMetadataValidation(
                            "post-marker payload bytes exceed report range".into(),
                        )
                    })?)
                    .ok_or_else(|| {
                        ApplicationError::RecordingMetadataValidation(
                            "post-marker payload bytes exceed report range".into(),
                        )
                    })
            })?;
    let mut issue_codes: BTreeSet<_> = additional_issues.into_iter().collect();
    if repair.is_some_and(|repair| repair.repaired_bytes > 0) {
        issue_codes.insert(RecoveryIssueCode::TailRepaired);
    }
    if reconciliation.scan.partial_tail.is_some() {
        issue_codes.insert(RecoveryIssueCode::PartialTail);
    }
    if reconciliation
        .metadata
        .counters
        .discarded_from_queue_due_to_wal_limit
        .records
        > 0
    {
        issue_codes.insert(RecoveryIssueCode::WalLimitLoss);
    }
    Ok(RecoveryReportV1 {
        version: RECOVERY_REPORT_VERSION,
        recording_id: reconciliation.metadata.recording_id,
        status: reconciliation.metadata.status,
        shutdown_reason: reconciliation.metadata.shutdown_reason,
        repaired_tail: repair.map(|repair| RecoveryTailRepairSummary {
            repaired_bytes: repair.repaired_bytes,
            truncated_to: repair.truncated_to,
        }),
        removed_segments: 0,
        last_valid_commit: reconciliation.metadata.last_valid_commit.clone(),
        committed: reconciliation.metadata.counters.committed.clone(),
        post_marker: PostMarkerUncertainty {
            records: u64::try_from(reconciliation.scan.uncommitted.len()).map_err(|_| {
                ApplicationError::RecordingMetadataValidation(
                    "post-marker record count exceeds report range".into(),
                )
            })?,
            bytes: uncommitted_bytes,
            first_sequence: reconciliation
                .scan
                .uncommitted
                .first()
                .map(|envelope| envelope.sequence),
            last_sequence: reconciliation
                .scan
                .uncommitted
                .last()
                .map(|envelope| envelope.sequence),
            partial_tail_offset: reconciliation
                .scan
                .partial_tail
                .as_ref()
                .map(|partial| partial.byte_offset),
        },
        wal_limit_loss: reconciliation
            .metadata
            .counters
            .discarded_from_queue_due_to_wal_limit
            .clone(),
        issue_codes: issue_codes.into_iter().collect(),
    })
}

pub fn build_recovery_report_for_reopen(
    reconciliation: &RecordingRecoveryReconciliation,
    preview: &RecoveryReopenPreview,
    additional_issues: impl IntoIterator<Item = RecoveryIssueCode>,
) -> Result<RecoveryReportV1, ApplicationError> {
    let mut report = build_recovery_report(reconciliation, None, additional_issues)?;
    if preview.truncated_bytes > 0 || preview.removed_segments > 0 {
        report.repaired_tail = Some(RecoveryTailRepairSummary {
            repaired_bytes: preview.truncated_bytes,
            truncated_to: preview.truncated_to,
        });
        if !report
            .issue_codes
            .contains(&RecoveryIssueCode::TailRepaired)
        {
            report.issue_codes.push(RecoveryIssueCode::TailRepaired);
            report.issue_codes.sort_unstable();
        }
    }
    report.removed_segments = preview.removed_segments;
    Ok(report)
}

pub fn render_recovery_report_json(report: &RecoveryReportV1) -> Result<String, ApplicationError> {
    render_json(report)
}

pub fn persist_recovery_report(
    wal_directory: impl AsRef<Path>,
    report: &RecoveryReportV1,
) -> Result<(), ApplicationError> {
    if report.version != RECOVERY_REPORT_VERSION {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recovery report version {}",
            report.version
        )));
    }
    write_private_atomic_json(
        &wal_directory.as_ref().join("etl"),
        "recovery-report.json",
        report,
        None,
    )
}

pub fn decode_recovery_report(bytes: &[u8]) -> Result<RecoveryReportV1, ApplicationError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            ApplicationError::RecordingMetadataValidation(
                "recovery report version is missing or invalid".into(),
            )
        })?;
    if version != u64::from(RECOVERY_REPORT_VERSION) {
        return Err(ApplicationError::RecordingMetadataValidation(format!(
            "unsupported recovery report version {version}"
        )));
    }
    serde_json::from_value(value).map_err(ApplicationError::JsonSerialization)
}

pub struct RecoveredRecordingForAppend {
    pub writer: GroupCommitWalWriter,
    pub reconciliation: RecordingRecoveryReconciliation,
    pub recovery_report: RecoveryReportV1,
    pub reopen_report: RecoveryReopenReport,
}

#[allow(clippy::too_many_arguments)]
pub fn recover_recording_for_append(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    expected_selector: Option<&RecordingSelectorIdentity>,
    max_segment_bytes: u64,
    max_total_bytes: u64,
    now_millis: u64,
) -> Result<RecoveredRecordingForAppend, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let scan = match lock.scan_v2(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES) {
        Ok(scan) => scan,
        Err(error) => {
            let failure_report = RecoveryReportV1 {
                version: RECOVERY_REPORT_VERSION,
                recording_id,
                status: RecordingStatus::Failed,
                shutdown_reason: Some(ShutdownReason::WalFailure),
                repaired_tail: None,
                removed_segments: 0,
                last_valid_commit: None,
                committed: RecordByteCount::default(),
                post_marker: PostMarkerUncertainty::default(),
                wal_limit_loss: RecordByteCount::default(),
                issue_codes: vec![recovery_issue_code(&error)],
            };
            persist_recovery_report(wal_directory, &failure_report)?;
            return Err(error.into());
        }
    };
    let prepared = prepare_group_commit_reopen_from_scan(
        lock,
        wal_directory,
        recording_id,
        scan,
        max_segment_bytes,
        max_total_bytes,
        now_millis,
    )?;
    let before_reconciliation = reconcile_recording_metadata_with_scan(
        prepared.lock(),
        wal_directory,
        recording_id,
        expected_selector,
        prepared.preview().scan_before.clone(),
    )?;
    let recovery_report = build_recovery_report_for_reopen(
        &before_reconciliation,
        prepared.preview(),
        std::iter::empty(),
    )?;
    persist_recovery_report(wal_directory, &recovery_report)?;
    let (writer, reopen_report) = prepared.apply()?;
    let reconciliation = reconcile_recording_metadata_with_scan(
        writer.recording_lock(),
        wal_directory,
        recording_id,
        expected_selector,
        reopen_report.scan_after.clone(),
    )?;
    Ok(RecoveredRecordingForAppend {
        writer,
        reconciliation,
        recovery_report,
        reopen_report,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWal {
    pub first_sequence: Option<u64>,
    pub checkpoint: Option<WalCheckpoint>,
    pub record_count: usize,
}

pub fn write_capture_to_wal(
    source: &mut impl CaptureSource,
    directory: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordedWal, ApplicationError> {
    let directory = directory.as_ref();
    if directory.exists() {
        return Err(ApplicationError::WalDestinationExists(
            directory.to_path_buf(),
        ));
    }

    let mut records = Vec::new();
    let mut encoded_bytes = 0_u64;
    while let Some(event) = source.next_event()? {
        let flags = u16::try_from(event.flags.0)
            .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(event.flags.0))?;
        let record = WalRecord {
            kind: RecordKind::CaptureEvent,
            flags,
            sequence: event.monotonic_sequence,
            payload: encode_event(&event)?,
        };
        encoded_bytes = encoded_bytes
            .saturating_add(u64::try_from(encode_record(&record)?.len()).unwrap_or(u64::MAX));
        records.push(record);
    }
    if encoded_bytes > max_segment_bytes {
        return Err(ApplicationError::FixtureWalTooLarge {
            bytes: encoded_bytes,
            limit: max_segment_bytes,
        });
    }

    let mut writer = SegmentedWalWriter::create(directory, max_segment_bytes)?;
    let first_sequence = records.first().map(|record| record.sequence);
    let mut checkpoint = None;
    for record in &records {
        checkpoint = Some(writer.append(record)?);
    }
    writer.flush()?;
    Ok(RecordedWal {
        first_sequence,
        checkpoint,
        record_count: records.len(),
    })
}

pub fn process_single_wal(
    path: impl AsRef<Path>,
    first_sequence: u64,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<(EtlOutput, WalCheckpoint), ApplicationError> {
    let mut reader = WalReader::new(fs::File::open(path)?, first_sequence);
    let output =
        EtlPipeline::new(SessionLimits::default()).process(&mut reader, registry, session_id)?;
    Ok((output, reader.checkpoint()))
}

/// Builds replay inputs solely from explicit command options; config cannot authorize execution.
pub fn replay_command_inputs(
    session: &chronicle_canonical::CanonicalSession,
    options: &LoopbackReplayOptions,
) -> Result<(TargetMap, ReplayPolicy), ApplicationError> {
    Ok((options.target_map_for(session)?, options.policy()))
}

/// Loads optional runtime Authorization without letting config grant replay permissions.
pub fn replay_context(config: &ReplayConfig) -> Result<ReplayContext, ApplicationError> {
    replay_context_from(config, |environment| std::env::var(environment).ok())
}

fn replay_context_from(
    config: &ReplayConfig,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Result<ReplayContext, ApplicationError> {
    let Some(environment) = config.authorization_env.as_deref() else {
        return Ok(ReplayContext::default());
    };
    let value = lookup(environment).ok_or_else(|| ApplicationError::MissingReplayCredential {
        environment: environment.into(),
    })?;
    if !value.bytes().all(valid_http_field_value_byte) {
        return Err(ApplicationError::InvalidReplayCredential);
    }
    let mut context = ReplayContext::default();
    context.credentials = [("authorization".into(), SecretBytes::new(value.into_bytes()))]
        .into_iter()
        .collect();
    Ok(context)
}

fn valid_http_field_value_byte(byte: u8) -> bool {
    byte == b'\t' || (0x20..=0x7e).contains(&byte) || byte >= 0x80
}

const MAX_RECORD_ISSUES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordIssueSummary {
    pub code: String,
    pub sequence: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectOperationSummary {
    pub sequence: u64,
    pub kind: String,
    pub effect: String,
    pub method: Option<String>,
    pub target: Option<String>,
    pub response_status: Option<String>,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub incomplete: bool,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectConnectionSummary {
    pub protocol: String,
    pub client: chronicle_common::Endpoint,
    pub server: chronicle_common::Endpoint,
    pub incomplete: bool,
    pub truncated: bool,
    pub operations: Vec<InspectOperationSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectSessionResult {
    pub session_id: chronicle_common::SessionId,
    pub complete: bool,
    pub replayable: bool,
    pub blockers: Vec<String>,
    pub issue_codes: Vec<String>,
    pub connections: Vec<InspectConnectionSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordFixtureResult {
    pub session_id: chronicle_common::SessionId,
    pub checkpoint: WalCheckpoint,
    pub issues: Vec<RecordIssueSummary>,
    pub replayability: Vec<String>,
    pub complete: bool,
}

/// Runs fixture capture through durable WAL and ETL before atomically publishing its session.
pub fn record_fixture(
    source: &mut FixtureCaptureSource,
    root: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordFixtureResult, ApplicationError> {
    let root = root.as_ref();
    let session_id = chronicle_common::SessionId::new();
    let wal_directory = root.join("wal").join(session_id.to_string());
    let recorded = write_capture_to_wal(source, &wal_directory, max_segment_bytes)?;
    let (output, checkpoint) = process_single_wal(
        wal_directory.join(chronicle_wal::segment_file_name(
            recorded.first_sequence.unwrap_or(1),
        )),
        recorded.first_sequence.unwrap_or(1),
        &chronicle_protocol_builtins::registry()?,
        session_id,
    )?;
    let issues = issue_summaries(&output.issues);
    let replayability = replayability_reasons(&output);
    let complete = session_complete(&output);
    FilesystemSessionStore::new(root).publish(PublishSession {
        session: output.session,
        checkpoint: Some(format!(
            "{}:{}:{}",
            checkpoint.segment_first_sequence, checkpoint.byte_offset, checkpoint.next_sequence
        )),
        issues: issues.iter().map(|issue| issue.code.clone()).collect(),
        replayability: replayability.clone(),
        complete,
    })?;
    Ok(RecordFixtureResult {
        session_id,
        checkpoint,
        issues,
        replayability,
        complete,
    })
}

/// Opens fixture input and delegates all record work to the application service.
pub fn record_fixture_file(
    input: impl AsRef<Path>,
    root: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordFixtureResult, ApplicationError> {
    let bytes = fs::read(input)?;
    let mut source = FixtureCaptureSource::from_json(&bytes)?;
    record_fixture(&mut source, root, max_segment_bytes)
}

fn issue_summaries(issues: &[EtlIssue]) -> Vec<RecordIssueSummary> {
    let mut summaries: Vec<_> = issues
        .iter()
        .take(MAX_RECORD_ISSUES)
        .map(|issue| RecordIssueSummary {
            code: match issue.kind {
                chronicle_etl::EtlIssueKind::MalformedCaptureEvent => "malformed_capture_event",
                chronicle_etl::EtlIssueKind::CaptureSequenceMismatch => "capture_sequence_mismatch",
                chronicle_etl::EtlIssueKind::PartialWalTail => "partial_wal_tail",
                chronicle_etl::EtlIssueKind::UnknownProtocol => "unknown_protocol",
                chronicle_etl::EtlIssueKind::ProtocolDecode => "protocol_decode",
            }
            .into(),
            sequence: issue.sequence,
        })
        .collect();
    if issues.len() > MAX_RECORD_ISSUES {
        summaries.push(RecordIssueSummary {
            code: "issue_summary_truncated".into(),
            sequence: None,
        });
    }
    summaries
}

fn replayability_reasons(output: &EtlOutput) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    if !output.issues.is_empty() {
        reasons.insert("etl_issues".into());
    }
    for connection in &output.session.connections {
        if connection.incomplete || connection.truncated {
            reasons.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if operation.incomplete || operation.truncated {
                reasons.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                reasons.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                reasons.insert(format!("warning:{}", warning.code));
            }
        }
    }
    reasons.into_iter().collect()
}

fn session_complete(output: &EtlOutput) -> bool {
    output.issues.is_empty() && canonical_session_complete(&output.session)
}

fn canonical_session_complete(session: &chronicle_canonical::CanonicalSession) -> bool {
    session.connections.iter().all(|connection| {
        !connection.incomplete
            && !connection.truncated
            && connection.operations.iter().all(|operation| {
                !operation.incomplete
                    && !operation.truncated
                    && operation.recorded_response.is_some()
            })
    })
}

/// Produces inspect-safe metadata without reading artifact body bytes.
pub fn inspect_session(
    root: impl AsRef<Path>,
    session_id: chronicle_common::SessionId,
) -> Result<InspectSessionResult, ApplicationError> {
    let stored = FilesystemSessionStore::new(root.as_ref()).inspect_with_metadata(session_id)?;
    let mut blockers: BTreeSet<_> = stored.replayability.into_iter().collect();
    for connection in &stored.session.connections {
        if connection.incomplete || connection.truncated {
            blockers.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if operation.incomplete || operation.truncated {
                blockers.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                blockers.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                blockers.insert(format!("warning:{}", warning.code));
            }
        }
    }
    let complete = stored.complete && canonical_session_complete(&stored.session);
    if !complete {
        blockers.insert("incomplete_capture".into());
    }
    let mut issue_codes = stored.issues;
    issue_codes.sort();
    let connections = stored
        .session
        .connections
        .iter()
        .map(|connection| InspectConnectionSummary {
            protocol: connection.protocol.to_string(),
            client: connection.client.clone(),
            server: connection.server.clone(),
            incomplete: connection.incomplete,
            truncated: connection.truncated,
            operations: connection
                .operations
                .iter()
                .map(|operation| InspectOperationSummary {
                    sequence: operation.sequence,
                    kind: format!("{:?}", operation.kind).to_lowercase(),
                    effect: format!("{:?}", operation.effect).to_lowercase(),
                    method: operation.attributes.get("http.method").cloned(),
                    target: operation.attributes.get("http.request_target").cloned(),
                    response_status: operation.attributes.get("http.response_status").cloned(),
                    request_bytes: payload_size(&operation.request),
                    response_bytes: operation.recorded_response.as_ref().and_then(payload_size),
                    incomplete: operation.incomplete,
                    truncated: operation.truncated,
                    warnings: operation
                        .warnings
                        .iter()
                        .map(|warning| warning.code.clone())
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Ok(InspectSessionResult {
        session_id,
        complete,
        replayable: complete && blockers.is_empty(),
        blockers: blockers.into_iter().collect(),
        issue_codes,
        connections,
    })
}

#[derive(Serialize)]
struct InspectJsonV1<'a> {
    version: u8,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

/// Renders stable, body-free inspect output for interactive use.
pub fn render_inspect_human(inspected: &InspectSessionResult) -> String {
    let mut output = format!(
        "session_id: {}\ncomplete: {}\nreplayable: {}\nblockers: {}\nissues: {}\n",
        inspected.session_id,
        inspected.complete,
        inspected.replayable,
        joined_or_none(&inspected.blockers),
        joined_or_none(&inspected.issue_codes),
    );
    for connection in &inspected.connections {
        writeln!(
            output,
            "connection: {} {}:{} -> {}:{} incomplete={} truncated={}",
            connection.protocol,
            connection.client.host,
            connection.client.port,
            connection.server.host,
            connection.server.port,
            connection.incomplete,
            connection.truncated,
        )
        .expect("writing into String cannot fail");
        for operation in &connection.operations {
            writeln!(
                output,
                "operation: sequence={} kind={} effect={} method={} target={} response_status={} request_bytes={} response_bytes={} incomplete={} truncated={} warnings={}",
                operation.sequence,
                operation.kind,
                operation.effect,
                operation.method.as_deref().unwrap_or("none"),
                operation.target.as_deref().unwrap_or("none"),
                operation.response_status.as_deref().unwrap_or("none"),
                optional_size(operation.request_bytes),
                optional_size(operation.response_bytes),
                operation.incomplete,
                operation.truncated,
                joined_or_none(&operation.warnings),
            )
            .expect("writing into String cannot fail");
        }
    }
    output
}

pub fn render_json<T: Serialize + ?Sized>(value: &T) -> Result<String, ApplicationError> {
    serde_json::to_string(value).map_err(ApplicationError::JsonSerialization)
}

/// Renders stable inspect JSON v1 containing only summary metadata.
pub fn render_inspect_json(inspected: &InspectSessionResult) -> Result<String, ApplicationError> {
    render_json(&InspectJsonV1 {
        version: 1,
        result: inspected,
    })
}

fn joined_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join(",")
    }
}

fn optional_size(size: Option<u64>) -> String {
    size.map_or_else(|| "unknown".into(), |size| size.to_string())
}

fn payload_size(payload: &chronicle_canonical::PayloadRef) -> Option<u64> {
    match payload {
        chronicle_canonical::PayloadRef::Inline { bytes, .. } => Some(bytes.len() as u64),
        chronicle_canonical::PayloadRef::Object { size, .. }
        | chronicle_canonical::PayloadRef::Artifact { size, .. } => Some(*size),
        chronicle_canonical::PayloadRef::Redacted { original_size, .. } => *original_size,
        chronicle_canonical::PayloadRef::Missing { .. } => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplayOperationSummary {
    pub operation_id: String,
    pub decision: String,
    pub state: OperationExecutionState,
    pub attempted: bool,
    pub verification: String,
    pub category: String,
    pub transport_error: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplaySessionResult {
    pub session_id: String,
    pub outcome: ReplayOutcome,
    pub dry_run: bool,
    pub preflight_denied: bool,
    pub transport_failed: bool,
    pub operations: Vec<ReplayOperationSummary>,
}

/// Replays hydrated filesystem session through explicit loopback-only options.
pub async fn replay_session(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    options: &LoopbackReplayOptions,
) -> Result<ReplaySessionResult, ApplicationError> {
    replay_session_with_plan(root, session_id, config, options, |_| {}).await
}

/// Replays a session after exposing its complete plan, before any network I/O.
pub async fn replay_session_with_plan<F>(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    options: &LoopbackReplayOptions,
    on_plan: F,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
{
    let parsed = uuid::Uuid::parse_str(session_id)
        .map(chronicle_common::SessionId)
        .map_err(|_| ApplicationError::InvalidConfig("session ID must be a UUID".into()))?;
    let session = FilesystemSessionStore::new(root.as_ref()).hydrate(parsed)?;
    let (targets, policy) = replay_command_inputs(&session, options)?;
    let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap)?;
    on_plan(&replay_plan_result(session.id.to_string(), &plan));
    let mut context = ReplayContext::default();
    if options.execute && plan.is_executable() {
        context = replay_context(config)?;
        context.authorize_execution_for(options.validate_target()?.endpoint().clone());
    }
    let contexts = BTreeMap::from([(chronicle_common::ProtocolId::new("http/1.1"), context)]);
    let execution = ReplayExecutor::new(&chronicle_protocol_builtins::registry()?)
        .execute(&plan, &contexts)
        .await?;
    Ok(ReplaySessionResult {
        session_id: session.id.to_string(),
        outcome: execution.outcome,
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed: execution
            .results
            .iter()
            .any(|result| result.transport_error.is_some()),
        operations: plan
            .operations()
            .iter()
            .zip(execution.results)
            .map(|(planned, result)| ReplayOperationSummary {
                operation_id: result.operation_id.to_string(),
                decision: format!("{:?}", planned.decision()).to_lowercase(),
                state: result.state,
                attempted: result.state != OperationExecutionState::NotAttempted,
                verification: result.verification.as_ref().map_or_else(
                    || "not_run".into(),
                    |verification| format!("{:?}", verification.status).to_lowercase(),
                ),
                category: result
                    .verification
                    .as_ref()
                    .and_then(|verification| verification.details.get("category"))
                    .cloned()
                    .unwrap_or_default(),
                transport_error: result
                    .transport_error
                    .map(|category| format!("{category:?}").to_lowercase())
                    .unwrap_or_default(),
            })
            .collect(),
    })
}

fn replay_plan_result(session_id: String, plan: &ReplayPlan) -> ReplaySessionResult {
    ReplaySessionResult {
        session_id,
        outcome: if plan.is_dry_run() {
            ReplayOutcome::DryRun
        } else if plan.is_executable() {
            ReplayOutcome::Completed
        } else {
            ReplayOutcome::StoppedPolicy
        },
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed: false,
        operations: plan
            .operations()
            .iter()
            .map(|planned| ReplayOperationSummary {
                operation_id: planned.operation().id.to_string(),
                decision: format!("{:?}", planned.decision()).to_lowercase(),
                state: OperationExecutionState::NotAttempted,
                attempted: false,
                verification: "not_run".into(),
                category: String::new(),
                transport_error: String::new(),
            })
            .collect(),
    }
}

pub fn render_replay_human(result: &ReplaySessionResult) -> String {
    let mut output = format!(
        "session_id: {}\noutcome: {}\ndry_run: {}\npreflight_denied: {}\ntransport_failed: {}\n",
        result.session_id,
        result.outcome.as_str(),
        result.dry_run,
        result.preflight_denied,
        result.transport_failed
    );
    for operation in &result.operations {
        writeln!(
            output,
            "operation: id={} decision={} state={} attempted={} verification={} category={} transport_error={}",
            operation.operation_id,
            operation.decision,
            operation.state.as_str(),
            operation.attempted,
            operation.verification,
            optional_text(&operation.category),
            optional_text(&operation.transport_error),
        )
        .expect("writing into String cannot fail");
    }
    output
}

pub fn render_replay_json(result: &ReplaySessionResult) -> Result<String, ApplicationError> {
    render_json(&serde_json::json!({
        "version": REPLAY_REPORT_VERSION,
        "result": result
    }))
}

fn optional_text(value: &str) -> &str {
    if value.is_empty() { "none" } else { value }
}

pub struct ChronicleApplication {
    config: AppConfig,
}

impl ChronicleApplication {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub fn execute(&self, command: &ApplicationCommand) -> Result<String, ApplicationError> {
        self.validate()?;
        match command {
            ApplicationCommand::Record => Err(ApplicationError::NotImplemented("record")),
            ApplicationCommand::Etl => Err(ApplicationError::NotImplemented("etl")),
            ApplicationCommand::Replay { .. } => Err(ApplicationError::NotImplemented("replay")),
            ApplicationCommand::Inspect { .. } => Err(ApplicationError::NotImplemented("inspect")),
            ApplicationCommand::Doctor => {
                Ok("configuration checks passed; external probes not implemented".into())
            }
        }
    }

    fn validate(&self) -> Result<(), ApplicationError> {
        if self.config.wal.segment_size_bytes <= chronicle_wal::HEADER_LEN as u64 {
            return Err(ApplicationError::InvalidConfig(
                "WAL segment size must exceed record header".into(),
            ));
        }
        if self.config.wal.disk_limit_bytes < self.config.wal.segment_size_bytes {
            return Err(ApplicationError::InvalidConfig(
                "WAL disk limit must be at least one segment".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_commands_return_typed_not_implemented_error() {
        let error = ChronicleApplication::new(AppConfig::default())
            .execute(&ApplicationCommand::Record)
            .unwrap_err();
        assert!(matches!(error, ApplicationError::NotImplemented("record")));
    }

    #[test]
    fn doctor_reports_only_completed_configuration_checks() {
        let output = ChronicleApplication::new(AppConfig::default())
            .execute(&ApplicationCommand::Doctor)
            .unwrap();
        assert_eq!(
            output,
            "configuration checks passed; external probes not implemented"
        );
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
        assert!(inspected.replayable);
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
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        let inspected = inspect_session(&root, recorded.session_id).unwrap();

        let human = render_inspect_human(&inspected);
        let json = render_inspect_json(&inspected).unwrap();
        assert_eq!(human, render_inspect_human(&inspected));
        assert_eq!(json, render_inspect_json(&inspected).unwrap());
        for rendered in [&human, &json] {
            assert!(!rendered.contains("x-tag"));
            assert!(!rendered.contains("two"));
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
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, TransportProtocol};
        use chronicle_wal::{ReadOutcome, WalReader, segment_file_name};
        use std::fs::File;

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"fixture".to_vec(),
            process: None,
            container: None,
            file_descriptor: Some(7),
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        let recorded = write_capture_to_wal(&mut source, &directory, 1024).unwrap();
        assert_eq!(recorded.record_count, 1);
        assert_eq!(recorded.first_sequence, Some(1));

        let mut reader =
            WalReader::new(File::open(directory.join(segment_file_name(1))).unwrap(), 1);
        assert!(
            matches!(reader.next_record().unwrap(), ReadOutcome::Record(record) if record.sequence == 1)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn corrupt_wal_prevents_processing_before_session_publication() {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, SessionId, TransportProtocol};
        use chronicle_wal::segment_file_name;

        let root =
            std::env::temp_dir().join(format!("chronicle-app-corrupt-{}", uuid::Uuid::new_v4()));
        let wal_directory = root.join("wal");
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"GET / HTTP/1.1\r\n\r\n".to_vec(),
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        };
        write_capture_to_wal(
            &mut InMemoryCaptureSource::new([event]),
            &wal_directory,
            1024,
        )
        .unwrap();
        let segment = wal_directory.join(segment_file_name(1));
        let mut bytes = std::fs::read(&segment).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        std::fs::write(&segment, bytes).unwrap();
        assert!(
            process_single_wal(
                segment,
                1,
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
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, TransportProtocol};

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-limit-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: vec![0; 128],
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        let error = write_capture_to_wal(&mut source, &directory, 64).unwrap_err();
        assert!(matches!(error, ApplicationError::FixtureWalTooLarge { .. }));
        assert!(!directory.exists());
    }

    #[test]
    fn single_wal_process_returns_checkpoint_after_last_valid_record() {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, SessionId, TransportProtocol};
        use chronicle_wal::segment_file_name;

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-etl-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"FAKE request".to_vec(),
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        write_capture_to_wal(&mut source, &directory, 1024).unwrap();
        let (output, checkpoint) = process_single_wal(
            directory.join(segment_file_name(1)),
            1,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(checkpoint.next_sequence, 2);
        assert_eq!(output.session.connections.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn recording_metadata(status: RecordingStatus) -> RecordingMetadataV1 {
        RecordingMetadataV1 {
            version: RECORDING_METADATA_SCHEMA_V1,
            recording_id: RecordingId::new(),
            selector: Some(RecordingSelectorIdentity {
                canonical_cgroup_path: "/sys/fs/cgroup/workload".into(),
                cgroup_id: 42,
            }),
            status,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: RecordingCounters::default(),
        }
    }

    #[cfg(unix)]
    #[test]
    #[allow(clippy::too_many_lines)] // Full metadata-lag and mismatch flow stays auditable together.
    fn recovery_reconciles_wal_authority_without_acknowledgement_claim() {
        use chronicle_wal::{
            GroupCommitWalWriter, MIN_V2_SEGMENT_BYTES, WalRecordEnvelope, encode_envelope,
            scan_v2_wal, v2_segment_file_name,
        };

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-reconcile-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let selector = RecordingSelectorIdentity {
            canonical_cgroup_path: "/sys/fs/cgroup/workload".into(),
            cgroup_id: 42,
        };
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_V2_SEGMENT_BYTES, 1, 0)
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
            .open(wal_directory.join("segments").join(v2_segment_file_name(1)))
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
        let scan = scan_v2_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
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
            MIN_V2_SEGMENT_BYTES, WalRecordEnvelope, encode_envelope, v2_segment_file_name,
        };

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-recover-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_V2_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"committed".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(v2_segment_file_name(1));
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
            MIN_V2_SEGMENT_BYTES,
            MIN_V2_SEGMENT_BYTES,
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
        use chronicle_wal::{MIN_V2_SEGMENT_BYTES, SEGMENT_HEADER_LEN, v2_segment_file_name};

        let wal_directory =
            std::env::temp_dir().join(format!("chronicle-corrupt-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer =
            GroupCommitWalWriter::create(&wal_directory, recording_id, MIN_V2_SEGMENT_BYTES, 1, 0)
                .unwrap();
        writer
            .append(RecordKind::CaptureEvent, 1, 0, b"secret-body".to_vec(), 0)
            .unwrap();
        writer.flush(1).unwrap();
        drop(writer);
        let segment_path = wal_directory.join("segments").join(v2_segment_file_name(1));
        let mut corrupted = fs::read(&segment_path).unwrap();
        corrupted[SEGMENT_HEADER_LEN + chronicle_wal::ENVELOPE_HEADER_LEN] ^= 0xff;
        fs::write(&segment_path, &corrupted).unwrap();

        assert!(matches!(
            recover_recording_for_append(
                &wal_directory,
                recording_id,
                None,
                MIN_V2_SEGMENT_BYTES,
                MIN_V2_SEGMENT_BYTES,
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
        for version in [0, RECORDING_METADATA_SCHEMA_V1 + 1] {
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

    #[test]
    fn replay_report_v2_serializes_outcome_and_operation_state() {
        let result = ReplaySessionResult {
            session_id: "session".into(),
            outcome: ReplayOutcome::DryRun,
            dry_run: true,
            preflight_denied: false,
            transport_failed: false,
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
        assert_eq!(value["result"]["operations"][0]["state"], "not_attempted");
    }
}
