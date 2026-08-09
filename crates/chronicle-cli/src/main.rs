#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use chronicle_application::record_selector;
use chronicle_application::{
    AppConfig, ApplicationError, ChildStdio, CleanupOutcome, CommandReplayOptions, DoctorProbe,
    DoctorReport, DoctorStatus, InspectSessionResult, ListedRecording, REPLAY_REPORT_VERSION,
    RecorderConfigV1, RecorderLease, RecorderStatusV1, RecordingCounters, RecordingEtlCheckpoint,
    RecordingStatus, ShutdownReason, data_dir_doctor_probe, inspect_session, list_recordings,
    load_recorder_metadata, process_and_publish_recording_wal, record_fixture_file,
    render_inspect_human, render_inspect_json, render_json, render_replay_human, replay_command,
    replay_config_doctor_probe, replay_session_with_plan, resolve_data_dir, resolve_recording,
    resolve_session, retry_recording,
};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use chronicle_application::{
    CgroupSelector, ChildExitResult, CommandRecordOptions, CommandRecordResult,
    ProductionRecordingBounds, ProductionSignalStop, load_recording_metadata,
    mark_recording_forced_termination, record_command, record_continuous_ebpf, record_live_ebpf,
    recording_physical_wal_bytes,
};
use chronicle_common::escape_control;
use chronicle_protocol::{ProtocolError, TransportErrorCategory};
use chronicle_replay::{LoopbackReplayOptions, ReplayError, ReplayOutcome, TimingMode};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "chronicle",
    about = "Record, inspect, and safely replay local HTTP fixtures"
)]
struct Cli {
    /// TOML configuration file. Secrets must be referenced through environment variables.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Public data directory (recordings, catalog, sessions). Overrides
    /// configured `data_dir` and `CHRONICLE_DATA_DIR`; not an alias of legacy `--root`.
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Record a command's HTTP traffic into a published recording.
    Record(RecordArgs),
    /// Replay a recording against a spawned or already-running application.
    Replay(ReplayArgs),
    /// List recordings.
    List,
    /// Inspect a recording by ID, name, or `latest`.
    Inspect(InspectArgs),
    /// Diagnose environment and storage readiness with actionable remediation.
    Doctor(DoctorArgs),
    /// Hidden internal entrypoints (0.1.x compatibility; removal targeted 0.2).
    #[command(subcommand, hide = true)]
    Internal(InternalCommand),
    /// Run continuous recorder in foreground until shutdown or configured limit.
    #[command(hide = true)]
    Recorder,
    /// Read recorder metadata without mutating recorder state.
    #[command(hide = true)]
    RecorderStatus {
        #[arg(long)]
        state_root: PathBuf,
    },
    /// Canonicalize and publish one finalized recording WAL.
    #[command(hide = true)]
    Etl {
        #[arg(long)]
        wal_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
}

/// Hidden explicit internal namespace: `chronicle internal recorder`, etc.
#[derive(Debug, Subcommand)]
enum InternalCommand {
    /// Run continuous recorder in foreground (hidden internal form).
    Recorder,
    /// Read recorder metadata (hidden internal form).
    RecorderStatus {
        #[arg(long)]
        state_root: PathBuf,
    },
    /// Canonicalize and publish one finalized recording WAL (hidden internal form).
    Etl {
        #[arg(long)]
        wal_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Record from a fixture file (hidden internal form).
    RecordFixture {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        root: PathBuf,
    },
    /// Hidden bootstrap for command-mode record/replay: blocks on the
    /// readiness pipe (fd 3), hardens credentials, and execs the target.
    Bootstrap {
        #[arg(long, hide = true)]
        uid: u32,
        #[arg(long, hide = true)]
        gid: u32,
        #[arg(last = true)]
        command: Vec<String>,
    },
}

#[derive(Debug, Args)]
struct RecordArgs {
    /// Command to run and record, after `--`.
    #[arg(last = true)]
    command: Vec<String>,
    /// Optional human-readable recording name (exact UTF-8, 1-128 bytes).
    #[arg(long)]
    name: Option<String>,
    /// Recording duration, e.g. `5m`, `2h`, or plain seconds. Default 600s, max 3600s.
    #[arg(long, value_parser = parse_duration)]
    duration: Option<u64>,
    /// Retry recovery/finalization/publication for a recoverable recording.
    #[arg(long)]
    retry: Option<String>,
    /// Record an already-running process by PID (never terminated).
    #[arg(long)]
    pid: Option<u32>,
    /// Record an already-running cgroup subtree (never terminated).
    #[arg(long)]
    cgroup: Option<PathBuf>,
    // Hidden 0.1.x compatibility form: `record --source fixture|ebpf ...`.
    #[arg(long, hide = true)]
    source: Option<Source>,
    #[arg(long, hide = true)]
    input: Option<PathBuf>,
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
    #[arg(long, hide = true)]
    wal_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    allow_shared_cgroup: bool,
    #[arg(long, hide = true)]
    segment_bytes: Option<u64>,
    #[arg(long, hide = true)]
    duration_seconds: Option<u64>,
    #[arg(long, hide = true)]
    max_wal_bytes: Option<u64>,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    /// Recording to replay: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    recording: String,
    /// Command to spawn and replay against, after `--` (command mode).
    #[arg(
        last = true,
        conflicts_with_all = ["target", "allow_hosts", "allow_read", "execute"]
    )]
    command: Vec<String>,
    /// Explicit loopback target for an already-running application (target mode).
    #[arg(long)]
    target: Option<String>,
    /// Exact allow-host (loopback IP literal), repeatable.
    #[arg(long = "allow-host")]
    allow_hosts: Vec<String>,
    /// Authorize read effects in explicit target mode.
    #[arg(long)]
    allow_read: bool,
    /// Authorize write effects.
    #[arg(long)]
    allow_write: bool,
    /// Execute (explicit target mode only).
    #[arg(long)]
    execute: bool,
    // Hidden 0.1.x compatibility form: `replay SESSION --root ROOT --target ...`.
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
    #[arg(long, hide = true, value_enum)]
    timing: Option<Timing>,
}

#[derive(Debug, Args)]
struct InspectArgs {
    /// Recording to inspect: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    recording: String,
    // Hidden 0.1.x compatibility form: `inspect SESSION --root ROOT`.
    #[arg(long, hide = true)]
    root: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    // Hidden 0.1.x compatibility/advanced probes.
    #[arg(long, hide = true)]
    wal_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    output: Option<PathBuf>,
    #[arg(long, hide = true)]
    state_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Source {
    Fixture,
    Ebpf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Format {
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Timing {
    Preserve,
    Asap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LegacyInvocation {
    Recorder,
    RecorderStatus,
    Etl,
    RecordFixture,
    RecordEbpf,
    ReplayRoot,
    InspectRoot,
}

impl LegacyInvocation {
    const fn id(self) -> &'static str {
        match self {
            Self::Recorder => "recorder",
            Self::RecorderStatus => "recorder_status",
            Self::Etl => "etl",
            Self::RecordFixture => "record_source_fixture",
            Self::RecordEbpf => "record_source_ebpf",
            Self::ReplayRoot => "replay_root",
            Self::InspectRoot => "inspect_root",
        }
    }

    const fn replacement(self) -> &'static str {
        match self {
            Self::Recorder => "chronicle internal recorder --config FILE",
            Self::RecorderStatus => "chronicle internal recorder-status --state-root DIRECTORY",
            Self::Etl => "chronicle internal etl --wal-dir DIRECTORY --output DIRECTORY",
            Self::RecordFixture => {
                "chronicle internal record-fixture --input FILE --root DIRECTORY"
            }
            Self::RecordEbpf => "chronicle record --pid PID | chronicle record --cgroup PATH",
            Self::ReplayRoot => {
                "chronicle replay RECORDING --target URL --allow-host HOST [--execute]"
            }
            Self::InspectRoot => "chronicle inspect RECORDING",
        }
    }
}

#[derive(Serialize)]
struct DeprecationJson {
    version: u8,
    level: &'static str,
    code: &'static str,
    invocation: &'static str,
    replacement: &'static str,
}

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

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
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

#[derive(Serialize)]
struct ErrorJson {
    code: i32,
    message: String,
}

impl From<Timing> for TimingMode {
    fn from(value: Timing) -> Self {
        match value {
            Timing::Preserve => Self::Preserve,
            Timing::Asap => Self::Asap,
        }
    }
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
    let raw_args: Vec<OsString> = std::env::args_os().collect();
    let parse_format = format_from_raw_args(&raw_args);
    let raw_legacy = legacy_invocation_from_raw_args(&raw_args);
    let cli = match Cli::try_parse_from(&raw_args) {
        Ok(cli) => cli,
        Err(error) => {
            if let Some(legacy) = raw_legacy {
                exit_legacy_error(parse_format, 2, "invalid deprecated CLI invocation", legacy);
            }
            error.exit();
        }
    };
    let legacy = legacy_invocation(&cli);
    if let Err(error) = validate_record_arguments(&cli) {
        if let Some(legacy) = legacy {
            exit_legacy_error(cli.format, 2, "invalid deprecated CLI invocation", legacy);
        }
        error.exit();
    }
    let format = cli.format;
    match run(cli).await {
        Ok((output, code)) => {
            if code != 0
                && let Some(legacy) = legacy
            {
                exit_legacy_error(format, code, "deprecated CLI invocation failed", legacy);
            }
            if let Err(error) = write_output(io::stdout().lock(), &output) {
                let _ = write_output(
                    io::stderr().lock(),
                    &format!("CLI output I/O failed: {error}"),
                );
                std::process::exit(3);
            }
            if let Some(legacy) = legacy
                && let Err(error) = write_deprecation_warning(format, legacy)
            {
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
            let message = legacy.map_or_else(
                || error.to_string(),
                |legacy| legacy_error_message(&error.to_string(), legacy),
            );
            let output = if matches!(format, Format::Json) {
                render_json(&ErrorJson { code, message })
                    .unwrap_or_else(|render_error| render_error.to_string())
            } else {
                message
            };
            let _ = write_output(io::stderr().lock(), &output);
            std::process::exit(code);
        }
    }
}

fn legacy_invocation(cli: &Cli) -> Option<LegacyInvocation> {
    match &cli.command {
        Command::Recorder => Some(LegacyInvocation::Recorder),
        Command::RecorderStatus { .. } => Some(LegacyInvocation::RecorderStatus),
        Command::Etl { .. } => Some(LegacyInvocation::Etl),
        Command::Record(args) => match args.source {
            Some(Source::Fixture) => Some(LegacyInvocation::RecordFixture),
            Some(Source::Ebpf) => Some(LegacyInvocation::RecordEbpf),
            None => None,
        },
        Command::Replay(args) if args.root.is_some() => Some(LegacyInvocation::ReplayRoot),
        Command::Inspect(args) if args.root.is_some() => Some(LegacyInvocation::InspectRoot),
        _ => None,
    }
}

fn legacy_invocation_from_raw_args(args: &[OsString]) -> Option<LegacyInvocation> {
    let mut skip_value = false;
    for (index, argument) in args.iter().enumerate().skip(1) {
        if skip_value {
            skip_value = false;
            continue;
        }
        let Some(argument) = argument.to_str() else {
            continue;
        };
        if argument == "--" {
            break;
        }
        if matches!(argument, "--format" | "--config" | "--data-dir") {
            skip_value = true;
            continue;
        }
        if argument.starts_with("--format=")
            || argument.starts_with("--config=")
            || argument.starts_with("--data-dir=")
        {
            continue;
        }
        let remaining = &args[index + 1..];
        let legacy = match argument {
            "recorder" => Some(LegacyInvocation::Recorder),
            "recorder-status" => Some(LegacyInvocation::RecorderStatus),
            "etl" => Some(LegacyInvocation::Etl),
            "record" => raw_legacy_record(remaining),
            "replay" if raw_has_flag(remaining, "--root") => Some(LegacyInvocation::ReplayRoot),
            "inspect" if raw_has_flag(remaining, "--root") => Some(LegacyInvocation::InspectRoot),
            _ => None,
        };
        if legacy.is_some() {
            return legacy;
        }
    }
    None
}

fn raw_legacy_record(args: &[OsString]) -> Option<LegacyInvocation> {
    for (index, argument) in args.iter().enumerate() {
        let argument = argument.to_str()?;
        if argument == "--" {
            break;
        }
        if argument == "--source" {
            return Some(
                if args.get(index + 1).and_then(|value| value.to_str()) == Some("fixture") {
                    LegacyInvocation::RecordFixture
                } else {
                    LegacyInvocation::RecordEbpf
                },
            );
        }
        if let Some(source) = argument.strip_prefix("--source=") {
            return Some(if source == "fixture" {
                LegacyInvocation::RecordFixture
            } else {
                LegacyInvocation::RecordEbpf
            });
        }
    }
    None
}

fn raw_has_flag(args: &[OsString], flag: &str) -> bool {
    args.iter()
        .map_while(|argument| argument.to_str())
        .take_while(|argument| *argument != "--")
        .any(|argument| argument == flag || argument.starts_with(&format!("{flag}=")))
}

fn format_from_raw_args(args: &[OsString]) -> Format {
    for (index, argument) in args.iter().enumerate().skip(1) {
        if argument == OsStr::new("--") {
            break;
        }
        if argument == OsStr::new("--format")
            && args
                .get(index + 1)
                .is_some_and(|value| value == OsStr::new("json"))
            || argument
                .to_str()
                .is_some_and(|value| value == "--format=json")
        {
            return Format::Json;
        }
    }
    Format::Human
}

fn legacy_error_message(message: &str, legacy: LegacyInvocation) -> String {
    format!("{message}\nHint: use {}", legacy.replacement())
}

fn exit_legacy_error(format: Format, code: i32, message: &str, legacy: LegacyInvocation) -> ! {
    let message = legacy_error_message(message, legacy);
    let output = if matches!(format, Format::Json) {
        serde_json::to_string(&ErrorJson { code, message })
            .expect("static legacy error is serializable")
    } else {
        message
    };
    let _ = write_output(io::stderr().lock(), &output);
    std::process::exit(code);
}

fn write_deprecation_warning(
    format: Format,
    legacy: LegacyInvocation,
) -> Result<(), std::io::Error> {
    let warning = match format {
        Format::Human => format!(
            "warning: deprecated CLI form '{}'; use: {} (removal in 0.2)",
            legacy.id(),
            legacy.replacement()
        ),
        Format::Json => serde_json::to_string(&DeprecationJson {
            version: 1,
            level: "warning",
            code: "deprecated_cli",
            invocation: legacy.id(),
            replacement: legacy.replacement(),
        })
        .expect("static deprecation warning is serializable"),
    };
    write_output(io::stderr().lock(), &warning)
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn spawn_signal_watcher(stop: ProductionSignalStop, wal_directory: PathBuf) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};

        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return;
        };
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return;
        };
        loop {
            let interrupt_received = tokio::select! {
                _ = interrupt.recv() => true,
                _ = terminate.recv() => false,
            };
            let first = if interrupt_received {
                stop.request_interrupt()
            } else {
                stop.request_termination()
            };
            if !first {
                let _ = mark_recording_forced_termination(&wal_directory);
                std::process::exit(if interrupt_received { 130 } else { 143 });
            }
        }
    });
}

fn write_output(mut writer: impl Write, output: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(output.len() + 1);
    bytes.extend_from_slice(output.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

fn validate_record_arguments(cli: &Cli) -> Result<(), clap::Error> {
    let Command::Record(args) = &cli.command else {
        return Ok(());
    };
    let Some(source) = args.source else {
        // New public form: require exactly one mode, and enforce retry
        // mutual exclusion with capture options (usage errors exit 2).
        if args.retry.is_some() {
            if !args.command.is_empty()
                || args.pid.is_some()
                || args.cgroup.is_some()
                || args.name.is_some()
                || args.duration.is_some()
            {
                return Err(Cli::command().error(
                    ErrorKind::ArgumentConflict,
                    "--retry conflicts with command, PID, cgroup, name, and duration options",
                ));
            }
            return Ok(());
        }
        if args.pid.is_some() && args.cgroup.is_some() {
            return Err(Cli::command().error(
                ErrorKind::ArgumentConflict,
                "--pid and --cgroup are mutually exclusive",
            ));
        }
        if args.command.is_empty() && args.pid.is_none() && args.cgroup.is_none() {
            return Err(Cli::command().error(
                ErrorKind::MissingRequiredArgument,
                "record requires one of: -- COMMAND..., --pid PID, --cgroup PATH, or --retry RECORDING",
            ));
        }
        return Ok(());
    };
    let invalid = match source {
        Source::Fixture => {
            args.wal_dir.is_some()
                || args.pid.is_some()
                || args.cgroup.is_some()
                || args.allow_shared_cgroup
                || args.segment_bytes.is_some()
                || args.duration_seconds.is_some()
                || args.max_wal_bytes.is_some()
        }
        Source::Ebpf => {
            args.input.is_some()
                || args.root.is_some()
                || (args.pid.is_some() == args.cgroup.is_some())
                || (args.allow_shared_cgroup && args.cgroup.is_none())
        }
    };
    if invalid {
        return Err(Cli::command().error(
            ErrorKind::ArgumentConflict,
            "record source options conflict",
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<(String, i32), ApplicationError> {
    let recorder_config_path = cli.config.clone();
    let is_recorder = matches!(
        &cli.command,
        Command::Recorder | Command::Internal(InternalCommand::Recorder)
    );
    let is_doctor = matches!(&cli.command, Command::Doctor(_));
    let config = if is_recorder || is_doctor {
        AppConfig::default()
    } else {
        match cli.config.clone() {
            Some(path) => AppConfig::from_file(path)?,
            None => AppConfig::default(),
        }
    };
    let format = cli.format;
    let cli_data_dir = cli.data_dir.clone();
    match cli.command {
        Command::RecorderStatus { state_root }
        | Command::Internal(InternalCommand::RecorderStatus { state_root }) => {
            let metadata = load_recorder_metadata(&state_root)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            let owner_live = RecorderLease::state_is_owned(&state_root)?;
            let status = RecorderStatusV1::from_metadata(&metadata, owner_live)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            let output = match format {
                Format::Human => status
                    .render_human()
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
                Format::Json => status
                    .render_json()
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
            };
            Ok((output, 0))
        }
        Command::Recorder | Command::Internal(InternalCommand::Recorder) => {
            let path = recorder_config_path.ok_or(ApplicationError::ProductionPreflight(
                "recorder requires --config FILE",
            ))?;
            let recorder_config = RecorderConfigV1::from_file(path)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
                .normalize()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            #[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
            {
                let wal_dir = recorder_config.state_root.join("wal");
                let stop = ProductionSignalStop::default();
                spawn_signal_watcher(stop.clone(), wal_dir.clone());
                let result = record_continuous_ebpf(
                    CgroupSelector::Explicit(recorder_config.scope.cgroup_path.clone()),
                    recorder_config.scope.shared_scope_acknowledged,
                    &recorder_config,
                    &wal_dir,
                    &stop,
                )?;
                let metadata = load_recording_metadata(&wal_dir)?.ok_or(
                    ApplicationError::RecordingMetadataValidation(
                        "recording metadata missing after finalization".into(),
                    ),
                )?;
                let capture = metadata.capture.as_ref().ok_or(
                    ApplicationError::RecordingMetadataValidation(
                        "capture metadata missing after finalization".into(),
                    ),
                )?;
                let physical_wal_bytes = recording_physical_wal_bytes(&wal_dir)?;
                let output = match format {
                    Format::Human => format!(
                        "recording_id: {}\nstatus: {:?}\nshutdown_reason: {:?}\nphysical_wal_bytes: {}",
                        result.recording_id,
                        result.status,
                        result.shutdown_reason,
                        physical_wal_bytes
                    ),
                    Format::Json => render_json(&ProductionRecordJson {
                        version: 1,
                        recording_id: result.recording_id.to_string(),
                        status: result.status,
                        shutdown_reason: result.shutdown_reason,
                        last_valid_commit: result.last_valid_commit.as_ref(),
                        physical_wal_bytes,
                        selector: metadata.selector.as_ref(),
                        direct_tgid_count: capture.scope.direct_tgid_count,
                        descendant_cgroup_count: capture.scope.descendant_cgroup_count,
                        selected_subtree: capture.scope.selected_subtree,
                        shared_scope_acknowledged: capture.scope.shared_scope_acknowledged,
                        configured_bounds: &capture.configured_bounds,
                        effective_bounds: &capture.effective_bounds,
                        counters: &result.counters,
                    })?,
                };
                Ok((output, 0))
            }
            #[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
            {
                let _ = recorder_config;
                Err(ApplicationError::ProductionPreflight(
                    "Linux eBPF capture unavailable",
                ))
            }
        }
        Command::Etl { wal_dir, output }
        | Command::Internal(InternalCommand::Etl { wal_dir, output }) => {
            let result = process_and_publish_recording_wal(
                &wal_dir,
                &output,
                &chronicle_protocol_builtins::registry()?,
            )?;
            let rendered = match format {
                Format::Human => format!(
                    "session_id: {}\noutput: {}\nalready_processed: {}\nignored_post_commit_records: {}",
                    result.session_id,
                    output.display(),
                    result.already_published,
                    result.ignored_post_commit_records,
                ),
                Format::Json => render_json(&EtlJson {
                    version: 1,
                    session_id: result.session_id.to_string(),
                    recording_id: result.checkpoint.recording_id.to_string(),
                    status: result.checkpoint.status,
                    shutdown_reason: result.checkpoint.shutdown_reason,
                    output,
                    already_processed: result.already_published,
                    ignored_post_commit_records: result.ignored_post_commit_records,
                    counters: &result.checkpoint.counters,
                    checkpoint: &result.checkpoint,
                })?,
            };
            Ok((rendered, 0))
        }
        Command::Internal(InternalCommand::RecordFixture { input, root }) => {
            let result = record_fixture_file(input, &root, config.wal.segment_size_bytes)?;
            let output = match format {
                Format::Human => format!(
                    "session_id: {}\nroot: {}",
                    result.session_id,
                    root.display()
                ),
                Format::Json => render_json(&RecordJson {
                    version: 1,
                    session_id: result.session_id.to_string(),
                    root,
                })?,
            };
            Ok((output, 0))
        }
        Command::Internal(InternalCommand::Bootstrap { .. }) => {
            unreachable!("bootstrap is handled synchronously before the runtime")
        }
        Command::Record(args) => run_record(args, cli_data_dir.as_deref(), &config, format),
        Command::Replay(args) => run_replay(args, cli_data_dir.as_deref(), &config, format).await,
        Command::List => run_list(cli_data_dir.as_deref(), &config, format),
        Command::Inspect(args) => run_inspect(args, cli_data_dir.as_deref(), &config, format),
        Command::Doctor(args) => {
            run_doctor(args, cli_data_dir.as_deref(), format, recorder_config_path)
        }
    }
}

fn resolve_public_data_dir(
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
) -> Result<PathBuf, ApplicationError> {
    let resolved = resolve_data_dir(
        cli_data_dir,
        config.data_dir.as_deref(),
        &|key| std::env::var(key).ok(),
        &std::env::home_dir,
    )?;
    Ok(resolved.path)
}

fn run_record(
    args: RecordArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    match args.source {
        Some(Source::Fixture) => record_fixture_legacy(args, config, format),
        Some(Source::Ebpf) => record_ebpf_legacy(args, config, format),
        None => public_record(args, cli_data_dir, config, format),
    }
}

fn record_fixture_legacy(
    args: RecordArgs,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    if args.wal_dir.is_some()
        || args.pid.is_some()
        || args.cgroup.is_some()
        || args.allow_shared_cgroup
        || args.segment_bytes.is_some()
        || args.duration_seconds.is_some()
        || args.max_wal_bytes.is_some()
    {
        return Err(ApplicationError::ProductionPreflight(
            "eBPF-only record options used with fixture source",
        ));
    }
    let input = args.input.ok_or(ApplicationError::ProductionPreflight(
        "fixture input missing",
    ))?;
    let root = args.root.ok_or(ApplicationError::ProductionPreflight(
        "fixture root missing",
    ))?;
    let result = record_fixture_file(input, &root, config.wal.segment_size_bytes)?;
    let output = match format {
        Format::Human => format!(
            "session_id: {}\nroot: {}",
            result.session_id,
            root.display()
        ),
        Format::Json => render_json(&RecordJson {
            version: 1,
            session_id: result.session_id.to_string(),
            root,
        })?,
    };
    Ok((output, 0))
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn record_ebpf_legacy(
    args: RecordArgs,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    if args.input.is_some() || args.root.is_some() {
        return Err(ApplicationError::ProductionPreflight(
            "fixture-only record options used with eBPF source",
        ));
    }
    let selector = match (args.pid, args.cgroup) {
        (Some(pid), None) => CgroupSelector::Pid(pid),
        (None, Some(cgroup)) => CgroupSelector::Explicit(cgroup),
        _ => {
            return Err(ApplicationError::ProductionPreflight(
                "select exactly one of PID or cgroup",
            ));
        }
    };
    let wal_dir = args.wal_dir.ok_or(ApplicationError::ProductionPreflight(
        "WAL directory missing",
    ))?;
    let bounds = ProductionRecordingBounds {
        duration_seconds: args.duration_seconds.unwrap_or(600),
        segment_bytes: args.segment_bytes.unwrap_or(config.wal.segment_size_bytes),
        max_wal_bytes: args.max_wal_bytes.unwrap_or(4 * 1024 * 1024 * 1024),
    };
    let stop = ProductionSignalStop::default();
    spawn_signal_watcher(stop.clone(), wal_dir.clone());
    let result = record_live_ebpf(
        selector,
        args.allow_shared_cgroup,
        &wal_dir,
        bounds,
        &stop,
        chronicle_common::RecordingId::new(),
    )?;
    let metadata =
        load_recording_metadata(&wal_dir)?.ok_or(ApplicationError::RecordingMetadataValidation(
            "recording metadata missing after finalization".into(),
        ))?;
    let capture =
        metadata
            .capture
            .as_ref()
            .ok_or(ApplicationError::RecordingMetadataValidation(
                "capture metadata missing after finalization".into(),
            ))?;
    let physical_wal_bytes = recording_physical_wal_bytes(&wal_dir)?;
    let output = match format {
        Format::Human => format!(
            "recording_id: {}\nstatus: {:?}\nshutdown_reason: {:?}\nlast_valid_commit: {:?}\nphysical_wal_bytes: {}\ndirect_tgid_count: {}\ndescendant_cgroup_count: {}\nselected_subtree: {}\nshared_scope_acknowledged: {}\ncommitted_records: {}",
            result.recording_id,
            result.status,
            result.shutdown_reason,
            result.last_valid_commit,
            physical_wal_bytes,
            capture.scope.direct_tgid_count,
            capture.scope.descendant_cgroup_count,
            capture.scope.selected_subtree,
            capture.scope.shared_scope_acknowledged,
            result.counters.committed.records,
        ),
        Format::Json => render_json(&ProductionRecordJson {
            version: 1,
            recording_id: result.recording_id.to_string(),
            status: result.status,
            shutdown_reason: result.shutdown_reason,
            last_valid_commit: result.last_valid_commit.as_ref(),
            physical_wal_bytes,
            selector: metadata.selector.as_ref(),
            direct_tgid_count: capture.scope.direct_tgid_count,
            descendant_cgroup_count: capture.scope.descendant_cgroup_count,
            selected_subtree: capture.scope.selected_subtree,
            shared_scope_acknowledged: capture.scope.shared_scope_acknowledged,
            configured_bounds: &capture.configured_bounds,
            effective_bounds: &capture.effective_bounds,
            counters: &result.counters,
        })?,
    };
    Ok((output, 0))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
fn record_ebpf_legacy(
    args: RecordArgs,
    _config: &AppConfig,
    _format: Format,
) -> Result<(String, i32), ApplicationError> {
    drop(args);
    Err(ApplicationError::ProductionPreflight(
        "Linux eBPF capture unavailable",
    ))
}

/// Public `record`: retry mode, selector mode, or command mode. Command-mode
/// supervised scopes, selector ETL, and retry orchestration land in the
/// record-orchestration tasks; dispatch is wired here.
fn public_record(
    args: RecordArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let RecordArgs {
        command,
        name,
        duration,
        retry,
        pid,
        cgroup,
        allow_shared_cgroup,
        ..
    } = args;
    let _ = (config, format, &name, &duration);
    if let Some(reference) = retry {
        let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
        let recording_id =
            retry_recording(&data_dir, config.domain_lock_root.as_deref(), &reference)?;
        let output = match format {
            Format::Human => format!(
                "Recording finalized.\n  id: {}\nTry:\n  chronicle inspect {}",
                recording_id.to_cli_string(),
                recording_id.to_cli_string()
            ),
            Format::Json => render_json(&serde_json::json!({
                "version": 1,
                "recording_id": recording_id.to_cli_string(),
                "status": "published",
            }))?,
        };
        return Ok((output, 0));
    }
    if pid.is_some() || cgroup.is_some() {
        return record_selector_mode(
            pid,
            cgroup,
            allow_shared_cgroup,
            cli_data_dir,
            config,
            format,
        );
    }
    if !command.is_empty() {
        return record_command_mode(command, name, duration, cli_data_dir, config, format);
    }
    Err(ApplicationError::ProductionPreflight(
        "record requires one of: -- COMMAND..., --pid PID, --cgroup PATH, or --retry RECORDING",
    ))
}

/// Command mode: supervised-scope record of `-- COMMAND...`.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn record_command_mode(
    command: Vec<String>,
    name: Option<String>,
    duration: Option<u64>,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    use rustix::process::{getgid, getuid};
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let stop = ProductionSignalStop::default();
    spawn_command_signal_watcher(stop.clone());
    let result = record_command(CommandRecordOptions {
        command,
        name,
        duration_seconds: duration.unwrap_or(600),
        data_dir,
        domain_lock_root: config.domain_lock_root.clone(),
        child_stdout: if matches!(format, Format::Json) {
            ChildStdio::Null
        } else {
            ChildStdio::Inherit
        },
        child_stderr: if matches!(format, Format::Json) {
            ChildStdio::Null
        } else {
            ChildStdio::Inherit
        },
        invoking_uid: getuid().as_raw(),
        invoking_gid: getgid().as_raw(),
        stop,
    })?;
    // Exit: orphan cleanup is a typed failure (3); otherwise the recording
    // outcome drives the code (completed 0, failed 3).
    let code = if matches!(result.cleanup, CleanupOutcome::TimedOut { .. })
        || !matches!(result.status, RecordingStatus::Completed)
    {
        3
    } else {
        0
    };
    let output = render_command_record(&result, format)?;
    Ok((output, code))
}

/// Selector mode: record an already-running workload by PID or cgroup path.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn record_selector_mode(
    pid: Option<u32>,
    cgroup: Option<std::path::PathBuf>,
    allow_shared_cgroup: bool,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    use rustix::process::{getgid, getuid};
    let selector = match (pid, cgroup) {
        (Some(pid), None) => CgroupSelector::Pid(pid),
        (None, Some(cgroup)) => CgroupSelector::Explicit(cgroup),
        _ => {
            return Err(ApplicationError::ProductionPreflight(
                "select exactly one of --pid or --cgroup",
            ));
        }
    };
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let stop = ProductionSignalStop::default();
    spawn_command_signal_watcher(stop.clone());
    let result = record_selector(
        selector,
        allow_shared_cgroup,
        CommandRecordOptions {
            command: vec!["selector".into()],
            name: None,
            duration_seconds: 600,
            data_dir,
            domain_lock_root: config.domain_lock_root.clone(),
            child_stdout: ChildStdio::Null,
            child_stderr: ChildStdio::Null,
            invoking_uid: getuid().as_raw(),
            invoking_gid: getgid().as_raw(),
            stop,
        },
    )?;
    let code = if matches!(result.status, RecordingStatus::Completed) {
        0
    } else {
        3
    };
    let output = render_command_record(&result, format)?;
    Ok((output, code))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
fn record_selector_mode(
    _pid: Option<u32>,
    _cgroup: Option<std::path::PathBuf>,
    _allow_shared_cgroup: bool,
    _cli_data_dir: Option<&Path>,
    _config: &AppConfig,
    _format: Format,
) -> Result<(String, i32), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --pid/--cgroup requires the Linux eBPF build",
    ))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
fn record_command_mode(
    _command: Vec<String>,
    _name: Option<String>,
    _duration: Option<u64>,
    _cli_data_dir: Option<&Path>,
    _config: &AppConfig,
    _format: Format,
) -> Result<(String, i32), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires the Linux eBPF build",
    ))
}

/// Signal watcher for command mode (no WAL marker: the recording directory is
/// internal to the orchestration; a second signal just exits).
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn spawn_command_signal_watcher(stop: ProductionSignalStop) {
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let Ok(mut interrupt) = signal(SignalKind::interrupt()) else {
            return;
        };
        let Ok(mut terminate) = signal(SignalKind::terminate()) else {
            return;
        };
        loop {
            let interrupt_received = tokio::select! {
                _ = interrupt.recv() => true,
                _ = terminate.recv() => false,
            };
            if !if interrupt_received {
                stop.request_interrupt()
            } else {
                stop.request_termination()
            } {
                std::process::exit(if interrupt_received { 130 } else { 143 });
            }
        }
    });
}

/// Human/JSON record completion summary (v1). Formalized further by the
/// output-contract task; raw argv never appears in output.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn render_command_record(
    result: &CommandRecordResult,
    format: Format,
) -> Result<String, ApplicationError> {
    let id = result.recording_id.to_cli_string();
    match format {
        Format::Human => {
            let status = match &result.status {
                RecordingStatus::Completed => "complete",
                RecordingStatus::Failed => "failed",
                RecordingStatus::Aborted => "aborted",
                _ => "interrupted",
            };
            let reason = result
                .shutdown_reason
                .map(|reason| format!(" ({reason:?})"))
                .unwrap_or_default();
            let duration = format_duration_millis(result.duration_ms);
            let mut lines = vec![format!("Recording {status}{reason}.")];
            lines.push(format!("  id: {id}"));
            lines.push(format!("  duration: {duration}"));
            lines.push(format!("  operations: {}", result.committed_records));
            lines.push("  dropped: 0".to_owned());
            if let Some(child_exit) = &result.child_exit {
                lines.push(format!("  child: {child_exit:?}"));
            }
            if matches!(result.cleanup, CleanupOutcome::TimedOut { .. }) {
                lines.push(
                    "  warning: supervised scope still populated; orphan processes may remain"
                        .to_owned(),
                );
            }
            lines.push("Try:".to_owned());
            lines.push(format!("  chronicle inspect {id}"));
            lines.push("  chronicle replay <id> -- COMMAND...".to_owned());
            Ok(lines.join(
                "
",
            ))
        }
        Format::Json => {
            #[derive(Serialize)]
            struct RecordResultJson<'a> {
                version: u8,
                recording_id: String,
                name: Option<&'a str>,
                status: &'a str,
                shutdown_reason: Option<&'a str>,
                duration_ms: u64,
                operations: u64,
                dropped: u64,
                child_exit: Option<&'a ChildExitResult>,
            }
            render_json(&RecordResultJson {
                version: 1,
                recording_id: id,
                name: result.name.as_deref(),
                status: match result.status {
                    RecordingStatus::Completed => "completed",
                    RecordingStatus::Failed => "failed",
                    RecordingStatus::Aborted => "aborted",
                    _ => "in_progress",
                },
                shutdown_reason: result.shutdown_reason.map(|reason| match reason {
                    ShutdownReason::UserInterrupt => "user_interrupt",
                    ShutdownReason::TerminationSignal => "termination_signal",
                    ShutdownReason::SourceCompleted => "source_completed",
                    ShutdownReason::DurationLimit => "duration_limit",
                    ShutdownReason::WalSizeLimit => "wal_size_limit",
                    ShutdownReason::CaptureFailure => "capture_failure",
                    ShutdownReason::WalFailure => "wal_failure",
                    ShutdownReason::ProcessCrashRecovered => "process_crash_recovered",
                    ShutdownReason::ForcedTermination => "forced_termination",
                }),
                duration_ms: result.duration_ms,
                operations: result.committed_records,
                dropped: 0,
                child_exit: result.child_exit.as_ref(),
            })
        }
    }
}

async fn run_replay(
    args: ReplayArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    if args.root.is_some() {
        return run_replay_legacy(args, config, format).await;
    }
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let recording_id = resolve_recording(&data_dir, &args.recording)?;
    let session_id = resolve_session(&data_dir, recording_id)?
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?
        .to_string();
    if !args.command.is_empty() {
        let child_stdio = match format {
            Format::Human => ChildStdio::Inherit,
            Format::Json => ChildStdio::Null,
        };
        let result = replay_command(
            CommandReplayOptions {
                command: args.command,
                data_dir,
                session_id,
                allow_writes: args.allow_write,
                child_stdout: child_stdio,
                child_stderr: child_stdio,
                invoking_uid: rustix::process::getuid().as_raw(),
                invoking_gid: rustix::process::getgid().as_raw(),
            },
            &config.replay,
        )
        .await?;
        let output = match format {
            Format::Human => format!(
                "plan:\n{}\nresult:\n{}\ncleanup: {}",
                render_replay_human(&result.plan),
                render_replay_human(&result.replay),
                replay_cleanup_human(&result.cleanup)
            ),
            Format::Json => render_json(&CommandReplayJson {
                version: REPLAY_REPORT_VERSION,
                plan: &result.plan,
                result: &result.replay,
                cleanup: replay_cleanup_json(&result.cleanup),
            })?,
        };
        let code = if matches!(result.cleanup, CleanupOutcome::TimedOut { .. }) {
            3
        } else {
            replay_exit_code(&result.replay)
        };
        return Ok((output, code));
    }
    let target = args.target.ok_or(ApplicationError::ProductionPreflight(
        "replay requires -- COMMAND... or --target URL",
    ))?;
    if args.allow_hosts.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "explicit target replay requires --allow-host",
        ));
    }
    let options = LoopbackReplayOptions {
        target: Some(target),
        allow_hosts: args.allow_hosts,
        execute: args.execute,
        allow_reads: args.allow_read,
        allow_writes: args.allow_write,
        timing: args.timing.unwrap_or(Timing::Asap).into(),
    };
    let mut plan = None;
    let result = replay_session_with_plan(
        &data_dir,
        &session_id,
        &config.replay,
        &options,
        |next_plan| plan = Some(next_plan.clone()),
    )
    .await?;
    let plan = plan.expect("replay plan callback must run");
    let output = match format {
        Format::Human => format!(
            "plan:\n{}\nresult:\n{}",
            render_replay_human(&plan),
            render_replay_human(&result)
        ),
        Format::Json => render_json(&ReplayJson {
            version: REPLAY_REPORT_VERSION,
            plan: &plan,
            result: &result,
        })?,
    };
    Ok((output, replay_exit_code(&result)))
}

async fn run_replay_legacy(
    args: ReplayArgs,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let root = args.root.expect("legacy replay has root");
    let target = args.target.ok_or(ApplicationError::ProductionPreflight(
        "legacy replay requires --target",
    ))?;
    if args.allow_hosts.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "legacy replay requires --allow-host",
        ));
    }
    let options = LoopbackReplayOptions {
        target: Some(target),
        allow_hosts: args.allow_hosts,
        execute: args.execute,
        allow_reads: args.allow_read,
        allow_writes: args.allow_write,
        timing: args.timing.unwrap_or(Timing::Asap).into(),
    };
    let mut plan = None;
    let result = replay_session_with_plan(
        root,
        &args.recording,
        &config.replay,
        &options,
        |next_plan| plan = Some(next_plan.clone()),
    )
    .await?;
    let plan = plan.expect("replay plan callback must run");
    let output = match format {
        Format::Human => format!(
            "plan:\n{}\nresult:\n{}",
            render_replay_human(&plan),
            render_replay_human(&result)
        ),
        Format::Json => render_json(&ReplayJson {
            version: REPLAY_REPORT_VERSION,
            plan: &plan,
            result: &result,
        })?,
    };
    Ok((output, replay_exit_code(&result)))
}

#[derive(Serialize)]
struct ListRecordingJson {
    recording_id: String,
    name: Option<String>,
    created_at: String,
    duration: Option<u64>,
    sessions: usize,
    operations: usize,
    status: chronicle_application::RecordingCatalogStatus,
}

#[derive(Serialize)]
struct ListJson {
    version: u8,
    recordings: Vec<ListRecordingJson>,
}

#[derive(Serialize)]
struct InspectRecordingJson<'a> {
    version: u8,
    recording_id: String,
    name: Option<&'a str>,
    created_at: String,
    duration_ms: Option<u64>,
    sessions: usize,
    operations: usize,
    status: chronicle_application::RecordingCatalogStatus,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

fn render_recording_inspect_human(
    recording: &ListedRecording,
    inspected: &InspectSessionResult,
) -> String {
    let name = recording
        .name
        .as_deref()
        .map_or_else(|| "none".to_owned(), escape_control);
    let duration = recording
        .duration_ms
        .map_or_else(|| "unknown".to_owned(), format_duration_millis);
    format!(
        "recording_id: {}\nname: {}\ncreated_at: {}\nduration: {}\nsessions: {}\noperations: {}\nstatus: {}\n{}",
        recording.recording_id.to_cli_string(),
        name,
        format_created_at(&recording.created_at),
        duration,
        recording.sessions,
        recording.operations,
        recording_status_human(recording.status),
        render_inspect_human(inspected),
    )
}

fn recording_status_human(status: chronicle_application::RecordingCatalogStatus) -> &'static str {
    match status {
        chronicle_application::RecordingCatalogStatus::InProgress => "in_progress",
        chronicle_application::RecordingCatalogStatus::Recoverable => "recoverable",
        chronicle_application::RecordingCatalogStatus::Published => "published",
        chronicle_application::RecordingCatalogStatus::Failed => "failed",
        chronicle_application::RecordingCatalogStatus::Inconsistent => "inconsistent",
    }
}

fn format_duration_millis(millis: u64) -> String {
    let seconds = millis.div_ceil(1000);
    let minutes = seconds / 60;
    let remainder = seconds % 60;
    if minutes == 0 {
        format!("{remainder}s")
    } else {
        format!("{minutes}m {remainder}s")
    }
}

fn format_created_at(timestamp: &chronicle_common::Timestamp) -> String {
    timestamp.to_string()
}

fn run_list(
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let rows = list_recordings(&data_dir)?;
    let output = match format {
        Format::Human => {
            if rows.is_empty() {
                "No recordings yet.\nRun 'chronicle record -- COMMAND...' to record your first application.".to_owned()
            } else {
                let header = "ID  NAME  CREATED  DURATION  SESSIONS  OPERATIONS  STATUS";
                let mut lines = vec![header.to_owned()];
                for row in &rows {
                    let name = row
                        .name
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), escape_control);
                    let duration = row
                        .duration_ms
                        .map_or_else(|| "—".to_owned(), format_duration_millis);
                    lines.push(format!(
                        "{}  {}  {}  {}  {}  {}  {}",
                        row.recording_id.to_cli_string(),
                        name,
                        format_created_at(&row.created_at),
                        duration,
                        row.sessions,
                        row.operations,
                        recording_status_human(row.status),
                    ));
                }
                lines.join("\n")
            }
        }
        Format::Json => render_json(&ListJson {
            version: 1,
            recordings: rows
                .into_iter()
                .map(|row| ListRecordingJson {
                    recording_id: row.recording_id.to_cli_string(),
                    name: row.name,
                    created_at: format_created_at(&row.created_at),
                    duration: row.duration_ms,
                    sessions: row.sessions,
                    operations: row.operations,
                    status: row.status,
                })
                .collect(),
        })?,
    };
    Ok((output, 0))
}

fn run_inspect(
    args: InspectArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    if let Some(root) = args.root {
        let id = parse_session_id(&args.recording)?;
        let result = inspect_session(root, id)?;
        let output = match format {
            Format::Human => render_inspect_human(&result),
            Format::Json => render_inspect_json(&result)?,
        };
        return Ok((output, 0));
    }
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let recording_id = resolve_recording(&data_dir, &args.recording)?;
    let recording = list_recordings(&data_dir)?
        .into_iter()
        .find(|row| row.recording_id == recording_id)
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?;
    let session_id = resolve_session(&data_dir, recording_id)?
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?;
    let result = inspect_session(&data_dir, session_id)?;
    let output = match format {
        Format::Human => render_recording_inspect_human(&recording, &result),
        Format::Json => render_json(&InspectRecordingJson {
            version: 1,
            recording_id: recording.recording_id.to_cli_string(),
            name: recording.name.as_deref(),
            created_at: format_created_at(&recording.created_at),
            duration_ms: recording.duration_ms,
            sessions: recording.sessions,
            operations: recording.operations,
            status: recording.status,
            result: &result,
        })?,
    };
    Ok((output, 0))
}

fn run_doctor(
    args: DoctorArgs,
    cli_data_dir: Option<&Path>,
    format: Format,
    config_path: Option<PathBuf>,
) -> Result<(String, i32), ApplicationError> {
    let (config, replay_probe) = if let Some(path) = config_path {
        match AppConfig::from_file(path) {
            Ok(config) => {
                let probe = replay_config_doctor_probe(&config.replay);
                (config, probe)
            }
            Err(_) => (
                AppConfig::default(),
                DoctorProbe {
                    required: false,
                    status: DoctorStatus::Unsupported,
                    code: "replay.configuration".into(),
                    message: "application configuration could not be loaded".into(),
                    remediation: "fix the TOML file supplied with --config, then rerun doctor"
                        .into(),
                },
            ),
        }
    } else {
        let config = AppConfig::default();
        let probe = replay_config_doctor_probe(&config.replay);
        (config, probe)
    };
    let resolved = resolve_data_dir(
        cli_data_dir,
        config.data_dir.as_deref(),
        &|key| std::env::var(key).ok(),
        &std::env::home_dir,
    );
    let data_dir_probe = data_dir_doctor_probe(resolved.as_ref());
    let base = chronicle_application::doctor_report(&chronicle_application::DoctorOptions {
        wal_dir: args.wal_dir,
        output: args.output,
        state_root: args.state_root,
        config: None,
    });
    let mut probes = vec![data_dir_probe, replay_probe];
    probes.extend(base.probes);
    let report = DoctorReport::new(probes);
    let rendered = match format {
        Format::Human => report
            .probes
            .iter()
            .map(|probe| {
                format!(
                    "{}: {:?} — {} — remediation: {}",
                    probe.code, probe.status, probe.message, probe.remediation
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Format::Json => render_json(&report)?,
    };
    Ok((rendered, report.exit_code()))
}

fn parse_session_id(value: &str) -> Result<chronicle_common::SessionId, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .map(chronicle_common::SessionId)
        .map_err(|_| ApplicationError::InvalidConfig("session ID must be a UUID".into()))
}

/// Parse `--duration`: plain seconds, `5m`, or `2h`; bounds 1..=3600 seconds
/// (default 600 is applied by the application). Values outside the bound are
/// Clap usage errors (exit 2).
fn parse_duration(value: &str) -> Result<u64, String> {
    let seconds = if let Some(minutes) = value.strip_suffix('m') {
        minutes
            .parse::<u64>()
            .map_err(|_| format!("invalid duration '{value}'"))?
            .saturating_mul(60)
    } else if let Some(hours) = value.strip_suffix('h') {
        hours
            .parse::<u64>()
            .map_err(|_| format!("invalid duration '{value}'"))?
            .saturating_mul(3600)
    } else {
        value
            .parse::<u64>()
            .map_err(|_| format!("invalid duration '{value}'"))?
    };
    if seconds == 0 || seconds > 3600 {
        return Err(format!(
            "duration must be between 1s and 3600s (1h), got '{value}'"
        ));
    }
    Ok(seconds)
}

fn replay_exit_code(result: &chronicle_application::ReplaySessionResult) -> i32 {
    match result.outcome {
        ReplayOutcome::Completed | ReplayOutcome::CompletedWithSkips | ReplayOutcome::DryRun => 0,
        ReplayOutcome::StoppedPolicy | ReplayOutcome::StoppedInvalidSession => 4,
        ReplayOutcome::StoppedTransport => 5,
        ReplayOutcome::StoppedVerification => 6,
    }
}

fn error_code(error: &ApplicationError) -> i32 {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_replay::OperationExecutionState;

    #[derive(Default)]
    struct TestWriter {
        bytes: Vec<u8>,
        fail_write: bool,
        fail_flush: bool,
        flushes: usize,
    }

    impl Write for TestWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.fail_write {
                return Err(io::Error::other("injected write failure"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes += 1;
            if self.fail_flush {
                return Err(io::Error::other("injected flush failure"));
            }
            Ok(())
        }
    }

    #[test]
    fn output_boundary_checks_write_and_flush() {
        let mut writer = TestWriter::default();
        write_output(&mut writer, "{\"ok\":true}").unwrap();
        assert_eq!(writer.bytes, b"{\"ok\":true}\n");
        assert_eq!(writer.flushes, 1);

        let mut write_failure = TestWriter {
            fail_write: true,
            ..TestWriter::default()
        };
        assert!(write_output(&mut write_failure, "{}").is_err());
        assert_eq!(write_failure.flushes, 0);

        let mut flush_failure = TestWriter {
            fail_flush: true,
            ..TestWriter::default()
        };
        assert!(write_output(&mut flush_failure, "{}").is_err());
        assert_eq!(flush_failure.bytes, b"{}\n");
        assert_eq!(flush_failure.flushes, 1);
    }

    #[test]
    fn cli_parses_recorder_status_command() {
        let cli =
            Cli::try_parse_from(["chronicle", "recorder-status", "--state-root", "state"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::RecorderStatus { state_root } if state_root.as_path() == std::path::Path::new("state")
        ));
    }

    #[test]
    fn cli_parses_foreground_recorder_command() {
        let cli =
            Cli::try_parse_from(["chronicle", "recorder", "--config", "recorder.toml"]).unwrap();
        assert!(matches!(cli.command, Command::Recorder));
        assert_eq!(cli.config, Some(PathBuf::from("recorder.toml")));
    }

    #[test]
    fn cli_parses_required_runnable_commands() {
        let record = Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "fixture",
            "--input",
            "fixture.json",
            "--root",
            "root",
        ])
        .unwrap();
        assert!(matches!(record.command, Command::Record { .. }));

        let etl = Cli::try_parse_from([
            "chronicle",
            "etl",
            "--wal-dir",
            "recording",
            "--output",
            "output",
        ])
        .unwrap();
        assert!(matches!(
            etl.command,
            Command::Etl { wal_dir, output }
                if wal_dir.as_path() == std::path::Path::new("recording")
                    && output.as_path() == std::path::Path::new("output")
        ));

        let replay = Cli::try_parse_from([
            "chronicle",
            "replay",
            "00000000-0000-0000-0000-000000000000",
            "--root",
            "root",
            "--target",
            "http://127.0.0.1:8080",
            "--allow-host",
            "127.0.0.1",
        ])
        .unwrap();
        let Command::Replay(args) = replay.command else {
            panic!("replay command expected")
        };
        assert!(args.timing.is_none());
        assert!(!args.execute);
        assert_eq!(args.root, Some(PathBuf::from("root")));
    }

    #[test]
    fn cli_parses_production_record_options() {
        let cli = Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "ebpf",
            "--cgroup",
            "/workload",
            "--allow-shared-cgroup",
            "--wal-dir",
            "wal",
            "--duration-seconds",
            "60",
            "--segment-bytes",
            "16777216",
            "--max-wal-bytes",
            "33554432",
        ])
        .unwrap();
        let Command::Record(args) = cli.command else {
            panic!("record command expected");
        };
        assert!(matches!(args.source, Some(Source::Ebpf)));
        assert!(args.input.is_none() && args.root.is_none() && args.pid.is_none());
        assert_eq!(args.cgroup, Some(PathBuf::from("/workload")));
        assert!(args.allow_shared_cgroup);
        assert_eq!(args.wal_dir, Some(PathBuf::from("wal")));
        assert_eq!(args.duration_seconds, Some(60));
        assert_eq!(args.segment_bytes, Some(16_777_216));
        assert_eq!(args.max_wal_bytes, Some(33_554_432));
    }

    #[test]
    fn legacy_invocations_use_fixed_non_secret_diagnostics() {
        let raw: Vec<OsString> = [
            "chronicle",
            "--format",
            "json",
            "replay",
            "session",
            "--root",
            "root",
            "--target",
            "http://user:super-secret@127.0.0.1:9",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        let legacy = legacy_invocation_from_raw_args(&raw).unwrap();
        assert_eq!(legacy, LegacyInvocation::ReplayRoot);
        assert!(matches!(format_from_raw_args(&raw), Format::Json));
        let warning = serde_json::to_string(&DeprecationJson {
            version: 1,
            level: "warning",
            code: "deprecated_cli",
            invocation: legacy.id(),
            replacement: legacy.replacement(),
        })
        .unwrap();
        assert!(!warning.contains("super-secret"));
        assert!(!warning.contains("user:"));

        let internal = Cli::try_parse_from([
            "chronicle",
            "internal",
            "etl",
            "--wal-dir",
            "wal",
            "--output",
            "output",
        ])
        .unwrap();
        assert_eq!(legacy_invocation(&internal), None);
    }

    #[test]
    fn production_record_help_hides_mechanics_and_shows_intent_options() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("record")
            .expect("record subcommand")
            .render_long_help()
            .to_string();
        // Public record help shows intent options and hides legacy mechanics.
        assert!(help.contains("--duration"));
        assert!(help.contains("--name"));
        assert!(help.contains("--retry"));
        assert!(!help.contains("--source"));
        assert!(!help.contains("--wal-dir"));
        assert!(!help.contains("--segment-bytes"));
    }

    #[test]
    fn production_record_arguments_reject_source_mixes_and_bad_selectors() {
        let mixed = Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "fixture",
            "--input",
            "fixture.json",
            "--root",
            "root",
            "--wal-dir",
            "wal",
        ])
        .unwrap();
        assert!(validate_record_arguments(&mixed).is_err());

        let missing_selector = Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "ebpf",
            "--wal-dir",
            "wal",
        ])
        .unwrap();
        assert!(validate_record_arguments(&missing_selector).is_err());

        let pid_with_acknowledgement = Cli::try_parse_from([
            "chronicle",
            "record",
            "--source",
            "ebpf",
            "--pid",
            "42",
            "--allow-shared-cgroup",
            "--wal-dir",
            "wal",
        ])
        .unwrap();
        assert!(validate_record_arguments(&pid_with_acknowledgement).is_err());
    }

    #[test]
    fn transport_errors_map_to_exit_five() {
        let error = ApplicationError::Protocol(ProtocolError::Transport {
            category: TransportErrorCategory::Timeout,
            message: "timeout".into(),
        });
        assert_eq!(error_code(&error), 5);
    }

    #[test]
    fn replay_outcomes_map_to_stable_exit_codes() {
        let result = |outcome| chronicle_application::ReplaySessionResult {
            session_id: "session".into(),
            replayability: chronicle_replay::Replayability::FullyReplayable,
            outcome,
            dry_run: outcome == ReplayOutcome::DryRun,
            preflight_denied: outcome == ReplayOutcome::StoppedPolicy,
            transport_failed: outcome == ReplayOutcome::StoppedTransport,
            counts: chronicle_application::ReplayCounts::default(),
            operations: vec![chronicle_application::ReplayOperationSummary {
                operation_id: "operation".into(),
                decision: "allowed".into(),
                state: OperationExecutionState::NotAttempted,
                attempted: false,
                verification: "not_run".into(),
                category: String::new(),
                transport_error: String::new(),
            }],
        };

        for outcome in [
            ReplayOutcome::Completed,
            ReplayOutcome::CompletedWithSkips,
            ReplayOutcome::DryRun,
        ] {
            assert_eq!(replay_exit_code(&result(outcome)), 0);
        }
        for outcome in [
            ReplayOutcome::StoppedPolicy,
            ReplayOutcome::StoppedInvalidSession,
        ] {
            assert_eq!(replay_exit_code(&result(outcome)), 4);
        }
        assert_eq!(
            replay_exit_code(&result(ReplayOutcome::StoppedTransport)),
            5
        );
        assert_eq!(
            replay_exit_code(&result(ReplayOutcome::StoppedVerification)),
            6
        );
    }

    #[test]
    fn command_replay_cleanup_timeout_reports_orphan_evidence() {
        let cleanup = CleanupOutcome::TimedOut {
            remaining: vec![41, 42],
        };
        let json = serde_json::to_value(replay_cleanup_json(&cleanup)).unwrap();
        assert_eq!(json["status"], "timed_out");
        assert_eq!(json["possible_orphan_pids"], serde_json::json!([41, 42]));
        assert!(replay_cleanup_human(&cleanup).contains("possible orphan pids"));
    }
}

#[cfg(test)]
mod new_surface_tests {
    use super::*;

    #[test]
    fn duration_parser_accepts_seconds_minutes_hours_and_bounds() {
        assert_eq!(parse_duration("30").unwrap(), 30);
        assert_eq!(parse_duration("5m").unwrap(), 300);
        assert_eq!(parse_duration("1h").unwrap(), 3600);
        // Over-max exits 2 via Clap usage error.
        assert!(parse_duration("2h").is_err());
        assert!(parse_duration("3601").is_err());
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("bogus").is_err());
    }

    #[test]
    fn retry_conflicts_with_capture_options_are_usage_errors() {
        let conflicts = [
            vec!["--retry", "rec_1", "--name", "x"],
            vec!["--retry", "rec_1", "--duration", "5m"],
            vec!["--retry", "rec_1", "--pid", "42"],
            vec!["--retry", "rec_1", "--cgroup", "/w"],
            vec!["--retry", "rec_1", "--", "echo", "hi"],
        ];
        for extra in conflicts {
            let mut argv = vec!["chronicle", "record"];
            argv.extend(extra.clone());
            let parsed = Cli::try_parse_from(argv).expect("parses");
            assert!(validate_record_arguments(&parsed).is_err(), "{extra:?}");
        }
    }

    #[test]
    fn record_requires_a_mode_and_selectors_are_mutually_exclusive() {
        let bare = Cli::try_parse_from(["chronicle", "record"]).unwrap();
        assert!(validate_record_arguments(&bare).is_err());

        let both_selectors =
            Cli::try_parse_from(["chronicle", "record", "--pid", "1", "--cgroup", "/w"]).unwrap();
        assert!(validate_record_arguments(&both_selectors).is_err());

        let command_mode =
            Cli::try_parse_from(["chronicle", "record", "--", "echo", "hi"]).unwrap();
        assert!(validate_record_arguments(&command_mode).is_ok());
    }

    #[test]
    fn replay_target_and_command_modes_conflict_in_clap() {
        let error = Cli::try_parse_from([
            "chronicle",
            "replay",
            "latest",
            "--target",
            "http://127.0.0.1:8080",
            "--",
            "/bin/true",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn replay_help_shows_both_modes_and_write_stays_explicit() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("replay")
            .expect("replay subcommand")
            .render_long_help()
            .to_string();
        assert!(help.contains("--allow-write"));
        assert!(help.contains("--target"));
        assert!(help.contains("command mode"));
        assert!(help.contains("COMMAND"));
    }

    #[test]
    fn public_help_lists_exactly_five_commands() {
        let help = Cli::command().render_long_help().to_string();
        for name in ["record", "replay", "list", "inspect", "doctor"] {
            assert!(help.contains(name), "missing {name}");
        }
        // Legacy mechanics are hidden from public help.
        assert!(!help.contains("recorder-status"));
        assert!(!help.contains("etl"));
        assert!(!help.contains("--wal-dir"));
    }
}
