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
use std::io::{ErrorKind, Read};
#[cfg(target_os = "linux")]
use std::os::fd::FromRawFd;
#[cfg(target_os = "linux")]
use std::os::unix::process::CommandExt;
#[cfg(target_os = "linux")]
use std::process::Command;

/// Exit code for bootstrap/hardening/exec failures (typed Chronicle failure).
pub const BOOTSTRAP_FAILURE_EXIT: i32 = 3;

/// Entry point for the hidden bootstrap subcommand. Returns the process exit
/// code; on success (target exec) it never returns.
#[cfg(target_os = "linux")]
pub fn run_bootstrap(uid: u32, gid: u32, command: &[String]) -> i32 {
    if command.is_empty() {
        eprintln!("chronicle bootstrap: no target command");
        return BOOTSTRAP_FAILURE_EXIT;
    }
    // 1. Block until the parent signals readiness. EOF (parent died before
    //    releasing) or a read failure aborts without running target code.
    if let Err(message) = read_readiness_byte() {
        eprintln!("chronicle bootstrap: readiness signal failed: {message}");
        return BOOTSTRAP_FAILURE_EXIT;
    }
    // 2. Harden credentials and process state before the target execs.
    if let Err(message) = harden(uid, gid) {
        eprintln!("chronicle bootstrap: hardening failed: {message}");
        return BOOTSTRAP_FAILURE_EXIT;
    }
    // 3. Verify the resulting credentials before exec.
    if !verify_credentials(uid, gid) {
        eprintln!(
            "chronicle bootstrap: credential verification failed (expected uid {uid}, gid {gid})"
        );
        return BOOTSTRAP_FAILURE_EXIT;
    }
    // 4. Directly exec the target; never returns on success.
    let mut target = Command::new(&command[0]);
    target.args(&command[1..]);
    let error = target.exec();
    eprintln!("chronicle bootstrap: exec failed: {error}");
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
fn harden(uid: u32, gid: u32) -> Result<(), String> {
    use rustix::process::{Gid, Uid};
    use rustix::thread::{
        CapabilitySet, CapabilitySets, clear_ambient_capability_set, set_capabilities,
        set_no_new_privs, set_thread_groups, set_thread_res_gid, set_thread_res_uid,
    };

    let target_uid = Uid::from_raw(uid);
    let target_gid = Gid::from_raw(gid);

    // Clear supplementary groups (requires CAP_SETGROUPS; EPERM is acceptable
    // for an already-unprivileged bootstrap, whose groups are the invoking
    // user's own).
    set_thread_groups(&[])
        .map_err(|error| format!("clearing supplementary groups: {error}"))
        .or_else(accept_permission_denied)?;

    // Real/effective/saved GID then UID. These must match exactly.
    set_thread_res_gid(target_gid, target_gid, target_gid)
        .map_err(|error| format!("setting real/effective/saved GID: {error}"))?;
    set_thread_res_uid(target_uid, target_uid, target_uid)
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

    reset_signal_dispositions()?;
    close_unneeded_descriptors()?;
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

/// Close every descriptor except stdio (0, 1, 2). The readiness fd 3 was
/// already closed after the signal was read.
#[cfg(target_os = "linux")]
fn close_unneeded_descriptors() -> Result<(), String> {
    let entries = match std::fs::read_dir("/proc/self/fd") {
        Ok(entries) => entries,
        Err(error) => return Err(format!("listing descriptors: {error}")),
    };
    let mut descriptors = Vec::new();
    for entry in entries.flatten() {
        if let Ok(number) = entry.file_name().to_string_lossy().parse::<i32>() {
            if !(0..=2).contains(&number) {
                descriptors.push(number);
            }
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

/// Verify the process now runs with the target UID and GID.
#[cfg(target_os = "linux")]
fn verify_credentials(uid: u32, gid: u32) -> bool {
    rustix::process::getuid().as_raw() == uid && rustix::process::getgid().as_raw() == gid
}
