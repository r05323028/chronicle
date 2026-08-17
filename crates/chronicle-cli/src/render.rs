use super::{ApplicationError, CleanupOutcome, Format};
use chronicle_application::{
    InspectSessionResult, ListedRecording, ParentRecordingViewV2, ParentReplayEpochPlan,
    RecordingCatalogStatus, RecordingCounters, RecordingStatus, ReplaySessionResult,
    RunLifecycleState, ShutdownReason, Timestamp, render_inspect_human, render_replay_human,
};
use serde::Serialize;

#[cfg(any(test, target_os = "linux"))]
use chronicle_application::{ChildExitResult, CommandRecordResult};

#[cfg(target_os = "linux")]
use chronicle_application::{RecordingBounds, RecordingCommitBoundary, RecordingSelectorIdentity};
use std::fmt::Write as _;
use std::path::PathBuf;

pub(super) use chronicle_application::render_json;

#[derive(Serialize)]
pub(super) struct RecordJson {
    pub(super) version: u8,
    pub(super) session_id: String,
    pub(super) root: PathBuf,
}

#[derive(Serialize)]
pub(super) struct EtlJson<'a> {
    pub(super) version: u8,
    pub(super) session_id: String,
    pub(super) recording_id: String,
    pub(super) status: RecordingStatus,
    pub(super) shutdown_reason: Option<ShutdownReason>,
    pub(super) output: PathBuf,
    pub(super) already_processed: bool,
    pub(super) ignored_post_commit_records: u64,
    pub(super) counters: &'a RecordingCounters,
    pub(super) checkpoint: &'a chronicle_application::RecordingEtlCheckpoint,
}

#[cfg(target_os = "linux")]
#[derive(Serialize)]
pub(super) struct ProductionRecordJson<'a> {
    pub(super) version: u8,
    pub(super) recording_id: String,
    pub(super) status: RecordingStatus,
    pub(super) shutdown_reason: ShutdownReason,
    pub(super) last_valid_commit: Option<&'a RecordingCommitBoundary>,
    pub(super) physical_wal_bytes: u64,
    pub(super) selector: Option<&'a RecordingSelectorIdentity>,
    pub(super) direct_tgid_count: usize,
    pub(super) descendant_cgroup_count: usize,
    pub(super) selected_subtree: bool,
    pub(super) shared_scope_acknowledged: bool,
    pub(super) configured_bounds: &'a RecordingBounds,
    pub(super) effective_bounds: &'a RecordingBounds,
    pub(super) counters: &'a RecordingCounters,
}

#[derive(Serialize)]
pub(super) struct ReplayJson<'a> {
    pub(super) version: u16,
    pub(super) plan: &'a ReplaySessionResult,
    pub(super) result: &'a ReplaySessionResult,
}

#[derive(Serialize)]
pub(super) struct CommandReplayJson<'a> {
    pub(super) version: u16,
    pub(super) plan: &'a ReplaySessionResult,
    pub(super) result: &'a ReplaySessionResult,
    pub(super) cleanup: ReplayCleanupJson<'a>,
}

#[derive(Serialize)]
pub(super) struct ReplayCleanupJson<'a> {
    pub(super) status: &'static str,
    pub(super) possible_orphan_pids: &'a [u32],
}

pub(super) fn replay_cleanup_json(cleanup: &CleanupOutcome) -> ReplayCleanupJson<'_> {
    match cleanup {
        CleanupOutcome::Clean => ReplayCleanupJson {
            status: "clean",
            possible_orphan_pids: &[],
        },
        CleanupOutcome::Killed => ReplayCleanupJson {
            status: "killed",
            possible_orphan_pids: &[],
        },
        CleanupOutcome::TimedOut { remaining } => ReplayCleanupJson {
            status: "timed_out",
            possible_orphan_pids: remaining,
        },
    }
}

pub(super) fn replay_cleanup_human(cleanup: &CleanupOutcome) -> String {
    match cleanup {
        CleanupOutcome::Clean => "clean".into(),
        CleanupOutcome::Killed => "killed".into(),
        CleanupOutcome::TimedOut { remaining } => {
            format!("timed_out (possible orphan pids: {remaining:?})")
        }
    }
}

pub(super) fn render_public_replay_human(
    plan: &ReplaySessionResult,
    result: &ReplaySessionResult,
    cleanup: Option<&CleanupOutcome>,
) -> String {
    let succeeded = result.succeeded()
        && !cleanup.is_some_and(|value| matches!(value, CleanupOutcome::TimedOut { .. }));
    let mut output = format!(
        "plan:\n{}\nresult:\n{}",
        render_replay_human(plan),
        render_replay_human(result)
    );
    if let Some(cleanup) = cleanup {
        write!(output, "\ncleanup: {}", replay_cleanup_human(cleanup))
            .expect("writing into String cannot fail");
    }
    write!(
        output,
        "\noperations: attempted={} completed={} failed={} unattempted={} verification_failed={}\n{}\n{}",
        result.counts.attempted,
        result.counts.completed,
        result.counts.failed,
        result.counts.unattempted,
        result.counts.verification_failed,
        if succeeded { "✓ passed" } else { "✗ failed" },
        if succeeded { "Replay passed." } else { "Replay failed." },
    )
    .expect("writing into String cannot fail");
    output
}

#[derive(Serialize)]
pub(super) struct ErrorJson {
    pub(super) version: u8,
    pub(super) code: i32,
    pub(super) message: String,
}

pub(super) fn render_error(error: &ApplicationError, code: i32, format: Format) -> String {
    if matches!(format, Format::Json) {
        render_json(&ErrorJson {
            version: 1,
            code,
            message: error.to_string(),
        })
        .unwrap_or_else(|render_error| render_error.to_string())
    } else {
        error.to_string()
    }
}

/// Human/JSON public record completion summary v1. Raw argv never appears.
#[cfg(any(test, target_os = "linux"))]
pub(super) fn render_command_record(
    result: &CommandRecordResult,
    format: Format,
) -> Result<String, ApplicationError> {
    let id = result.recording_id.to_cli_string();
    let dropped = dropped_records(&result.counters);
    match format {
        Format::Human => {
            let heading = match result.status {
                RecordingStatus::Completed => "Recording complete.",
                RecordingStatus::Failed => "Recording failed.",
                RecordingStatus::Aborted => "Recording aborted.",
                _ => "Recording interrupted.",
            };
            let mut lines = vec![heading.to_owned()];
            lines.push(format!("  id: {id}"));
            lines.push(format!(
                "  duration: {}",
                format_duration_millis(result.duration_ms)
            ));
            lines.push(format!("  operations: {}", result.operations));
            lines.push(format!("  dropped: {dropped}"));
            if let Some(child_exit) = &result.child_exit {
                lines.push(match child_exit {
                    ChildExitResult::ExitCode { code } => format!("  child: exit_code {code}"),
                    ChildExitResult::Signal { signal } => format!("  child: signal {signal}"),
                });
            }
            if matches!(result.cleanup, CleanupOutcome::TimedOut { .. }) {
                lines.push(
                    "  warning: supervised scope still populated; orphan processes may remain"
                        .to_owned(),
                );
            }
            lines.push("Try:".to_owned());
            lines.push(format!("  chronicle inspect {id}"));
            lines.push(format!("  chronicle replay {id} -- COMMAND..."));
            Ok(lines.join("\n"))
        }
        Format::Json => {
            #[derive(Serialize)]
            struct RecordResultJson<'a> {
                version: u8,
                recording_id: String,
                name: Option<&'a str>,
                status: &'static str,
                shutdown_reason: Option<&'static str>,
                duration_ms: u64,
                operations: u64,
                dropped: u64,
                counters: &'a RecordingCounters,
                child_exit: Option<&'a ChildExitResult>,
            }
            render_json(&RecordResultJson {
                version: 1,
                recording_id: id,
                name: result.name.as_deref(),
                status: recording_status_json(result.status),
                shutdown_reason: result.shutdown_reason.map(shutdown_reason_json),
                duration_ms: result.duration_ms,
                operations: result.operations,
                dropped,
                counters: &result.counters,
                child_exit: result.child_exit.as_ref(),
            })
        }
    }
}

#[cfg(any(test, target_os = "linux"))]
fn dropped_records(counters: &RecordingCounters) -> u64 {
    counters
        .discarded_from_queue_due_to_wal_limit
        .records
        .saturating_add(counters.kernel_or_backend_dropped.records)
        .saturating_add(counters.rejected_after_stop.records)
        .saturating_add(counters.rejected_due_to_quota.records)
}

#[cfg(any(test, target_os = "linux"))]
const fn recording_status_json(status: RecordingStatus) -> &'static str {
    match status {
        RecordingStatus::Completed => "completed",
        RecordingStatus::Failed => "failed",
        RecordingStatus::Aborted => "aborted",
        _ => "in_progress",
    }
}

#[cfg(any(test, target_os = "linux"))]
const fn shutdown_reason_json(reason: ShutdownReason) -> &'static str {
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

#[derive(Serialize)]
pub(super) struct ListRecordingJson {
    pub(super) recording_id: String,
    pub(super) name: Option<String>,
    pub(super) created_at: String,
    pub(super) duration: Option<u64>,
    pub(super) sessions: usize,
    pub(super) operations: usize,
    pub(super) epoch_count: usize,
    pub(super) active_epoch: Option<u64>,
    pub(super) published_epoch_count: usize,
    pub(super) status: RunLifecycleState,
    pub(super) warnings: Vec<String>,
}

#[derive(Serialize)]
pub(super) struct ListJson {
    pub(super) version: u8,
    pub(super) recordings: Vec<ListRecordingJson>,
}

#[derive(Serialize)]
pub(super) struct InspectRecordingJson<'a> {
    pub(super) version: u8,
    pub(super) recording_id: String,
    pub(super) name: Option<&'a str>,
    pub(super) created_at: String,
    pub(super) duration_ms: Option<u64>,
    pub(super) sessions: usize,
    pub(super) operations: usize,
    pub(super) status: RecordingCatalogStatus,
    pub(super) epoch_count: usize,
    pub(super) active_epoch: Option<u64>,
    pub(super) published_epoch_count: usize,
    pub(super) warnings: &'a [String],
    pub(super) epochs: Vec<ParentReplayEpochPlan>,
    #[serde(flatten)]
    pub(super) result: &'a InspectSessionResult,
}

pub(super) fn render_recording_inspect_human(
    recording: &ListedRecording,
    parent: &ParentRecordingViewV2,
    epochs: &[ParentReplayEpochPlan],
    inspected: &InspectSessionResult,
) -> String {
    let name = recording
        .name
        .as_deref()
        .map_or_else(|| "none".to_owned(), chronicle_application::escape_control);
    let duration = recording
        .duration_ms
        .map_or_else(|| "unknown".to_owned(), format_duration_millis);
    let epoch_lines = epochs
        .iter()
        .map(|epoch| {
            format!(
                "epoch: ordinal={} id={} session={} selected={} reason={}",
                epoch.ordinal,
                epoch.epoch_id.to_cli_string(),
                epoch
                    .session_id
                    .map_or_else(|| "none".to_owned(), |id| id.to_string()),
                epoch.selected,
                epoch.reason.as_deref().unwrap_or("none"),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "recording_id: {}\nname: {}\ncreated_at: {}\nduration: {}\nsessions: {}\noperations: {}\nstatus: {}\nepoch_count: {}\nactive_epoch: {:?}\npublished_epoch_count: {}\nparent_state: {}\nwarnings: {}\nepochs:\n{}\n{}",
        recording.recording_id.to_cli_string(),
        name,
        format_created_at(&recording.created_at),
        duration,
        recording.sessions,
        recording.operations,
        recording_status_human(recording.status),
        parent.epoch_count,
        parent.active_epoch,
        parent.published_epoch_count,
        parent_status_human(parent.state),
        parent.warnings.len(),
        epoch_lines,
        render_inspect_human(inspected),
    )
}

pub(super) fn recording_status_human(status: RecordingCatalogStatus) -> &'static str {
    match status {
        RecordingCatalogStatus::InProgress => "in_progress",
        RecordingCatalogStatus::Recoverable => "recoverable",
        RecordingCatalogStatus::Published => "published",
        RecordingCatalogStatus::Failed => "failed",
        RecordingCatalogStatus::Inconsistent => "inconsistent",
    }
}

pub(super) fn parent_status_human(status: RunLifecycleState) -> &'static str {
    match status {
        RunLifecycleState::Starting => "starting",
        RunLifecycleState::Running => "running",
        RunLifecycleState::Draining => "draining",
        RunLifecycleState::Completed => "completed",
        RunLifecycleState::Stopped => "stopped",
        RunLifecycleState::Failed => "failed",
        RunLifecycleState::Inconsistent => "inconsistent",
    }
}

pub(super) fn format_duration_millis(millis: u64) -> String {
    let seconds = millis.div_ceil(1000);
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes == 0 {
        format!("{remainder}s")
    } else {
        format!("{minutes}m {remainder}s")
    }
}

pub(super) fn format_created_at(timestamp: &Timestamp) -> String {
    timestamp.to_string()
}
