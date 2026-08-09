//! Hidden bootstrap process for command-mode record/replay.
//!
//! The parent spawns `chronicle internal bootstrap --uid U --gid G -- TARGET...`
//! with the readiness pipe read end dup2'd onto fixed fd 3 (see the CLI
//! orchestration). The bootstrap blocks on fd 3; when the parent signals that
//! capture is attached and the WAL writer is durable, it hardens credentials
//! (cleared supplementary groups, real/effective/saved GID and UID, cleared
//! capability sets, `no_new_privs`, reset signal dispositions, closed
//! descriptors) and directly `exec`s the target. No target-executable
//! instruction runs before the readiness signal.
//!
//! Any bootstrap failure is a typed Chronicle failure (exit 3), never a
//! synthesized child exit 127. Linux-only; non-Linux returns the typed
//! unsupported code.

#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::{ErrorKind, Read, Write};
#[cfg(target_os = "linux")]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
#[cfg(target_os = "linux")]
use std::os::unix::process::{CommandExt, ExitStatusExt};
#[cfg(target_os = "linux")]
use std::process::{Child, Command, Stdio};

/// Exit code for bootstrap/hardening/exec failures (typed Chronicle failure).
pub const BOOTSTRAP_FAILURE_EXIT: i32 = 3;
#[cfg(target_os = "linux")]
const BOOTSTRAP_STATUS_FD: i32 = 4;
#[cfg(target_os = "linux")]
const BOOTSTRAP_STATUS_ENV: &str = "CHRONICLE_BOOTSTRAP_STATUS_FD";

/// Target child blocked on Chronicle's readiness pipe before hardening/exec.
#[cfg(target_os = "linux")]
pub(crate) struct BlockedBootstrap {
    child: Child,
    readiness: Option<OwnedFd>,
    status: Option<OwnedFd>,
}

#[cfg(target_os = "linux")]
impl BlockedBootstrap {
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn release(&mut self) -> Result<(), crate::ApplicationError> {
        let readiness = self.readiness.take().ok_or_else(|| {
            crate::ApplicationError::InvalidConfig("bootstrap already released".into())
        })?;
        File::from(readiness).write_all(b"G").map_err(|error| {
            crate::ApplicationError::InvalidConfig(format!("releasing bootstrap: {error}"))
        })
    }

    /// Abort a still-blocked bootstrap even when scope placement failed.
    pub(crate) fn abort(&mut self) {
        self.readiness.take();
        let _ = self.child.kill();
    }

    /// Factual target exit; bootstrap/hardening/exec failure is a Chronicle error.
    pub(crate) fn wait_result(
        &mut self,
    ) -> Result<Option<crate::ChildExitResult>, crate::ApplicationError> {
        let status = self.child.wait()?;
        self.classify_result(status)
    }

    /// Non-blocking factual result for orphan/cleanup-timeout paths.
    pub(crate) fn try_result(
        &mut self,
    ) -> Result<Option<crate::ChildExitResult>, crate::ApplicationError> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        self.classify_result(status)
    }

    fn classify_result(
        &mut self,
        status: std::process::ExitStatus,
    ) -> Result<Option<crate::ChildExitResult>, crate::ApplicationError> {
        let status_pipe = self.status.take().ok_or_else(|| {
            crate::ApplicationError::InvalidConfig("bootstrap result already collected".into())
        })?;
        let mut status_pipe = File::from(status_pipe);
        let mut failure = [0_u8; 1];
        match status_pipe.read(&mut failure) {
            Ok(0) => Ok(child_exit_result(status)),
            Ok(_) => Err(crate::ApplicationError::TargetLaunchFailed),
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(target_os = "linux")]
fn child_exit_result(status: std::process::ExitStatus) -> Option<crate::ChildExitResult> {
    status.code().map_or_else(
        || {
            status
                .signal()
                .map(|signal| crate::ChildExitResult::Signal { signal })
        },
        |code| Some(crate::ChildExitResult::ExitCode { code }),
    )
}

/// Spawn hidden bootstrap with fd 3 as its readiness pipe. Target code cannot
/// run until [`BlockedBootstrap::release`] succeeds.
#[cfg(target_os = "linux")]
pub(crate) fn spawn_blocked_bootstrap(
    command: &[String],
    uid: u32,
    gid: u32,
    stdout: Stdio,
    stderr: Stdio,
) -> Result<BlockedBootstrap, crate::ApplicationError> {
    if command.is_empty() {
        return Err(crate::ApplicationError::InvalidConfig(
            "target command is empty".into(),
        ));
    }
    let (read_end, write_end) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|error| {
            crate::ApplicationError::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
    let (status_read, status_write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)
        .map_err(|error| {
            crate::ApplicationError::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
    let read_end_raw = read_end.as_raw_fd();
    let status_write_raw = status_write.as_raw_fd();
    let current_exe = std::env::current_exe().map_err(|error| {
        crate::ApplicationError::InvalidConfig(format!("current executable unavailable: {error}"))
    })?;
    let mut bootstrap = Command::new(current_exe);
    bootstrap
        .args([
            "internal",
            "bootstrap",
            "--uid",
            &uid.to_string(),
            "--gid",
            &gid.to_string(),
            "--",
        ])
        .args(command)
        .env(BOOTSTRAP_STATUS_ENV, BOOTSTRAP_STATUS_FD.to_string())
        .stdout(stdout)
        .stderr(stderr);
    unsafe {
        bootstrap.pre_exec(move || {
            let source = File::from_raw_fd(read_end_raw);
            let mut target = OwnedFd::from_raw_fd(3);
            let result = if read_end_raw == 3 {
                rustix::io::fcntl_setfd(&target, rustix::io::FdFlags::empty())
            } else {
                rustix::io::dup2(&source, &mut target)
            };
            std::mem::forget(source);
            std::mem::forget(target);
            result.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))?;

            let source = File::from_raw_fd(status_write_raw);
            let mut target = OwnedFd::from_raw_fd(BOOTSTRAP_STATUS_FD);
            let result = if status_write_raw == BOOTSTRAP_STATUS_FD {
                rustix::io::fcntl_setfd(&target, rustix::io::FdFlags::empty())
            } else {
                rustix::io::dup2(&source, &mut target)
            };
            std::mem::forget(source);
            std::mem::forget(target);
            result.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
        });
    }
    let child = bootstrap.spawn().map_err(|error| {
        crate::ApplicationError::InvalidConfig(format!("spawning bootstrap: {error}"))
    })?;
    drop(read_end);
    drop(status_write);
    Ok(BlockedBootstrap {
        child,
        readiness: Some(write_end),
        status: Some(status_read),
    })
}

/// Entry point for the hidden bootstrap subcommand. Returns the process exit
/// code; on success (target exec) it never returns.
#[cfg(target_os = "linux")]
pub fn run_bootstrap(uid: u32, gid: u32, command: &[String]) -> i32 {
    let mut status = bootstrap_status_file();
    if command.is_empty() {
        return bootstrap_failure(&mut status, "no target command");
    }
    if rustix::process::getuid().as_raw() != uid || rustix::process::getgid().as_raw() != gid {
        return bootstrap_failure(
            &mut status,
            "requested credentials do not match inherited real credentials",
        );
    }
    // 1. Block until the parent signals readiness. EOF (parent died before
    //    releasing) or a read failure aborts without running target code.
    if let Err(message) = read_readiness_byte() {
        return bootstrap_failure(&mut status, &format!("readiness signal failed: {message}"));
    }
    // 2. Harden credentials and process state before the target execs.
    if let Err(message) = harden(uid, gid, status.as_ref().map(AsRawFd::as_raw_fd)) {
        return bootstrap_failure(&mut status, &format!("hardening failed: {message}"));
    }
    // 3. Verify the resulting credentials and hardening state before exec.
    if let Err(message) = verify_hardening(uid, gid) {
        return bootstrap_failure(
            &mut status,
            &format!("hardening verification failed: {message}"),
        );
    }
    if let Some(status_file) = status.as_ref()
        && let Err(error) = rustix::io::fcntl_setfd(status_file, rustix::io::FdFlags::CLOEXEC)
    {
        return bootstrap_failure(
            &mut status,
            &format!("setting bootstrap status close-on-exec: {error}"),
        );
    }
    // 4. Directly exec the target; successful exec closes the status pipe.
    let mut target = Command::new(&command[0]);
    target.args(&command[1..]).env_remove(BOOTSTRAP_STATUS_ENV);
    let error = target.exec();
    bootstrap_failure(&mut status, &format!("exec failed: {error}"))
}

#[cfg(target_os = "linux")]
fn bootstrap_status_file() -> Option<File> {
    let requested = std::env::var(BOOTSTRAP_STATUS_ENV)
        .ok()
        .and_then(|value| value.parse::<i32>().ok())
        == Some(BOOTSTRAP_STATUS_FD);
    let descriptor_exists =
        std::fs::symlink_metadata(format!("/proc/self/fd/{BOOTSTRAP_STATUS_FD}")).is_ok();
    if !requested || !descriptor_exists {
        return None;
    }
    // SAFETY: /proc/self/fd confirmed fd 4 exists. Bootstrap is single-threaded
    // and no code can close it between that check and ownership adoption.
    Some(unsafe { File::from_raw_fd(BOOTSTRAP_STATUS_FD) })
}

#[cfg(target_os = "linux")]
fn bootstrap_failure(status: &mut Option<File>, message: &str) -> i32 {
    if let Some(status) = status.as_mut() {
        let _ = status.write_all(b"F");
    }
    eprintln!("chronicle bootstrap: {message}");
    BOOTSTRAP_FAILURE_EXIT
}

#[cfg(not(target_os = "linux"))]
pub fn run_bootstrap(_uid: u32, _gid: u32, _command: &[String]) -> i32 {
    eprintln!("chronicle bootstrap requires Linux");
    BOOTSTRAP_FAILURE_EXIT
}

/// Read one readiness byte from fixed fd 3. `Ok(())` on a received byte;
/// `Err` on EOF or any read error.
#[cfg(target_os = "linux")]
fn read_readiness_byte() -> Result<(), String> {
    let mut file = unsafe { File::from_raw_fd(3) };
    let mut byte = [0_u8; 1];
    match file.read_exact(&mut byte) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
            Err("parent closed the readiness pipe before releasing".into())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Hardening sequence per the record design: clear supplementary groups, set
/// real/effective/saved GID and UID, set `no_new_privs`, clear capability
/// sets, reset signal dispositions, close every descriptor except stdio.
///
/// Steps that require elevated privileges (groups, capability clearing) are
/// best-effort: when the bootstrap already runs with the invoking credentials
/// (no elevation to drop), the kernel rejects them with `EPERM` and the state
/// they would have produced already holds. Every other failure aborts.
#[cfg(target_os = "linux")]
fn harden(uid: u32, gid: u32, preserved_fd: Option<i32>) -> Result<(), String> {
    use rustix::process::{Gid, Uid};
    use rustix::thread::{
        CapabilitySet, CapabilitySets, clear_ambient_capability_set, set_capabilities,
        set_no_new_privs, set_thread_groups, set_thread_res_gid, set_thread_res_uid,
    };

    let user_identity = Uid::from_raw(uid);
    let group_identity = Gid::from_raw(gid);

    // Clear supplementary groups (requires CAP_SETGROUPS; EPERM is acceptable
    // for an already-unprivileged bootstrap, whose groups are the invoking
    // user's own).
    set_thread_groups(&[])
        .map_err(|error| format!("clearing supplementary groups: {error}"))
        .or_else(accept_permission_denied)?;

    // Real/effective/saved GID then UID. These must match exactly.
    set_thread_res_gid(group_identity, group_identity, group_identity)
        .map_err(|error| format!("setting real/effective/saved GID: {error}"))?;
    set_thread_res_uid(user_identity, user_identity, user_identity)
        .map_err(|error| format!("setting real/effective/saved UID: {error}"))?;

    // no_new_privs: file capabilities and setuid binaries cannot elevate the
    // target. Always permitted.
    set_no_new_privs(true).map_err(|error| format!("setting no_new_privs: {error}"))?;

    // Clear every capability set. Effective capabilities were already cleared
    // by the UID drop; clearing inheritable/permitted needs CAP_SETPCAP,
    // which a now-unprivileged bootstrap no longer holds (EPERM accepted).
    let empty = CapabilitySets {
        effective: CapabilitySet::empty(),
        permitted: CapabilitySet::empty(),
        inheritable: CapabilitySet::empty(),
    };
    set_capabilities(None, empty)
        .map_err(|error| format!("clearing capability sets: {error}"))
        .or_else(accept_permission_denied)?;
    clear_ambient_capability_set()
        .map_err(|error| format!("clearing ambient capabilities: {error}"))
        .or_else(accept_permission_denied)?;

    reset_signal_mask()?;
    reset_signal_dispositions()?;
    close_unneeded_descriptors(preserved_fd)?;
    Ok(())
}

/// Accept `EPERM` (permission denied) failures for best-effort hardening
/// steps; propagate everything else.
#[cfg(target_os = "linux")]
fn accept_permission_denied(error: String) -> Result<(), String> {
    if error.contains("Permission denied") || error.contains("Operation not permitted") {
        Ok(())
    } else {
        Err(error)
    }
}

/// Clear the inherited thread signal mask before target exec.
#[cfg(target_os = "linux")]
fn reset_signal_mask() -> Result<(), String> {
    use rustix::runtime::{How, KernelSigSet, kernel_sigprocmask};
    // SAFETY: an empty set contains no libc-reserved signal values; the
    // bootstrap is single-threaded and changes only its current thread mask.
    unsafe { kernel_sigprocmask(How::SETMASK, Some(&KernelSigSet::empty())) }
        .map(|_| ())
        .map_err(|error| format!("resetting signal mask: {error}"))
}

/// Reset every standard signal disposition to `SIG_DFL` so the target does
/// not inherit handlers installed by the Chronicle runtime.
#[cfg(target_os = "linux")]
fn reset_signal_dispositions() -> Result<(), String> {
    use rustix::process::Signal;
    use rustix::runtime::{
        KERNEL_SIG_DFL, KernelSigSet, KernelSigaction, KernelSigactionFlags, kernel_sigaction,
    };
    // Every catchable standard signal, in numeric order; SIGKILL and SIGSTOP
    // cannot be caught or reset and are skipped.
    let signals = [
        Signal::HUP,
        Signal::INT,
        Signal::QUIT,
        Signal::ILL,
        Signal::TRAP,
        Signal::ABORT,
        Signal::BUS,
        Signal::FPE,
        Signal::USR1,
        Signal::SEGV,
        Signal::USR2,
        Signal::PIPE,
        Signal::ALARM,
        Signal::TERM,
        Signal::CHILD,
        Signal::CONT,
        Signal::TSTP,
        Signal::TTIN,
        Signal::TTOU,
        Signal::URG,
        Signal::XCPU,
        Signal::XFSZ,
        Signal::VTALARM,
        Signal::PROF,
        Signal::WINCH,
        Signal::IO,
        Signal::POWER,
    ];
    for signal in signals {
        let action = KernelSigaction {
            sa_handler_kernel: KERNEL_SIG_DFL,
            sa_flags: KernelSigactionFlags::empty(),
            sa_restorer: None,
            sa_mask: KernelSigSet::empty(),
        };
        // SAFETY: resetting dispositions in the single-threaded bootstrap
        // before exec is safe; no handlers are active in this process.
        unsafe {
            kernel_sigaction(signal, Some(action))
                .map_err(|error| format!("resetting signal {signal:?}: {error}"))?;
        }
    }
    Ok(())
}

/// Close every descriptor except stdio and the optional bootstrap status pipe.
/// The readiness fd 3 was already closed after the signal was read.
#[cfg(target_os = "linux")]
fn close_unneeded_descriptors(preserved_fd: Option<i32>) -> Result<(), String> {
    let entries = match std::fs::read_dir("/proc/self/fd") {
        Ok(entries) => entries,
        Err(error) => return Err(format!("listing descriptors: {error}")),
    };
    let mut descriptors = Vec::new();
    for entry in entries.flatten() {
        if let Ok(number) = entry.file_name().to_string_lossy().parse::<i32>()
            && !(0..=2).contains(&number)
            && Some(number) != preserved_fd
        {
            descriptors.push(number);
        }
    }
    // The iterator (and its directory fd) is dropped here, closing it. The
    // existence check skips descriptors that were closed concurrently (the
    // directory fd), because a second close would return EBADF.
    for descriptor in descriptors {
        if !std::path::Path::new(&format!("/proc/self/fd/{descriptor}")).exists() {
            continue;
        }
        // SAFETY: the descriptor is listed in /proc/self/fd and is still open.
        unsafe { rustix::io::close(descriptor) };
    }
    Ok(())
}

/// Verify credentials and every security-sensitive hardening state before exec.
#[cfg(target_os = "linux")]
fn verify_hardening(uid: u32, gid: u32) -> Result<(), String> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|error| format!("reading process status: {error}"))?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
            .ok_or_else(|| format!("missing {name} status field"))
    };
    let uid_text = uid.to_string();
    if field("Uid:")?
        .split_whitespace()
        .any(|value| value != uid_text)
    {
        return Err("real/effective/saved/filesystem UID mismatch".into());
    }
    let gid_text = gid.to_string();
    if field("Gid:")?
        .split_whitespace()
        .any(|value| value != gid_text)
    {
        return Err("real/effective/saved/filesystem GID mismatch".into());
    }
    if !field("Groups:")?.is_empty() {
        return Err("supplementary groups remain".into());
    }
    for capability in ["CapInh:", "CapPrm:", "CapEff:", "CapAmb:"] {
        if field(capability)? != "0000000000000000" {
            return Err(format!("{capability} is not empty"));
        }
    }
    if field("NoNewPrivs:")? != "1" {
        return Err("no_new_privs is not set".into());
    }
    if field("SigBlk:")? != "0000000000000000" {
        return Err("signal mask is not empty".into());
    }
    Ok(())
}
