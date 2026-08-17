use super::{Args, Duration, Parser, PathBuf, ReplayTiming, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    name = "chronicle",
    version,
    about = "Record, inspect, and safely replay local application behavior",
    long_about = "Record production-like HTTP behavior as portable regression tests.\n\nQuick start:\n  chronicle record -- COMMAND...\n  chronicle list\n  chronicle inspect latest\n  chronicle replay latest -- COMMAND..."
)]
pub(super) struct Cli {
    /// TOML configuration file. Secrets must be referenced through environment variables.
    #[arg(long, global = true)]
    pub(super) config: Option<PathBuf>,
    /// Public data directory (recordings, catalog, sessions). Overrides
    /// configured `data_dir` and `CHRONICLE_DATA_DIR`; not an alias of legacy `--root`.
    #[arg(long, global = true)]
    pub(super) data_dir: Option<PathBuf>,
    #[arg(long, global = true, value_enum, default_value_t = Format::Human)]
    pub(super) format: Format,
    #[command(subcommand)]
    pub(super) command: Command,
}

#[derive(Debug, Subcommand)]
pub(super) enum Command {
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
pub(super) enum InternalCommand {
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
pub(super) struct RecordArgs {
    /// Command to run and record, after `--`.
    #[arg(last = true)]
    pub(super) command: Vec<String>,
    /// Optional human-readable recording name (exact UTF-8, 1-128 bytes).
    #[arg(long)]
    pub(super) name: Option<String>,
    /// Optional whole-recording deadline, e.g. `10m`, `24h`, or plain seconds.
    /// Omitted means run until source completion, explicit stop, or fatal failure.
    #[arg(long, value_parser = parse_duration)]
    pub(super) duration: Option<Duration>,
    /// Retry recovery/finalization/publication for a recoverable recording.
    #[arg(long)]
    pub(super) retry: Option<String>,
    /// Record an already-running process by PID (never terminated).
    #[arg(long)]
    pub(super) pid: Option<u32>,
    /// Record an already-running cgroup subtree (never terminated).
    #[arg(long)]
    pub(super) cgroup: Option<PathBuf>,
    /// Acknowledge a shared cgroup subtree (more than one direct TGID).
    #[arg(long, hide = true)]
    pub(super) allow_shared_cgroup: bool,
}

#[derive(Debug, Args)]
pub(super) struct ReplayArgs {
    /// Recording to replay: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    pub(super) recording: String,
    /// Explicit epoch ordinal for multi-epoch parent replay.
    #[arg(long)]
    pub(super) epoch: Option<u64>,
    /// Command to spawn and replay against, after `--` (command mode).
    #[arg(
        last = true,
        conflicts_with_all = ["target", "allow_hosts", "allow_read", "execute"]
    )]
    pub(super) command: Vec<String>,
    /// Explicit loopback target for an already-running application (target mode).
    #[arg(long)]
    pub(super) target: Option<String>,
    /// Exact allow-host (loopback IP literal), repeatable.
    #[arg(long = "allow-host")]
    pub(super) allow_hosts: Vec<String>,
    /// Authorize read effects in explicit target mode.
    #[arg(long)]
    pub(super) allow_read: bool,
    /// Authorize write effects.
    #[arg(long)]
    pub(super) allow_write: bool,
    /// Execute (explicit target mode only).
    #[arg(long)]
    pub(super) execute: bool,
    /// Replay timing control (advanced).
    #[arg(long, hide = true, value_enum)]
    pub(super) timing: Option<Timing>,
}

#[derive(Debug, Args)]
pub(super) struct InspectArgs {
    /// Recording to inspect: `latest`, `rec_<uuid>`, bare UUID, or exact name.
    pub(super) recording: String,
}

#[derive(Debug, Args)]
pub(super) struct DoctorArgs {
    // Hidden advanced diagnostic probes.
    #[arg(long, hide = true)]
    pub(super) wal_dir: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub(super) output: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub(super) state_root: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum Format {
    Human,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(super) enum Timing {
    Preserve,
    Asap,
}

pub(super) fn parse_duration(value: &str) -> Result<Duration, String> {
    chronicle_application::parse_recording_duration(value).map_err(|error| error.to_string())
}

impl From<Timing> for ReplayTiming {
    fn from(value: Timing) -> Self {
        match value {
            Timing::Preserve => Self::Preserve,
            Timing::Asap => Self::Asap,
        }
    }
}
