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

use crate::{
    ApplicationError, CaptureMode, ChildExitResult, RecordingId, RecordingLifetime, RecordingStatus,
};
#[cfg(target_os = "linux")]
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
    /// Legacy metadata field; zero means no public recording deadline.
    pub duration_seconds: u64,
    pub lifetime: RecordingLifetime,
    pub capture_mode: CaptureMode,
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
    pub operations: u64,
    pub counters: crate::RecordingCounters,
}

/// Non-mutating preflight: every predictable denial happens before the domain
/// lock, scope, ID, sidecar, WAL, catalog entry, or target exists.
#[cfg(target_os = "linux")]
fn preflight_command_record(options: &CommandRecordOptions) -> Result<(), ApplicationError> {
    use crate::preflight_embedded_ebpf;
    use crate::supervised_scope::{discover_delegated_root, preflight_scope_access};
    if options.command.is_empty() {
        return Err(ApplicationError::ProductionPreflight(
            "record requires a command after --",
        ));
    }
    preflight_embedded_ebpf()?;
    let (hierarchy, delegated) = discover_delegated_root(options.invoking_uid)?;
    preflight_scope_access(&hierarchy, &delegated, options.invoking_uid)?;
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn preflight_command_record(_options: &CommandRecordOptions) -> Result<(), ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires a Linux build with live capture",
    ))
}

/// Run one command-mode recording end to end. The domain lock is acquired
/// before any durable allocation and released last.
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_lines)] // Linear capture/scope lifecycle keeps cleanup order explicit.
pub fn record_command(
    options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    use crate::bootstrap::spawn_blocked_bootstrap;
    use crate::recording_catalog::RecordingIntentV1;
    use crate::supervised_scope::{RealClock, SCOPE_POLL_INTERVAL, create_supervised_scope};
    use crate::{
        CgroupSelection, CgroupSelector, ProductionRecordingBounds, RecordingMetadata,
        RecordingSelectorIdentity, ShutdownReason, acquire_domain_lock, claim_recording_name,
        ensure_private_data_dir, reconcile_catalog, recordings_root, save_catalog,
        write_recording_intent,
    };
    use std::collections::BTreeSet;
    use std::process::Stdio;
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
    let stdout = match options.child_stdout {
        ChildStdio::Inherit => Stdio::inherit(),
        ChildStdio::Null => Stdio::null(),
    };
    let stderr = match options.child_stderr {
        ChildStdio::Inherit => Stdio::inherit(),
        ChildStdio::Null => Stdio::null(),
    };
    let mut child = spawn_blocked_bootstrap(
        &options.command,
        options.invoking_uid,
        options.invoking_gid,
        stdout,
        stderr,
    )?;

    // The bootstrap must be in the scope before capture attaches.
    if let Err(error) = scope.move_process(child.pid()) {
        child.abort();
        let _ = scope.destroy();
        let _ = child.wait_result();
        return Err(error.into());
    }
    scope.revalidate()?;

    let selection = CgroupSelection {
        canonical_path: scope.identity().canonical_path.clone(),
        cgroup_id: scope.identity().cgroup_id,
        direct_tgids: BTreeSet::from([child.pid()]),
        descendant_cgroup_ids: BTreeSet::new(),
        shared_scope_acknowledged: true,
    };
    let bounds = ProductionRecordingBounds {
        duration_seconds: options.duration_seconds,
        segment_bytes: crate::DEFAULT_SEGMENT_BYTES,
        max_wal_bytes: 4 * 1024 * 1024 * 1024,
    };
    let metadata = RecordingMetadata {
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

    let recorder_config = crate::public_continuous_config(&options.data_dir, &selection, bounds)?;
    let signal_stop = options.stop.clone();
    let stop_scope = scope.clone();
    let on_ready = || -> Result<(), ApplicationError> {
        child.release()?;
        released.store(true, Ordering::SeqCst);
        Ok(())
    };
    let result = crate::record_continuous_ebpf_with_source(
        CgroupSelector::Explicit(scope.identity().canonical_path.clone()),
        true,
        &recorder_config,
        &wal_dir,
        &options.stop,
        Some(recording_id),
        options.capture_mode,
        options.lifetime,
        |_, _| crate::load_production_ebpf_source(&selection, None),
        move || {
            if let Some(reason) = signal_stop.shutdown_reason() {
                return Some(reason);
            }
            stop_scope
                .members()
                .is_ok_and(|members| members.is_empty())
                .then_some(ShutdownReason::SourceCompleted)
        },
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
    let child_exit = match cleanup {
        crate::CleanupOutcome::TimedOut { .. } => child.try_result(),
        crate::CleanupOutcome::Clean | crate::CleanupOutcome::Killed => child.wait_result(),
    }?;
    let _ = watchdog;

    match result {
        Ok(result) => {
            let operations = finalize_and_publish(
                &guard,
                &options,
                recording_id,
                &metadata,
                &result,
                child_exit.as_ref(),
                &cleanup,
            )?;
            Ok(CommandRecordResult {
                recording_id,
                name: options.name,
                status: result.status,
                shutdown_reason: Some(result.shutdown_reason),
                child_exit,
                cleanup,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
                operations,
                counters: result.counters,
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
#[cfg(target_os = "linux")]
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
        Err(ApplicationError::Wal(chronicle_wal::WalError::NoPublishedSegments)) => {
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

#[cfg(not(target_os = "linux"))]
pub fn retry_recording(
    _data_dir: &Path,
    _domain_lock_root: Option<&Path>,
    _reference: &str,
) -> Result<RecordingId, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --retry requires the Linux eBPF build",
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn record_command(
    _options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "command-mode recording requires a Linux build with live capture",
    ))
}

/// `record --pid PID` / `record --cgroup PATH`: record an already-running
/// workload through the existing `record_live_ebpf` path, then run the same
/// internal WAL -> ETL -> publish -> catalog workflow as command mode while
/// holding the exact data-domain lock for the whole transaction. The workload
/// is never terminated.
#[cfg(target_os = "linux")]
pub fn record_selector(
    selector: crate::CgroupSelector,
    allow_shared_cgroup: bool,
    options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    use crate::recording_catalog::RecordingIntentV1;
    use crate::{
        ProductionRecordingBounds, RecordingMetadata, acquire_domain_lock, claim_recording_name,
        ensure_private_data_dir, preflight_cgroup_selection, preflight_pid_cgroup_selection,
        public_continuous_config, reconcile_catalog, recordings_root, save_catalog,
        write_recording_intent,
    };
    use time::OffsetDateTime;

    let started = std::time::Instant::now();
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
    let initial_selection = match &selector {
        crate::CgroupSelector::Pid(pid) => preflight_pid_cgroup_selection(*pid)
            .map_err(|_| ApplicationError::ProductionPreflight("PID cgroup selection invalid"))?
            .selection()
            .clone(),
        crate::CgroupSelector::Explicit(path) => preflight_cgroup_selection(
            crate::CgroupSelector::Explicit(path.clone()),
            allow_shared_cgroup,
        )
        .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?,
    };
    let recorder_config = public_continuous_config(&options.data_dir, &initial_selection, bounds)?;
    let signal_stop = options.stop.clone();
    let result = crate::record_continuous_ebpf_with_source(
        selector,
        allow_shared_cgroup,
        &recorder_config,
        &wal_dir,
        &options.stop,
        Some(recording_id),
        options.capture_mode,
        options.lifetime,
        crate::load_production_ebpf_source,
        move || signal_stop.shutdown_reason(),
        || Ok(()),
    )?;
    let operations = finalize_and_publish(
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
        &result,
        None,
        &crate::CleanupOutcome::Clean,
    )?;
    Ok(CommandRecordResult {
        recording_id,
        name: options.name,
        status: result.status,
        shutdown_reason: Some(result.shutdown_reason),
        child_exit: None,
        cleanup: crate::CleanupOutcome::Clean,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        operations,
        counters: result.counters,
    })
}

#[cfg(not(target_os = "linux"))]
pub fn record_selector(
    _selector: crate::CgroupSelector,
    _allow_shared_cgroup: bool,
    _options: CommandRecordOptions,
) -> Result<CommandRecordResult, ApplicationError> {
    Err(ApplicationError::UnsupportedLivePreflight(
        "record --pid/--cgroup requires a Linux build with live capture",
    ))
}

#[cfg(target_os = "linux")]
fn finalize_continuous_and_publish(
    guard: &DomainLockGuard,
    options: &CommandRecordOptions,
    recording_id: RecordingId,
    mut catalog: crate::EpochCatalogV2,
    recording_result: &crate::ProductionRecordingResult,
    child_exit: Option<&ChildExitResult>,
    cleanup: &crate::CleanupOutcome,
) -> Result<u64, ApplicationError> {
    if catalog.parent_id != recording_id {
        return Err(ApplicationError::RecordingMetadataValidation(
            "public recording parent identity disagrees with epoch catalog".into(),
        ));
    }
    let wal_root = crate::recordings_root(&options.data_dir).join(recording_id.to_string());
    for entry in catalog.epochs.clone() {
        let directory = wal_root.join(&entry.path);
        let metadata = crate::load_recording_metadata(&directory)?.ok_or(
            ApplicationError::RecordingMetadataValidation(
                "catalog epoch metadata missing during public finalization".into(),
            ),
        )?;
        let commit = metadata.last_valid_commit.as_ref();
        catalog
            .update_summary(
                entry.epoch_id,
                crate::EpochCatalogSummaryV2 {
                    committed_through: commit.map(|value| value.marker_sequence),
                    committed_bytes: commit.map_or(0, |value| value.durable_payload_bytes),
                    checkpoint_through: commit.map(|value| value.marker_sequence),
                    published: crate::has_published_wal(&directory)?,
                    retention_state: Some("retained".into()),
                    ..crate::EpochCatalogSummaryV2::default()
                },
            )
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    }
    catalog
        .write_atomic(&wal_root)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;

    let stop_reason = match recording_result.shutdown_reason {
        crate::ShutdownReason::SourceCompleted => crate::RunStopReason::SourceCompleted,
        crate::ShutdownReason::DurationLimit => crate::RunStopReason::Deadline,
        crate::ShutdownReason::UserInterrupt | crate::ShutdownReason::TerminationSignal => {
            crate::RunStopReason::UserRequested
        }
        crate::ShutdownReason::ProcessCrashRecovered => crate::RunStopReason::RestartRecovery,
        crate::ShutdownReason::CaptureFailure
        | crate::ShutdownReason::WalFailure
        | crate::ShutdownReason::WalSizeLimit
        | crate::ShutdownReason::ForcedTermination => crate::RunStopReason::FatalFailure,
    };
    let state = if matches!(recording_result.status, crate::RecordingStatus::Completed) {
        crate::RunLifecycleState::Completed
    } else {
        crate::RunLifecycleState::Failed
    };
    let created_at_unix_millis = crate::load_recording_intent(&options.data_dir, recording_id)?
        .map_or_else(
            || {
                u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
                    .unwrap_or(0)
            },
            |intent| {
                u64::try_from(intent.created_at.unix_timestamp_nanos() / 1_000_000).unwrap_or(0)
            },
        );
    crate::RecordingRunV2::regenerate_from_catalog(
        &wal_root,
        &catalog,
        options.name.clone(),
        created_at_unix_millis,
        options.lifetime,
        state,
        Some(stop_reason),
    )
    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;

    let published_session = match crate::resolve_session(&options.data_dir, recording_id)? {
        Some(session_id) => session_id,
        None => crate::command_record::publish_empty_session(&options.data_dir, recording_id)?,
    };
    let mut view = crate::reconcile_catalog(&options.data_dir)?;
    for entry in &mut view.entries {
        if entry.recording_id == recording_id {
            entry.status = crate::RecordingCatalogStatus::Published;
            entry.ended_at = Some(time::OffsetDateTime::now_utc());
            entry.session_id = Some(published_session);
            entry.child_exit = child_exit.cloned();
        }
    }
    crate::save_catalog(&options.data_dir, &view)?;
    let operations = crate::list_parent_recording_views(&options.data_dir)?
        .into_iter()
        .find(|view| view.parent_id == recording_id)
        .map_or(0, |view| view.operations);
    let _ = (guard, cleanup);
    Ok(u64::try_from(operations).unwrap_or(u64::MAX))
}

/// ETL, canonical publication, and catalog update under the held domain lock.
#[cfg(target_os = "linux")]
fn finalize_and_publish(
    guard: &DomainLockGuard,
    options: &CommandRecordOptions,
    recording_id: RecordingId,
    metadata: &RecordingMetadata,
    recording_result: &crate::ProductionRecordingResult,
    child_exit: Option<&ChildExitResult>,
    cleanup: &crate::CleanupOutcome,
) -> Result<u64, ApplicationError> {
    use crate::{
        EpochCatalogSummaryV2, EpochCatalogV2, EpochId, RecordingCatalogStatus, RecordingRunV2,
        RunLifecycleState, RunStopReason, process_and_publish_recording_wal, reconcile_catalog,
        recordings_root, save_catalog,
    };
    let wal_dir = recordings_root(&options.data_dir).join(recording_id.to_string());
    let catalog_path = wal_dir.join(crate::EPOCH_CATALOG_FILE);
    if catalog_path.is_file() {
        let catalog = EpochCatalogV2::load(&wal_dir)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        return finalize_continuous_and_publish(
            guard,
            options,
            recording_id,
            catalog,
            recording_result,
            child_exit,
            cleanup,
        );
    }
    let registry = chronicle_protocol_builtins::registry()?;
    let published_session =
        match process_and_publish_recording_wal(&wal_dir, &options.data_dir, &registry) {
            Ok(published) => published.session_id,
            Err(ApplicationError::Wal(chronicle_wal::WalError::NoPublishedSegments)) => {
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
            entry.child_exit = child_exit.cloned();
        }
    }
    save_catalog(&options.data_dir, &view)?;

    // Public one-shot paths still use legacy WAL identity bytes. Persist the
    // parent/epoch DTOs with an explicit equal-byte compatibility mapping;
    // future continuous roots supply distinct typed identities.
    let epoch_id = EpochId::from_uuid(recording_id.0);
    let mut epoch_catalog = EpochCatalogV2::new(recording_id, epoch_id, ".")
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    epoch_catalog
        .update_summary(
            epoch_id,
            EpochCatalogSummaryV2 {
                committed_through: metadata
                    .last_valid_commit
                    .as_ref()
                    .map(|boundary| boundary.marker_sequence),
                committed_bytes: metadata
                    .last_valid_commit
                    .as_ref()
                    .map_or(0, |boundary| boundary.durable_payload_bytes),
                checkpoint_through: metadata
                    .last_valid_commit
                    .as_ref()
                    .map(|boundary| boundary.marker_sequence),
                published: true,
                ..EpochCatalogSummaryV2::default()
            },
        )
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    epoch_catalog
        .write_atomic(&wal_dir)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    let run = RecordingRunV2::from_catalog(
        &epoch_catalog,
        options.name.clone(),
        u64::try_from(time::OffsetDateTime::now_utc().unix_timestamp_nanos() / 1_000_000)
            .unwrap_or(0),
        options.lifetime,
        RunLifecycleState::Completed,
        metadata.shutdown_reason.map(|reason| match reason {
            crate::ShutdownReason::SourceCompleted => RunStopReason::SourceCompleted,
            crate::ShutdownReason::DurationLimit => RunStopReason::Deadline,
            crate::ShutdownReason::UserInterrupt | crate::ShutdownReason::TerminationSignal => {
                RunStopReason::UserRequested
            }
            crate::ShutdownReason::ProcessCrashRecovered => RunStopReason::RestartRecovery,
            crate::ShutdownReason::CaptureFailure
            | crate::ShutdownReason::WalFailure
            | crate::ShutdownReason::WalSizeLimit
            | crate::ShutdownReason::ForcedTermination => RunStopReason::FatalFailure,
        }),
    )
    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    run.write_atomic(&wal_dir)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;

    let operations = crate::list_recordings(&options.data_dir)?
        .into_iter()
        .find(|recording| recording.recording_id == recording_id)
        .map_or(0, |recording| recording.operations);
    let _ = (guard, cleanup);
    Ok(u64::try_from(operations).unwrap_or(u64::MAX))
}

/// Publish an empty canonical session for a zero-traffic recording.
#[cfg(target_os = "linux")]
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
