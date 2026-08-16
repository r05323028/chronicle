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

#[derive(Debug, Parser)]
#[command(
    name = "chronicle",
    version,
    about = "Record, inspect, and safely replay local application behavior",
    long_about = "Record production-like HTTP behavior as portable regression tests.\n\nQuick start:\n  chronicle record -- COMMAND...\n  chronicle list\n  chronicle inspect latest\n  chronicle replay latest -- COMMAND..."
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
    /// Hidden internal operational namespace: `internal recorder`, `internal
    /// recorder-status`, `internal etl`, `internal record-fixture`, and
    /// `internal bootstrap` (foreground operation and deterministic fixture
    /// support; not user-facing product commands).
    #[command(subcommand, hide = true)]
    Internal(InternalCommand),
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
    /// Optional whole-recording deadline, e.g. `10m`, `24h`, or plain seconds.
    /// Omitted means run until source completion, explicit stop, or fatal failure.
    #[arg(long, value_parser = parse_duration)]
    duration: Option<Duration>,
    /// Retry recovery/finalization/publication for a recoverable recording.
    #[arg(long)]
    retry: Option<String>,
    /// Record an already-running process by PID (never terminated).
    #[arg(long)]
    pid: Option<u32>,
    /// Record an already-running cgroup subtree (never terminated).
    #[arg(long)]
    cgroup: Option<PathBuf>,
    /// Acknowledge a shared cgroup subtree (more than one direct TGID).
    #[arg(long, hide = true)]
    allow_shared_cgroup: bool,
}

#[derive(Debug, Args)]
struct ReplayArgs {
    /// Recording to replay: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    recording: String,
    /// Explicit epoch ordinal for multi-epoch parent replay.
    #[arg(long)]
    epoch: Option<u64>,
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
    /// Replay timing control (advanced).
    #[arg(long, hide = true, value_enum)]
    timing: Option<Timing>,
}

#[derive(Debug, Args)]
struct InspectArgs {
    /// Recording to inspect: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    recording: String,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    // Hidden advanced diagnostic probes.
    #[arg(long, hide = true)]
    wal_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    output: Option<PathBuf>,
    #[arg(long, hide = true)]
    state_root: Option<PathBuf>,
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

impl From<Timing> for ReplayTiming {
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

#[cfg(target_os = "linux")]
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
    // Public form: require exactly one mode, and enforce retry mutual
    // exclusion with capture options (usage errors exit 2).
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
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn run(cli: Cli) -> Result<(String, i32), ApplicationError> {
    let recorder_config_path = cli.config.clone();
    let is_recorder = matches!(&cli.command, Command::Internal(InternalCommand::Recorder));
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
        Command::Internal(InternalCommand::RecorderStatus { state_root }) => {
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
        Command::Internal(InternalCommand::Recorder) => {
            let path = recorder_config_path.ok_or(ApplicationError::ProductionPreflight(
                "recorder requires --config FILE",
            ))?;
            let recorder_config = RecorderConfigV1::from_file(path)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
                .normalize()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            #[cfg(target_os = "linux")]
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
            #[cfg(not(target_os = "linux"))]
            {
                let _ = recorder_config;
                Err(ApplicationError::ProductionPreflight(
                    "live capture is unavailable on this platform",
                ))
            }
        }
        Command::Internal(InternalCommand::Etl { wal_dir, output }) => {
            let registry = chronicle_application::protocol_registry()?;
            let result = match chronicle_application::resolve_epoch_parent_context(&wal_dir)? {
                Some((parent_id, epoch_id, epoch_ordinal)) => {
                    process_and_publish_recording_wal_with_parent(
                        &wal_dir,
                        &output,
                        &registry,
                        parent_id,
                        epoch_id,
                        epoch_ordinal,
                    )?
                }
                None => process_and_publish_recording_wal(&wal_dir, &output, &registry)?,
            };
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
        Command::Inspect(args) => run_inspect(&args, cli_data_dir.as_deref(), &config, format),
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
    public_record(args, cli_data_dir, config, format)
}

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
            (name, duration),
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
#[cfg(target_os = "linux")]
fn record_command_mode(
    command: Vec<String>,
    name: Option<String>,
    duration: Option<Duration>,
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
        duration_seconds: duration.map_or(0, |value| value.as_secs()),
        lifetime: RecordingLifetime {
            deadline: duration,
            stop_policy: RunStopPolicy::AnyTerminalCondition,
        },
        capture_mode: CaptureMode::Command,
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
#[cfg(target_os = "linux")]
fn record_selector_mode(
    pid: Option<u32>,
    cgroup: Option<std::path::PathBuf>,
    allow_shared_cgroup: bool,
    intent: (Option<String>, Option<Duration>),
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    use rustix::process::{getgid, getuid};
    let (name, duration) = intent;
    let selector = match (pid, cgroup) {
        (Some(pid), None) => CgroupSelector::Pid(pid),
        (None, Some(cgroup)) => CgroupSelector::Explicit(cgroup),
        _ => {
            return Err(ApplicationError::ProductionPreflight(
                "select exactly one of --pid or --cgroup",
            ));
        }
    };
    let capture_mode = match &selector {
        CgroupSelector::Pid(_) => CaptureMode::Pid,
        CgroupSelector::Explicit(_) => CaptureMode::Cgroup,
    };
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let stop = ProductionSignalStop::default();
    spawn_command_signal_watcher(stop.clone());
    let result = record_selector(
        selector,
        allow_shared_cgroup,
        CommandRecordOptions {
            command: vec!["selector".into()],
            name,
            duration_seconds: duration.map_or(0, |value| value.as_secs()),
            lifetime: RecordingLifetime {
                deadline: duration,
                stop_policy: RunStopPolicy::AnyTerminalCondition,
            },
            capture_mode,
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

#[cfg(not(target_os = "linux"))]
fn record_selector_mode(
    _pid: Option<u32>,
    _cgroup: Option<std::path::PathBuf>,
    _allow_shared_cgroup: bool,
    _intent: (Option<String>, Option<Duration>),
    _cli_data_dir: Option<&Path>,
    _config: &AppConfig,
    _format: Format,
) -> Result<(String, i32), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --pid/--cgroup requires a Linux build with live capture",
    ))
}

#[cfg(not(target_os = "linux"))]
fn record_command_mode(
    _command: Vec<String>,
    _name: Option<String>,
    _duration: Option<Duration>,
    _cli_data_dir: Option<&Path>,
    _config: &AppConfig,
    _format: Format,
) -> Result<(String, i32), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires a Linux build with live capture",
    ))
}

/// Signal watcher for command mode (no WAL marker: the recording directory is
/// internal to the orchestration; a second signal just exits).
#[cfg(target_os = "linux")]
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

/// Human/JSON public record completion summary v1. Raw argv never appears.
#[cfg(any(test, target_os = "linux"))]
fn render_command_record(
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
                    ChildExitResult::ExitCode { code } => {
                        format!("  child: exit_code {code}")
                    }
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

#[allow(clippy::too_many_lines)]
async fn run_replay(
    args: ReplayArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let recording_id = resolve_recording(&data_dir, &args.recording)?;
    let parent_plan = build_parent_replay_plan(&data_dir, recording_id)?;
    let selected_session_id = match args.epoch {
        Some(ordinal) => Some(
            parent_plan
                .epochs
                .iter()
                .find(|epoch| epoch.ordinal == ordinal && epoch.selected)
                .and_then(|epoch| epoch.session_id)
                .ok_or_else(|| {
                    ApplicationError::InvalidConfig(
                        "requested epoch is not a contiguous published replay epoch".into(),
                    )
                })?,
        ),
        None => match parent_plan.selected_session_ids.as_slice() {
            [session_id] => Some(*session_id),
            [] => {
                return Err(ApplicationError::InvalidConfig(
                    "recording has no published replay epoch".into(),
                ));
            }
            _ => None,
        },
    };
    if !args.command.is_empty() {
        let session_id = selected_session_id.ok_or_else(|| {
            ApplicationError::InvalidConfig(
                "command-mode parent replay requires explicit --epoch".into(),
            )
        })?;
        let child_stdio = match format {
            Format::Human => ChildStdio::Inherit,
            Format::Json => ChildStdio::Null,
        };
        let result = replay_command(
            CommandReplayOptions {
                command: args.command,
                data_dir,
                session_id: session_id.to_string(),
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
            Format::Human => {
                render_public_replay_human(&result.plan, &result.replay, Some(&result.cleanup))
            }
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
    let request = ReplayRequest {
        target: Some(target),
        allow_hosts: args.allow_hosts,
        execute: args.execute,
        allow_reads: args.allow_read,
        allow_writes: args.allow_write,
        timing: args.timing.unwrap_or(Timing::Asap).into(),
    };
    let mut plan = None;
    let result = match selected_session_id {
        Some(session_id) => {
            replay_session_with_plan(
                &data_dir,
                &session_id.to_string(),
                &config.replay,
                &request,
                |next_plan| plan = Some(next_plan.clone()),
            )
            .await?
        }
        None => {
            replay_parent_session_with_plan(
                &data_dir,
                recording_id,
                &parent_plan.selected_session_ids,
                &config.replay,
                &request,
                |next_plan| plan = Some(next_plan.clone()),
            )
            .await?
        }
    };
    let plan = plan.expect("replay plan callback must run");
    let output = match format {
        Format::Human => render_public_replay_human(&plan, &result, None),
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
    epoch_count: usize,
    active_epoch: Option<u64>,
    published_epoch_count: usize,
    status: chronicle_application::RunLifecycleState,
    warnings: Vec<String>,
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
    epoch_count: usize,
    active_epoch: Option<u64>,
    published_epoch_count: usize,
    warnings: &'a [String],
    epochs: Vec<chronicle_application::ParentReplayEpochPlan>,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

fn render_recording_inspect_human(
    recording: &ListedRecording,
    parent: &chronicle_application::ParentRecordingViewV2,
    epochs: &[chronicle_application::ParentReplayEpochPlan],
    inspected: &InspectSessionResult,
) -> String {
    let name = recording
        .name
        .as_deref()
        .map_or_else(|| "none".to_owned(), escape_control);
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

fn recording_status_human(status: chronicle_application::RecordingCatalogStatus) -> &'static str {
    match status {
        chronicle_application::RecordingCatalogStatus::InProgress => "in_progress",
        chronicle_application::RecordingCatalogStatus::Recoverable => "recoverable",
        chronicle_application::RecordingCatalogStatus::Published => "published",
        chronicle_application::RecordingCatalogStatus::Failed => "failed",
        chronicle_application::RecordingCatalogStatus::Inconsistent => "inconsistent",
    }
}

fn parent_status_human(status: chronicle_application::RunLifecycleState) -> &'static str {
    match status {
        chronicle_application::RunLifecycleState::Starting => "starting",
        chronicle_application::RunLifecycleState::Running => "running",
        chronicle_application::RunLifecycleState::Draining => "draining",
        chronicle_application::RunLifecycleState::Completed => "completed",
        chronicle_application::RunLifecycleState::Stopped => "stopped",
        chronicle_application::RunLifecycleState::Failed => "failed",
        chronicle_application::RunLifecycleState::Inconsistent => "inconsistent",
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

fn format_created_at(timestamp: &chronicle_application::Timestamp) -> String {
    timestamp.to_string()
}

fn run_list(
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let rows = list_parent_recording_views(&data_dir)?;
    let output = match format {
        Format::Human => {
            if rows.is_empty() {
                "No recordings yet.\nRun 'chronicle record -- COMMAND...' to record your first application.".to_owned()
            } else {
                let header = "ID  NAME  CREATED  DURATION  SESSIONS  OPERATIONS  EPOCHS  ACTIVE  PUBLISHED  STATUS";
                let mut lines = vec![header.to_owned()];
                for row in &rows {
                    let name = row
                        .name
                        .as_deref()
                        .map_or_else(|| "—".to_owned(), escape_control);
                    let duration = row
                        .duration_ms
                        .map_or_else(|| "—".to_owned(), format_duration_millis);
                    let active = row
                        .active_epoch
                        .map_or_else(|| "—".to_owned(), |epoch| epoch.to_string());
                    lines.push(format!(
                        "{}  {}  {}  {}  {}  {}  {}  {}  {}  {}",
                        row.parent_id.to_cli_string(),
                        name,
                        row.created_at_unix_millis,
                        duration,
                        row.sessions,
                        row.operations,
                        row.epoch_count,
                        active,
                        row.published_epoch_count,
                        parent_status_human(row.state),
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
                    recording_id: row.parent_id.to_cli_string(),
                    name: row.name,
                    created_at: row.created_at_unix_millis.to_string(),
                    duration: row.duration_ms,
                    sessions: row.sessions,
                    operations: row.operations,
                    epoch_count: row.epoch_count,
                    active_epoch: row.active_epoch,
                    published_epoch_count: row.published_epoch_count,
                    status: row.state,
                    warnings: row.warnings,
                })
                .collect(),
        })?,
    };
    Ok((output, 0))
}

fn run_inspect(
    args: &InspectArgs,
    cli_data_dir: Option<&Path>,
    config: &AppConfig,
    format: Format,
) -> Result<(String, i32), ApplicationError> {
    let data_dir = resolve_public_data_dir(cli_data_dir, config)?;
    let recording_id = resolve_recording(&data_dir, &args.recording)?;
    let recording = list_recordings(&data_dir)?
        .into_iter()
        .find(|row| row.recording_id == recording_id)
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?;
    let parent = list_parent_recording_views(&data_dir)?
        .into_iter()
        .find(|view| view.parent_id == recording_id)
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?;
    let parent_plan = build_parent_replay_plan(&data_dir, recording_id)?;
    let session_id = resolve_session(&data_dir, recording_id)?
        .ok_or_else(|| ApplicationError::RecordingNotFound(args.recording.clone()))?;
    let result = inspect_session(&data_dir, session_id)?;
    let output = match format {
        Format::Human => {
            render_recording_inspect_human(&recording, &parent, &parent_plan.epochs, &result)
        }
        Format::Json => render_json(&InspectRecordingJson {
            version: 1,
            recording_id: recording.recording_id.to_cli_string(),
            name: recording.name.as_deref(),
            created_at: format_created_at(&recording.created_at),
            duration_ms: recording.duration_ms,
            sessions: recording.sessions,
            operations: recording.operations,
            status: recording.status,
            epoch_count: parent.epoch_count,
            active_epoch: parent.active_epoch,
            published_epoch_count: parent.published_epoch_count,
            warnings: &parent.warnings,
            epochs: parent_plan.epochs,
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

/// Parse checked optional whole-recording duration. Public parsing imposes no
/// arbitrary recording-wide maximum; epoch/WAL bounds remain separate.
fn parse_duration(value: &str) -> Result<Duration, String> {
    chronicle_application::parse_recording_duration(value).map_err(|error| error.to_string())
}

fn replay_exit_code(result: &chronicle_application::ReplaySessionResult) -> i32 {
    chronicle_application::replay_result_exit_code(result)
}

fn error_code(error: &ApplicationError) -> i32 {
    chronicle_application::application_error_exit_code(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_application::{ReplayStatus, ReplayabilityStatus};

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
    fn cli_parses_internal_recorder_status_command() {
        let cli = Cli::try_parse_from([
            "chronicle",
            "internal",
            "recorder-status",
            "--state-root",
            "state",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Internal(InternalCommand::RecorderStatus { state_root })
                if state_root.as_path() == std::path::Path::new("state")
        ));
    }

    #[test]
    fn cli_parses_internal_foreground_recorder_command() {
        let cli = Cli::try_parse_from([
            "chronicle",
            "internal",
            "recorder",
            "--config",
            "recorder.toml",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Internal(InternalCommand::Recorder)
        ));
        assert_eq!(cli.config, Some(PathBuf::from("recorder.toml")));
    }

    #[test]
    fn cli_parses_internal_record_fixture_and_etl() {
        let record = Cli::try_parse_from([
            "chronicle",
            "internal",
            "record-fixture",
            "--input",
            "fixture.json",
            "--root",
            "root",
        ])
        .unwrap();
        assert!(matches!(
            record.command,
            Command::Internal(InternalCommand::RecordFixture { .. })
        ));

        let etl = Cli::try_parse_from([
            "chronicle",
            "internal",
            "etl",
            "--wal-dir",
            "recording",
            "--output",
            "output",
        ])
        .unwrap();
        assert!(matches!(
            etl.command,
            Command::Internal(InternalCommand::Etl { wal_dir, output })
                if wal_dir.as_path() == std::path::Path::new("recording")
                    && output.as_path() == std::path::Path::new("output")
        ));
    }

    #[test]
    fn record_source_flag_is_rejected() {
        // The pre-0.1 record --source entrypoints were removed entirely;
        // clap rejects any --source form.
        assert!(
            Cli::try_parse_from([
                "chronicle",
                "record",
                "--source",
                "fixture",
                "--input",
                "fixture.json",
                "--root",
                "root",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "chronicle",
                "record",
                "--source",
                "ebpf",
                "--cgroup",
                "/workload",
            ])
            .is_err()
        );
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
    fn production_record_arguments_reject_bad_selectors() {
        let mixed = Cli::try_parse_from([
            "chronicle",
            "record",
            "--pid",
            "42",
            "--cgroup",
            "/workload",
        ])
        .unwrap();
        assert!(validate_record_arguments(&mixed).is_err());

        let no_mode = Cli::try_parse_from(["chronicle", "record", "--name", "x"]).unwrap();
        assert!(validate_record_arguments(&no_mode).is_err());
    }

    #[test]
    fn public_record_v1_human_and_json_are_stable() {
        let result = CommandRecordResult {
            recording_id: chronicle_application::RecordingId(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            ),
            name: None,
            status: RecordingStatus::Completed,
            shutdown_reason: Some(ShutdownReason::DurationLimit),
            child_exit: None,
            cleanup: CleanupOutcome::Clean,
            duration_ms: 62_000,
            operations: 3,
            counters: RecordingCounters::default(),
        };
        assert_eq!(
            render_command_record(&result, Format::Human).unwrap(),
            "Recording complete.\n  id: rec_00000000-0000-0000-0000-000000000001\n  duration: 1m 2s\n  operations: 3\n  dropped: 0\nTry:\n  chronicle inspect rec_00000000-0000-0000-0000-000000000001\n  chronicle replay rec_00000000-0000-0000-0000-000000000001 -- COMMAND..."
        );
        let json: serde_json::Value =
            serde_json::from_str(&render_command_record(&result, Format::Json).unwrap()).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(
            json["recording_id"],
            "rec_00000000-0000-0000-0000-000000000001"
        );
        assert!(json["name"].is_null());
        assert_eq!(json["status"], "completed");
        assert_eq!(json["shutdown_reason"], "duration_limit");
        assert_eq!(json["duration_ms"], 62_000);
        assert_eq!(json["operations"], 3);
        assert_eq!(json["dropped"], 0);
        assert!(json["child_exit"].is_null());
        assert_eq!(json["counters"]["committed"]["records"], 0);

        let mut with_child = result.clone();
        with_child.child_exit = Some(ChildExitResult::ExitCode { code: 42 });
        let child_json: serde_json::Value =
            serde_json::from_str(&render_command_record(&with_child, Format::Json).unwrap())
                .unwrap();
        assert_eq!(child_json["child_exit"]["kind"], "exit_code");
        assert_eq!(child_json["child_exit"]["code"], 42);
        assert!(
            render_command_record(&with_child, Format::Human)
                .unwrap()
                .contains("child: exit_code 42")
        );

        let mut lossy = result;
        lossy.name = Some("escaped\nname".into());
        lossy.counters.kernel_or_backend_dropped.records = 2;
        lossy.counters.rejected_due_to_quota.records = 3;
        let lossy_json = render_command_record(&lossy, Format::Json).unwrap();
        assert_eq!(lossy_json.lines().count(), 1);
        assert!(lossy_json.contains("escaped\\nname"));
        let lossy_json: serde_json::Value = serde_json::from_str(&lossy_json).unwrap();
        assert_eq!(lossy_json["dropped"], 5);
        assert_eq!(
            lossy_json["counters"]["kernel_or_backend_dropped"]["records"],
            2
        );
    }

    #[test]
    fn public_replay_human_pass_and_fail_are_stable() {
        let result =
            |outcome, attempted, completed, failed| chronicle_application::ReplaySessionResult {
                session_id: "session".into(),
                replayability: ReplayabilityStatus::FullyReplayable,
                outcome,
                dry_run: outcome == ReplayStatus::DryRun,
                preflight_denied: outcome == ReplayStatus::StoppedPolicy,
                transport_failed: outcome == ReplayStatus::StoppedTransport,
                counts: chronicle_application::ReplayCounts {
                    attempted,
                    completed,
                    failed,
                    ..chronicle_application::ReplayCounts::default()
                },
                operations: Vec::new(),
            };
        let passed = result(ReplayStatus::Completed, 1, 1, 0);
        let passed_text = render_public_replay_human(&passed, &passed, None);
        assert!(passed_text.contains(
            "operations: attempted=1 completed=1 failed=0 unattempted=0 verification_failed=0"
        ));
        assert!(passed_text.ends_with("✓ passed\nReplay passed."));

        let mut failed = result(ReplayStatus::StoppedVerification, 1, 1, 0);
        failed.counts.verification_failed = 1;
        let failed_text = render_public_replay_human(&failed, &failed, None);
        assert!(failed_text.contains(
            "operations: attempted=1 completed=1 failed=0 unattempted=0 verification_failed=1"
        ));
        assert!(failed_text.ends_with("✗ failed\nReplay failed."));
        let orphaned = render_public_replay_human(
            &passed,
            &passed,
            Some(&CleanupOutcome::TimedOut { remaining: vec![7] }),
        );
        assert!(orphaned.ends_with("✗ failed\nReplay failed."));
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
    fn duration_parser_accepts_optional_uncapped_deadlines() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_mins(5));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_hours(1));
        assert_eq!(parse_duration("24h").unwrap(), Duration::from_hours(24));
        assert_eq!(
            parse_duration("1500ms").unwrap(),
            Duration::from_millis(1500)
        );
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("1w").is_err());
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
        assert!(help.contains("Quick start:"));
        assert!(help.contains("chronicle record -- COMMAND..."));
        assert!(help.contains("chronicle inspect latest"));
        assert!(!help.contains("HTTP fixtures"));
        // Legacy mechanics are hidden from public help.
        assert!(!help.contains("recorder-status"));
        assert!(!help.contains("etl"));
        assert!(!help.contains("--wal-dir"));
    }
}
