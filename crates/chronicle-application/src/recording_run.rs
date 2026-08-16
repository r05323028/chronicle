use crate::{EpochCatalogState, EpochCatalogSummaryV2, EpochCatalogV2, EpochId, RecordingId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};
use thiserror::Error;

pub const RECORDING_RUN_FILE: &str = "recording-run.json";
pub const RECORDING_RUN_SCHEMA_VERSION: u16 = 2;
const MAX_RUN_EPOCHS: usize = 4096;
const MAX_RUN_WARNINGS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycleState {
    Starting,
    Running,
    Draining,
    Completed,
    Stopped,
    Failed,
    Inconsistent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStopPolicy {
    SourceCompletion,
    ExplicitStop,
    Deadline,
    AnyTerminalCondition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStopReason {
    SourceCompleted,
    UserRequested,
    Deadline,
    FatalFailure,
    RestartRecovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecordingLifetime {
    pub deadline: Option<Duration>,
    pub stop_policy: RunStopPolicy,
}

impl Default for RecordingLifetime {
    fn default() -> Self {
        Self {
            deadline: None,
            stop_policy: RunStopPolicy::AnyTerminalCondition,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EpochBounds {
    pub max_age: Duration,
    pub max_bytes: u64,
}

impl Default for EpochBounds {
    fn default() -> Self {
        Self {
            max_age: Duration::from_hours(1),
            max_bytes: chronicle_wal::DEFAULT_MAX_WAL_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SegmentBounds {
    pub max_age: Duration,
    pub max_bytes: u64,
}

impl Default for SegmentBounds {
    fn default() -> Self {
        Self {
            max_age: Duration::from_mins(5),
            max_bytes: chronicle_wal::DEFAULT_SEGMENT_BYTES,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecordingDurationError {
    #[error("duration is empty")]
    Empty,
    #[error("duration must be positive")]
    Zero,
    #[error("duration number is invalid")]
    InvalidNumber,
    #[error("duration unit is unsupported")]
    UnsupportedUnit,
    #[error("duration overflows runtime clock")]
    Overflow,
}

/// Parse checked positive durations (`ms`, `s`, `m`, `h`, `d`) or bare seconds.
/// No public recording-wide maximum is imposed here.
pub fn parse_recording_duration(input: &str) -> Result<Duration, RecordingDurationError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(RecordingDurationError::Empty);
    }
    let split = input
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(input.len());
    let (number, unit) = input.split_at(split);
    if number.is_empty() {
        return Err(RecordingDurationError::InvalidNumber);
    }
    let value = number
        .parse::<u64>()
        .map_err(|_| RecordingDurationError::InvalidNumber)?;
    let multiplier = match unit {
        "" | "s" => 1_000_u64,
        "ms" => 1,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        _ => return Err(RecordingDurationError::UnsupportedUnit),
    };
    let millis = value
        .checked_mul(multiplier)
        .ok_or(RecordingDurationError::Overflow)?;
    if millis == 0 {
        return Err(RecordingDurationError::Zero);
    }
    Ok(Duration::from_millis(millis))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunEpochSummaryV2 {
    pub ordinal: u64,
    pub epoch_id: EpochId,
    pub predecessor: Option<EpochId>,
    pub successor: Option<EpochId>,
    pub path: String,
    pub state: EpochCatalogState,
    pub summary: EpochCatalogSummaryV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingRunV2 {
    pub version: u16,
    pub parent_id: RecordingId,
    pub name: Option<String>,
    pub created_at_unix_millis: u64,
    pub deadline_unix_millis: Option<u64>,
    pub stop_policy: RunStopPolicy,
    pub state: RunLifecycleState,
    pub stop_reason: Option<RunStopReason>,
    pub epochs: Vec<RunEpochSummaryV2>,
    pub warnings: Vec<String>,
    pub checksum: String,
}

#[derive(Debug, Error)]
pub enum RunPersistenceError {
    #[error("run persistence I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("run persistence encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("run persistence validation failed: {0}")]
    Invalid(&'static str),
    #[error("run persistence checksum mismatch")]
    ChecksumMismatch,
    #[error("run parent identity does not match epoch catalog")]
    ParentIdentityMismatch,
    #[error("run deadline overflows runtime clock")]
    DeadlineOverflow,
}

impl RecordingRunV2 {
    pub fn from_catalog(
        catalog: &EpochCatalogV2,
        name: Option<String>,
        created_at_unix_millis: u64,
        lifetime: RecordingLifetime,
        state: RunLifecycleState,
        stop_reason: Option<RunStopReason>,
    ) -> Result<Self, RunPersistenceError> {
        catalog
            .validate()
            .map_err(|_| RunPersistenceError::Invalid("epoch catalog is invalid"))?;
        let deadline_unix_millis = lifetime
            .deadline
            .map(|duration| {
                u64::try_from(duration.as_millis())
                    .ok()
                    .and_then(|millis| created_at_unix_millis.checked_add(millis))
                    .ok_or(RunPersistenceError::DeadlineOverflow)
            })
            .transpose()?;
        let run = Self {
            version: RECORDING_RUN_SCHEMA_VERSION,
            parent_id: catalog.parent_id,
            name,
            created_at_unix_millis,
            deadline_unix_millis,
            stop_policy: lifetime.stop_policy,
            state,
            stop_reason,
            epochs: catalog
                .epochs
                .iter()
                .map(|entry| RunEpochSummaryV2 {
                    ordinal: entry.ordinal,
                    epoch_id: entry.epoch_id,
                    predecessor: entry.predecessor,
                    successor: entry.successor,
                    path: entry.path.clone(),
                    state: entry.state,
                    summary: entry.summary.clone(),
                })
                .collect(),
            warnings: Vec::new(),
            checksum: String::new(),
        };
        run.with_checksum()
    }

    pub fn with_warning(mut self, warning: impl Into<String>) -> Result<Self, RunPersistenceError> {
        let warning = warning.into();
        if warning.is_empty() || warning.len() > 256 || self.warnings.len() >= MAX_RUN_WARNINGS {
            return Err(RunPersistenceError::Invalid("run warning bound exceeded"));
        }
        self.warnings.push(warning);
        self.with_checksum()
    }

    pub fn regenerate_from_catalog(
        root: impl AsRef<Path>,
        catalog: &EpochCatalogV2,
        name: Option<String>,
        created_at_unix_millis: u64,
        lifetime: RecordingLifetime,
        state: RunLifecycleState,
        stop_reason: Option<RunStopReason>,
    ) -> Result<Self, RunPersistenceError> {
        let run = Self::from_catalog(
            catalog,
            name,
            created_at_unix_millis,
            lifetime,
            state,
            stop_reason,
        )?;
        run.write_atomic(root)?;
        Ok(run)
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self, RunPersistenceError> {
        let value: Self =
            serde_json::from_slice(&fs::read(root.as_ref().join(RECORDING_RUN_FILE))?)?;
        value.validate()?;
        Ok(value)
    }

    pub fn write_atomic(&self, root: impl AsRef<Path>) -> Result<(), RunPersistenceError> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let bytes = self.encode()?;
        let temporary = root.join(format!(
            ".{RECORDING_RUN_FILE}.tmp-{}",
            uuid::Uuid::new_v4()
        ));
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, root.join(RECORDING_RUN_FILE))?;
            fs::File::open(root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn encode(&self) -> Result<Vec<u8>, RunPersistenceError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum = value.compute_checksum()?;
        let mut bytes = serde_json::to_vec_pretty(&value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn validate(&self) -> Result<(), RunPersistenceError> {
        self.validate_without_checksum()?;
        if self.checksum != self.compute_checksum()? {
            return Err(RunPersistenceError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn active_epoch(&self) -> Option<&RunEpochSummaryV2> {
        self.epochs
            .iter()
            .find(|epoch| epoch.state == EpochCatalogState::Active)
    }

    fn with_checksum(mut self) -> Result<Self, RunPersistenceError> {
        self.validate_without_checksum()?;
        self.checksum = self.compute_checksum()?;
        Ok(self)
    }

    fn validate_without_checksum(&self) -> Result<(), RunPersistenceError> {
        if self.version != RECORDING_RUN_SCHEMA_VERSION
            || self.parent_id.as_uuid().is_nil()
            || self.epochs.is_empty()
            || self.epochs.len() > MAX_RUN_EPOCHS
            || self.warnings.len() > MAX_RUN_WARNINGS
            || self
                .warnings
                .iter()
                .any(|warning| warning.is_empty() || warning.len() > 256)
        {
            return Err(RunPersistenceError::Invalid("run fields"));
        }
        if self.epochs[0].ordinal != 0 || self.epochs[0].predecessor.is_some() {
            return Err(RunPersistenceError::Invalid("run epoch lineage"));
        }
        for (index, epoch) in self.epochs.iter().enumerate() {
            if epoch.epoch_id.as_uuid().is_nil()
                || epoch.path.is_empty()
                || std::path::Path::new(&epoch.path).is_absolute()
                || std::path::Path::new(&epoch.path)
                    .components()
                    .any(|component| component == std::path::Component::ParentDir)
                || self.epochs[..index].iter().any(|previous| {
                    previous.epoch_id == epoch.epoch_id || previous.path == epoch.path
                })
                || (epoch.state == EpochCatalogState::Prepared && index + 1 != self.epochs.len())
            {
                return Err(RunPersistenceError::Invalid("run epoch lineage"));
            }
        }
        for pair in self.epochs.windows(2) {
            if pair[1].ordinal != pair[0].ordinal.saturating_add(1)
                || pair[1].predecessor != Some(pair[0].epoch_id)
                || pair[0].successor != Some(pair[1].epoch_id)
            {
                return Err(RunPersistenceError::Invalid("run epoch lineage"));
            }
        }
        if self
            .epochs
            .last()
            .is_some_and(|epoch| epoch.successor.is_some())
        {
            return Err(RunPersistenceError::Invalid("run epoch lineage"));
        }
        if self
            .epochs
            .iter()
            .filter(|epoch| epoch.state == EpochCatalogState::Active)
            .count()
            != 1
        {
            return Err(RunPersistenceError::Invalid("run active epoch count"));
        }
        Ok(())
    }

    fn compute_checksum(&self) -> Result<String, RunPersistenceError> {
        use std::fmt::Write as _;

        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        let digest = Sha256::digest(serde_json::to_vec(&unsigned)?);
        let mut checksum = String::with_capacity(64);
        for byte in digest {
            write!(&mut checksum, "{byte:02x}")
                .map_err(|_| RunPersistenceError::Invalid("checksum formatting failed"))?;
        }
        Ok(checksum)
    }
}

#[derive(Clone, Debug)]
pub struct RunStopController {
    lifetime: RecordingLifetime,
    started: Instant,
    requested: Option<RunStopReason>,
}

impl RunStopController {
    pub fn new(lifetime: RecordingLifetime, started: Instant) -> Self {
        Self {
            lifetime,
            started,
            requested: None,
        }
    }

    pub fn request_stop(&mut self) {
        self.requested = Some(RunStopReason::UserRequested);
    }

    pub fn poll(
        &mut self,
        now: Instant,
        source_completed: bool,
        fatal_failure: bool,
    ) -> Option<RunStopReason> {
        if fatal_failure {
            return Some(RunStopReason::FatalFailure);
        }
        if let Some(reason) = self.requested {
            return Some(reason);
        }
        if source_completed
            && matches!(
                self.lifetime.stop_policy,
                RunStopPolicy::SourceCompletion | RunStopPolicy::AnyTerminalCondition
            )
        {
            return Some(RunStopReason::SourceCompleted);
        }
        if let Some(deadline) = self.lifetime.deadline
            && now.saturating_duration_since(self.started) >= deadline
            && matches!(
                self.lifetime.stop_policy,
                RunStopPolicy::Deadline | RunStopPolicy::AnyTerminalCondition
            )
        {
            return Some(RunStopReason::Deadline);
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Command,
    Pid,
    Cgroup,
    Daemon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetOwnership {
    SupervisedChild,
    AttachedTarget,
    Service,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoordinatorAction {
    Continue,
    Drain(RunStopReason),
}

#[derive(Clone, Debug)]
pub struct ContinuousRecordingCoordinator {
    mode: CaptureMode,
    stop_controller: RunStopController,
}

impl ContinuousRecordingCoordinator {
    pub fn new(mode: CaptureMode, lifetime: RecordingLifetime, started: Instant) -> Self {
        Self {
            mode,
            stop_controller: RunStopController::new(lifetime, started),
        }
    }

    pub const fn mode(&self) -> CaptureMode {
        self.mode
    }

    pub const fn target_ownership(&self) -> TargetOwnership {
        match self.mode {
            CaptureMode::Command => TargetOwnership::SupervisedChild,
            CaptureMode::Pid | CaptureMode::Cgroup => TargetOwnership::AttachedTarget,
            CaptureMode::Daemon => TargetOwnership::Service,
        }
    }

    pub const fn attached_target_survives_stop(&self) -> bool {
        matches!(self.target_ownership(), TargetOwnership::AttachedTarget)
    }

    pub fn request_stop(&mut self) {
        self.stop_controller.request_stop();
    }

    pub fn poll(
        &mut self,
        now: Instant,
        source_completed: bool,
        fatal_failure: bool,
    ) -> CoordinatorAction {
        let source_completed =
            source_completed && !matches!(self.target_ownership(), TargetOwnership::AttachedTarget);
        self.stop_controller
            .poll(now, source_completed, fatal_failure)
            .map_or(CoordinatorAction::Continue, CoordinatorAction::Drain)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentRecordingViewV2 {
    pub parent_id: RecordingId,
    pub name: Option<String>,
    pub created_at_unix_millis: u64,
    pub state: RunLifecycleState,
    pub stop_reason: Option<RunStopReason>,
    pub epoch_count: usize,
    pub active_epoch: Option<u64>,
    pub published_epoch_count: usize,
    pub sessions: usize,
    pub operations: usize,
    pub duration_ms: Option<u64>,
    pub warnings: Vec<String>,
}

impl From<&RecordingRunV2> for ParentRecordingViewV2 {
    fn from(run: &RecordingRunV2) -> Self {
        Self {
            parent_id: run.parent_id,
            name: run.name.clone(),
            created_at_unix_millis: run.created_at_unix_millis,
            state: run.state,
            stop_reason: run.stop_reason,
            epoch_count: run.epochs.len(),
            active_epoch: run.active_epoch().map(|epoch| epoch.ordinal),
            published_epoch_count: run
                .epochs
                .iter()
                .filter(|epoch| epoch.summary.published)
                .count(),
            sessions: run
                .epochs
                .iter()
                .filter(|epoch| epoch.summary.published)
                .count(),
            operations: 0,
            duration_ms: None,
            warnings: run.warnings.clone(),
        }
    }
}

pub fn sort_parent_recording_views(views: &mut [ParentRecordingViewV2]) {
    views.sort_by(|left, right| {
        left.created_at_unix_millis
            .cmp(&right.created_at_unix_millis)
            .then_with(|| left.parent_id.cmp(&right.parent_id))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_parser_has_no_arbitrary_public_cap() {
        assert_eq!(
            parse_recording_duration("10m").unwrap(),
            Duration::from_mins(10)
        );
        assert_eq!(
            parse_recording_duration("24h").unwrap(),
            Duration::from_hours(24)
        );
        assert_eq!(
            parse_recording_duration("1500ms").unwrap(),
            Duration::from_millis(1500)
        );
        assert!(parse_recording_duration("0s").is_err());
        assert!(parse_recording_duration("1w").is_err());
        assert!(parse_recording_duration("18446744073709551615d").is_err());
    }

    #[test]
    fn stop_controller_uses_whole_run_deadline_without_rollover_reset() {
        let started = Instant::now();
        let mut controller = RunStopController::new(
            RecordingLifetime {
                deadline: Some(Duration::from_secs(5)),
                stop_policy: RunStopPolicy::AnyTerminalCondition,
            },
            started,
        );
        assert_eq!(controller.poll(started, false, false), None);
        assert_eq!(
            controller.poll(started + Duration::from_secs(5), false, false),
            Some(RunStopReason::Deadline)
        );
    }

    #[test]
    fn run_summary_regenerates_only_from_catalog_lineage() {
        let parent = RecordingId::new();
        let first = EpochId::new();
        let second = EpochId::new();
        let mut catalog = EpochCatalogV2::new(parent, first, ".").unwrap();
        catalog.append_successor(second, "epochs/1").unwrap();
        let run = RecordingRunV2::from_catalog(
            &catalog,
            Some("checkout".into()),
            10,
            RecordingLifetime::default(),
            RunLifecycleState::Running,
            None,
        )
        .unwrap();
        assert_eq!(run.parent_id, parent);
        assert_eq!(run.epochs.len(), 2);
        assert_eq!(run.active_epoch().unwrap().epoch_id, first);
        assert!(run.validate().is_ok());
    }

    #[test]
    fn run_persistence_round_trip_rejects_tampering() {
        let root =
            std::env::temp_dir().join(format!("chronicle-recording-run-{}", uuid::Uuid::new_v4()));
        let parent = RecordingId::new();
        let epoch = EpochId::new();
        let catalog = EpochCatalogV2::new(parent, epoch, ".").unwrap();
        let run = RecordingRunV2::from_catalog(
            &catalog,
            Some("restartable".into()),
            100,
            RecordingLifetime {
                deadline: Some(Duration::from_mins(1)),
                stop_policy: RunStopPolicy::Deadline,
            },
            RunLifecycleState::Running,
            None,
        )
        .unwrap();
        run.write_atomic(&root).unwrap();
        assert_eq!(RecordingRunV2::load(&root).unwrap(), run);
        let path = root.join(RECORDING_RUN_FILE);
        let mut bytes = fs::read(&path).unwrap();
        let old = b"restartable";
        let offset = bytes
            .windows(old.len())
            .position(|window| window == old)
            .unwrap();
        bytes[offset..offset + old.len()].copy_from_slice(b"tampered---");
        fs::write(path, bytes).unwrap();
        assert!(matches!(
            RecordingRunV2::load(&root),
            Err(RunPersistenceError::ChecksumMismatch)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn coordinator_unifies_mode_stop_and_ownership_semantics() {
        let started = Instant::now();
        let mut command = ContinuousRecordingCoordinator::new(
            CaptureMode::Command,
            RecordingLifetime::default(),
            started,
        );
        assert_eq!(command.target_ownership(), TargetOwnership::SupervisedChild);
        assert_eq!(
            command.poll(started, true, false),
            CoordinatorAction::Drain(RunStopReason::SourceCompleted)
        );

        let mut attached = ContinuousRecordingCoordinator::new(
            CaptureMode::Pid,
            RecordingLifetime::default(),
            started,
        );
        assert!(attached.attached_target_survives_stop());
        assert_eq!(
            attached.poll(started, true, false),
            CoordinatorAction::Continue
        );
        attached.request_stop();
        assert_eq!(
            attached.poll(started, false, false),
            CoordinatorAction::Drain(RunStopReason::UserRequested)
        );

        let mut daemon = ContinuousRecordingCoordinator::new(
            CaptureMode::Daemon,
            RecordingLifetime::default(),
            started,
        );
        assert_eq!(
            daemon.poll(started, false, true),
            CoordinatorAction::Drain(RunStopReason::FatalFailure)
        );
    }

    #[test]
    fn parent_views_sort_by_creation_then_stable_parent_id() {
        let parent_a = RecordingId::from_uuid(uuid::Uuid::from_u128(1));
        let parent_b = RecordingId::from_uuid(uuid::Uuid::from_u128(2));
        let view_a = ParentRecordingViewV2 {
            parent_id: parent_a,
            name: None,
            created_at_unix_millis: 20,
            state: RunLifecycleState::Running,
            stop_reason: None,
            epoch_count: 1,
            active_epoch: Some(0),
            published_epoch_count: 0,
            sessions: 0,
            operations: 0,
            duration_ms: None,
            warnings: Vec::new(),
        };
        let view_b = ParentRecordingViewV2 {
            created_at_unix_millis: 10,
            parent_id: parent_b,
            ..view_a.clone()
        };
        let mut views = vec![view_a, view_b];
        sort_parent_recording_views(&mut views);
        assert_eq!(views[0].parent_id, parent_b);
        assert_eq!(views[1].parent_id, parent_a);
    }
}
