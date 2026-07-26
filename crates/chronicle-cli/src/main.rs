use chronicle_application::{
    AppConfig, ApplicationError, REPLAY_REPORT_VERSION, inspect_session, record_fixture_file,
    render_inspect_human, render_inspect_json, render_json, render_replay_human,
    replay_session_with_plan,
};
use chronicle_protocol::{ProtocolError, TransportErrorCategory};
use chronicle_replay::{LoopbackReplayOptions, ReplayError, ReplayOutcome, TimingMode};
use clap::{Parser, Subcommand, ValueEnum};
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
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        root: PathBuf,
    },
    /// Explicit scaffold boundary; record performs ETL internally.
    Etl,
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
    Doctor,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Source {
    Fixture,
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
struct ReplayJson<'a> {
    version: u16,
    plan: &'a chronicle_application::ReplaySessionResult,
    result: &'a chronicle_application::ReplaySessionResult,
}

#[derive(Serialize)]
struct DoctorJson<'a> {
    version: u8,
    status: &'a str,
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

fn write_output(mut writer: impl Write, output: &str) -> io::Result<()> {
    let mut bytes = Vec::with_capacity(output.len() + 1);
    bytes.extend_from_slice(output.as_bytes());
    bytes.push(b'\n');
    writer.write_all(&bytes)?;
    writer.flush()
}

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
        } => {
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
        Command::Etl => Err(ApplicationError::NotImplemented("etl")),
        Command::Doctor => {
            let status = "configuration checks passed; external probes not implemented";
            Ok((
                match cli.format {
                    Format::Human => status.into(),
                    Format::Json => render_json(&DoctorJson { version: 1, status })?,
                },
                0,
            ))
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
            outcome,
            dry_run: outcome == ReplayOutcome::DryRun,
            preflight_denied: outcome == ReplayOutcome::StoppedPolicy,
            transport_failed: outcome == ReplayOutcome::StoppedTransport,
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
