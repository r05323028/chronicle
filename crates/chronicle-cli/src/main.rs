use chronicle_application::escape_control;
#[cfg(target_os = "linux")]
use chronicle_application::record_selector;
use chronicle_application::{
    AppConfig, ApplicationError, ChildStdio, CleanupOutcome, CommandReplayOptions, DoctorProbe,
    DoctorReport, DoctorStatus, InspectSessionResult, ListedRecording, REPLAY_REPORT_VERSION,
    RecorderConfigV1, RecorderLease, RecorderStatusV1, RecordingCounters, RecordingEtlCheckpoint,
    RecordingStatus, ShutdownReason, build_parent_replay_plan, data_dir_doctor_probe,
    inspect_session, list_parent_recording_views, list_recordings, load_recorder_metadata,
    process_and_publish_recording_wal, process_and_publish_recording_wal_with_parent,
    record_fixture_file, render_inspect_human, render_json, render_replay_human, replay_command,
    replay_config_doctor_probe, replay_parent_session_with_plan, replay_session_with_plan,
    resolve_data_dir, resolve_recording, resolve_session, retry_recording,
};
#[cfg(target_os = "linux")]
use chronicle_application::{
    CaptureMode, CgroupSelector, CommandRecordOptions, ProductionSignalStop, RecordingLifetime,
    RunStopPolicy, load_recording_metadata, mark_recording_forced_termination, record_command,
    record_continuous_ebpf, recording_physical_wal_bytes,
};
#[cfg(any(test, target_os = "linux"))]
use chronicle_application::{ChildExitResult, CommandRecordResult};
use chronicle_application::{ReplayRequest, ReplayTiming};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

mod args;
mod dispatch;
mod signals;

use args::{
    Cli, Command, DoctorArgs, Format, InspectArgs, InternalCommand, RecordArgs, ReplayArgs, Timing,
};
#[cfg(test)]
use dispatch::render_command_record;
use dispatch::{error_code, run, validate_record_arguments, write_output};
#[derive(Serialize)]
struct RecordJson {
    version: u8,
    session_id: String,
    root: PathBuf,
}

#[derive(Serialize)]
struct EtlJson<'a> {
    version: u8,
    session_id: String,
    recording_id: String,
    status: RecordingStatus,
    shutdown_reason: Option<ShutdownReason>,
    output: PathBuf,
    already_processed: bool,
    ignored_post_commit_records: u64,
    counters: &'a RecordingCounters,
    checkpoint: &'a RecordingEtlCheckpoint,
}

#[cfg(target_os = "linux")]
#[derive(Serialize)]
struct ProductionRecordJson<'a> {
    version: u8,
    recording_id: String,
    status: chronicle_application::RecordingStatus,
    shutdown_reason: chronicle_application::ShutdownReason,
    last_valid_commit: Option<&'a chronicle_application::RecordingCommitBoundary>,
    physical_wal_bytes: u64,
    selector: Option<&'a chronicle_application::RecordingSelectorIdentity>,
    direct_tgid_count: usize,
    descendant_cgroup_count: usize,
    selected_subtree: bool,
    shared_scope_acknowledged: bool,
    configured_bounds: &'a chronicle_application::RecordingBounds,
    effective_bounds: &'a chronicle_application::RecordingBounds,
    counters: &'a chronicle_application::RecordingCounters,
}

#[derive(Serialize)]
struct ReplayJson<'a> {
    version: u16,
    plan: &'a chronicle_application::ReplaySessionResult,
    result: &'a chronicle_application::ReplaySessionResult,
}

#[derive(Serialize)]
struct CommandReplayJson<'a> {
    version: u16,
    plan: &'a chronicle_application::ReplaySessionResult,
    result: &'a chronicle_application::ReplaySessionResult,
    cleanup: ReplayCleanupJson<'a>,
}

#[derive(Serialize)]
struct ReplayCleanupJson<'a> {
    status: &'static str,
    possible_orphan_pids: &'a [u32],
}

fn replay_cleanup_json(cleanup: &CleanupOutcome) -> ReplayCleanupJson<'_> {
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

fn replay_cleanup_human(cleanup: &CleanupOutcome) -> String {
    match cleanup {
        CleanupOutcome::Clean => "clean".into(),
        CleanupOutcome::Killed => "killed".into(),
        CleanupOutcome::TimedOut { remaining } => {
            format!("timed_out (possible orphan pids: {remaining:?})")
        }
    }
}

fn render_public_replay_human(
    plan: &chronicle_application::ReplaySessionResult,
    result: &chronicle_application::ReplaySessionResult,
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
        if succeeded {
            "✓ passed"
        } else {
            "✗ failed"
        },
        if succeeded {
            "Replay passed."
        } else {
            "Replay failed."
        }
    )
    .expect("writing into String cannot fail");
    output
}

#[derive(Serialize)]
struct ErrorJson {
    version: u8,
    code: i32,
    message: String,
}

/// Entry point. The hidden bootstrap runs synchronously before the tokio
/// runtime is initialized so that no runtime descriptor can collide with the
/// readiness pipe on fixed fd 3.
fn main() {
    if is_bootstrap_invocation() {
        let code = run_bootstrap_from_args();
        std::process::exit(code);
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async_main());
}

fn is_bootstrap_invocation() -> bool {
    std::env::args().nth(1).as_deref() == Some("internal")
        && std::env::args().nth(2).as_deref() == Some("bootstrap")
}

fn run_bootstrap_from_args() -> i32 {
    let args: Vec<String> = std::env::args().skip(3).collect();
    let mut uid = None;
    let mut gid = None;
    let mut command = Vec::new();
    let mut after_dashdash = false;
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if !after_dashdash && argument == "--" {
            after_dashdash = true;
        } else if !after_dashdash && argument == "--uid" {
            uid = args
                .get(index + 1)
                .and_then(|value| value.parse::<u32>().ok());
            index += 1;
        } else if !after_dashdash && argument == "--gid" {
            gid = args
                .get(index + 1)
                .and_then(|value| value.parse::<u32>().ok());
            index += 1;
        } else {
            command.push(argument.clone());
        }
        index += 1;
    }
    let (Some(uid), Some(gid)) = (uid, gid) else {
        eprintln!("chronicle bootstrap: --uid and --gid are required");
        return chronicle_application::BOOTSTRAP_FAILURE_EXIT;
    };
    chronicle_application::run_bootstrap(uid, gid, &command)
}

async fn async_main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
    let cli = match Cli::try_parse_from(std::env::args_os()) {
        Ok(cli) => cli,
        Err(error) => error.exit(),
    };
    if let Err(error) = validate_record_arguments(&cli) {
        error.exit();
    }
    let format = cli.format;
    match run(cli).await {
        Ok((output, code)) => {
            if let Err(error) = write_output(io::stdout().lock(), &output) {
                let _ = write_output(
                    io::stderr().lock(),
                    &format!("CLI output I/O failed: {error}"),
                );
                std::process::exit(3);
            }
            std::process::exit(code);
        }
        Err(error) => {
            let code = error_code(&error);
            let output = if matches!(format, Format::Json) {
                render_json(&ErrorJson {
                    version: 1,
                    code,
                    message: error.to_string(),
                })
                .unwrap_or_else(|render_error| render_error.to_string())
            } else {
                error.to_string()
            };
            let _ = write_output(io::stderr().lock(), &output);
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests;
