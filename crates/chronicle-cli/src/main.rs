use chronicle_application::{
    AppConfig, ApplicationError, REPLAY_REPORT_VERSION, inspect_session,
    process_and_publish_recording_wal, record_fixture_file, render_inspect_human,
    render_inspect_json, render_json, render_replay_human, replay_session_with_plan,
};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use chronicle_application::{
    CgroupSelector, ProductionRecordingBounds, ProductionSignalStop, load_recording_metadata,
    mark_recording_forced_termination, record_live_ebpf, recording_physical_wal_bytes,
};
use chronicle_protocol::{ProtocolError, TransportErrorCategory};
use chronicle_replay::{LoopbackReplayOptions, ReplayError, ReplayOutcome, TimingMode};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use serde::Serialize;
use std::io::{self, Write};
use std::path::PathBuf;
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
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    format: Format,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Record {
        #[arg(long, value_enum)]
        source: Source,
        #[arg(long, required_if_eq("source", "fixture"))]
        input: Option<PathBuf>,
        #[arg(long, required_if_eq("source", "fixture"))]
        root: Option<PathBuf>,
        #[arg(long, required_if_eq("source", "ebpf"))]
        wal_dir: Option<PathBuf>,
        #[arg(long)]
        pid: Option<u32>,
        #[arg(
            long,
            help = "Host cgroup path; capture attaches exact selected subtree"
        )]
        cgroup: Option<PathBuf>,
        #[arg(
            long,
            requires = "cgroup",
            help = "Acknowledge shared scope based on direct cgroup TGID count or descendants"
        )]
        allow_shared_cgroup: bool,
        #[arg(long)]
        segment_bytes: Option<u64>,
        #[arg(long)]
        duration_seconds: Option<u64>,
        #[arg(long)]
        max_wal_bytes: Option<u64>,
    },
    /// Canonicalize and publish one finalized recording WAL.
    Etl {
        #[arg(long)]
        wal_dir: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    Replay {
        session_id: String,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        target: String,
        #[arg(long = "allow-host", required = true)]
        allow_hosts: Vec<String>,
        #[arg(long)]
        allow_read: bool,
        #[arg(long)]
        allow_write: bool,
        #[arg(long, value_enum, default_value_t = Timing::Asap)]
        timing: Timing,
        #[arg(long)]
        execute: bool,
    },
    Inspect {
        session_id: String,
        #[arg(long)]
        root: PathBuf,
    },
    /// Validate local configuration only; no external probes.
    Doctor {
        #[arg(long)]
        wal_dir: Option<PathBuf>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
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

#[derive(Serialize)]
struct RecordJson {
    version: u8,
    session_id: String,
    root: PathBuf,
}

#[derive(Serialize)]
struct EtlJson {
    version: u8,
    session_id: String,
    output: PathBuf,
    already_processed: bool,
    ignored_post_commit_records: u64,
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

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
    let cli = Cli::parse();
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
    let Command::Record {
        source,
        input,
        root,
        wal_dir,
        pid,
        cgroup,
        allow_shared_cgroup,
        segment_bytes,
        duration_seconds,
        max_wal_bytes,
    } = &cli.command
    else {
        return Ok(());
    };
    let invalid = match source {
        Source::Fixture => {
            wal_dir.is_some()
                || pid.is_some()
                || cgroup.is_some()
                || *allow_shared_cgroup
                || segment_bytes.is_some()
                || duration_seconds.is_some()
                || max_wal_bytes.is_some()
        }
        Source::Ebpf => {
            input.is_some()
                || root.is_some()
                || (pid.is_some() == cgroup.is_some())
                || (*allow_shared_cgroup && cgroup.is_none())
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
    let config = match cli.config {
        Some(path) => AppConfig::from_file(path)?,
        None => AppConfig::default(),
    };
    match cli.command {
        Command::Record {
            source: Source::Fixture,
            input,
            root,
            wal_dir,
            pid,
            cgroup,
            allow_shared_cgroup,
            segment_bytes,
            duration_seconds,
            max_wal_bytes,
        } => {
            if wal_dir.is_some()
                || pid.is_some()
                || cgroup.is_some()
                || allow_shared_cgroup
                || segment_bytes.is_some()
                || duration_seconds.is_some()
                || max_wal_bytes.is_some()
            {
                return Err(ApplicationError::ProductionPreflight(
                    "eBPF-only record options used with fixture source",
                ));
            }
            let input = input.ok_or(ApplicationError::ProductionPreflight(
                "fixture input missing",
            ))?;
            let root = root.ok_or(ApplicationError::ProductionPreflight(
                "fixture root missing",
            ))?;
            let result = record_fixture_file(input, &root, config.wal.segment_size_bytes)?;
            let output = match cli.format {
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
        Command::Record {
            source: Source::Ebpf,
            input,
            root,
            wal_dir,
            pid,
            cgroup,
            allow_shared_cgroup,
            segment_bytes,
            duration_seconds,
            max_wal_bytes,
        } => {
            if input.is_some() || root.is_some() {
                return Err(ApplicationError::ProductionPreflight(
                    "fixture-only record options used with eBPF source",
                ));
            }
            #[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
            {
                let selector = match (pid, cgroup) {
                    (Some(pid), None) => CgroupSelector::Pid(pid),
                    (None, Some(cgroup)) => CgroupSelector::Explicit(cgroup),
                    _ => {
                        return Err(ApplicationError::ProductionPreflight(
                            "select exactly one of PID or cgroup",
                        ));
                    }
                };
                let wal_dir = wal_dir.ok_or(ApplicationError::ProductionPreflight(
                    "WAL directory missing",
                ))?;
                let bounds = ProductionRecordingBounds {
                    duration_seconds: duration_seconds.unwrap_or(600),
                    segment_bytes: segment_bytes.unwrap_or(config.wal.segment_size_bytes),
                    max_wal_bytes: max_wal_bytes.unwrap_or(4 * 1024 * 1024 * 1024),
                };
                let stop = ProductionSignalStop::default();
                spawn_signal_watcher(stop.clone(), wal_dir.clone());
                let result =
                    record_live_ebpf(selector, allow_shared_cgroup, &wal_dir, bounds, stop)?;
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
                let output = match cli.format {
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
            {
                let _ = (
                    wal_dir,
                    pid,
                    cgroup,
                    allow_shared_cgroup,
                    segment_bytes,
                    duration_seconds,
                    max_wal_bytes,
                );
                Err(ApplicationError::ProductionPreflight(
                    "Linux eBPF capture unavailable",
                ))
            }
        }
        Command::Inspect { session_id, root } => {
            let id = parse_session_id(&session_id)?;
            let result = inspect_session(root, id)?;
            let output = match cli.format {
                Format::Human => render_inspect_human(&result),
                Format::Json => render_inspect_json(&result)?,
            };
            Ok((output, 0))
        }
        Command::Replay {
            session_id,
            root,
            target,
            allow_hosts,
            allow_read,
            allow_write,
            timing,
            execute,
        } => {
            let options = LoopbackReplayOptions {
                target: Some(target),
                allow_hosts,
                execute,
                allow_reads: allow_read,
                allow_writes: allow_write,
                timing: timing.into(),
            };
            let mut plan = None;
            let result = replay_session_with_plan(
                root,
                &session_id,
                &config.replay,
                &options,
                |next_plan| plan = Some(next_plan.clone()),
            )
            .await?;
            let plan = plan.expect("replay plan callback must run");
            let output = match cli.format {
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
        Command::Etl { wal_dir, output } => {
            let result = process_and_publish_recording_wal(
                &wal_dir,
                &output,
                &chronicle_protocol_builtins::registry()?,
            )?;
            let rendered = match cli.format {
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
                    output,
                    already_processed: result.already_published,
                    ignored_post_commit_records: result.ignored_post_commit_records,
                })?,
            };
            Ok((rendered, 0))
        }
        Command::Doctor { wal_dir, output } => {
            let report =
                chronicle_application::doctor_report(&chronicle_application::DoctorOptions {
                    wal_dir,
                    output,
                });
            let rendered = match cli.format {
                Format::Human => report
                    .probes
                    .iter()
                    .map(|probe| format!("{}: {:?} — {}", probe.code, probe.status, probe.message))
                    .collect::<Vec<_>>()
                    .join("\n"),
                Format::Json => render_json(&report)?,
            };
            Ok((rendered, report.exit_code()))
        }
    }
}

fn parse_session_id(value: &str) -> Result<chronicle_common::SessionId, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .map(chronicle_common::SessionId)
        .map_err(|_| ApplicationError::InvalidConfig("session ID must be a UUID".into()))
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
        ApplicationError::ReplayTarget(_)
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
        let Command::Replay {
            timing, execute, ..
        } = replay.command
        else {
            panic!("replay command expected")
        };
        assert!(matches!(timing, Timing::Asap));
        assert!(!execute);
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
        let Command::Record {
            source,
            input,
            root,
            wal_dir,
            pid,
            cgroup,
            allow_shared_cgroup,
            duration_seconds,
            segment_bytes,
            max_wal_bytes,
        } = cli.command
        else {
            panic!("record command expected");
        };
        assert!(matches!(source, Source::Ebpf));
        assert!(input.is_none() && root.is_none() && pid.is_none());
        assert_eq!(cgroup, Some(PathBuf::from("/workload")));
        assert!(allow_shared_cgroup);
        assert_eq!(wal_dir, Some(PathBuf::from("wal")));
        assert_eq!(duration_seconds, Some(60));
        assert_eq!(segment_bytes, Some(16_777_216));
        assert_eq!(max_wal_bytes, Some(33_554_432));
    }

    #[test]
    fn production_record_help_uses_direct_tgid_scope_terms() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("record")
            .expect("record subcommand")
            .render_long_help()
            .to_string();
        assert!(help.contains("direct cgroup TGID count"));
        assert!(help.contains("selected subtree"));
        assert!(!help.contains("POSIX PGID"));
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
        ]);
        assert!(pid_with_acknowledgement.is_err());
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
}
