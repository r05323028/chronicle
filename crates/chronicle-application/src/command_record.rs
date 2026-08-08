//! Command-mode record orchestration: `chronicle record -- COMMAND...`.
//!
//! Sequence (design D2): non-mutating preflight first; exact domain lock +
//! revalidation; supervised-scope creation before durable allocation;
//! caller-allocated identity passed into the lower-level `record_production`
//! path; hidden bootstrap blocked on a readiness pipe (fd 3); capture
//! attached and WAL writer durable before the go byte releases the bootstrap
//! to harden credentials and directly exec the target. Bounded scope cleanup,
//! ETL, publication, and catalog update follow, with the domain lock released
//! last. A 30 s attach-readiness watchdog kills the still-blocked bootstrap
//! (and the scope) so no recording or target survives an attach hang.

use crate::{ApplicationError, ChildExitResult, RecordingId, RecordingStatus};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use crate::{DomainLockGuard, FilesystemSessionStore, RecordingMetadata};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Hard deadline for capture attachment (bootstrap release).
pub const COMMAND_RECORD_ATTACH_DEADLINE: Duration = Duration::from_secs(30);
/// Scope-name prefix for command-mode scopes.
pub const COMMAND_RECORD_SCOPE_PREFIX: &str = "cr-";

/// Child stdio routing: human mode passes through; JSON mode uses the null
/// sink so Chronicle stdout/stderr stay atomic and secret-free.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChildStdio {
    Inherit,
    Null,
}

#[derive(Clone, Debug)]
pub struct CommandRecordOptions {
    pub command: Vec<String>,
    pub name: Option<String>,
    pub duration_seconds: u64,
    pub data_dir: PathBuf,
    pub domain_lock_root: Option<PathBuf>,
    pub child_stdout: ChildStdio,
    pub child_stderr: ChildStdio,
    /// Trusted invoking credentials captured before any elevation.
    pub invoking_uid: u32,
    pub invoking_gid: u32,
    /// Signal stop observed by the CLI's signal watcher.
    pub stop: crate::ProductionSignalStop,
}

#[derive(Clone, Debug)]
pub struct CommandRecordResult {
    pub recording_id: RecordingId,
    pub name: Option<String>,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<crate::ShutdownReason>,
    pub child_exit: Option<ChildExitResult>,
    pub cleanup: crate::CleanupOutcome,
    pub duration_ms: u64,
    pub committed_records: u64,
}

/// Non-mutating preflight: every predictable denial happens before the domain
/// lock, scope, ID, sidecar, WAL, catalog entry, or target exists.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn preflight_command_record(options: &CommandRecordOptions) -> Result<(), ApplicationError> {
    use crate::preflight_embedded_ebpf;
    use crate::supervised_scope::{discover_delegated_root, preflight_scope_access};
    if options.command.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "record requires a command after --",
        ));
    }
    if !(1..=3600).contains(&options.duration_seconds) {
        return Err(ApplicationError::ProductionPreflight(
            "duration must be between 1s and 3600s",
        ));
    }
    preflight_embedded_ebpf()?;
    let (hierarchy, delegated) = discover_delegated_root(options.invoking_uid)?;
    preflight_scope_access(&hierarchy, &delegated, options.invoking_uid)?;
    Ok(())
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
fn preflight_command_record(_options: &CommandRecordOptions) -> Result<(), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires the Linux eBPF build",
    ))
}

/// Run one command-mode recording end to end. The domain lock is acquired
/// before any durable allocation and released last.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn record_command(
    options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    use crate::recording_catalog::RecordingIntentV1;
    use crate::supervised_scope::{RealClock, SCOPE_POLL_INTERVAL, create_supervised_scope};
    use crate::{
        CgroupSelection, DomainLockGuard, ProductionRecordingBounds, RecordingMetadata,
        RecordingSelectorIdentity, ShutdownReason, acquire_domain_lock, claim_recording_name,
        ensure_private_data_dir, live_capture_metadata, monotonic_millis, reconcile_catalog,
        record_production, recordings_root, save_catalog, write_recording_intent,
    };
    use std::collections::BTreeSet;
    use std::fs::File;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;
    use time::OffsetDateTime;

    let started = Instant::now();
    preflight_command_record(&options)?;
    ensure_private_data_dir(&options.data_dir)?;
    let guard = acquire_domain_lock(&options.data_dir, options.domain_lock_root.as_deref())?;

    // Under the lock: claim the name, create the supervised scope, allocate
    // the identity, and persist the intent sidecar plus the initial catalog
    // entry before any capture work.
    let recording_id = RecordingId::new();
    if let Some(name) = &options.name {
        claim_recording_name(&guard, &options.data_dir, name)?;
    }
    let (hierarchy, delegated) =
        crate::supervised_scope::discover_delegated_root(options.invoking_uid)?;
    let scope_name = format!("{COMMAND_RECORD_SCOPE_PREFIX}{}", recording_id.0.simple());
    let scope = create_supervised_scope(&hierarchy, &delegated, &scope_name, options.invoking_uid)?;
    let intent = RecordingIntentV1 {
        version: 1,
        recording_id,
        name: options.name.clone(),
        created_at: OffsetDateTime::now_utc(),
    };
    write_recording_intent(&guard, &options.data_dir, &intent)?;
    let wal_dir = recordings_root(&options.data_dir).join(recording_id.to_string());
    // The reconciled view already carries the sidecar-derived in-progress
    // entry (id, name, allocation timestamp); persist it as the initial
    // catalog state so list/inspect see the recording from the start.
    {
        let view = reconcile_catalog(&options.data_dir)?;
        save_catalog(&options.data_dir, &view)?;
    }

    // Spawn the hidden bootstrap blocked on the readiness pipe (fd 3).
    let (read_end, write_end) =
        rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).map_err(|error| {
            ApplicationError::Io(std::io::Error::from_raw_os_error(error.raw_os_error()))
        })?;
    let read_end_raw = read_end.as_raw_fd();
    let current_exe = std::env::current_exe().map_err(|error| {
        ApplicationError::InvalidConfig(format!("current executable unavailable: {error}"))
    })?;
    let mut command = Command::new(&current_exe);
    command.args([
        "internal",
        "bootstrap",
        "--uid",
        &options.invoking_uid.to_string(),
        "--gid",
        &options.invoking_gid.to_string(),
        "--",
    ]);
    command.args(&options.command);
    match options.child_stdout {
        ChildStdio::Inherit => {
            command.stdout(Stdio::inherit());
        }
        ChildStdio::Null => {
            command.stdout(Stdio::null());
        }
    }
    match options.child_stderr {
        ChildStdio::Inherit => {
            command.stderr(Stdio::inherit());
        }
        ChildStdio::Null => {
            command.stderr(Stdio::null());
        }
    }
    unsafe {
        command.pre_exec(move || {
            // Place the readiness pipe read end on fixed fd 3. When it already
            // is fd 3, only clear CLOEXEC. Both wrappers are forgotten on every
            // path so no descriptor drops in the child.
            let source = File::from_raw_fd(read_end_raw);
            let mut target = std::os::fd::OwnedFd::from_raw_fd(3);
            let result = if read_end_raw == 3 {
                rustix::io::fcntl_setfd(&target, rustix::io::FdFlags::empty())
            } else {
                rustix::io::dup2(&source, &mut target)
            };
            std::mem::forget(source);
            std::mem::forget(target);
            result.map_err(|error| std::io::Error::from_raw_os_error(error.raw_os_error()))
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| ApplicationError::InvalidConfig(format!("spawning bootstrap: {error}")))?;

    // The bootstrap must be in the scope before capture attaches.
    scope.move_process(child.id()).map_err(|error| {
        let _ = scope.kill_all();
        ApplicationError::from(error)
    })?;
    scope.revalidate()?;

    let selection = CgroupSelection {
        canonical_path: scope.identity().canonical_path.clone(),
        cgroup_id: scope.identity().cgroup_id,
        direct_tgids: BTreeSet::from([child.id()]),
        descendant_cgroup_ids: BTreeSet::new(),
        shared_scope_acknowledged: true,
    };
    let bounds = ProductionRecordingBounds {
        duration_seconds: options.duration_seconds,
        segment_bytes: crate::DEFAULT_SEGMENT_BYTES,
        max_wal_bytes: 4 * 1024 * 1024 * 1024,
    };
    let capture_metadata = live_capture_metadata(&selection, bounds)?;
    let mut metadata = RecordingMetadata {
        version: crate::RECORDING_METADATA_SCHEMA_VERSION,
        recording_id,
        selector: Some(RecordingSelectorIdentity {
            canonical_cgroup_path: scope.identity().canonical_path.display().to_string(),
            cgroup_id: scope.identity().cgroup_id,
        }),
        status: RecordingStatus::Starting,
        shutdown_reason: None,
        last_valid_commit: None,
        counters: Default::default(),
        terminal_wal_loss: None,
        capture: None,
    };

    // Watchdog: if capture attachment does not complete within the hard
    // deadline, kill the still-blocked bootstrap so no target runs and no
    // recording survives the hang.
    let released = Arc::new(AtomicBool::new(false));
    let watchdog_scope = scope.clone();
    let watchdog_released = released.clone();
    let watchdog = std::thread::spawn(move || {
        std::thread::sleep(COMMAND_RECORD_ATTACH_DEADLINE);
        if !watchdog_released.load(Ordering::SeqCst) {
            let _ = watchdog_scope.kill_all();
        }
    });

    let signal_stop = options.stop.clone();
    let stop_scope = scope.clone();
    let mut requested_stop = move || {
        if let Some(reason) = signal_stop.shutdown_reason() {
            return Some(reason);
        }
        let empty = stop_scope
            .members()
            .map(|members| members.is_empty())
            .unwrap_or(false);
        empty.then_some(ShutdownReason::SourceCompleted)
    };
    let on_ready = move || -> Result<(), ApplicationError> {
        released.store(true, Ordering::SeqCst);
        let mut file = File::from(write_end);
        use std::io::Write;
        file.write_all(b"G").map_err(|error| {
            ApplicationError::InvalidConfig(format!("releasing bootstrap: {error}"))
        })
    };

    let result = crate::record_production(
        &wal_dir,
        &mut metadata,
        capture_metadata,
        bounds,
        || crate::load_production_ebpf_source(&selection, None),
        monotonic_millis,
        &mut requested_stop,
        on_ready,
    );

    // Scope cleanup regardless of capture outcome; the target is either gone
    // (scope empty) or still running (duration limit) and must be reaped.
    let cleanup =
        scope
            .cleanup(&RealClock, SCOPE_POLL_INTERVAL)
            .unwrap_or(crate::CleanupOutcome::TimedOut {
                remaining: scope.members().unwrap_or_default(),
            });
    let child_exit = reap_child_exit(&mut child);
    let _ = watchdog;

    match result {
        Ok(result) => {
            finalize_and_publish(
                &guard,
                &options,
                recording_id,
                &metadata,
                &child_exit,
                &cleanup,
            )?;
            Ok(CommandRecordResult {
                recording_id,
                name: options.name.clone(),
                status: result.status,
                shutdown_reason: Some(result.shutdown_reason),
                child_exit,
                cleanup,
                duration_ms: started.elapsed().as_millis() as u64,
                committed_records: result.counters.committed.records,
            })
        }
        Err(error) => {
            // Pre-attach failure: remove the recording dir (no durable
            // evidence worth keeping) and surface the typed error. Post-attach
            // failures keep the WAL as recoverable via reconcile.
            let _ = std::fs::remove_dir_all(&wal_dir);
            Err(error)
        }
    }
}

/// `record --retry RECORDING`: reacquire the exact domain lock, then retry
/// recovery/ETL/publication/catalog completion for one recording id.
/// Never starts a target or capture and never creates a duplicate session
/// (publication is idempotent). Only recovery-authoritative `recoverable`
/// entries may be retried; everything else fails safely.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn retry_recording(
    data_dir: &Path,
    domain_lock_root: Option<&Path>,
    reference: &str,
) -> Result<RecordingId, ApplicationError> {
    use crate::{
        RecordingCatalogStatus, acquire_domain_lock, ensure_private_data_dir,
        process_and_publish_recording_wal, reconcile_catalog, recordings_root, resolve_recording,
        save_catalog,
    };
    use chronicle_common::SessionId;

    ensure_private_data_dir(data_dir)?;
    let guard = acquire_domain_lock(data_dir, domain_lock_root)?;
    let recording_id = resolve_recording(data_dir, reference)?;
    let view = reconcile_catalog(data_dir)?;
    let entry = view
        .entries
        .iter()
        .find(|entry| entry.recording_id == recording_id)
        .ok_or_else(|| ApplicationError::RecordingNotFound(reference.to_owned()))?;
    if entry.status != RecordingCatalogStatus::Recoverable {
        return Err(ApplicationError::InvalidConfig(format!(
            "recording {} is not recoverable (status {:?}); only recovery-authoritative recoverable recordings can be retried",
            recording_id.to_cli_string(),
            entry.status
        )));
    }
    let wal_dir = recordings_root(data_dir).join(recording_id.to_string());
    let registry = chronicle_protocol_builtins::registry()?;
    let published_session = match process_and_publish_recording_wal(&wal_dir, data_dir, &registry) {
        Ok(published) => published.session_id,
        Err(error)
            if matches!(
                error,
                ApplicationError::Wal(chronicle_wal::WalError::NoPublishedSegments)
            ) =>
        {
            crate::command_record::publish_empty_session(data_dir, recording_id)?
        }
        Err(error) => return Err(error),
    };
    let mut view = reconcile_catalog(data_dir)?;
    for entry in &mut view.entries {
        if entry.recording_id == recording_id {
            entry.status = RecordingCatalogStatus::Published;
            entry.ended_at = Some(time::OffsetDateTime::now_utc());
            entry.session_id = Some(published_session);
        }
    }
    save_catalog(data_dir, &view)?;
    let _ = guard;
    Ok(recording_id)
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
pub fn retry_recording(
    _data_dir: &Path,
    _domain_lock_root: Option<&Path>,
    _reference: &str,
) -> Result<RecordingId, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --retry requires the Linux eBPF build",
    ))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
pub fn record_command(
    _options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires the Linux eBPF build",
    ))
}

/// `record --pid PID` / `record --cgroup PATH`: record an already-running
/// workload through the existing `record_live_ebpf` path, then run the same
/// internal WAL -> ETL -> publish -> catalog workflow as command mode while
/// holding the exact data-domain lock for the whole transaction. The workload
/// is never terminated.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn record_selector(
    selector: crate::CgroupSelector,
    allow_shared_cgroup: bool,
    options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    use crate::recording_catalog::RecordingIntentV1;
    use crate::{
        ProductionRecordingBounds, RecordingMetadata, acquire_domain_lock, claim_recording_name,
        ensure_private_data_dir, reconcile_catalog, record_live_ebpf, recordings_root,
        save_catalog, write_recording_intent,
    };
    use time::OffsetDateTime;

    ensure_private_data_dir(&options.data_dir)?;
    let guard = acquire_domain_lock(&options.data_dir, options.domain_lock_root.as_deref())?;
    let recording_id = RecordingId::new();
    if let Some(name) = &options.name {
        claim_recording_name(&guard, &options.data_dir, name)?;
    }
    let intent = RecordingIntentV1 {
        version: 1,
        recording_id,
        name: options.name.clone(),
        created_at: OffsetDateTime::now_utc(),
    };
    write_recording_intent(&guard, &options.data_dir, &intent)?;
    {
        let view = reconcile_catalog(&options.data_dir)?;
        save_catalog(&options.data_dir, &view)?;
    }
    let wal_dir = recordings_root(&options.data_dir).join(recording_id.to_string());
    let bounds = ProductionRecordingBounds {
        duration_seconds: options.duration_seconds,
        segment_bytes: crate::DEFAULT_SEGMENT_BYTES,
        max_wal_bytes: 4 * 1024 * 1024 * 1024,
    };
    let result = record_live_ebpf(
        selector,
        allow_shared_cgroup,
        &wal_dir,
        bounds,
        &options.stop,
        recording_id,
    )?;
    finalize_and_publish(
        &guard,
        &options,
        recording_id,
        &RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id,
            selector: None,
            status: RecordingStatus::Completed,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: Default::default(),
            terminal_wal_loss: None,
            capture: None,
        },
        &None,
        &crate::CleanupOutcome::Clean,
    )?;
    let _ = ();
    Ok(CommandRecordResult {
        recording_id,
        name: options.name.clone(),
        status: result.status,
        shutdown_reason: Some(result.shutdown_reason),
        child_exit: None,
        cleanup: crate::CleanupOutcome::Clean,
        duration_ms: 0,
        committed_records: result.counters.committed.records,
    })
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
pub fn record_selector(
    _selector: crate::CgroupSelector,
    _allow_shared_cgroup: bool,
    _options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --pid/--cgroup requires the Linux eBPF build",
    ))
}

/// Reap the bootstrap child (which exec'd the target): its exit status is the
/// target's factual exit, reported but never forwarded.
#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
fn reap_child_exit(child: &mut std::process::Child) -> Option<ChildExitResult> {
    use std::os::unix::process::ExitStatusExt;
    let status = child.wait().ok()?;
    if let Some(code) = status.code() {
        Some(ChildExitResult::ExitCode { code })
    } else {
        status
            .signal()
            .map(|signal| ChildExitResult::Signal { signal })
    }
}

/// ETL, canonical publication, and catalog update under the held domain lock.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn finalize_and_publish(
    guard: &DomainLockGuard,
    options: &CommandRecordOptions,
    recording_id: RecordingId,
    metadata: &RecordingMetadata,
    child_exit: &Option<ChildExitResult>,
    cleanup: &crate::CleanupOutcome,
) -> Result<(), ApplicationError> {
    use crate::{
        RecordingCatalogStatus, process_and_publish_recording_wal, reconcile_catalog,
        recordings_root, save_catalog,
    };
    use chronicle_common::SessionId;
    let wal_dir = recordings_root(&options.data_dir).join(recording_id.to_string());
    let registry = chronicle_protocol_builtins::registry()?;
    let published_session =
        match process_and_publish_recording_wal(&wal_dir, &options.data_dir, &registry) {
            Ok(published) => published.session_id,
            Err(error)
                if matches!(
                    error,
                    ApplicationError::Wal(chronicle_wal::WalError::NoPublishedSegments)
                ) =>
            {
                // Zero-traffic recording: the target made no HTTP requests, so
                // the WAL has no segments. Publish an empty canonical session
                // so the recording is a normal published entry (0 operations).
                publish_empty_session(&options.data_dir, recording_id)?
            }
            Err(error) => return Err(error),
        };

    let mut view = reconcile_catalog(&options.data_dir)?;
    for entry in &mut view.entries {
        if entry.recording_id == recording_id {
            entry.status = RecordingCatalogStatus::Published;
            entry.ended_at = Some(time::OffsetDateTime::now_utc());
            entry.session_id = Some(published_session);
            entry.child_exit = child_exit.clone();
        }
    }
    save_catalog(&options.data_dir, &view)?;
    let _ = (guard, metadata, cleanup);
    Ok(())
}

/// Publish an empty canonical session for a zero-traffic recording.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn publish_empty_session(
    data_dir: &Path,
    recording_id: RecordingId,
) -> Result<chronicle_common::SessionId, ApplicationError> {
    use chronicle_canonical::{CANONICAL_SCHEMA_VERSION, CanonicalSession, SourceMetadata};
    use chronicle_storage::PublishSession;
    let now = time::OffsetDateTime::now_utc();
    let session = CanonicalSession {
        schema_version: CANONICAL_SCHEMA_VERSION,
        id: chronicle_common::SessionId(recording_id.0),
        started_at: now,
        ended_at: Some(now),
        source: SourceMetadata::default(),
        source_provenance: chronicle_canonical::SourceProvenance {
            recording_id: Some(recording_id),
            ..Default::default()
        },
        connections: Vec::new(),
        connection_completeness: Default::default(),
        operation_completeness: Default::default(),
        timeline: Vec::new(),
        replay: chronicle_canonical::ReplayMetadata::default(),
        replay_attributes: chronicle_canonical::Attributes::new(),
    };
    FilesystemSessionStore::new(data_dir).publish(PublishSession {
        session,
        checkpoint: None,
        issues: Vec::new(),
        replayability: Vec::new(),
        complete: true,
    })?;
    Ok(chronicle_common::SessionId(recording_id.0))
}
