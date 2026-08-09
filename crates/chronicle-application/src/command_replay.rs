//! Command-mode replay orchestration: pre-plan before spawn, then one owned
//! cgroup-v2 scope, hardened target bootstrap, stable listener discovery,
//! existing replay planner/executor/verifier, and bounded scope cleanup.

use crate::{
    ApplicationError, ChildExitResult, ChildStdio, CleanupOutcome, ReplayConfig,
    ReplaySessionResult, command_replay_plan, hydrate_replay_session, replay_plan_result,
};
use std::path::PathBuf;

pub const COMMAND_REPLAY_SCOPE_PREFIX: &str = "rp-";

#[derive(Clone, Debug)]
pub struct CommandReplayOptions {
    pub command: Vec<String>,
    pub data_dir: PathBuf,
    pub session_id: String,
    pub allow_writes: bool,
    pub child_stdout: ChildStdio,
    pub child_stderr: ChildStdio,
    /// Trusted invoking credentials captured before any elevation.
    pub invoking_uid: u32,
    pub invoking_gid: u32,
}

#[derive(Clone, Debug)]
pub struct CommandReplayResult {
    pub plan: ReplaySessionResult,
    pub replay: ReplaySessionResult,
    pub child_exit: Option<ChildExitResult>,
    pub cleanup: CleanupOutcome,
    pub target_origin: Option<String>,
}

/// Target-independent planning always runs first, including on non-Linux
/// hosts. Predictable policy/invalid-session denial therefore never creates a
/// scope or target process.
pub async fn replay_command(
    options: CommandReplayOptions,
    config: &ReplayConfig,
) -> Result<CommandReplayResult, ApplicationError> {
    if options.command.is_empty() {
        return Err(ApplicationError::InvalidConfig(
            "replay command is empty".into(),
        ));
    }
    let session = hydrate_replay_session(&options.data_dir, &options.session_id)?;
    let preplan = command_replay_plan(&session, options.allow_writes)?;
    let preplan_result = replay_plan_result(session.id.to_string(), &preplan);
    if !preplan.is_executable() {
        return Ok(CommandReplayResult {
            plan: preplan_result.clone(),
            replay: preplan_result,
            child_exit: None,
            cleanup: CleanupOutcome::Clean,
            target_origin: None,
        });
    }
    #[cfg(target_os = "linux")]
    {
        replay_command_after_preplan(options, config, preplan).await
    }
    #[cfg(not(target_os = "linux"))]
    {
        replay_command_after_preplan(options, config, preplan)
    }
}

#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines)] // Linear scope lifecycle keeps every cleanup edge visible.
async fn replay_command_after_preplan(
    options: CommandReplayOptions,
    config: &ReplayConfig,
    preplan: chronicle_replay::ReplayPlan,
) -> Result<CommandReplayResult, ApplicationError> {
    use crate::bootstrap::spawn_blocked_bootstrap;
    use crate::listener_discovery::{
        LISTENER_READINESS_DEADLINE, discover_listener, listener_requirements, revalidate_listener,
    };
    use crate::replay_session_with_plan_guard;
    use crate::supervised_scope::{
        create_supervised_scope, discover_delegated_root, preflight_scope_access,
    };
    use chronicle_replay::{LoopbackReplayOptions, TimingMode};
    use std::process::Stdio;
    use std::time::Instant;

    let requirements = listener_requirements(&preplan);
    if requirements.is_empty() {
        return Err(ApplicationError::ReplayReadiness(
            "recording has no executable HTTP/1.1 listener requirement; use explicit --target mode"
                .into(),
        ));
    }

    let (hierarchy, delegated) = discover_delegated_root(options.invoking_uid)?;
    preflight_scope_access(&hierarchy, &delegated, options.invoking_uid)?;
    let scope_name = format!(
        "{COMMAND_REPLAY_SCOPE_PREFIX}{}",
        uuid::Uuid::new_v4().simple()
    );
    let scope = create_supervised_scope(&hierarchy, &delegated, &scope_name, options.invoking_uid)?;

    let stdout = match options.child_stdout {
        ChildStdio::Inherit => Stdio::inherit(),
        ChildStdio::Null => Stdio::null(),
    };
    let stderr = match options.child_stderr {
        ChildStdio::Inherit => Stdio::inherit(),
        ChildStdio::Null => Stdio::null(),
    };
    let mut child = match spawn_blocked_bootstrap(
        &options.command,
        options.invoking_uid,
        options.invoking_gid,
        stdout,
        stderr,
    ) {
        Ok(child) => child,
        Err(error) => {
            cleanup_empty_scope(&scope)?;
            return Err(error);
        }
    };

    if let Err(error) = scope.move_process(child.pid()) {
        child.abort();
        let cleanup = cleanup_scope(&scope)?;
        let _ = child_result_after_cleanup(&mut child, &cleanup);
        return cleanup_failure_or(cleanup, error.into());
    }
    if let Err(error) = scope.revalidate() {
        let cleanup = cleanup_scope(&scope)?;
        let _ = child_result_after_cleanup(&mut child, &cleanup);
        return cleanup_failure_or(cleanup, error.into());
    }
    if let Err(error) = child.release() {
        let cleanup = cleanup_scope(&scope)?;
        let _ = child_result_after_cleanup(&mut child, &cleanup);
        return cleanup_failure_or(cleanup, error);
    }

    let discovered = match discover_listener(
        &scope,
        std::path::Path::new("/proc"),
        &requirements,
        Instant::now() + LISTENER_READINESS_DEADLINE,
    ) {
        Ok(discovered) => discovered,
        Err(error) => {
            let cleanup = cleanup_scope(&scope)?;
            let _ = child_result_after_cleanup(&mut child, &cleanup);
            return cleanup_failure_or(cleanup, error.into());
        }
    };
    let origin = discovered.origin();
    let inferred = LoopbackReplayOptions {
        target: Some(origin.clone()),
        allow_hosts: vec![discovered.host()],
        execute: true,
        allow_reads: true,
        allow_writes: options.allow_writes,
        timing: TimingMode::Asap,
    };
    let mut final_plan = None;
    let replay = replay_session_with_plan_guard(
        &options.data_dir,
        &options.session_id,
        config,
        &inferred,
        |plan| final_plan = Some(plan.clone()),
        || {
            revalidate_listener(
                &scope,
                std::path::Path::new("/proc"),
                &requirements,
                &discovered,
            )?;
            Ok(())
        },
    )
    .await;

    let cleanup = cleanup_scope(&scope)?;
    let child_exit = child_result_after_cleanup(&mut child, &cleanup);
    let replay = match replay {
        Ok(replay) => replay,
        Err(error) => return cleanup_failure_or(cleanup, error),
    };
    let plan = final_plan.expect("replay plan callback must run");
    Ok(CommandReplayResult {
        plan,
        replay,
        child_exit,
        cleanup,
        target_origin: Some(origin),
    })
}

#[cfg(not(target_os = "linux"))]
fn replay_command_after_preplan(
    _options: CommandReplayOptions,
    _config: &ReplayConfig,
    _preplan: chronicle_replay::ReplayPlan,
) -> Result<CommandReplayResult, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode replay requires Linux cgroup v2; use portable --target mode",
    ))
}

#[cfg(target_os = "linux")]
fn cleanup_empty_scope(scope: &crate::SupervisedScope) -> Result<(), ApplicationError> {
    scope.destroy().map_err(Into::into)
}

#[cfg(target_os = "linux")]
fn cleanup_scope(scope: &crate::SupervisedScope) -> Result<CleanupOutcome, ApplicationError> {
    scope
        .cleanup(&crate::RealClock, crate::SCOPE_POLL_INTERVAL)
        .map_err(|error| {
            ApplicationError::InvalidConfig(format!(
                "supervised replay cleanup failed; possible orphan state: {error}"
            ))
        })
}

#[cfg(target_os = "linux")]
fn child_result_after_cleanup(
    child: &mut crate::bootstrap::BlockedBootstrap,
    cleanup: &CleanupOutcome,
) -> Option<ChildExitResult> {
    match cleanup {
        CleanupOutcome::TimedOut { .. } => child.try_result(),
        CleanupOutcome::Clean | CleanupOutcome::Killed => child.wait_result(),
    }
}

#[cfg(target_os = "linux")]
fn cleanup_failure_or<T>(
    cleanup: CleanupOutcome,
    error: ApplicationError,
) -> Result<T, ApplicationError> {
    match cleanup {
        CleanupOutcome::TimedOut { remaining } => Err(ApplicationError::InvalidConfig(format!(
            "supervised replay cleanup timed out; possible orphan pids: {remaining:?}; original failure: {error}"
        ))),
        CleanupOutcome::Clean | CleanupOutcome::Killed => Err(error),
    }
}
