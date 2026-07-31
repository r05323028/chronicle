//! Application services and configuration. CLI remains an outer adapter.

mod cgroup_selection;

pub use cgroup_selection::{
    CgroupSelection, CgroupSelectionError, CgroupSelector, PidCgroupSelection,
    preflight_cgroup_selection, preflight_pid_cgroup_selection,
};

use chronicle_canonical::{
    CommitMarkerProvenance, Completeness, ProvenanceEntry, ProvenanceKind, SourceProvenance,
    SourceStatus,
};
use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureError, CaptureEvent, CaptureEventKind, CaptureSource,
    CaptureSourceSummary, ClockIdentity, FixtureCaptureSource, MonotonicTimestamp, encode_event,
};
use chronicle_common::RecordingId;
use chronicle_etl::{
    ETL_PIPELINE_VERSION, EtlError, EtlIssue, EtlOutput, EtlPipeline,
    assign_recording_ids_with_snapshot,
};
use chronicle_protocol::{ProtocolError, ProtocolRegistry, ReplayContext, SecretBytes};
use chronicle_replay::{
    LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayExecutor, ReplayOutcome,
    ReplayPlan, ReplayPlanner, ReplayPolicy, ReplayTargetError, Replayability, TargetMap,
    TimingMode,
};
use chronicle_session::SessionLimits;
use chronicle_storage::{FilesystemSessionStore, PublishSession, StorageError};
use chronicle_wal::{
    COMMIT_MARKER_FRAME_LEN, DEFAULT_MAX_RECORD_BYTES, DEFAULT_MAX_WAL_BYTES,
    DEFAULT_SEGMENT_BYTES, GroupCommitWalWriter, MAX_SEGMENT_BYTES, MIN_SEGMENT_BYTES, RecordKind,
    RecordingLock, RecoveryReopenPreview, RecoveryReopenReport, RecoveryRepair, RecoveryScan,
    SEGMENT_HEADER_LEN, TERMINAL_WAL_LOSS_SCHEMA_VERSION, TerminalWalLoss,
    TerminalWalLossAmbiguity, TerminalWalLossInterval, TerminalWalLossReason, WalCheckpoint,
    WalError, WalRecordEnvelope, encode_envelope, encode_terminal_wal_loss,
    prepare_group_commit_reopen_from_scan, verified_snapshot_sha256,
};
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
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use std::time::{SystemTime, UNIX_EPOCH};
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Supported,
    SupportedWithWarnings,
    Unsupported,
    NotChecked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorProbe {
    pub required: bool,
    pub status: DoctorStatus,
    pub code: String,
    pub message: String,
    pub remediation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub version: u16,
    pub status: DoctorStatus,
    pub probes: Vec<DoctorProbe>,
}

impl DoctorReport {
    pub fn new(probes: Vec<DoctorProbe>) -> Self {
        let status = aggregate_doctor_status(&probes);
        Self {
            version: DOCTOR_REPORT_VERSION,
            status,
            probes,
        }
    }

    pub const fn exit_code(&self) -> i32 {
        doctor_exit_code(self.status)
    }
}

pub fn aggregate_doctor_status(probes: &[DoctorProbe]) -> DoctorStatus {
    if probes
        .iter()
        .any(|probe| probe.required && probe.status == DoctorStatus::Unsupported)
    {
        DoctorStatus::Unsupported
    } else if probes
        .iter()
        .any(|probe| probe.required && probe.status == DoctorStatus::NotChecked)
    {
        DoctorStatus::NotChecked
    } else if probes.iter().any(|probe| {
        probe.status == DoctorStatus::SupportedWithWarnings
            || (!probe.required
                && matches!(
                    probe.status,
                    DoctorStatus::Unsupported | DoctorStatus::NotChecked
                ))
    }) {
        DoctorStatus::SupportedWithWarnings
    } else {
        DoctorStatus::Supported
    }
}

pub const fn doctor_exit_code(status: DoctorStatus) -> i32 {
    match status {
        DoctorStatus::Unsupported | DoctorStatus::NotChecked => 4,
        DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings => 0,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    pub wal_dir: Option<PathBuf>,
    pub output: Option<PathBuf>,
}

/// Runs only local, non-attaching probes. Supplied paths are inspected but never created.
pub fn doctor_report(options: &DoctorOptions) -> DoctorReport {
    let mut probes = capture_doctor_probes();
    probes.extend([
        path_doctor_probe(options.wal_dir.as_deref(), "wal_dir", "WAL directory"),
        path_doctor_probe(options.output.as_deref(), "output", "output path"),
        protocol_doctor_probe(),
        DoctorProbe {
            required: false,
            status: DoctorStatus::Supported,
            code: "replay.target_not_checked".into(),
            message: "replay target is command-specific and was not contacted".into(),
            remediation: "supply --target to replay; doctor never contacts targets".into(),
        },
    ]);
    DoctorReport::new(probes)
}

fn protocol_doctor_probe() -> DoctorProbe {
    let status = if chronicle_protocol_builtins::registry().is_ok() {
        DoctorStatus::Supported
    } else {
        DoctorStatus::Unsupported
    };
    DoctorProbe {
        required: false,
        status,
        code: "replay.protocol_capabilities".into(),
        message: "registered replay protocol capabilities inspected without network access".into(),
        remediation: "build with supported protocol adapters".into(),
    }
}

fn path_doctor_probe(path: Option<&Path>, name: &str, label: &str) -> DoctorProbe {
    let code = format!("storage.{name}");
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code,
            message: format!("{label} was not supplied"),
            remediation: format!("supply --{} to check this path", name.replace('_', "-")),
        };
    };

    let directory = match fs::metadata(path) {
        Ok(metadata) => metadata.is_dir().then_some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            fs::metadata(parent)
                .ok()
                .filter(std::fs::Metadata::is_dir)
                .map(|_| parent)
        }
        Err(_) => None,
    };

    let status = directory.map_or(DoctorStatus::Unsupported, |directory| {
        let Ok(stats) = rustix::fs::statvfs(directory) else {
            return DoctorStatus::Unsupported;
        };
        let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
        if available_bytes == 0 || !probe_directory_io(directory) {
            DoctorStatus::Unsupported
        } else if available_bytes < DEFAULT_MAX_WAL_BYTES {
            DoctorStatus::SupportedWithWarnings
        } else {
            DoctorStatus::Supported
        }
    });
    DoctorProbe {
        required: false,
        status,
        code,
        message: format!("{label} inspected without creating or overwriting it"),
        remediation: "choose a new path below a writable existing directory".into(),
    }
}

fn probe_directory_io(directory: &Path) -> bool {
    let temporary = directory.join(format!(".chronicle-doctor-{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut created = false;
    let io_result = options.open(&temporary).and_then(|mut file| {
        created = true;
        file.try_lock()?;
        file.write_all(b"doctor-data")?;
        file.write_all(b"doctor-commit-marker")?;
        file.sync_all()?;
        let expected_bytes = (b"doctor-data".len() + b"doctor-commit-marker".len()) as u64;
        (file.metadata()?.len() == expected_bytes)
            .then_some(())
            .ok_or_else(|| std::io::Error::other("doctor probe byte accounting failed"))
    });
    let publication_result =
        io_result.is_ok() && probe_no_replace_publication(directory, &temporary);
    let cleanup_result = if temporary.exists() {
        created && fs::remove_file(&temporary).is_ok() && !temporary.exists()
    } else {
        true
    };
    publication_result && cleanup_result
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn probe_no_replace_publication(directory: &Path, temporary: &Path) -> bool {
    let published = directory.join(format!(
        ".chronicle-doctor-published-{}",
        uuid::Uuid::new_v4()
    ));
    let renamed = renameat_with(CWD, temporary, CWD, &published, RenameFlags::NOREPLACE).is_ok();
    let synced = renamed
        && File::open(directory)
            .and_then(|directory| directory.sync_all())
            .is_ok();
    let cleaned = if renamed {
        fs::remove_file(&published).is_ok() && !published.exists()
    } else {
        true
    };
    renamed && synced && cleaned
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn probe_no_replace_publication(_directory: &Path, _temporary: &Path) -> bool {
    false
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn capture_doctor_probes() -> Vec<DoctorProbe> {
    let checks = chronicle_capture_ebpf::probe_embedded();
    [
        ("capture.platform", true, checks.platform),
        ("capture.architecture", true, checks.architecture),
        ("capture.cgroup_v2", true, checks.cgroup_v2),
        ("capture.btf", true, checks.btf),
        ("capture.object", true, checks.embedded_object),
        ("capture.programs", true, checks.required_programs),
        ("capture.attach", false, checks.attach),
        ("capture.cap_bpf", true, checks.cap_bpf),
        ("capture.cap_net_admin", true, checks.cap_net_admin),
    ]
    .into_iter()
    .map(|(code, required, check)| {
        let (status, message) = match check {
            chronicle_capture_ebpf::PreflightCheck::Available => {
                (DoctorStatus::Supported, "available")
            }
            chronicle_capture_ebpf::PreflightCheck::Unavailable(reason) => {
                (DoctorStatus::Unsupported, reason)
            }
            chronicle_capture_ebpf::PreflightCheck::NotChecked(reason) => {
                (DoctorStatus::NotChecked, reason)
            }
        };
        DoctorProbe {
            required,
            status,
            code: code.into(),
            message: message.into(),
            remediation: "resolve this local eBPF prerequisite before recording".into(),
        }
    })
    .collect()
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
fn capture_doctor_probes() -> Vec<DoctorProbe> {
    vec![DoctorProbe {
        required: true,
        status: DoctorStatus::Unsupported,
        code: "capture.platform".into(),
        message: "Linux eBPF capture is unavailable in this build".into(),
        remediation: "run a Linux build with --features linux-ebpf".into(),
    }]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProductionRecordingBounds {
    pub duration_seconds: u64,
    pub segment_bytes: u64,
    pub max_wal_bytes: u64,
}

impl Default for ProductionRecordingBounds {
    fn default() -> Self {
        Self {
            duration_seconds: DEFAULT_RECORDING_DURATION_SECONDS,
            segment_bytes: DEFAULT_SEGMENT_BYTES,
            max_wal_bytes: DEFAULT_MAX_WAL_BYTES,
        }
    }
}

/// Signal state shared by platform adapters and synchronous recording.
#[derive(Clone, Default)]
pub struct ProductionSignalStop(Arc<AtomicU8>);

impl ProductionSignalStop {
    /// Requests graceful shutdown. Returns false when this is a second signal.
    pub fn request_interrupt(&self) -> bool {
        self.request(1)
    }

    /// Requests graceful shutdown. Returns false when this is a second signal.
    pub fn request_termination(&self) -> bool {
        self.request(2)
    }

    pub fn shutdown_reason(&self) -> Option<ShutdownReason> {
        match self.0.load(Ordering::SeqCst) {
            1 => Some(ShutdownReason::UserInterrupt),
            2 => Some(ShutdownReason::TerminationSignal),
            _ => None,
        }
    }

    fn request(&self, signal: u8) -> bool {
        self.0
            .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

pub fn validate_production_recording_bounds(
    bounds: ProductionRecordingBounds,
) -> Result<(), ApplicationError> {
    if !(1..=MAX_RECORDING_DURATION_SECONDS).contains(&bounds.duration_seconds) {
        return Err(ApplicationError::ProductionPreflight(
            "duration bound invalid",
        ));
    }
    if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&bounds.segment_bytes)
        || bounds.max_wal_bytes < bounds.segment_bytes
        || bounds.max_wal_bytes > DEFAULT_MAX_WAL_BYTES
    {
        return Err(ApplicationError::ProductionPreflight("WAL bounds invalid"));
    }
    Ok(())
}

/// Combines selector and eBPF prerequisites before a source can attach or metadata can succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionWalPreflight {
    pub available_bytes: u64,
    pub low_space_warning: bool,
}

/// Checks writable exclusive WAL destination and available capacity without creating recording metadata.
pub fn preflight_wal_destination(
    wal_directory: &Path,
    bounds: ProductionRecordingBounds,
) -> Result<ProductionWalPreflight, ApplicationError> {
    validate_production_recording_bounds(bounds)?;
    if wal_directory.exists() {
        return Err(ApplicationError::ProductionPreflight(
            "WAL destination already exists",
        ));
    }
    let parent = wal_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(ApplicationError::ProductionPreflight(
            "WAL parent unavailable",
        ))?;
    fs::create_dir_all(parent)
        .map_err(|_| ApplicationError::ProductionPreflight("WAL parent unwritable"))?;
    let stats = rustix::fs::statvfs(parent)
        .map_err(|_| ApplicationError::ProductionPreflight("WAL free space unavailable"))?;
    let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
    Ok(ProductionWalPreflight {
        available_bytes,
        low_space_warning: available_bytes < bounds.max_wal_bytes,
    })
}

/// Combines selector and eBPF prerequisites before a source can attach or metadata can succeed.
pub fn preflight_production_record(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    bounds: ProductionRecordingBounds,
    wal_directory: &Path,
) -> Result<CgroupSelection, ApplicationError> {
    preflight_wal_destination(wal_directory, bounds)?;
    let selection = preflight_cgroup_selection(selector, allow_shared_cgroup)
        .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?;
    preflight_embedded_ebpf()?;
    Ok(selection)
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn preflight_embedded_ebpf() -> Result<(), ApplicationError> {
    chronicle_capture_ebpf::probe_embedded()
        .is_ready()
        .then_some(())
        .ok_or(ApplicationError::ProductionPreflight(
            "eBPF prerequisites unavailable",
        ))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
fn preflight_embedded_ebpf() -> Result<(), ApplicationError> {
    Err(ApplicationError::ProductionPreflight(
        "Linux eBPF capture unavailable",
    ))
}

/// Runs live eBPF recording. Signal adapters request stop through `stop`.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn record_live_ebpf(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    wal_directory: impl AsRef<Path>,
    bounds: ProductionRecordingBounds,
    stop: ProductionSignalStop,
) -> Result<ProductionRecordingResult, ApplicationError> {
    preflight_wal_destination(wal_directory.as_ref(), bounds)?;
    preflight_embedded_ebpf()?;
    if matches!(&selector, CgroupSelector::Pid(_)) && allow_shared_cgroup {
        return Err(ApplicationError::ProductionPreflight(
            "shared cgroup acknowledgement requires explicit cgroup",
        ));
    }
    let pid_baseline = match &selector {
        CgroupSelector::Pid(pid) => {
            Some(preflight_pid_cgroup_selection(*pid).map_err(|_| {
                ApplicationError::ProductionPreflight("PID cgroup selection invalid")
            })?)
        }
        CgroupSelector::Explicit(_) => None,
    };
    let selection = match &pid_baseline {
        Some(baseline) => baseline.selection().clone(),
        None => preflight_cgroup_selection(selector, allow_shared_cgroup)
            .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?,
    };
    let capture_metadata = live_capture_metadata(&selection, bounds)?;
    let mut metadata = RecordingMetadata {
        version: RECORDING_METADATA_SCHEMA_VERSION,
        recording_id: RecordingId::new(),
        selector: Some(RecordingSelectorIdentity {
            canonical_cgroup_path: selection.canonical_path.display().to_string(),
            cgroup_id: selection.cgroup_id,
        }),
        status: RecordingStatus::Starting,
        shutdown_reason: None,
        last_valid_commit: None,
        counters: RecordingCounters::default(),
        terminal_wal_loss: None,
        capture: None,
    };
    record_production(
        wal_directory,
        &mut metadata,
        capture_metadata,
        bounds,
        || load_production_ebpf_source(&selection, pid_baseline.as_ref()),
        monotonic_millis,
        || stop.shutdown_reason(),
    )
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn live_capture_metadata(
    selection: &CgroupSelection,
    bounds: ProductionRecordingBounds,
) -> Result<RecordingCaptureMetadata, ApplicationError> {
    let kernel_release = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map_err(|_| ApplicationError::ProductionPreflight("kernel identity unavailable"))?
        .trim()
        .to_owned();
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| ApplicationError::ProductionPreflight("boot clock identity unavailable"))?
        .trim()
        .to_owned();
    let ebpf_object_sha256 = chronicle_capture_ebpf::embedded_object_sha256().ok_or(
        ApplicationError::ProductionPreflight("eBPF object unavailable"),
    )?;
    let recording_bounds = RecordingBounds {
        duration_seconds: bounds.duration_seconds,
        max_wal_bytes: bounds.max_wal_bytes,
        segment_bytes: bounds.segment_bytes,
        ring_bytes: 8 * 1024 * 1024,
    };
    Ok(RecordingCaptureMetadata {
        version: RECORDING_CAPTURE_METADATA_SCHEMA_VERSION,
        build: RecordingBuildIdentity {
            chronicle_version: env!("CARGO_PKG_VERSION").into(),
            aya_version: "0.14.0".into(),
            aya_ebpf_version: "0.2.1".into(),
            ebpf_object_sha256,
        },
        host: RecordingHostIdentity {
            kernel_release,
            architecture: std::env::consts::ARCH.into(),
            boot_id,
        },
        scope: RecordingSelectorScope {
            direct_tgid_count: selection.direct_tgid_count(),
            descendant_cgroup_count: selection.descendant_cgroup_count(),
            selected_subtree: true,
            shared_scope_acknowledged: selection.shared_scope_acknowledged,
        },
        capabilities: BTreeSet::from(["CAP_BPF".into(), "CAP_NET_ADMIN".into()]),
        configured_bounds: recording_bounds.clone(),
        effective_bounds: recording_bounds,
        errors: Vec::new(),
    })
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn monotonic_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

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

pub const INGEST_QUEUE_CAPACITY: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedWalRecord {
    pub kind: RecordKind,
    pub schema_version: u16,
    pub flags: u16,
    pub payload: Vec<u8>,
    /// Capture-domain time. Never substitute queue, flush, or wall-clock time.
    pub capture_timestamp: Option<MonotonicTimestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestAdmission {
    Accepted,
    RejectedAfterStop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IngestState {
    Accepting,
    Stopping(ShutdownReason),
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingIngestResult {
    pub status: RecordingStatus,
    pub shutdown_reason: ShutdownReason,
    pub counters: RecordingCounters,
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionRecordingResult {
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub shutdown_reason: ShutdownReason,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub counters: RecordingCounters,
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
    pub source_summary: CaptureSourceSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWalLossPersistence {
    PendingWal,
    PersistedWal,
    MetadataOnly,
    NotPersistedDueToWalFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWalLossTimeSource {
    Observed,
    FallbackLastPersisted,
    TimestampUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWalLossSummaryEntry {
    pub clock: Option<ClockIdentity>,
    pub start: Option<MonotonicTimestamp>,
    pub end: Option<MonotonicTimestamp>,
    pub discarded: RecordByteCount,
    pub time_source: TerminalWalLossTimeSource,
    pub persistence: TerminalWalLossPersistence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWalLossSummary {
    pub entries: Vec<TerminalWalLossSummaryEntry>,
    pub discarded: RecordByteCount,
}

#[derive(Debug)]
pub struct RecordingIngestFailure {
    pub error: WalError,
    pub result: RecordingIngestResult,
}

impl std::fmt::Display for RecordingIngestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "recording ingest failed: {}", self.error)
    }
}

impl std::error::Error for RecordingIngestFailure {}

/// Application-owned bounded queue. WAL remains sole authority for physical capacity.
pub struct RecordingIngest {
    writer: GroupCommitWalWriter,
    queue: VecDeque<QueuedWalRecord>,
    state: IngestState,
    counters: RecordingCounters,
    written: RecordByteCount,
    written_records: BTreeMap<u64, RecordByteCount>,
    written_capture_timestamps: BTreeMap<u64, MonotonicTimestamp>,
    persisted_capture_timestamps: BTreeMap<ClockIdentity, MonotonicTimestamp>,
    terminal_discarded: BTreeMap<ClockIdentity, TerminalDiscardAccumulator>,
    terminal_discarded_without_time: RecordByteCount,
    terminal_wal_loss: Option<TerminalWalLossSummary>,
}

#[derive(Clone, Debug, Default)]
struct TerminalDiscardAccumulator {
    discarded: RecordByteCount,
    start: Option<MonotonicTimestamp>,
    end: Option<MonotonicTimestamp>,
}

impl RecordingIngest {
    pub fn new(writer: GroupCommitWalWriter) -> Self {
        Self {
            writer,
            queue: VecDeque::with_capacity(INGEST_QUEUE_CAPACITY),
            state: IngestState::Accepting,
            counters: RecordingCounters::default(),
            written: RecordByteCount::default(),
            written_records: BTreeMap::new(),
            written_capture_timestamps: BTreeMap::new(),
            persisted_capture_timestamps: BTreeMap::new(),
            terminal_discarded: BTreeMap::new(),
            terminal_discarded_without_time: RecordByteCount::default(),
            terminal_wal_loss: None,
        }
    }

    /// Returns the unqueued record on backpressure so callers can block and retry without loss.
    pub fn admit(&mut self, record: QueuedWalRecord) -> Result<IngestAdmission, QueuedWalRecord> {
        let bytes = record_bytes(&record);
        match self.state {
            IngestState::Accepting if self.queue.len() < INGEST_QUEUE_CAPACITY => {
                add_count(&mut self.counters.received, bytes);
                add_count(&mut self.counters.accepted_into_queue, bytes);
                self.queue.push_back(record);
                Ok(IngestAdmission::Accepted)
            }
            IngestState::Accepting => Err(record),
            IngestState::Stopping(_) | IngestState::Terminal => {
                add_count(&mut self.counters.rejected_after_stop, bytes);
                Ok(IngestAdmission::RejectedAfterStop)
            }
        }
    }

    pub fn record_kernel_or_backend_drop(&mut self, records: u64, bytes: u64) {
        self.counters.kernel_or_backend_dropped.records = self
            .counters
            .kernel_or_backend_dropped
            .records
            .saturating_add(records);
        self.counters.kernel_or_backend_dropped.bytes = self
            .counters
            .kernel_or_backend_dropped
            .bytes
            .saturating_add(bytes);
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn counters(&self) -> &RecordingCounters {
        &self.counters
    }

    fn commit_boundary(&self) -> Option<RecordingCommitBoundary> {
        let authority = self.writer.authority();
        Some(RecordingCommitBoundary {
            marker_sequence: authority.marker_sequence?,
            durable_through_sequence: authority.durable_through_sequence?,
            durable_record_count: authority.durable_record_count,
            durable_payload_bytes: authority.durable_payload_bytes,
            segment_ordinal: authority.segment_ordinal?,
        })
    }

    pub fn stop_reason(&self) -> Option<ShutdownReason> {
        match self.state {
            IngestState::Stopping(reason) => Some(reason),
            IngestState::Accepting | IngestState::Terminal => None,
        }
    }

    /// Writes FIFO until empty or WAL's physical-capacity check refuses the next record.
    pub fn drain(&mut self, now_millis: u64) -> Result<(), WalError> {
        while self.state == IngestState::Accepting {
            let Some(record) = self.queue.pop_front() else {
                break;
            };
            let bytes = record_bytes(&record);
            let QueuedWalRecord {
                kind,
                schema_version,
                flags,
                payload,
                capture_timestamp,
            } = record;
            // On a write/flush error WAL may have accepted the frame before reporting failure.
            // Count it as written-not-committed uncertainty; capacity rejection is preflight-only.
            add_count(&mut self.written, bytes);
            match self
                .writer
                .append(kind, schema_version, flags, payload, now_millis)
            {
                Ok(append) => {
                    self.written_records
                        .insert(append.sequence, RecordByteCount { records: 1, bytes });
                    if let Some(timestamp) = capture_timestamp {
                        self.written_capture_timestamps
                            .insert(append.sequence, timestamp);
                    }
                    self.refresh_durability();
                }
                Err(error) if error.is_capacity_exhausted() => {
                    subtract_count(&mut self.written, bytes);
                    self.record_wal_limit_discard(bytes, capture_timestamp);
                    while let Some(discarded) = self.queue.pop_front() {
                        let bytes = record_bytes(&discarded);
                        self.record_wal_limit_discard(bytes, discarded.capture_timestamp);
                    }
                    self.build_terminal_wal_loss_summary();
                    self.state = IngestState::Stopping(ShutdownReason::WalSizeLimit);
                    self.refresh_durability();
                    break;
                }
                Err(error) => {
                    self.state = IngestState::Stopping(ShutdownReason::WalFailure);
                    self.refresh_durability();
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// WAL-limit detection wins over duration when both occur in one shutdown cycle.
    #[allow(clippy::result_large_err)] // Failure includes safe final counters needed by callers.
    pub fn finish(
        &mut self,
        requested_reason: ShutdownReason,
        now_millis: u64,
    ) -> Result<RecordingIngestResult, RecordingIngestFailure> {
        if let Err(error) = self.drain(now_millis) {
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        let shutdown_reason = match self.state {
            IngestState::Accepting => {
                self.state = IngestState::Stopping(requested_reason);
                requested_reason
            }
            IngestState::Stopping(reason) => reason,
            IngestState::Terminal => requested_reason,
        };
        if shutdown_reason == ShutdownReason::WalSizeLimit
            && let Err(error) = self.append_terminal_wal_loss(now_millis)
        {
            self.refresh_durability();
            self.mark_terminal_loss_not_persisted();
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        if let Err(error) = self.writer.shutdown(now_millis) {
            self.refresh_durability();
            self.mark_terminal_loss_not_persisted();
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        self.refresh_durability();
        self.mark_terminal_loss_persisted();
        self.state = IngestState::Terminal;
        Ok(RecordingIngestResult {
            status: RecordingStatus::Completed,
            shutdown_reason,
            counters: self.counters.clone(),
            terminal_wal_loss: self.terminal_wal_loss.clone(),
        })
    }

    fn failure_result(&self) -> RecordingIngestResult {
        RecordingIngestResult {
            status: RecordingStatus::Failed,
            shutdown_reason: ShutdownReason::WalFailure,
            counters: self.counters.clone(),
            terminal_wal_loss: self.terminal_wal_loss.clone(),
        }
    }

    fn record_wal_limit_discard(&mut self, bytes: u64, timestamp: Option<MonotonicTimestamp>) {
        add_count(
            &mut self.counters.discarded_from_queue_due_to_wal_limit,
            bytes,
        );
        let Some(timestamp) = timestamp.filter(valid_capture_timestamp) else {
            add_count(&mut self.terminal_discarded_without_time, bytes);
            return;
        };
        let accumulator = self
            .terminal_discarded
            .entry(timestamp.clock.clone())
            .or_default();
        add_count(&mut accumulator.discarded, bytes);
        if accumulator
            .start
            .as_ref()
            .is_none_or(|start| timestamp.nanoseconds < start.nanoseconds)
        {
            accumulator.start = Some(timestamp.clone());
        }
        if accumulator
            .end
            .as_ref()
            .is_none_or(|end| timestamp.nanoseconds > end.nanoseconds)
        {
            accumulator.end = Some(timestamp);
        }
    }

    fn build_terminal_wal_loss_summary(&mut self) {
        let mut entries: Vec<_> = self
            .terminal_discarded
            .iter()
            .map(|(clock, accumulator)| TerminalWalLossSummaryEntry {
                clock: Some(clock.clone()),
                start: accumulator.start.clone(),
                end: accumulator.end.clone(),
                discarded: accumulator.discarded.clone(),
                time_source: TerminalWalLossTimeSource::Observed,
                persistence: TerminalWalLossPersistence::PendingWal,
            })
            .collect();
        if self.terminal_discarded_without_time.records != 0 {
            let fallback = (self.persisted_capture_timestamps.len() == 1)
                .then(|| self.persisted_capture_timestamps.values().next().cloned())
                .flatten();
            entries.push(TerminalWalLossSummaryEntry {
                clock: fallback.as_ref().map(|timestamp| timestamp.clock.clone()),
                start: fallback,
                end: None,
                discarded: self.terminal_discarded_without_time.clone(),
                time_source: if self.persisted_capture_timestamps.len() == 1 {
                    TerminalWalLossTimeSource::FallbackLastPersisted
                } else {
                    TerminalWalLossTimeSource::TimestampUnavailable
                },
                persistence: TerminalWalLossPersistence::MetadataOnly,
            });
        }
        self.terminal_wal_loss = (!entries.is_empty()).then_some(TerminalWalLossSummary {
            entries,
            discarded: self.counters.discarded_from_queue_due_to_wal_limit.clone(),
        });
    }

    fn append_terminal_wal_loss(&mut self, now_millis: u64) -> Result<(), WalError> {
        let Some(summary) = &mut self.terminal_wal_loss else {
            return Ok(());
        };
        for entry in &mut summary.entries {
            if entry.persistence != TerminalWalLossPersistence::PendingWal {
                continue;
            }
            let (Some(start), Some(end)) = (entry.start.clone(), entry.end.clone()) else {
                entry.persistence = TerminalWalLossPersistence::MetadataOnly;
                continue;
            };
            let loss = TerminalWalLoss {
                interval: TerminalWalLossInterval { start, end },
                discarded_records: entry.discarded.records,
                discarded_payload_bytes: entry.discarded.bytes,
                reason: TerminalWalLossReason::WalHardLimit,
                ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
            };
            let payload = encode_terminal_wal_loss(&loss)?;
            match self.writer.append(
                RecordKind::TerminalWalLoss,
                TERMINAL_WAL_LOSS_SCHEMA_VERSION,
                0,
                payload,
                now_millis,
            ) {
                Ok(_) => {}
                Err(error) if error.is_capacity_exhausted() => {
                    entry.persistence = TerminalWalLossPersistence::MetadataOnly;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn mark_terminal_loss_persisted(&mut self) {
        if let Some(summary) = &mut self.terminal_wal_loss {
            for entry in &mut summary.entries {
                if entry.persistence == TerminalWalLossPersistence::PendingWal {
                    entry.persistence = TerminalWalLossPersistence::PersistedWal;
                }
            }
        }
    }

    fn mark_terminal_loss_not_persisted(&mut self) {
        if let Some(summary) = &mut self.terminal_wal_loss {
            for entry in &mut summary.entries {
                if entry.persistence == TerminalWalLossPersistence::PendingWal {
                    entry.persistence = TerminalWalLossPersistence::NotPersistedDueToWalFailure;
                }
            }
        }
    }

    fn refresh_durability(&mut self) {
        let authority = self.writer.authority();
        if let Some(durable_through) = authority.durable_through_sequence {
            let durable_sequences: Vec<_> = self
                .written_capture_timestamps
                .range(..=durable_through)
                .map(|(sequence, timestamp)| (*sequence, timestamp.clone()))
                .collect();
            for (sequence, timestamp) in durable_sequences {
                self.written_capture_timestamps.remove(&sequence);
                self.persisted_capture_timestamps
                    .entry(timestamp.clock.clone())
                    .and_modify(|current| {
                        if current.nanoseconds < timestamp.nanoseconds {
                            *current = timestamp.clone();
                        }
                    })
                    .or_insert(timestamp);
            }
        }
        if let Some(durable_through) = authority.durable_through_sequence {
            let durable_sequences: Vec<_> = self
                .written_records
                .range(..=durable_through)
                .map(|(sequence, count)| (*sequence, count.clone()))
                .collect();
            for (sequence, count) in durable_sequences {
                self.written_records.remove(&sequence);
                self.counters.committed.records = self
                    .counters
                    .committed
                    .records
                    .saturating_add(count.records);
                self.counters.committed.bytes =
                    self.counters.committed.bytes.saturating_add(count.bytes);
            }
        }
        self.counters.written_not_committed.records = self
            .written
            .records
            .saturating_sub(self.counters.committed.records);
        self.counters.written_not_committed.bytes = self
            .written
            .bytes
            .saturating_sub(self.counters.committed.bytes);
    }
}

fn valid_capture_timestamp(timestamp: &MonotonicTimestamp) -> bool {
    !timestamp.clock.boot_id.is_empty()
}

fn record_bytes(record: &QueuedWalRecord) -> u64 {
    u64::try_from(record.payload.len()).unwrap_or(u64::MAX)
}

fn add_count(count: &mut RecordByteCount, bytes: u64) {
    count.records = count.records.saturating_add(1);
    count.bytes = count.bytes.saturating_add(bytes);
}

fn subtract_count(count: &mut RecordByteCount, bytes: u64) {
    count.records = count.records.saturating_sub(1);
    count.bytes = count.bytes.saturating_sub(bytes);
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
pub struct RecordingBuildIdentity {
    pub chronicle_version: String,
    pub aya_version: String,
    pub aya_ebpf_version: String,
    pub ebpf_object_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingHostIdentity {
    pub kernel_release: String,
    pub architecture: String,
    pub boot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSelectorScope {
    pub direct_tgid_count: usize,
    pub descendant_cgroup_count: usize,
    pub selected_subtree: bool,
    pub shared_scope_acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingBounds {
    pub duration_seconds: u64,
    pub max_wal_bytes: u64,
    pub segment_bytes: u64,
    pub ring_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingCaptureErrorCode {
    Unsupported,
    Capability,
    Object,
    Attach,
    Source,
    LossSample,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCaptureError {
    pub code: RecordingCaptureErrorCode,
}

/// Capture-specific fields appended to core metadata after feasibility validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCaptureMetadata {
    pub version: u16,
    pub build: RecordingBuildIdentity,
    pub host: RecordingHostIdentity,
    pub scope: RecordingSelectorScope,
    pub capabilities: BTreeSet<String>,
    pub configured_bounds: RecordingBounds,
    pub effective_bounds: RecordingBounds,
    pub errors: Vec<RecordingCaptureError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub version: u16,
    pub recording_id: RecordingId,
    pub selector: Option<RecordingSelectorIdentity>,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub counters: RecordingCounters,
    #[serde(default)]
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
    #[serde(default)]
    pub capture: Option<RecordingCaptureMetadata>,
}

impl RecordingMetadata {
    /// Persist feasibility-checked capture context before live eBPF work starts.
    pub fn transition_to_recording(
        &mut self,
        capture: RecordingCaptureMetadata,
    ) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Starting || self.shutdown_reason.is_some() {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a starting recording may transition to recording".into(),
            ));
        }
        self.capture = Some(capture);
        self.status = RecordingStatus::Recording;
        validate_recording_metadata(self)
    }

    pub fn fail_start(&mut self, reason: ShutdownReason) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Starting {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a starting recording may fail before attachment".into(),
            ));
        }
        self.status = RecordingStatus::Failed;
        self.shutdown_reason = Some(reason);
        validate_recording_metadata(self)
    }

    pub fn finalize(
        &mut self,
        status: RecordingStatus,
        reason: ShutdownReason,
    ) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Recording
            || !matches!(status, RecordingStatus::Completed | RecordingStatus::Failed)
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a recording may finalize as completed or failed".into(),
            ));
        }
        self.status = status;
        self.shutdown_reason = Some(reason);
        validate_recording_metadata(self)
    }
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
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

pub fn decode_recording_metadata(bytes: &[u8]) -> Result<RecordingMetadata, ApplicationError> {
    let metadata: RecordingMetadata = serde_json::from_slice(bytes)
        .map_err(|error| ApplicationError::RecordingMetadataValidation(error.to_string()))?;
    validate_recording_metadata(&metadata)?;
    Ok(metadata)
}

pub fn load_recording_metadata(
    wal_directory: impl AsRef<Path>,
) -> Result<Option<RecordingMetadata>, ApplicationError> {
    match fs::read(wal_directory.as_ref().join("recording.json")) {
        Ok(bytes) => decode_recording_metadata(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_recording_metadata(
    wal_directory: impl AsRef<Path>,
    metadata: &RecordingMetadata,
) -> Result<(), ApplicationError> {
    write_recording_metadata_inner(wal_directory.as_ref(), metadata, None)
}

/// Best-effort persistence for a second signal before forced process exit.
pub fn mark_recording_forced_termination(
    wal_directory: impl AsRef<Path>,
) -> Result<(), ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let Some(mut metadata) = load_recording_metadata(wal_directory)? else {
        return Ok(());
    };
    if matches!(
        metadata.status,
        RecordingStatus::Starting | RecordingStatus::Recording
    ) {
        metadata.status = RecordingStatus::Aborted;
        metadata.shutdown_reason = Some(ShutdownReason::ForcedTermination);
        write_recording_metadata(wal_directory, &metadata)?;
    }
    Ok(())
}

impl RecordingIngestResult {
    /// Applies final ingest evidence before atomically persisting `recording.json`.
    pub fn persist_metadata(
        &self,
        wal_directory: impl AsRef<Path>,
        metadata: &mut RecordingMetadata,
    ) -> Result<(), ApplicationError> {
        metadata.status = self.status;
        metadata.shutdown_reason = Some(self.shutdown_reason);
        metadata.counters = self.counters.clone();
        metadata
            .terminal_wal_loss
            .clone_from(&self.terminal_wal_loss);
        write_recording_metadata(wal_directory, metadata)
    }
}

/// Drives one already-preflighted production source through bounded WAL persistence.
/// `requested_stop` supplies source completion or future signal handling; duration is enforced here.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub fn record_production<S, Build, Now, Stop>(
    wal_directory: impl AsRef<Path>,
    metadata: &mut RecordingMetadata,
    capture_metadata: RecordingCaptureMetadata,
    bounds: ProductionRecordingBounds,
    build_source: Build,
    mut now_millis: Now,
    mut requested_stop: Stop,
) -> Result<ProductionRecordingResult, ApplicationError>
where
    S: CaptureSource,
    Build: FnOnce() -> Result<S, ApplicationError>,
    Now: FnMut() -> u64,
    Stop: FnMut() -> Option<ShutdownReason>,
{
    validate_production_recording_bounds(bounds)?;
    if metadata.status != RecordingStatus::Starting || metadata.shutdown_reason.is_some() {
        return Err(ApplicationError::RecordingMetadataValidation(
            "production recording must begin in starting state".into(),
        ));
    }

    let wal_directory = wal_directory.as_ref();
    write_recording_metadata(wal_directory, metadata)?;
    let mut source = match build_source() {
        Ok(source) => source,
        Err(error) => {
            persist_pre_attach_failure(
                wal_directory,
                metadata,
                capture_metadata,
                RecordingCaptureErrorCode::Attach,
            )?;
            return Err(error);
        }
    };
    if let Err(error) = source.start() {
        persist_pre_attach_failure(
            wal_directory,
            metadata,
            capture_metadata,
            RecordingCaptureErrorCode::Source,
        )?;
        return Err(error.into());
    }

    metadata.transition_to_recording(capture_metadata)?;
    write_recording_metadata(wal_directory, metadata)?;
    let started_at = now_millis();
    let writer = match GroupCommitWalWriter::create_with_total_limit(
        wal_directory,
        metadata.recording_id,
        bounds.segment_bytes,
        bounds.max_wal_bytes,
        1,
        started_at,
    ) {
        Ok(writer) => writer,
        Err(error) => {
            let _ = shutdown_source_without_wal(&mut source);
            return persist_live_failure(
                wal_directory,
                metadata,
                RecordingCaptureErrorCode::Source,
                ShutdownReason::WalFailure,
                error.into(),
            );
        }
    };
    let mut ingest = RecordingIngest::new(writer);
    let mut capture_failed = false;
    let mut wal_failed = false;
    let mut requested_reason = loop {
        let now = now_millis();
        if let Some(reason) = requested_stop().or_else(|| {
            (now.saturating_sub(started_at) >= bounds.duration_seconds.saturating_mul(1_000))
                .then_some(ShutdownReason::DurationLimit)
        }) {
            break reason;
        }
        match source.poll() {
            Ok(Some(event)) => match persist_capture_event(&mut ingest, &event, now) {
                Ok(()) => {
                    if let Some(reason) = ingest.stop_reason() {
                        break reason;
                    }
                }
                Err(error) => match recording_failure_reason(&error) {
                    ShutdownReason::WalFailure => {
                        wal_failed = true;
                        break ShutdownReason::WalFailure;
                    }
                    ShutdownReason::CaptureFailure => {
                        capture_failed = true;
                        break ShutdownReason::CaptureFailure;
                    }
                    _ => unreachable!("recording failures have one of two terminal reasons"),
                },
            },
            Ok(None) => {}
            Err(_) => {
                capture_failed = true;
                break ShutdownReason::CaptureFailure;
            }
        }
    };

    let source_summary = match stop_and_finalize_source(&mut source, &mut ingest, &mut now_millis) {
        Ok(summary) => summary,
        Err(ApplicationError::Wal(_)) => {
            requested_reason = ShutdownReason::WalFailure;
            wal_failed = true;
            CaptureSourceSummary::default()
        }
        Err(_) => {
            requested_reason = ShutdownReason::CaptureFailure;
            capture_failed = true;
            CaptureSourceSummary::default()
        }
    };
    let mut result = match ingest.finish(requested_reason, now_millis()) {
        Ok(result) => result,
        Err(failure) => {
            wal_failed = true;
            failure.result
        }
    };
    if capture_failed {
        result.status = RecordingStatus::Failed;
        result.shutdown_reason = ShutdownReason::CaptureFailure;
        append_capture_error(metadata, RecordingCaptureErrorCode::Source);
    } else if wal_failed {
        result.status = RecordingStatus::Failed;
        result.shutdown_reason = ShutdownReason::WalFailure;
    }
    metadata.last_valid_commit = ingest.commit_boundary();
    result.persist_metadata(wal_directory, metadata)?;
    Ok(ProductionRecordingResult {
        recording_id: metadata.recording_id,
        status: result.status,
        shutdown_reason: result.shutdown_reason,
        last_valid_commit: metadata.last_valid_commit.clone(),
        counters: result.counters,
        terminal_wal_loss: result.terminal_wal_loss,
        source_summary,
    })
}

/// Counts every physical byte below the WAL segments directory.
pub fn recording_physical_wal_bytes(
    wal_directory: impl AsRef<Path>,
) -> Result<u64, ApplicationError> {
    fn count(path: &Path) -> Result<u64, ApplicationError> {
        fs::read_dir(path)?.try_fold(0_u64, |total, entry| {
            let entry = entry?;
            let metadata = entry.metadata()?;
            let bytes = if metadata.is_dir() {
                count(&entry.path())?
            } else {
                metadata.len()
            };
            total.checked_add(bytes).ok_or_else(|| {
                ApplicationError::RecordingMetadataValidation(
                    "physical WAL size exceeds u64".into(),
                )
            })
        })
    }

    let segments = wal_directory.as_ref().join("segments");
    if !segments.is_dir() {
        return Ok(0);
    }
    count(&segments)
}

/// Builds the Linux eBPF source after `recording.json` reached `starting`.
/// PID baselines are rechecked after links attach; source construction drops links on failure.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn load_production_ebpf_source(
    selection: &CgroupSelection,
    pid_baseline: Option<&PidCgroupSelection>,
) -> Result<chronicle_capture_ebpf::EbpfCaptureSource, ApplicationError> {
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if boot_id.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "boot clock identity unavailable",
        ));
    }
    let adapter = chronicle_capture_ebpf::CaptureAdapter::new(
        ClockIdentity { boot_id },
        chronicle_capture_ebpf::RecordingScopeConfig {
            identity: chronicle_capture::RecordingScopeIdentity {
                cgroup_id: selection.cgroup_id,
                canonical_path: selection.canonical_path.display().to_string(),
                namespace: None,
            },
            descendant_cgroup_ids: selection.descendant_cgroup_ids.clone(),
        },
        0,
    )?;
    let cgroup = File::open(&selection.canonical_path)?;
    chronicle_capture_ebpf::EbpfCaptureSource::load_embedded_with_post_attach(
        &cgroup,
        adapter,
        || {
            if let Some(baseline) = pid_baseline {
                baseline.revalidate().map_err(|_| {
                    chronicle_capture_ebpf::EbpfCaptureError::MissingIdentity(
                        "PID cgroup scope changed after attach",
                    )
                })?;
            }
            Ok(())
        },
    )
    .map_err(ApplicationError::from)
}

fn persist_pre_attach_failure(
    wal_directory: &Path,
    metadata: &mut RecordingMetadata,
    mut capture_metadata: RecordingCaptureMetadata,
    error: RecordingCaptureErrorCode,
) -> Result<(), ApplicationError> {
    if capture_metadata.errors.len() < MAX_RECORDING_CAPTURE_ERRORS {
        capture_metadata
            .errors
            .push(RecordingCaptureError { code: error });
    }
    metadata.capture = Some(capture_metadata);
    metadata.fail_start(ShutdownReason::CaptureFailure)?;
    write_recording_metadata(wal_directory, metadata)
}

fn persist_live_failure(
    wal_directory: &Path,
    metadata: &mut RecordingMetadata,
    error: RecordingCaptureErrorCode,
    reason: ShutdownReason,
    original: ApplicationError,
) -> Result<ProductionRecordingResult, ApplicationError> {
    if reason == ShutdownReason::CaptureFailure {
        append_capture_error(metadata, error);
    }
    metadata.finalize(RecordingStatus::Failed, reason)?;
    write_recording_metadata(wal_directory, metadata)?;
    Err(original)
}

fn recording_failure_reason(error: &ApplicationError) -> ShutdownReason {
    if matches!(error, ApplicationError::Wal(_)) {
        ShutdownReason::WalFailure
    } else {
        ShutdownReason::CaptureFailure
    }
}

fn append_capture_error(metadata: &mut RecordingMetadata, error: RecordingCaptureErrorCode) {
    if let Some(capture) = &mut metadata.capture
        && capture.errors.len() < MAX_RECORDING_CAPTURE_ERRORS
    {
        capture.errors.push(RecordingCaptureError { code: error });
    }
}

fn persist_capture_event(
    ingest: &mut RecordingIngest,
    event: &CaptureEvent,
    now_millis: u64,
) -> Result<(), ApplicationError> {
    let (kind, capture_timestamp, kernel_drops) = match &event.kind {
        CaptureEventKind::LossWindowObserved(window) => (
            RecordKind::LossWindow,
            Some(window.end.clone()),
            window.drop_delta,
        ),
        CaptureEventKind::SocketConnectObserved(intent) => (
            RecordKind::CaptureEvent,
            Some(intent.timestamp.clone()),
            None,
        ),
        CaptureEventKind::SocketConnected(evidence)
        | CaptureEventKind::SocketClosedObserved(evidence)
        | CaptureEventKind::SocketResetObserved(evidence) => (
            RecordKind::CaptureEvent,
            Some(evidence.timestamp.clone()),
            None,
        ),
        CaptureEventKind::SocketStateChangedObserved(observed) => (
            RecordKind::CaptureEvent,
            Some(observed.socket.timestamp.clone()),
            None,
        ),
        CaptureEventKind::PayloadFragment(fragment) => (
            RecordKind::CaptureEvent,
            Some(fragment.timestamp.clone()),
            None,
        ),
    };
    if let Some(drops) = kernel_drops {
        ingest.record_kernel_or_backend_drop(drops, 0);
    }
    let mut record = QueuedWalRecord {
        kind,
        schema_version: event.schema_version,
        flags: match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => u16::try_from(fragment.flags.0)
                .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(fragment.flags.0))?,
            _ => 0,
        },
        payload: encode_event(event)?,
        capture_timestamp,
    };
    loop {
        match ingest.admit(record) {
            Ok(IngestAdmission::Accepted) => {
                ingest.drain(now_millis)?;
                return Ok(());
            }
            Ok(IngestAdmission::RejectedAfterStop) => return Ok(()),
            Err(backpressured) => {
                record = backpressured;
                ingest.drain(now_millis)?;
                if ingest.stop_reason().is_some() {
                    return Ok(());
                }
            }
        }
    }
}

fn stop_and_finalize_source<Now>(
    source: &mut impl CaptureSource,
    ingest: &mut RecordingIngest,
    now_millis: &mut Now,
) -> Result<CaptureSourceSummary, ApplicationError>
where
    Now: FnMut() -> u64,
{
    let deadline = now_millis().saturating_add(RECORDING_FINALIZATION_GRACE_MILLIS);
    let mut failure = source.request_shutdown().err().map(ApplicationError::from);
    loop {
        if now_millis() > deadline {
            failure.get_or_insert(CaptureError::Drain("finalization grace exceeded".into()).into());
            break;
        }
        match source.drain() {
            Ok(Some(event)) => {
                if let Err(error) = persist_capture_event(ingest, &event, now_millis()) {
                    failure.get_or_insert(error);
                }
            }
            Ok(None) => break,
            Err(error) => {
                failure.get_or_insert(error.into());
                break;
            }
        }
    }
    let summary = source.finalize();
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(summary?)
}

fn shutdown_source_without_wal(source: &mut impl CaptureSource) -> Result<(), CaptureError> {
    let mut failure = source.request_shutdown().err();
    loop {
        match source.drain() {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(error) => {
                failure.get_or_insert(error);
                break;
            }
        }
    }
    let finalized = source.finalize();
    if let Some(error) = failure {
        return Err(error);
    }
    finalized.map(|_| ())
}

fn validate_recording_metadata(metadata: &RecordingMetadata) -> Result<(), ApplicationError> {
    if metadata.version != RECORDING_METADATA_SCHEMA_VERSION {
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
    if let Some(capture) = &metadata.capture {
        if capture.version != RECORDING_CAPTURE_METADATA_SCHEMA_VERSION {
            return Err(ApplicationError::RecordingMetadataValidation(format!(
                "unsupported recording capture metadata version {}",
                capture.version
            )));
        }
        if [
            &capture.build.chronicle_version,
            &capture.build.aya_version,
            &capture.build.aya_ebpf_version,
            &capture.build.ebpf_object_sha256,
            &capture.host.kernel_release,
            &capture.host.architecture,
            &capture.host.boot_id,
        ]
        .iter()
        .any(|value| value.is_empty())
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture identity fields must be non-empty".into(),
            ));
        }
        if !capture.scope.selected_subtree {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture scope must be selected subtree".into(),
            ));
        }
        if capture.capabilities.len() > MAX_RECORDING_CAPABILITIES
            || capture.errors.len() > MAX_RECORDING_CAPTURE_ERRORS
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "recording capture metadata exceeds bounded fields".into(),
            ));
        }
    }
    Ok(())
}

fn write_recording_metadata_inner(
    wal_directory: &Path,
    metadata: &RecordingMetadata,
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
    pub metadata: RecordingMetadata,
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
    let scan = lock.scan(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
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
    let mut metadata = original.clone().unwrap_or_else(|| RecordingMetadata {
        version: RECORDING_METADATA_SCHEMA_VERSION,
        recording_id,
        selector: expected_selector.cloned(),
        status: RecordingStatus::Aborted,
        shutdown_reason: Some(ShutdownReason::ProcessCrashRecovered),
        last_valid_commit: None,
        counters: RecordingCounters::default(),
        terminal_wal_loss: None,
        capture: None,
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
pub struct RecoveryReport {
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
) -> Result<RecoveryReport, ApplicationError> {
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
    Ok(RecoveryReport {
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
) -> Result<RecoveryReport, ApplicationError> {
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

pub fn render_recovery_report_json(report: &RecoveryReport) -> Result<String, ApplicationError> {
    render_json(report)
}

pub fn persist_recovery_report(
    wal_directory: impl AsRef<Path>,
    report: &RecoveryReport,
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

pub fn decode_recovery_report(bytes: &[u8]) -> Result<RecoveryReport, ApplicationError> {
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
    pub recovery_report: RecoveryReport,
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
    let scan = match lock.scan(wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES) {
        Ok(scan) => scan,
        Err(error) => {
            let failure_report = RecoveryReport {
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
    pub recording_id: RecordingId,
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

    let recording_id = RecordingId::new();
    let mut records = Vec::new();
    let mut encoded_bytes =
        u64::try_from(SEGMENT_HEADER_LEN + COMMIT_MARKER_FRAME_LEN).unwrap_or(u64::MAX);
    while let Some(event) = source.next_event()? {
        let flags = match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => u16::try_from(fragment.flags.0)
                .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(fragment.flags.0))?,
            _ => 0,
        };
        let kind = if matches!(event.kind, CaptureEventKind::LossWindowObserved(_)) {
            RecordKind::LossWindow
        } else {
            RecordKind::CaptureEvent
        };
        let payload = encode_event(&event)?;
        let sequence = u64::try_from(records.len() + 1).unwrap_or(u64::MAX);
        encoded_bytes = encoded_bytes.saturating_add(
            u64::try_from(
                encode_envelope(&WalRecordEnvelope::unplaced(
                    recording_id,
                    sequence,
                    kind,
                    CAPTURE_EVENT_SCHEMA_VERSION,
                    flags,
                    payload.clone(),
                ))?
                .len(),
            )
            .unwrap_or(u64::MAX),
        );
        records.push((kind, flags, payload));
    }
    if encoded_bytes > max_segment_bytes {
        return Err(ApplicationError::FixtureWalTooLarge {
            bytes: encoded_bytes,
            limit: max_segment_bytes,
        });
    }

    let mut writer = GroupCommitWalWriter::create(
        directory,
        recording_id,
        max_segment_bytes.max(MIN_SEGMENT_BYTES),
        1,
        0,
    )?;
    for (kind, flags, payload) in records.iter().cloned() {
        writer.append(kind, CAPTURE_EVENT_SCHEMA_VERSION, flags, payload, 0)?;
    }
    writer.flush(0)?;
    drop(writer);
    let scan = chronicle_wal::scan_wal(directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    let checkpoint = if let Some(marker) = scan.final_marker.as_ref() {
        let encoded_len = u64::try_from(encode_envelope(marker)?.len()).unwrap_or(u64::MAX);
        marker.provenance.as_ref().map(|provenance| WalCheckpoint {
            segment_first_sequence: provenance.segment_first_sequence,
            byte_offset: provenance.byte_offset.saturating_add(encoded_len),
            next_sequence: marker.sequence.saturating_add(1),
        })
    } else {
        None
    };
    Ok(RecordedWal {
        recording_id,
        first_sequence: (!records.is_empty()).then_some(1),
        checkpoint,
        record_count: records.len(),
    })
}

pub fn process_fixture_wal(
    directory: impl AsRef<Path>,
    recording_id: RecordingId,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<(EtlOutput, WalCheckpoint), ApplicationError> {
    let scan = chronicle_wal::scan_wal(directory, recording_id, DEFAULT_MAX_RECORD_BYTES)?;
    let marker = scan
        .final_marker
        .as_ref()
        .ok_or(WalError::NoAuthoritativeCommit)?;
    let provenance = marker
        .provenance
        .as_ref()
        .ok_or(WalError::MissingProvenance {
            sequence: marker.sequence,
        })?;
    let checkpoint = WalCheckpoint {
        segment_first_sequence: provenance.segment_first_sequence,
        byte_offset: provenance.byte_offset
            + u64::try_from(encode_envelope(marker)?.len()).unwrap_or(u64::MAX),
        next_sequence: marker.sequence.saturating_add(1),
    };
    let output = EtlPipeline::new(SessionLimits::default()).process_envelopes(
        &scan.committed,
        registry,
        session_id,
    )?;
    Ok((output, checkpoint))
}

#[derive(Clone, Debug)]
pub struct RecordingEtlResult {
    pub output: EtlOutput,
    /// Complete frames beyond final marker are visible but never canonicalized.
    pub ignored_post_commit_records: u64,
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub counters: RecordingCounters,
    pub commit_boundary: RecordingCommitBoundary,
    pub commit_marker_byte_offset: u64,
    pub wal_snapshot_sha256: [u8; 32],
    pub recovery_sha256: [u8; 32],
}

/// ETL one finalized production recording while holding its exclusive WAL lock.
#[allow(clippy::too_many_lines)] // Validation and provenance share one recovery-authoritative boundary.
pub fn process_recording_wal(
    wal_directory: impl AsRef<Path>,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<RecordingEtlResult, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let lock = RecordingLock::acquire(wal_directory)?;
    let metadata = load_recording_metadata(wal_directory)?.ok_or_else(|| {
        ApplicationError::RecordingMetadataValidation("recording metadata is missing".into())
    })?;
    let source_status = match metadata.status {
        RecordingStatus::Completed => SourceStatus::Completed,
        RecordingStatus::Failed => SourceStatus::Failed,
        RecordingStatus::Aborted => SourceStatus::Aborted,
        RecordingStatus::Starting | RecordingStatus::Recording => {
            return Err(ApplicationError::RecordingMetadataValidation(
                "cannot process an active recording".into(),
            ));
        }
    };
    let scan = lock.scan(
        wal_directory,
        metadata.recording_id,
        DEFAULT_MAX_RECORD_BYTES,
    )?;
    let Some(boundary) = metadata.last_valid_commit.as_ref() else {
        return Err(ApplicationError::RecordingMetadataValidation(
            "finalized recording is missing commit boundary".into(),
        ));
    };
    if scan.authority.marker_sequence != Some(boundary.marker_sequence)
        || scan.authority.durable_through_sequence != Some(boundary.durable_through_sequence)
        || scan.authority.durable_record_count != boundary.durable_record_count
        || scan.authority.durable_payload_bytes != boundary.durable_payload_bytes
        || scan.authority.segment_ordinal != Some(boundary.segment_ordinal)
    {
        return Err(ApplicationError::RecordingMetadataValidation(
            "recording commit boundary does not match recovered WAL".into(),
        ));
    }

    let marker_provenance = scan
        .final_marker
        .as_ref()
        .and_then(|marker| marker.provenance.as_ref())
        .ok_or_else(|| {
            ApplicationError::RecordingMetadataValidation(
                "recovered commit marker is missing segment provenance".into(),
            )
        })?;
    let metadata_terminal_losses =
        metadata_terminal_wal_losses(metadata.terminal_wal_loss.as_ref());
    let wal_envelope_range = scan
        .committed
        .first()
        .zip(scan.committed.last())
        .map(|(first, last)| (first.sequence, last.sequence));
    let mut output = EtlPipeline::new(SessionLimits::default())
        .process_envelopes_with_terminal_losses(
            &scan.committed,
            &metadata_terminal_losses,
            registry,
            session_id,
        )?;
    let wal_snapshot_sha256 = verified_snapshot_sha256(wal_directory, &scan)?;
    let recovery_sha256 = recovery_sha256(&scan);
    assign_recording_ids_with_snapshot(&mut output.session, wal_snapshot_sha256)?;
    let connection_evidence: Vec<_> = output
        .session
        .connections
        .iter()
        .filter_map(|connection| {
            let mut sequences = connection
                .operations
                .iter()
                .map(|operation| operation.sequence);
            let first = sequences.next()?;
            let (first, last) = sequences.fold((first, first), |(first, last), sequence| {
                (first.min(sequence), last.max(sequence))
            });
            Some(ProvenanceEntry {
                kind: ProvenanceKind::Connection,
                sequence_range: Some((first, last)),
                reason: None,
            })
        })
        .collect();
    let mut source_evidence: Vec<_> = wal_envelope_range
        .map(|sequence_range| ProvenanceEntry {
            kind: ProvenanceKind::WalEnvelope,
            sequence_range: Some(sequence_range),
            reason: None,
        })
        .into_iter()
        .chain(
            output
                .evidence
                .commit_marker_sequences
                .iter()
                .map(|&sequence| ProvenanceEntry {
                    kind: ProvenanceKind::CommitMarker,
                    sequence_range: Some((sequence, sequence)),
                    reason: None,
                }),
        )
        .chain(connection_evidence)
        .chain(
            output
                .evidence
                .loss_windows
                .iter()
                .map(|loss| ProvenanceEntry {
                    kind: ProvenanceKind::RingLoss,
                    sequence_range: loss.sequence.map(|sequence| (sequence, sequence)),
                    reason: loss.value.ambiguity.reason.clone(),
                }),
        )
        .chain(
            output
                .evidence
                .terminal_wal_losses
                .iter()
                .map(|loss| ProvenanceEntry {
                    kind: ProvenanceKind::WalLimitLoss,
                    sequence_range: loss.sequence.map(|sequence| (sequence, sequence)),
                    reason: Some("wal_hard_limit".into()),
                }),
        )
        .collect();
    source_evidence.sort_by_key(|entry| entry.sequence_range);
    output.session.source_provenance = SourceProvenance {
        recording_id: Some(metadata.recording_id),
        status: source_status,
        reason: metadata
            .shutdown_reason
            .map(shutdown_reason_name)
            .map(str::to_owned),
        commit_marker: Some(CommitMarkerProvenance {
            segment_ordinal: boundary.segment_ordinal,
            byte_offset: marker_provenance.byte_offset,
            sequence: boundary.marker_sequence,
        }),
        wal_snapshot_sha256: Some(sha256_string(&wal_snapshot_sha256)),
        pipeline_version: Some(ETL_PIPELINE_VERSION.into()),
        evidence: source_evidence,
    };
    Ok(RecordingEtlResult {
        output,
        ignored_post_commit_records: u64::try_from(scan.uncommitted.len()).map_err(|_| {
            ApplicationError::RecordingMetadataValidation(
                "post-commit record count exceeds metadata range".into(),
            )
        })?,
        recording_id: metadata.recording_id,
        status: metadata.status,
        counters: metadata.counters,
        commit_boundary: boundary.clone(),
        commit_marker_byte_offset: marker_provenance.byte_offset,
        wal_snapshot_sha256,
        recovery_sha256,
    })
}

pub const RECORDING_ETL_CHECKPOINT_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingEtlCheckpoint {
    pub version: u16,
    pub recording_id: RecordingId,
    pub commit_marker_segment_ordinal: u64,
    pub commit_marker_byte_offset: u64,
    pub commit_marker_sequence: u64,
    pub wal_snapshot_sha256: String,
    pub recovery_sha256: String,
    pub wal_format_version: u16,
    pub pipeline_version: String,
    pub canonical_schema_version: u16,
    pub session_id: chronicle_common::SessionId,
    pub manifest_checksum: String,
    pub output_root: String,
    pub output_identity: String,
    pub status: RecordingStatus,
    pub counters: RecordingCounters,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedRecordingResult {
    pub session_id: chronicle_common::SessionId,
    pub already_published: bool,
    pub ignored_post_commit_records: u64,
}

/// Processes one finalized recording and atomically publishes its deterministic session.
pub fn process_and_publish_recording_wal(
    wal_directory: impl AsRef<Path>,
    root: impl AsRef<Path>,
    registry: &ProtocolRegistry,
) -> Result<PublishedRecordingResult, ApplicationError> {
    let wal_directory = wal_directory.as_ref();
    let root = root.as_ref();
    let processed =
        process_recording_wal(wal_directory, registry, chronicle_common::SessionId::new())?;
    if let Some(checkpoint) = load_recording_etl_checkpoint(wal_directory)?
        && checkpoint.recording_id == processed.recording_id
        && checkpoint.wal_snapshot_sha256 != sha256_string(&processed.wal_snapshot_sha256)
    {
        return Err(ApplicationError::CheckpointContradiction);
    }
    let issues = issue_summaries(&processed.output.issues);
    let replayability = replayability_reasons(&processed.output);
    let complete = session_complete(&processed.output);
    let expected = PublishSession {
        session: processed.output.session,
        checkpoint: None,
        issues: issues.into_iter().map(|issue| issue.code).collect(),
        replayability,
        complete,
    };
    let session_id = expected.session.id;
    let store = FilesystemSessionStore::new(root);
    let already_published = match store.publish(expected.clone()) {
        Ok(()) => false,
        Err(error) => match published_recording_matches(&store, &expected) {
            Ok(true) => true,
            Ok(false) => {
                return Err(ApplicationError::PublishedRecordingMismatch { session_id });
            }
            Err(_) => return Err(error.into()),
        },
    };
    let inspection = store.verify_existing_manifest(session_id)?;
    let checkpoint = RecordingEtlCheckpoint {
        version: RECORDING_ETL_CHECKPOINT_VERSION,
        recording_id: processed.recording_id,
        commit_marker_segment_ordinal: processed.commit_boundary.segment_ordinal,
        commit_marker_byte_offset: processed.commit_marker_byte_offset,
        commit_marker_sequence: processed.commit_boundary.marker_sequence,
        wal_snapshot_sha256: sha256_string(&processed.wal_snapshot_sha256),
        recovery_sha256: sha256_string(&processed.recovery_sha256),
        wal_format_version: chronicle_wal::WAL_FORMAT_VERSION,
        pipeline_version: ETL_PIPELINE_VERSION.into(),
        canonical_schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
        session_id,
        manifest_checksum: inspection.session_checksum,
        output_root: fs::canonicalize(root)?.display().to_string(),
        output_identity: format!("sessions/{session_id}/manifest.json"),
        status: processed.status,
        counters: processed.counters,
    };
    write_private_atomic_json(wal_directory, "etl-checkpoint.json", &checkpoint, None)?;
    Ok(PublishedRecordingResult {
        session_id,
        already_published,
        ignored_post_commit_records: processed.ignored_post_commit_records,
    })
}

fn load_recording_etl_checkpoint(
    wal_directory: &Path,
) -> Result<Option<RecordingEtlCheckpoint>, ApplicationError> {
    match fs::read(wal_directory.join("etl-checkpoint.json")) {
        Ok(bytes) => {
            let checkpoint: RecordingEtlCheckpoint = serde_json::from_slice(&bytes)?;
            if checkpoint.version != RECORDING_ETL_CHECKPOINT_VERSION {
                return Err(ApplicationError::RecordingMetadataValidation(format!(
                    "unsupported recording ETL checkpoint version {}",
                    checkpoint.version
                )));
            }
            Ok(Some(checkpoint))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn recovery_sha256(scan: &RecoveryScan) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"chronicle/recovery/v1\0");
    digest.update(
        scan.authority
            .marker_sequence
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(
        scan.authority
            .durable_through_sequence
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(scan.authority.durable_record_count.to_le_bytes());
    digest.update(scan.authority.durable_payload_bytes.to_le_bytes());
    digest.update(
        scan.authority
            .segment_ordinal
            .unwrap_or_default()
            .to_le_bytes(),
    );
    digest.update(
        u64::try_from(scan.committed.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update(
        u64::try_from(scan.uncommitted.len())
            .unwrap_or(u64::MAX)
            .to_le_bytes(),
    );
    digest.update([u8::from(scan.partial_tail.is_some())]);
    digest.finalize().into()
}

fn sha256_string(bytes: &[u8; 32]) -> String {
    let mut value = String::from("sha256:");
    for byte in bytes {
        write!(value, "{byte:02x}").expect("writing to String cannot fail");
    }
    value
}

fn published_recording_matches(
    store: &FilesystemSessionStore,
    expected: &PublishSession,
) -> Result<bool, StorageError> {
    let inspection = store.inspect_with_metadata(expected.session.id)?;
    let hydrated = store.hydrate(expected.session.id)?;
    Ok(hydrated == expected.session
        && inspection.checkpoint == expected.checkpoint
        && inspection.issues == expected.issues
        && inspection.replayability == expected.replayability
        && inspection.complete == expected.complete)
}

fn metadata_terminal_wal_losses(summary: Option<&TerminalWalLossSummary>) -> Vec<TerminalWalLoss> {
    summary
        .into_iter()
        .flat_map(|summary| &summary.entries)
        .filter(|entry| entry.persistence != TerminalWalLossPersistence::PersistedWal)
        .filter_map(|entry| {
            let (start, end) = (entry.start.clone()?, entry.end.clone()?);
            (start.clock == end.clock).then_some(TerminalWalLoss {
                interval: TerminalWalLossInterval { start, end },
                discarded_records: entry.discarded.records,
                discarded_payload_bytes: entry.discarded.bytes,
                reason: TerminalWalLossReason::WalHardLimit,
                ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
            })
        })
        .collect()
}

fn shutdown_reason_name(reason: ShutdownReason) -> &'static str {
    match reason {
        ShutdownReason::UserInterrupt => "user_interrupt",
        ShutdownReason::TerminationSignal => "termination_signal",
        ShutdownReason::SourceCompleted => "source_completed",
        ShutdownReason::DurationLimit => "duration_limit",
        ShutdownReason::WalSizeLimit => "wal_size_limit",
        ShutdownReason::CaptureFailure => "capture_failure",
        ShutdownReason::WalFailure => "wal_failure",
        ShutdownReason::ProcessCrashRecovered => "process_crash_recovered",
        ShutdownReason::ForcedTermination => "forced_termination",
    }
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
    pub completeness: Completeness,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectConnectionSummary {
    pub protocol: String,
    pub client: chronicle_common::Endpoint,
    pub server: chronicle_common::Endpoint,
    pub completeness: Completeness,
    pub operations: Vec<InspectOperationSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectSessionResult {
    pub session_id: chronicle_common::SessionId,
    pub complete: bool,
    /// Retained compatibility flag: true only for fully replayable sessions.
    pub replayable: bool,
    pub replayability: Replayability,
    pub executable_operations: usize,
    pub non_executable_operations: usize,
    pub integrity_valid: bool,
    pub source_provenance: SourceProvenance,
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
    let (output, checkpoint) = process_fixture_wal(
        &wal_directory,
        recorded.recording_id,
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
                chronicle_etl::EtlIssueKind::PartialWalTail => "partial_wal_tail",
                chronicle_etl::EtlIssueKind::UnknownProtocol => "unknown_protocol",
                chronicle_etl::EtlIssueKind::ProtocolDecode => "protocol_decode",
                chronicle_etl::EtlIssueKind::MalformedLossWindow => "malformed_loss_window",
                chronicle_etl::EtlIssueKind::MalformedTerminalWalLoss => {
                    "malformed_terminal_wal_loss"
                }
                chronicle_etl::EtlIssueKind::MalformedCommitMarker => "malformed_commit_marker",
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
        if output.session.connection_state(&connection.id) != Completeness::Complete {
            reasons.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if output.session.operation_state(&operation.id) != Completeness::Complete {
                reasons.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                reasons.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                if warning.code != "missing_timestamp" {
                    reasons.insert(format!("warning:{}", warning.code));
                }
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
        session.connection_state(&connection.id) == Completeness::Complete
            && connection.operations.iter().all(|operation| {
                session.operation_state(&operation.id) == Completeness::Complete
                    && operation.recorded_response.is_some()
            })
    })
}

/// Produces inspect-safe metadata without reading artifact body bytes.
#[allow(clippy::too_many_lines)]
pub fn inspect_session(
    root: impl AsRef<Path>,
    session_id: chronicle_common::SessionId,
) -> Result<InspectSessionResult, ApplicationError> {
    let stored = FilesystemSessionStore::new(root.as_ref()).inspect_with_metadata(session_id)?;
    let mut blockers: BTreeSet<_> = stored.replayability.into_iter().collect();
    for connection in &stored.session.connections {
        if stored.session.connection_state(&connection.id) != Completeness::Complete {
            blockers.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if stored.session.operation_state(&operation.id) != Completeness::Complete {
                blockers.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                blockers.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                if warning.code != "missing_timestamp" {
                    blockers.insert(format!("warning:{}", warning.code));
                }
            }
        }
    }
    let complete = stored.complete && canonical_session_complete(&stored.session);
    if !complete {
        blockers.insert("incomplete_capture".into());
    }
    let (executable_operations, non_executable_operations) = stored
        .session
        .connections
        .iter()
        .flat_map(|connection| &connection.operations)
        .fold((0, 0), |(executable, non_executable), operation| {
            if stored.session.operation_state(&operation.id) == Completeness::Complete
                && operation.recorded_response.is_some()
                && operation
                    .warnings
                    .iter()
                    .all(|warning| warning.code == "missing_timestamp")
            {
                (executable + 1, non_executable)
            } else {
                (executable, non_executable + 1)
            }
        });
    let replayability = match (executable_operations, non_executable_operations) {
        (0, _) => Replayability::NotReplayable,
        (_, 0) => Replayability::FullyReplayable,
        _ => Replayability::PartiallyReplayable,
    };
    if non_executable_operations > 0 {
        for entry in &stored.session.source_provenance.evidence {
            match entry.kind {
                ProvenanceKind::RingLoss => {
                    blockers.insert("ring_loss_window".into());
                }
                ProvenanceKind::WalLimitLoss => {
                    blockers.insert("wal_limit_loss_window".into());
                }
                _ => {}
            }
        }
    }
    let mut issue_codes = stored.issues;
    issue_codes.sort();
    let source_provenance = stored.session.source_provenance.clone();
    let connections = stored
        .session
        .connections
        .iter()
        .map(|connection| InspectConnectionSummary {
            protocol: connection.protocol.to_string(),
            client: connection.client.clone(),
            server: connection.server.clone(),
            completeness: stored.session.connection_state(&connection.id),
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
                    completeness: stored.session.operation_state(&operation.id),
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
        replayable: replayability == Replayability::FullyReplayable,
        replayability,
        executable_operations,
        non_executable_operations,
        integrity_valid: true,
        source_provenance,
        blockers: blockers.into_iter().collect(),
        issue_codes,
        connections,
    })
}

#[derive(Serialize)]
struct InspectJson<'a> {
    version: u8,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

/// Renders stable, body-free inspect output for interactive use.
pub fn render_inspect_human(inspected: &InspectSessionResult) -> String {
    let mut output = format!(
        "session_id: {}\ncomplete: {}\nreplayable: {}\nreplayability: {:?}\noperations: executable={} non_executable={}\nintegrity_valid: {}\nsource_status: {:?}\nsource_reason: {}\nblockers: {}\nissues: {}\n",
        inspected.session_id,
        inspected.complete,
        inspected.replayable,
        inspected.replayability,
        inspected.executable_operations,
        inspected.non_executable_operations,
        inspected.integrity_valid,
        inspected.source_provenance.status,
        inspected
            .source_provenance
            .reason
            .as_deref()
            .unwrap_or("none"),
        joined_or_none(&inspected.blockers),
        joined_or_none(&inspected.issue_codes),
    );
    for connection in &inspected.connections {
        writeln!(
            output,
            "connection: {} {}:{} -> {}:{} completeness={:?}",
            connection.protocol,
            connection.client.host,
            connection.client.port,
            connection.server.host,
            connection.server.port,
            connection.completeness,
        )
        .expect("writing into String cannot fail");
        for operation in &connection.operations {
            writeln!(
                output,
                "operation: sequence={} kind={} effect={} method={} target={} response_status={} request_bytes={} response_bytes={} completeness={:?} warnings={}",
                operation.sequence,
                operation.kind,
                operation.effect,
                operation.method.as_deref().unwrap_or("none"),
                operation.target.as_deref().unwrap_or("none"),
                operation.response_status.as_deref().unwrap_or("none"),
                optional_size(operation.request_bytes),
                optional_size(operation.response_bytes),
                operation.completeness,
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
    render_json(&InspectJson {
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReplayCounts {
    pub executable: usize,
    pub non_executable: usize,
    pub attempted: usize,
    pub completed: usize,
    pub failed: usize,
    pub unattempted: usize,
    pub verification_passed: usize,
    pub verification_failed: usize,
    pub verification_skipped: usize,
    pub verification_inconclusive: usize,
    pub verification_unsupported: usize,
    pub verification_not_run: usize,
}

impl ReplayCounts {
    fn from_operations(operations: &[ReplayOperationSummary]) -> Self {
        operations
            .iter()
            .fold(Self::default(), |mut counts, operation| {
                if operation.decision == "allowed" {
                    counts.executable += 1;
                } else {
                    counts.non_executable += 1;
                }
                counts.attempted += usize::from(operation.attempted);
                match operation.state {
                    OperationExecutionState::Completed => counts.completed += 1,
                    OperationExecutionState::Failed => counts.failed += 1,
                    OperationExecutionState::NotAttempted => counts.unattempted += 1,
                }
                match operation.verification.as_str() {
                    "passed" => counts.verification_passed += 1,
                    "failed" => counts.verification_failed += 1,
                    "skipped" => counts.verification_skipped += 1,
                    "inconclusive" => counts.verification_inconclusive += 1,
                    "unsupported" => counts.verification_unsupported += 1,
                    _ => counts.verification_not_run += 1,
                }
                counts
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplaySessionResult {
    pub session_id: String,
    pub replayability: Replayability,
    pub outcome: ReplayOutcome,
    pub dry_run: bool,
    pub preflight_denied: bool,
    pub transport_failed: bool,
    pub counts: ReplayCounts,
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
    let transport_failed = execution
        .results
        .iter()
        .any(|result| result.transport_error.is_some());
    let operations: Vec<_> = plan
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
        .collect();
    Ok(ReplaySessionResult {
        session_id: session.id.to_string(),
        replayability: plan.replayability(),
        outcome: execution.outcome,
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed,
        counts: ReplayCounts::from_operations(&operations),
        operations,
    })
}

fn replay_plan_result(session_id: String, plan: &ReplayPlan) -> ReplaySessionResult {
    let operations: Vec<_> = plan
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
        .collect();
    ReplaySessionResult {
        session_id,
        replayability: plan.replayability(),
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
        counts: ReplayCounts::from_operations(&operations),
        operations,
    }
}

pub fn render_replay_human(result: &ReplaySessionResult) -> String {
    let mut output = format!(
        "session_id: {}\nreplayability: {:?}\noutcome: {}\ndry_run: {}\npreflight_denied: {}\ntransport_failed: {}\ncounts: executable={} non_executable={} attempted={} completed={} failed={} unattempted={} verification_passed={} verification_failed={} verification_skipped={} verification_inconclusive={} verification_unsupported={} verification_not_run={}\n",
        result.session_id,
        result.replayability,
        result.outcome.as_str(),
        result.dry_run,
        result.preflight_denied,
        result.transport_failed,
        result.counts.executable,
        result.counts.non_executable,
        result.counts.attempted,
        result.counts.completed,
        result.counts.failed,
        result.counts.unattempted,
        result.counts.verification_passed,
        result.counts.verification_failed,
        result.counts.verification_skipped,
        result.counts.verification_inconclusive,
        result.counts.verification_unsupported,
        result.counts.verification_not_run,
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
        if !(chronicle_wal::MIN_SEGMENT_BYTES..=chronicle_wal::MAX_SEGMENT_BYTES)
            .contains(&self.config.wal.segment_size_bytes)
        {
            return Err(ApplicationError::InvalidConfig(
                "WAL segment size must be between 16 MiB and 4 GiB".into(),
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
    fn recording_scoped_etl_rejects_active_metadata_before_wal_access() {
        let directory = ingest_directory("active-recording-etl");
        write_recording_metadata(&directory, &recording_metadata(RecordingStatus::Starting))
            .unwrap();

        assert!(matches!(
            process_recording_wal(
                &directory,
                &chronicle_protocol_builtins::registry().unwrap(),
                chronicle_common::SessionId::new(),
            ),
            Err(ApplicationError::RecordingMetadataValidation(message))
                if message == "cannot process an active recording"
        ));
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
        std::fs::create_dir(&wal).unwrap();
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
