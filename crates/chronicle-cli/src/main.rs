use chronicle_application::{AppConfig, ApplicationCommand, ChronicleApplication, ReplayTiming};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "chronicle",
    about = "Production-native traffic capture and replay"
)]
struct Cli {
    /// TOML configuration file. Secrets should be referenced through environment variables.
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Scaffold boundary; recording is not implemented.
    Record,
    /// Scaffold boundary; ETL command wiring is not implemented.
    Etl,
    /// Scaffold boundary; replay command wiring is not implemented.
    Replay {
        session_id: String,
        #[arg(long, value_enum, default_value_t = TimingArg::Preserve)]
        timing: TimingArg,
    },
    /// Scaffold boundary; inspect command wiring is not implemented.
    Inspect { session_id: String },
    /// Validate local configuration; external probes are not implemented.
    Doctor,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum TimingArg {
    Preserve,
    Asap,
}

impl From<TimingArg> for ReplayTiming {
    fn from(value: TimingArg) -> Self {
        match value {
            TimingArg::Preserve => Self::Preserve,
            TimingArg::Asap => Self::Asap,
        }
    }
}

impl From<Command> for ApplicationCommand {
    fn from(value: Command) -> Self {
        match value {
            Command::Record => Self::Record,
            Command::Etl => Self::Etl,
            Command::Replay { session_id, timing } => Self::Replay {
                session_id,
                timing: timing.into(),
            },
            Command::Inspect { session_id } => Self::Inspect { session_id },
            Command::Doctor => Self::Doctor,
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init()?;
    let cli = Cli::parse();
    let config = match cli.config {
        Some(path) => AppConfig::from_file(path)?,
        None => AppConfig::default(),
    };
    let command = cli.command.into();
    let output = ChronicleApplication::new(config).execute(&command)?;
    println!("{output}");
    Ok(())
}
