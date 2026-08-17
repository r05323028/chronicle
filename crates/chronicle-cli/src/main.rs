use chronicle_application::escape_control;
#[cfg(target_os = "linux")]
use chronicle_application::record_selector;
use chronicle_application::{
    AppConfig, ApplicationError, ChildStdio, CleanupOutcome, CommandReplayOptions, DoctorProbe,
    DoctorReport, DoctorStatus, REPLAY_REPORT_VERSION, build_parent_replay_plan,
    data_dir_doctor_probe, inspect_session, list_parent_recording_views, list_recordings,
    replay_command, replay_config_doctor_probe, replay_parent_session_with_plan,
    replay_session_with_plan, resolve_data_dir, resolve_recording, resolve_session,
    retry_recording,
};
#[cfg(target_os = "linux")]
use chronicle_application::{
    CaptureMode, CgroupSelector, CommandRecordOptions, ProductionSignalStop, RecordingLifetime,
    RunStopPolicy, record_command,
};
use chronicle_application::{ReplayRequest, ReplayTiming};
use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum, error::ErrorKind};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing_subscriber::EnvFilter;

mod args;
mod dispatch;
mod internal;
mod render;
mod signals;

use args::{
    Cli, Command, DoctorArgs, Format, InspectArgs, InternalCommand, RecordArgs, ReplayArgs, Timing,
};
use dispatch::{error_code, run, validate_record_arguments, write_output};
use render::render_error;

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
            let output = render_error(&error, code, format);
            let _ = write_output(io::stderr().lock(), &output);
            std::process::exit(code);
        }
    }
}

#[cfg(test)]
mod tests;
