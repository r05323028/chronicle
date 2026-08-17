use super::internal;
#[cfg(target_os = "linux")]
use super::render::render_command_record;
use super::render::{
    CommandReplayJson, InspectRecordingJson, ListJson, ListRecordingJson, ReplayJson,
    format_created_at, format_duration_millis, parent_status_human, render_json,
    render_public_replay_human, render_recording_inspect_human, replay_cleanup_json,
};
#[cfg(target_os = "linux")]
use super::signals::spawn_command_signal_watcher;
use super::{
    AppConfig, ApplicationError, ChildStdio, CleanupOutcome, Cli, Command, CommandFactory,
    CommandReplayOptions, DoctorArgs, DoctorProbe, DoctorReport, DoctorStatus, Duration, ErrorKind,
    Format, InspectArgs, InternalCommand, Path, PathBuf, REPLAY_REPORT_VERSION, RecordArgs,
    ReplayArgs, ReplayRequest, Timing, Write, build_parent_replay_plan, data_dir_doctor_probe,
    escape_control, inspect_session, io, list_parent_recording_views, list_recordings,
    replay_command, replay_config_doctor_probe, replay_parent_session_with_plan,
    replay_session_with_plan, resolve_data_dir, resolve_recording, resolve_session,
    retry_recording,
};
#[cfg(target_os = "linux")]
use super::{
    CaptureMode, CgroupSelector, CommandRecordOptions, ProductionSignalStop, RecordingLifetime,
    RunStopPolicy, record_command, record_selector,
};
#[cfg(target_os = "linux")]
use chronicle_application::RecordingStatus;

pub(super) fn write_output(mut writer: impl Write, output: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(output.len() + 1);
    bytes.extend_from_slice(output.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

pub(super) fn validate_record_arguments(cli: &Cli) -> Result<(), clap::Error> {
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
pub(super) async fn run(cli: Cli) -> Result<(String, i32), ApplicationError> {
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
        Command::Internal(command) => internal::run(command, format, &config, recorder_config_path),
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

fn replay_exit_code(result: &chronicle_application::ReplaySessionResult) -> i32 {
    chronicle_application::replay_result_exit_code(result)
}

pub(super) fn error_code(error: &ApplicationError) -> i32 {
    chronicle_application::application_error_exit_code(error)
}
