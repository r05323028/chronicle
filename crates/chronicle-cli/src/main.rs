use chronicle_application::{
    AppConfig, ApplicationError, inspect_session, record_fixture_file, render_inspect_human,
    render_inspect_json, render_replay_human, render_replay_json, replay_session_with_plan,
};
use chronicle_protocol::{ProtocolError, TransportErrorCategory};
use chronicle_replay::{LoopbackReplayOptions, ReplayError, TimingMode};
use clap::{Parser, Subcommand, ValueEnum};
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
            println!("{output}");
            std::process::exit(code);
        }
        Err(error) => {
            if matches!(format, Format::Json) {
                eprintln!(
                    "{}",
                    serde_json::json!({"code": error_code(&error), "message": error.to_string()})
                );
            } else {
                eprintln!("{error}");
            }
            std::process::exit(error_code(&error));
        }
    }
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
                Format::Human => format!("session_id: {}\nroot: {}", result.session_id, root.display()),
                Format::Json => serde_json::json!({"version": 1, "session_id": result.session_id.to_string(), "root": root}).to_string(),
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
                |next_plan| match cli.format {
                    Format::Human => println!("plan:\n{}", render_replay_human(next_plan)),
                    Format::Json => plan = Some(next_plan.clone()),
                },
            )
            .await?;
            let output = match cli.format {
                Format::Human => render_replay_human(&result),
                Format::Json => serde_json::json!({
                    "version": 1,
                    "plan": plan.expect("replay plan callback must run"),
                    "result": serde_json::from_str::<serde_json::Value>(&render_replay_json(&result)?)?,
                })
                .to_string(),
            };
            Ok((output, replay_exit_code(&result)))
        }
        Command::Etl => Err(ApplicationError::NotImplemented("etl")),
        Command::Doctor => Ok((
            "configuration checks passed; external probes not implemented".into(),
            0,
        )),
    }
}

fn parse_session_id(value: &str) -> Result<chronicle_common::SessionId, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .map(chronicle_common::SessionId)
        .map_err(|_| ApplicationError::InvalidConfig("session ID must be a UUID".into()))
}

fn replay_exit_code(result: &chronicle_application::ReplaySessionResult) -> i32 {
    if result.dry_run {
        0
    } else if result.preflight_denied {
        4
    } else if result.transport_failed {
        5
    } else if result.operations.iter().any(|operation| {
        matches!(
            operation.verification.as_str(),
            "failed" | "inconclusive" | "unsupported"
        )
    }) {
        6
    } else {
        0
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
        let result = |dry_run, preflight_denied, transport_failed, verification: &str| {
            chronicle_application::ReplaySessionResult {
                session_id: "session".into(),
                dry_run,
                preflight_denied,
                transport_failed,
                operations: vec![chronicle_application::ReplayOperationSummary {
                    operation_id: "operation".into(),
                    decision: "allowed".into(),
                    attempted: false,
                    verification: verification.into(),
                    category: String::new(),
                    transport_error: String::new(),
                }],
            }
        };

        assert_eq!(replay_exit_code(&result(true, true, true, "failed")), 0);
        assert_eq!(replay_exit_code(&result(false, true, false, "skipped")), 4);
        assert_eq!(replay_exit_code(&result(false, false, true, "skipped")), 5);
        assert_eq!(replay_exit_code(&result(false, false, false, "failed")), 6);
        assert_eq!(replay_exit_code(&result(false, false, false, "passed")), 0);
    }
}
