//! Production record orchestration: preflight, live/continuous capture, and
//! bounded recording bounds, quota reservation, and ingest state.

use super::*;

pub fn validate_production_recording_bounds(
    bounds: ProductionRecordingBounds,
) -> Result<(), ApplicationError> {
    if !(1..=MAX_RECORDING_DURATION_SECONDS).contains(&bounds.duration_seconds) {
        return Err(ApplicationError::ProductionPreflight(
            "duration bound invalid",
        ));
    }
    if !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&bounds.segment_bytes)
        || bounds.max_wal_bytes < bounds.segment_bytes
        || bounds.max_wal_bytes > DEFAULT_MAX_WAL_BYTES
    {
        return Err(ApplicationError::ProductionPreflight("WAL bounds invalid"));
    }
    Ok(())
}

/// Combines selector and eBPF prerequisites before a source can attach or metadata can succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionWalPreflight {
    pub available_bytes: u64,
    pub low_space_warning: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingStorePreflight {
    pub state_root: PathBuf,
    pub store_root: PathBuf,
    pub same_filesystem: bool,
    pub local_active_spool: bool,
}

/// Validates recorder-store placement without acquiring a store or WAL lock.
pub fn preflight_recording_store(
    config: &NormalizedRecorderConfig,
) -> Result<RecordingStorePreflight, ApplicationError> {
    let state_parent = config
        .state_root
        .parent()
        .unwrap_or(config.state_root.as_path());
    let store_parent = config
        .store_root
        .parent()
        .unwrap_or(config.store_root.as_path());
    let state_metadata = std::fs::metadata(state_parent)
        .map_err(|_| ApplicationError::ProductionPreflight("state parent is unavailable"))?;
    let store_metadata = std::fs::metadata(store_parent)
        .map_err(|_| ApplicationError::ProductionPreflight("store parent is unavailable"))?;
    if !state_metadata.is_dir() || !store_metadata.is_dir() {
        return Err(ApplicationError::ProductionPreflight(
            "recorder store parent is not a directory",
        ));
    }
    Ok(RecordingStorePreflight {
        state_root: config.state_root.clone(),
        store_root: config.store_root.clone(),
        same_filesystem: config.state_domain.device == config.store_domain.device,
        local_active_spool: true,
    })
}

/// Checks writable exclusive WAL destination and available capacity without creating recording metadata.
pub fn preflight_wal_destination(
    wal_directory: &Path,
    bounds: ProductionRecordingBounds,
) -> Result<ProductionWalPreflight, ApplicationError> {
    validate_production_recording_bounds(bounds)?;
    if wal_directory.exists() {
        // The caller may have created the directory for the private intent
        // sidecar; only an existing WAL (metadata or segments) is a conflict.
        let has_wal = wal_directory.join("recording.json").exists()
            || wal_directory.join("segments").exists();
        if has_wal {
            return Err(ApplicationError::ProductionPreflight(
                "WAL destination already exists",
            ));
        }
    }
    let parent = wal_directory
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(ApplicationError::ProductionPreflight(
            "WAL parent unavailable",
        ))?;
    fs::create_dir_all(parent)
        .map_err(|_| ApplicationError::ProductionPreflight("WAL parent unwritable"))?;
    let stats = rustix::fs::statvfs(parent)
        .map_err(|_| ApplicationError::ProductionPreflight("WAL free space unavailable"))?;
    let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
    Ok(ProductionWalPreflight {
        available_bytes,
        low_space_warning: available_bytes < bounds.max_wal_bytes,
    })
}

/// Combines selector and eBPF prerequisites before a source can attach or metadata can succeed.
pub fn preflight_production_record(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    bounds: ProductionRecordingBounds,
    wal_directory: &Path,
) -> Result<CgroupSelection, ApplicationError> {
    preflight_wal_destination(wal_directory, bounds)?;
    let selection = preflight_cgroup_selection(selector, allow_shared_cgroup)
        .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?;
    preflight_embedded_ebpf()?;
    Ok(selection)
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) fn preflight_embedded_ebpf() -> Result<(), ApplicationError> {
    chronicle_capture_ebpf::probe_embedded()
        .is_ready()
        .then_some(())
        .ok_or(ApplicationError::ProductionPreflight(
            "eBPF prerequisites unavailable",
        ))
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf")))]
pub(crate) fn preflight_embedded_ebpf() -> Result<(), ApplicationError> {
    Err(ApplicationError::ProductionPreflight(
        "Linux eBPF capture unavailable",
    ))
}

/// Runs live eBPF recording. Signal adapters request stop through `stop`.
/// `recording_id` is caller-allocated so orchestration can persist intent and
/// catalog linkage before capture.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub fn record_live_ebpf(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    wal_directory: impl AsRef<Path>,
    bounds: ProductionRecordingBounds,
    stop: &ProductionSignalStop,
    recording_id: RecordingId,
) -> Result<ProductionRecordingResult, ApplicationError> {
    preflight_wal_destination(wal_directory.as_ref(), bounds)?;
    preflight_embedded_ebpf()?;
    if matches!(&selector, CgroupSelector::Pid(_)) && allow_shared_cgroup {
        return Err(ApplicationError::ProductionPreflight(
            "shared cgroup acknowledgement requires explicit cgroup",
        ));
    }
    let pid_baseline = match &selector {
        CgroupSelector::Pid(pid) => {
            Some(preflight_pid_cgroup_selection(*pid).map_err(|_| {
                ApplicationError::ProductionPreflight("PID cgroup selection invalid")
            })?)
        }
        CgroupSelector::Explicit(_) => None,
    };
    let selection = match &pid_baseline {
        Some(baseline) => baseline.selection().clone(),
        None => preflight_cgroup_selection(selector, allow_shared_cgroup)
            .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?,
    };
    let capture_metadata = live_capture_metadata(&selection, bounds)?;
    let mut metadata = RecordingMetadata {
        version: RECORDING_METADATA_SCHEMA_VERSION,
        recording_id,
        selector: Some(RecordingSelectorIdentity {
            canonical_cgroup_path: selection.canonical_path.display().to_string(),
            cgroup_id: selection.cgroup_id,
        }),
        status: RecordingStatus::Starting,
        shutdown_reason: None,
        last_valid_commit: None,
        counters: RecordingCounters::default(),
        terminal_wal_loss: None,
        capture: None,
    };
    record_production(
        wal_directory,
        &mut metadata,
        capture_metadata,
        bounds,
        || load_production_ebpf_source(&selection, pid_baseline.as_ref()),
        monotonic_millis,
        || stop.shutdown_reason(),
        || Ok(()),
    )
}

/// Runs capture through the continuous startup/rollover/drain runtime.
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
#[allow(clippy::too_many_lines)]
fn recover_cleanup_for_root(root: &Path) -> Result<(), RecorderStartupError> {
    // The cleanup intent is written beside its source segment (inside the
    // epoch's segments directory), so the recovery scan must descend into
    // every subdirectory of the WAL root.
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(RecorderStartupError::Recovery),
        };
        for entry in entries {
            let entry = entry.map_err(|_| RecorderStartupError::Recovery)?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|_| RecorderStartupError::Recovery)?;
            if file_type.is_dir() {
                pending.push(path);
            } else if file_type.is_file()
                && entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cleanup-intent")
            {
                recover_cleanup(&path).map_err(|_| RecorderStartupError::Recovery)?;
            }
        }
    }
    Ok(())
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
fn rollover_path_matches(expected: &str, actual: &Path) -> bool {
    match (fs::canonicalize(expected), fs::canonicalize(actual)) {
        (Ok(expected), Ok(actual)) => expected == actual,
        _ => Path::new(expected) == actual,
    }
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
#[allow(clippy::too_many_lines)]
pub(crate) fn recover_rollover_transition_for_root(
    state_root: &Path,
    root: &Path,
    catalog: Option<&mut EpochCatalogV1>,
    quotas: &[QuotaReservationAuthority],
    configured_reservation_bytes: u64,
) -> Result<(), RecorderStartupError> {
    let Some(transition) = load_transition(state_root)
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?
    else {
        return Ok(());
    };
    let successor = PathBuf::from(&transition.new_path);
    if !successor.starts_with(root) {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover successor escaped catalog root".into(),
        ));
    }
    if transition.reservation_bytes != configured_reservation_bytes {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover reservation does not match configured epoch limit".into(),
        ));
    }
    if !quotas.is_empty() {
        let authorities = quotas
            .iter()
            .filter(|quota| successor.starts_with(Path::new(&quota.quota().domain)))
            .collect::<Vec<_>>();
        if authorities.len() != 1
            || transition.reservation_bytes > authorities[0].quota().quota_bytes
        {
            return Err(RecorderStartupError::RecoveryDetail(
                "rollover successor quota domain or reservation mismatch".into(),
            ));
        }
    }
    let (catalog_old_matches, catalog_successor_matches) =
        if let Some(catalog_view) = catalog.as_ref() {
            let old = catalog_view.epochs.iter().any(|entry| {
                entry.ordinal == transition.old_epoch_ordinal
                    && entry.recording_id.0 == transition.old_recording_id
                    && rollover_path_matches(&transition.old_path, &root.join(&entry.path))
            });
            let successor_entry = catalog_view.epochs.iter().any(|entry| {
                entry.ordinal == transition.new_epoch_ordinal
                    && entry.recording_id.0 == transition.new_recording_id
                    && entry.predecessor
                        == Some(chronicle_common::RecordingId(transition.old_recording_id))
                    && rollover_path_matches(&transition.new_path, &root.join(&entry.path))
            });
            (old, successor_entry)
        } else {
            (true, false)
        };
    if !catalog_old_matches {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover transition predecessor identity mismatch".into(),
        ));
    }
    let physical_bytes = if successor.exists() {
        recording_physical_wal_bytes(&successor)
            .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?
    } else {
        0
    };
    if physical_bytes > transition.reservation_bytes {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover successor exceeded reserved quota".into(),
        ));
    }
    let metadata = load_recording_metadata(&successor)
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
    if let Some(metadata) = &metadata
        && metadata.recording_id.0 != transition.new_recording_id
    {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover successor metadata identity mismatch".into(),
        ));
    }
    let committed = if successor.exists() && successor.join("segments").is_dir() {
        match chronicle_wal::read_committed_snapshot_with_records(
            &successor,
            chronicle_common::RecordingId(transition.new_recording_id),
            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
        ) {
            Ok(_) => true,
            Err(chronicle_wal::WalError::NoPublishedSegments) => false,
            Err(error) => {
                return Err(RecorderStartupError::RecoveryDetail(error.to_string()));
            }
        }
    } else {
        false
    };
    if metadata.is_none() && committed {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover successor has committed evidence without metadata".into(),
        ));
    }
    if metadata.is_some() && !catalog_successor_matches {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover transition successor identity mismatch".into(),
        ));
    }
    if metadata.is_none() && successor.exists() {
        fs::remove_dir_all(&successor).map_err(|error| {
            RecorderStartupError::RecoveryDetail(format!(
                "rollover successor rollback failed: {error}"
            ))
        })?;
    }
    let Some(catalog) = catalog else {
        if metadata.is_some() {
            return Err(RecorderStartupError::RecoveryDetail(
                "rollover successor metadata has no catalog authority".into(),
            ));
        }
        for quota in quotas {
            quota
                .rebuild_managed_usage()
                .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
        }
        remove_transition(state_root)
            .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
        return Ok(());
    };
    catalog
        .recover_prepared_tail(root)
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
    if metadata.is_some()
        && catalog.active().map(|entry| entry.recording_id.0) != Some(transition.new_recording_id)
    {
        return Err(RecorderStartupError::RecoveryDetail(
            "rollover successor was not activated".into(),
        ));
    }
    for quota in quotas {
        quota
            .rebuild_managed_usage()
            .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
    }
    remove_transition(state_root)
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
    Ok(())
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) const LIFECYCLE_INDEX_FILE: &str = "lifecycle-index.json";
#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) const LIFECYCLE_INDEX_MAX_ENTRIES: usize = 4096;
#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) const LIFECYCLE_INDEX_MAX_BYTES: usize = 1 << 20;

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) fn load_lifecycle_index(root: &Path) -> Result<LifecycleIndexV1, ApplicationError> {
    match fs::read(root.join(LIFECYCLE_INDEX_FILE)) {
        Ok(bytes) => {
            let index: LifecycleIndexV1 = serde_json::from_slice(&bytes)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            index
                .validate()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            Ok(index)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LifecycleIndexV1 {
            version: 1,
            generation: 0,
            transaction_id: String::new(),
            source_revision: 0,
            source_digest: String::new(),
            first_retained_epoch: 0,
            last_compacted_epoch: None,
            entries: Vec::new(),
            max_entries: LIFECYCLE_INDEX_MAX_ENTRIES,
            max_bytes: LIFECYCLE_INDEX_MAX_BYTES,
        }),
        Err(error) => Err(ApplicationError::Io(error)),
    }
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) fn save_lifecycle_index(
    root: &Path,
    index: &LifecycleIndexV1,
) -> Result<(), ApplicationError> {
    index
        .validate()
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    write_private_atomic_json(root, LIFECYCLE_INDEX_FILE, index, None)
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) fn mark_epoch_cleanup_complete(
    index: &mut LifecycleIndexV1,
    ordinal: u64,
    digest: String,
) {
    match index
        .entries
        .iter_mut()
        .find(|entry| entry.epoch_ordinal == ordinal)
    {
        Some(entry) => entry.cleanup_complete = true,
        None => index.entries.push(LifecycleEpochEntry {
            epoch_ordinal: ordinal,
            predecessor_epoch: ordinal.checked_sub(1),
            digest,
            cleanup_complete: true,
        }),
    }
    index.entries.sort_by_key(|entry| entry.epoch_ordinal);
}

/// Drop the oldest half of cleanup-complete entries once the entry bound is
/// reached, keeping full lineage of not-yet-cleaned epochs.
#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
pub(crate) fn compact_lifecycle_index(
    index: &mut LifecycleIndexV1,
) -> Result<bool, ApplicationError> {
    if index.entries.len() < index.max_entries {
        return Ok(false);
    }
    let retain_from = index.entries[index.max_entries / 2].epoch_ordinal;
    index
        .compact(retain_from)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    Ok(true)
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
#[allow(clippy::too_many_lines)]
pub(crate) fn apply_retention_cleanup(
    root: &Path,
    catalog: &EpochCatalogV1,
    config: &NormalizedRecorderConfig,
    quotas: &[QuotaReservationAuthority],
) -> Result<(), ApplicationError> {
    let RetentionMode::DeleteAfter {
        min_age_seconds,
        max_retained_bytes,
    } = config.retention
    else {
        return Ok(());
    };
    let active_path = catalog.active().map(|entry| root.join(&entry.path));
    let mut index = load_lifecycle_index(root)?;
    let mut cleaned_epochs = BTreeSet::new();
    let mut cleaned_digests = BTreeMap::new();
    let now = SystemTime::now();
    let mut candidates = Vec::new();
    for entry in &catalog.epochs {
        let directory = root.join(&entry.path);
        if active_path.as_ref() == Some(&directory) {
            continue;
        }
        let segments = directory.join("segments");
        if !segments.is_dir() {
            continue;
        }
        for item in fs::read_dir(&segments).map_err(ApplicationError::Io)? {
            let item = item.map_err(ApplicationError::Io)?;
            if item
                .path()
                .extension()
                .is_none_or(|extension| extension != "chwal")
            {
                continue;
            }
            let metadata = item.metadata().map_err(ApplicationError::Io)?;
            let age = now
                .duration_since(metadata.modified().unwrap_or(now))
                .unwrap_or_default();
            if age < Duration::from_secs(min_age_seconds) {
                continue;
            }
            candidates.push((
                metadata.modified().unwrap_or(now),
                entry.ordinal,
                item.path(),
                metadata.len(),
            ));
        }
    }
    candidates.sort_by_key(|candidate| candidate.0);
    let mut retained = candidates.iter().map(|candidate| candidate.3).sum::<u64>();
    for (_, ordinal, source, bytes) in candidates {
        let eligible = max_retained_bytes.is_none_or(|limit| retained > limit);
        if !eligible {
            break;
        }
        let authorities = quotas
            .iter()
            .filter(|quota| source.starts_with(Path::new(&quota.quota().domain)))
            .collect::<Vec<_>>();
        if authorities.len() != 1 {
            return Err(ApplicationError::InvalidConfig(
                "retention source must belong to exactly one quota domain".into(),
            ));
        }
        let quota = authorities[0];
        quota
            .reserve(ReservationKind::Trash, bytes)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        let source_bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = quota.release(ReservationKind::Trash, bytes);
                return Err(ApplicationError::Io(error));
            }
        };
        let digest: [u8; 32] = Sha256::digest(&source_bytes).into();
        let digest = sha256_string(&digest)
            .trim_start_matches("sha256:")
            .to_owned();
        let intent = source
            .parent()
            .unwrap_or(root)
            .join(format!("cleanup-intent-{ordinal}-{digest}.json"));
        let cleanup = cleanup_finalized_segment_verified(
            &source,
            &digest,
            root.join("retention-trash"),
            &intent,
            format!("epoch-{ordinal}-{digest}"),
        );
        if let Err(error) = cleanup {
            let _ = quota.release(ReservationKind::Trash, bytes);
            return Err(ApplicationError::InvalidConfig(error.to_string()));
        }
        quota
            .consume(ReservationKind::Trash, bytes)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        quota
            .rebuild_managed_usage()
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        retained = retained.saturating_sub(bytes);
        cleaned_epochs.insert(ordinal);
        cleaned_digests.insert(ordinal, digest);
    }
    if !cleaned_epochs.is_empty() {
        for ordinal in cleaned_epochs {
            let Some(entry) = catalog.epochs.iter().find(|entry| entry.ordinal == ordinal) else {
                continue;
            };
            let directory = root.join(&entry.path);
            let remaining = match fs::read_dir(directory.join("segments")) {
                Ok(entries) => entries
                    .filter_map(Result::ok)
                    .any(|item| item.path().extension().is_some_and(|ext| ext == "chwal")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => return Err(ApplicationError::Io(error)),
            };
            if !remaining {
                let digest = cleaned_digests.remove(&ordinal).unwrap_or_default();
                mark_epoch_cleanup_complete(&mut index, ordinal, digest);
            }
        }
        compact_lifecycle_index(&mut index)?;
        save_lifecycle_index(root, &index)?;
    }
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn reserve_recording_quota(
    config: &NormalizedRecorderConfig,
    wal_root: &Path,
    current_wal_bytes: u64,
    quotas: &[QuotaReservationAuthority],
) -> Result<(), RecorderStartupError> {
    let wal_authorities = quotas
        .iter()
        .filter(|quota| wal_root.starts_with(Path::new(&quota.quota().domain)))
        .collect::<Vec<_>>();
    if wal_authorities.len() != 1 {
        return Err(RecorderStartupError::RecoveryDetail(
            "WAL root must belong to exactly one quota domain".into(),
        ));
    }
    let wal_quota = wal_authorities[0];
    wal_quota.register_managed_root(wal_root)?;
    wal_quota.refresh_from_filesystem()?;
    let wal_reservation = config.epoch.max_bytes.saturating_sub(current_wal_bytes);
    if wal_reservation > 0 {
        wal_quota.reserve(ReservationKind::Wal, wal_reservation)?;
    }

    let store_authorities = quotas
        .iter()
        .filter(|quota| {
            config
                .store_root
                .starts_with(Path::new(&quota.quota().domain))
        })
        .collect::<Vec<_>>();
    if store_authorities.len() != 1 {
        if wal_reservation > 0 {
            let _ = wal_quota.release(ReservationKind::Wal, wal_reservation);
        }
        return Err(RecorderStartupError::RecoveryDetail(
            "store root must belong to exactly one quota domain".into(),
        ));
    }
    let store_quota = store_authorities[0];
    if let Err(error) = store_quota.refresh_from_filesystem() {
        if wal_reservation > 0 {
            let _ = wal_quota.release(ReservationKind::Wal, wal_reservation);
        }
        return Err(error.into());
    }
    if let Err(error) =
        store_quota.reserve(ReservationKind::Staging, config.store.max_staging_bytes)
    {
        if wal_reservation > 0 {
            let _ = wal_quota.release(ReservationKind::Wal, wal_reservation);
        }
        return Err(error.into());
    }
    Ok(())
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
fn persist_startup_recorder_metadata(
    config: &NormalizedRecorderConfig,
    catalog: Option<&EpochCatalogV1>,
    quotas: &[QuotaReservationAuthority],
    lifecycle: RecorderLifecycleState,
    recorder_id: uuid::Uuid,
    attempt_id: uuid::Uuid,
) -> Result<(), RecorderStartupError> {
    let current_epoch = catalog.and_then(|value| {
        value.active().map(|entry| EpochMetadata {
            ordinal: entry.ordinal,
            recording_id: entry.recording_id.0,
        })
    });
    let config_digest = config
        .stable_digest()
        .map_err(|_| RecorderStartupError::Recovery)?;
    let quota = quotas
        .iter()
        .map(|authority| {
            Ok(QuotaStatus {
                domain: authority.quota().domain.clone(),
                quota_bytes: authority.quota().quota_bytes,
                free_bytes: authority.available_bytes(),
                reserved_bytes: authority
                    .reserved_bytes()
                    .map_err(|_| RecorderStartupError::Recovery)?,
            })
        })
        .collect::<Result<Vec<_>, RecorderStartupError>>()?;
    write_recorder_metadata(
        &config.state_root,
        &RecorderMetadataV1 {
            version: RECORDER_METADATA_SCHEMA_VERSION,
            recorder_id,
            attempt_id,
            config_digest,
            scope: config.scope.clone(),
            boot_clock_identity: "continuous-recorder".into(),
            lifecycle,
            capture_readiness: RecorderReadiness::NotReady,
            processing_readiness: RecorderReadiness::NotReady,
            health: RecorderHealth::Degraded,
            current_epoch,
            previous_epoch: None,
            active_segment: None,
            commit: None,
            incremental_checkpoint: None,
            lag: LagSummary {
                records: 0,
                bytes: 0,
                age_seconds: 0,
            },
            quota,
            counters: RecorderCounters::default(),
            recovery: Some(RecoverySummary {
                code: MetadataCode::CrashRecovered,
                repaired_tail: false,
            }),
            shutdown: None,
            failure: None,
            updated_at_unix_seconds: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            checksum: String::new(),
        },
    )
    .map(|_| ())
    .map_err(|_| RecorderStartupError::Recovery)
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
#[allow(clippy::too_many_lines)]
pub fn record_continuous_ebpf(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    config: &NormalizedRecorderConfig,
    wal_directory: impl AsRef<Path>,
    stop: &ProductionSignalStop,
) -> Result<ProductionRecordingResult, ApplicationError> {
    let root_wal_directory = wal_directory.as_ref().to_path_buf();
    let mut wal_directory = root_wal_directory.clone();
    let mut epoch_catalog = match fs::metadata(root_wal_directory.join(EPOCH_CATALOG_FILE)) {
        Ok(_) => Some(
            EpochCatalogV1::load(&root_wal_directory)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ApplicationError::Io(error)),
    };
    let mut starting_epoch_ordinal = if let Some(catalog) = &epoch_catalog {
        wal_directory = catalog
            .resolve_active_path(&root_wal_directory)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        catalog.active().map_or(0, |entry| entry.ordinal)
    } else {
        0
    };
    if matches!(&selector, CgroupSelector::Pid(_)) && allow_shared_cgroup {
        return Err(ApplicationError::ProductionPreflight(
            "shared cgroup acknowledgement requires explicit cgroup",
        ));
    }
    let pid_baseline = match &selector {
        CgroupSelector::Pid(pid) => {
            Some(preflight_pid_cgroup_selection(*pid).map_err(|_| {
                ApplicationError::ProductionPreflight("PID cgroup selection invalid")
            })?)
        }
        CgroupSelector::Explicit(_) => None,
    };
    let selection = match &pid_baseline {
        Some(baseline) => baseline.selection().clone(),
        None => preflight_cgroup_selection(selector, allow_shared_cgroup)
            .map_err(|_| ApplicationError::ProductionPreflight("cgroup selection invalid"))?,
    };
    let bounds = ProductionRecordingBounds {
        duration_seconds: config.epoch.max_age_seconds,
        segment_bytes: config.segment.max_bytes,
        max_wal_bytes: config.epoch.max_bytes,
    };
    preflight_embedded_ebpf()?;
    let capture_metadata = live_capture_metadata(&selection, bounds)?;
    if !config.domains.iter().any(|domain| {
        let root = Path::new(&domain.identity.canonical_root);
        wal_directory.starts_with(root)
            && config.state_root.starts_with(root)
            && config.store_root.starts_with(root)
    }) {
        return Err(ApplicationError::InvalidConfig(
            "WAL, state, and store must share a managed filesystem domain".into(),
        ));
    }
    let initial_wal_directory = wal_directory.clone();
    let initial_wal_bytes = recording_physical_wal_bytes(&initial_wal_directory)?;
    let metadata_catalog = epoch_catalog.clone();
    let startup_recorder_id = metadata_catalog
        .as_ref()
        .and_then(|catalog| catalog.active())
        .map_or_else(uuid::Uuid::new_v4, |entry| entry.recording_id.0);
    let startup_attempt_id = uuid::Uuid::new_v4();
    let startup = RecorderStartup::prepare_foundation_with_metadata(
        config,
        |quotas, lifecycle| {
            persist_startup_recorder_metadata(
                config,
                metadata_catalog.as_ref(),
                quotas,
                lifecycle,
                startup_recorder_id,
                startup_attempt_id,
            )
        },
        |quotas| {
            recover_cleanup_for_root(&root_wal_directory)?;
            load_lifecycle_index(&root_wal_directory)
                .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
            recover_rollover_transition_for_root(
                &config.state_root,
                &root_wal_directory,
                epoch_catalog.as_mut(),
                quotas,
                config.epoch.max_bytes,
            )?;
            if let Some(catalog) = epoch_catalog.as_mut() {
                catalog
                    .recover_prepared_tail(&root_wal_directory)
                    .map_err(|_| RecorderStartupError::Recovery)?;
            }
            Ok(())
        },
        |quotas| reserve_recording_quota(config, &root_wal_directory, initial_wal_bytes, quotas),
    )
    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    if let Some(catalog) = &epoch_catalog {
        let recovered_directory = catalog
            .resolve_active_path(&root_wal_directory)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        if recovered_directory != initial_wal_directory {
            let old_reserved = config.epoch.max_bytes.saturating_sub(initial_wal_bytes);
            if old_reserved > 0 {
                for quota in startup.quota_authorities() {
                    if initial_wal_directory.starts_with(Path::new(&quota.quota().domain)) {
                        quota
                            .release(ReservationKind::Wal, old_reserved)
                            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
                    }
                }
            }
            let recovered_bytes = recording_physical_wal_bytes(&recovered_directory)?;
            for quota in startup.quota_authorities() {
                if recovered_directory.starts_with(Path::new(&quota.quota().domain)) {
                    quota
                        .reserve(
                            ReservationKind::Wal,
                            config.epoch.max_bytes.saturating_sub(recovered_bytes),
                        )
                        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
                }
            }
        }
        wal_directory = recovered_directory;
        starting_epoch_ordinal = catalog.active().map_or(0, |entry| entry.ordinal);
    }
    let startup_predecessor = epoch_catalog
        .as_ref()
        .and_then(|catalog| catalog.active())
        .and_then(|entry| entry.predecessor);
    let mut startup_successor = None;
    let existing_recording_id = if wal_directory.exists() {
        let metadata = load_recording_metadata(&wal_directory)?.ok_or(
            ApplicationError::RecordingMetadataValidation(
                "recording metadata missing during WAL recovery".into(),
            ),
        )?;
        if matches!(
            metadata.status,
            RecordingStatus::Starting | RecordingStatus::Recording
        ) {
            let _ = fs::remove_file(wal_directory.join("etl-checkpoint.json"));
            Some(metadata.recording_id)
        } else {
            let next_recording_id = RecordingId::new();
            wal_directory = root_wal_directory.join("epochs").join(format!(
                "{}-{}",
                starting_epoch_ordinal.saturating_add(1),
                next_recording_id.0
            ));
            starting_epoch_ordinal = starting_epoch_ordinal.saturating_add(1);
            startup_successor = Some((metadata.recording_id, next_recording_id));
            Some(next_recording_id)
        }
    } else {
        if epoch_catalog.is_some() {
            return Err(ApplicationError::RecordingMetadataValidation(
                "active epoch path missing from WAL recovery".into(),
            ));
        }
        None
    };
    if let Some((predecessor_id, next_recording_id)) = startup_successor {
        preflight_wal_destination(&wal_directory, bounds)?;
        let current_wal_bytes = recording_physical_wal_bytes(&initial_wal_directory)?;
        let old_reserved = config.epoch.max_bytes.saturating_sub(current_wal_bytes);
        if old_reserved > 0 {
            for quota in startup.quota_authorities() {
                if initial_wal_directory.starts_with(Path::new(&quota.quota().domain)) {
                    quota
                        .release(ReservationKind::Wal, old_reserved)
                        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
                }
            }
        }
        let reserve_result = startup
            .quota_authorities()
            .iter()
            .filter(|quota| wal_directory.starts_with(Path::new(&quota.quota().domain)))
            .try_for_each(|quota| {
                quota
                    .reserve(ReservationKind::Wal, config.epoch.max_bytes)
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))
            });
        if let Err(error) = reserve_result {
            if old_reserved > 0 {
                for quota in startup.quota_authorities() {
                    if initial_wal_directory.starts_with(Path::new(&quota.quota().domain)) {
                        let _ = quota.reserve(ReservationKind::Wal, old_reserved);
                    }
                }
            }
            return Err(error);
        }
        if epoch_catalog.is_none() {
            epoch_catalog = Some(
                EpochCatalogV1::new(EpochCatalogEntry {
                    ordinal: starting_epoch_ordinal.saturating_sub(1),
                    recording_id: predecessor_id,
                    predecessor: None,
                    path: ".".into(),
                    state: EpochCatalogState::Active,
                })
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
            );
        }
        let path = wal_directory
            .strip_prefix(&root_wal_directory)
            .map_err(|_| ApplicationError::InvalidConfig("epoch path escaped root".into()))?;
        let catalog = epoch_catalog.as_mut().ok_or_else(|| {
            ApplicationError::InvalidConfig("epoch catalog initialization failed".into())
        })?;
        catalog
            .append(EpochCatalogEntry {
                ordinal: starting_epoch_ordinal,
                recording_id: next_recording_id,
                predecessor: Some(predecessor_id),
                path: path.to_string_lossy().into_owned(),
                state: EpochCatalogState::Prepared,
            })
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        catalog
            .write_atomic(&root_wal_directory)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    }
    if !wal_directory.exists() && epoch_catalog.is_none() {
        // WAL destination mutation and appendability preflight require startup ownership.
        preflight_wal_destination(&wal_directory, bounds)?;
    }
    let recording_id = existing_recording_id.unwrap_or_else(RecordingId::new);
    let writer = if wal_directory.exists() {
        prepare_group_commit_reopen(
            &wal_directory,
            recording_id,
            config.segment.max_bytes,
            config.epoch.max_bytes,
            monotonic_millis(),
        )?
        .apply()?
        .0
    } else {
        GroupCommitWalWriter::create_with_total_limit_and_age(
            &wal_directory,
            recording_id,
            config.segment.max_bytes,
            config.epoch.max_bytes,
            1,
            monotonic_millis(),
            config.segment.max_age_seconds.saturating_mul(1_000),
        )?
    };
    let source = load_production_ebpf_source(&selection, pid_baseline.as_ref())?;
    let predecessor = startup_predecessor;
    let recorder = RecorderOrchestrator::new_with_epoch_and_predecessor(
        source,
        RecordingIngest::new(writer),
        starting_epoch_ordinal,
        predecessor,
    );
    let mut service = ContinuousRecorderService::start_with_prepared_startup(
        config,
        &wal_directory,
        starting_epoch_ordinal,
        recorder,
        startup,
        || Ok(()),
        || Ok(()),
    )
    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    service
        .set_capture_metadata(capture_metadata)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    if startup_successor.is_some() {
        let catalog = epoch_catalog.as_mut().ok_or_else(|| {
            ApplicationError::InvalidConfig("epoch catalog initialization failed".into())
        })?;
        catalog
            .activate_latest()
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        catalog
            .write_atomic(&root_wal_directory)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    }
    let mut epoch_started = Instant::now();
    let reason = loop {
        if let Some(requested) = stop.shutdown_reason() {
            break requested;
        }
        let now = monotonic_millis();
        let _ = service
            .poll(now)
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
        let physical = recording_physical_wal_bytes(&wal_directory)?;
        let age_expired = epoch_started.elapsed().as_secs() >= config.epoch.max_age_seconds;
        let size_expired = physical
            >= config
                .epoch
                .max_bytes
                .saturating_sub(config.segment.max_bytes);
        if age_expired || size_expired {
            if service.recorder().ingest().commit_boundary().is_none() {
                // Do not create durable epochs for time/segment headers alone.
                epoch_started = Instant::now();
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let next_recording_id = RecordingId::new();
            let next_wal_directory = root_wal_directory.join("epochs").join(format!(
                "{}-{}",
                service.recorder().epoch_ordinal().saturating_add(1),
                next_recording_id.0
            ));
            // Reserve successor peak before creating its WAL. If headroom is
            // unavailable, old epoch remains authoritative and rollover is
            // not attempted.
            service
                .reserve_successor_quota(&next_wal_directory, config.epoch.max_bytes)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            if let Err(error) = service.begin_rollover_transition(
                &next_wal_directory,
                next_recording_id,
                config.epoch.max_bytes,
            ) {
                let _ =
                    service.release_successor_quota(&next_wal_directory, config.epoch.max_bytes);
                return Err(ApplicationError::InvalidConfig(error.to_string()));
            }
            let old_wal_directory = wal_directory.clone();
            let old_path = old_wal_directory
                .strip_prefix(&root_wal_directory)
                .map_err(|_| ApplicationError::InvalidConfig("epoch path escaped root".into()))?;
            let new_path = next_wal_directory
                .strip_prefix(&root_wal_directory)
                .map_err(|_| ApplicationError::InvalidConfig("epoch path escaped root".into()))?;
            if epoch_catalog.is_none() {
                let old_path = if old_path.as_os_str().is_empty() {
                    ".".to_owned()
                } else {
                    old_path.to_string_lossy().into_owned()
                };
                epoch_catalog = Some(
                    EpochCatalogV1::new(EpochCatalogEntry {
                        ordinal: service.recorder().epoch_ordinal(),
                        recording_id: service.recorder().ingest().recording_id(),
                        predecessor: None,
                        path: old_path,
                        state: EpochCatalogState::Active,
                    })
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
                );
            }
            let catalog = epoch_catalog.as_mut().ok_or_else(|| {
                ApplicationError::InvalidConfig("epoch catalog initialization failed".into())
            })?;
            catalog
                .append(EpochCatalogEntry {
                    ordinal: service.recorder().epoch_ordinal().saturating_add(1),
                    recording_id: next_recording_id,
                    predecessor: Some(service.recorder().ingest().recording_id()),
                    path: new_path.to_string_lossy().into_owned(),
                    state: EpochCatalogState::Prepared,
                })
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            if let Err(error) = catalog.write_atomic(&root_wal_directory) {
                let _ =
                    service.release_successor_quota(&next_wal_directory, config.epoch.max_bytes);
                let _ = service.complete_rollover_transition();
                return Err(ApplicationError::InvalidConfig(error.to_string()));
            }
            if let Err(error) = fs::create_dir_all(&next_wal_directory) {
                let _ = catalog.epochs.pop();
                let catalog_rolled_back = catalog.write_atomic(&root_wal_directory).is_ok();
                let _ =
                    service.release_successor_quota(&next_wal_directory, config.epoch.max_bytes);
                if catalog_rolled_back {
                    let _ = service.complete_rollover_transition();
                }
                return Err(ApplicationError::Io(error));
            }
            let next_writer = match GroupCommitWalWriter::create_with_total_limit_and_age(
                &next_wal_directory,
                next_recording_id,
                config.segment.max_bytes,
                config.epoch.max_bytes,
                1,
                monotonic_millis(),
                config.segment.max_age_seconds.saturating_mul(1_000),
            ) {
                Ok(writer) => writer,
                Err(error) => {
                    let _ = fs::remove_dir_all(&next_wal_directory);
                    let _ = catalog.epochs.pop();
                    let catalog_rolled_back = catalog.write_atomic(&root_wal_directory).is_ok();
                    let _ = service
                        .release_successor_quota(&next_wal_directory, config.epoch.max_bytes);
                    if catalog_rolled_back {
                        let _ = service.complete_rollover_transition();
                    }
                    return Err(error.into());
                }
            };
            let boundary = match service.rollover_to(
                &next_wal_directory,
                next_writer,
                now,
                old_wal_directory.join("epoch-outcomes.json"),
            ) {
                Ok(boundary) => boundary,
                Err(error) => {
                    // Service may have crossed a durable boundary; leave evidence
                    // and transition journal for lease-owned restart recovery.
                    return Err(ApplicationError::InvalidConfig(error.to_string()));
                }
            };
            if boundary.new_recording_id != next_recording_id {
                return Err(ApplicationError::RecordingMetadataValidation(
                    "successor recording identity changed during handoff".into(),
                ));
            }
            catalog
                .activate_latest()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            catalog
                .write_atomic(&root_wal_directory)
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            service
                .complete_rollover_transition()
                .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
            wal_directory = next_wal_directory;
            epoch_started = Instant::now();
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let result = service
        .shutdown(
            reason,
            monotonic_millis(),
            Duration::from_secs(config.shutdown.timeout_seconds),
        )
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    service
        .finalize_incremental_session()
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    let last_valid_commit = service.recorder().ingest().commit_boundary();
    let recording_id = service.recorder().ingest().recording_id();
    let registry = chronicle_protocol_builtins::registry()?;
    let epoch_directories = if let Some(catalog) = &epoch_catalog {
        let mut directories = Vec::with_capacity(catalog.epochs.len());
        for entry in &catalog.epochs {
            let directory = root_wal_directory.join(&entry.path);
            let metadata = load_recording_metadata(&directory)?.ok_or(
                ApplicationError::RecordingMetadataValidation(
                    "catalog epoch metadata missing during ETL".into(),
                ),
            )?;
            if metadata.recording_id != entry.recording_id {
                return Err(ApplicationError::RecordingMetadataValidation(
                    "catalog epoch identity mismatch during ETL".into(),
                ));
            }
            directories.push(directory);
        }
        let epochs_directory = root_wal_directory.join("epochs");
        if epochs_directory.exists() {
            for epoch in fs::read_dir(&epochs_directory)
                .map_err(ApplicationError::Io)?
                .collect::<Result<Vec<_>, _>>()?
            {
                if epoch.file_type().map_err(ApplicationError::Io)?.is_dir()
                    && !catalog
                        .epochs
                        .iter()
                        .any(|entry| root_wal_directory.join(&entry.path) == epoch.path())
                {
                    return Err(ApplicationError::RecordingMetadataValidation(
                        "orphan epoch directory is not in catalog".into(),
                    ));
                }
            }
        }
        directories
    } else {
        vec![root_wal_directory.clone()]
    };
    let store_authorities = service
        .startup()
        .quota_authorities()
        .iter()
        .filter(|quota| {
            config
                .store_root
                .starts_with(Path::new(&quota.quota().domain))
        })
        .collect::<Vec<_>>();
    if store_authorities.len() != 1 {
        return Err(ApplicationError::InvalidConfig(
            "store root must belong to exactly one quota domain".into(),
        ));
    }
    let store_quota = store_authorities[0];
    let etl_result = (|| {
        for directory in epoch_directories {
            if has_published_wal(&directory)? {
                process_and_publish_recording_wal_owned(
                    &directory,
                    &config.store_root,
                    &registry,
                    store_quota,
                )?;
            }
        }
        if let Some(catalog) = &epoch_catalog {
            apply_retention_cleanup(
                &root_wal_directory,
                catalog,
                config,
                service.startup().quota_authorities(),
            )?;
        }
        Ok::<(), ApplicationError>(())
    })();
    let release_result = service.release_production_reservations();
    etl_result?;
    release_result.map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    drop(service);
    Ok(ProductionRecordingResult {
        recording_id,
        status: result.status,
        shutdown_reason: result.shutdown_reason,
        last_valid_commit,
        counters: result.counters,
        terminal_wal_loss: result.terminal_wal_loss,
        source_summary: CaptureSourceSummary::default(),
    })
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) fn live_capture_metadata(
    selection: &CgroupSelection,
    bounds: ProductionRecordingBounds,
) -> Result<RecordingCaptureMetadata, ApplicationError> {
    let kernel_release = fs::read_to_string("/proc/sys/kernel/osrelease")
        .map_err(|_| ApplicationError::ProductionPreflight("kernel identity unavailable"))?
        .trim()
        .to_owned();
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| ApplicationError::ProductionPreflight("boot clock identity unavailable"))?
        .trim()
        .to_owned();
    let ebpf_object_sha256 = chronicle_capture_ebpf::embedded_object_sha256().ok_or(
        ApplicationError::ProductionPreflight("eBPF object unavailable"),
    )?;
    let recording_bounds = RecordingBounds {
        duration_seconds: bounds.duration_seconds,
        max_wal_bytes: bounds.max_wal_bytes,
        segment_bytes: bounds.segment_bytes,
        ring_bytes: 8 * 1024 * 1024,
    };
    Ok(RecordingCaptureMetadata {
        version: RECORDING_CAPTURE_METADATA_SCHEMA_VERSION,
        build: RecordingBuildIdentity {
            chronicle_version: env!("CARGO_PKG_VERSION").into(),
            aya_version: "0.14.0".into(),
            aya_ebpf_version: "0.2.1".into(),
            ebpf_object_sha256,
        },
        host: RecordingHostIdentity {
            kernel_release,
            architecture: std::env::consts::ARCH.into(),
            boot_id,
        },
        scope: RecordingSelectorScope {
            direct_tgid_count: selection.direct_tgid_count(),
            descendant_cgroup_count: selection.descendant_cgroup_count(),
            selected_subtree: true,
            shared_scope_acknowledged: selection.shared_scope_acknowledged,
        },
        capabilities: BTreeSet::from(["CAP_BPF".into(), "CAP_NET_ADMIN".into()]),
        configured_bounds: recording_bounds.clone(),
        effective_bounds: recording_bounds,
        errors: Vec::new(),
    })
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub(crate) fn monotonic_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingStatus {
    Starting,
    Recording,
    Completed,
    Failed,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShutdownReason {
    UserInterrupt,
    TerminationSignal,
    SourceCompleted,
    DurationLimit,
    WalSizeLimit,
    CaptureFailure,
    WalFailure,
    ProcessCrashRecovered,
    ForcedTermination,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSelectorIdentity {
    pub canonical_cgroup_path: String,
    pub cgroup_id: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordByteCount {
    pub records: u64,
    pub bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCounters {
    pub received: RecordByteCount,
    pub accepted_into_queue: RecordByteCount,
    pub written_not_committed: RecordByteCount,
    pub committed: RecordByteCount,
    pub discarded_from_queue_due_to_wal_limit: RecordByteCount,
    pub kernel_or_backend_dropped: RecordByteCount,
    pub rejected_after_stop: RecordByteCount,
    #[serde(default)]
    pub rejected_due_to_quota: RecordByteCount,
    pub etl_checkpointed: RecordByteCount,
}

pub const INGEST_QUEUE_CAPACITY: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedWalRecord {
    pub kind: RecordKind,
    pub schema_version: u16,
    pub flags: u16,
    pub payload: Vec<u8>,
    /// Capture-domain time. Never substitute queue, flush, or wall-clock time.
    pub capture_timestamp: Option<MonotonicTimestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IngestAdmission {
    Accepted,
    RejectedAfterStop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IngestState {
    Accepting,
    Stopping(ShutdownReason),
    Terminal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingIngestResult {
    pub status: RecordingStatus,
    pub shutdown_reason: ShutdownReason,
    pub counters: RecordingCounters,
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProductionRecordingResult {
    pub recording_id: RecordingId,
    pub status: RecordingStatus,
    pub shutdown_reason: ShutdownReason,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub counters: RecordingCounters,
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
    pub source_summary: CaptureSourceSummary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWalLossPersistence {
    PendingWal,
    PersistedWal,
    MetadataOnly,
    NotPersistedDueToWalFailure,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalWalLossTimeSource {
    Observed,
    FallbackLastPersisted,
    TimestampUnavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWalLossSummaryEntry {
    pub clock: Option<ClockIdentity>,
    pub start: Option<MonotonicTimestamp>,
    pub end: Option<MonotonicTimestamp>,
    pub discarded: RecordByteCount,
    pub time_source: TerminalWalLossTimeSource,
    pub persistence: TerminalWalLossPersistence,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalWalLossSummary {
    pub entries: Vec<TerminalWalLossSummaryEntry>,
    pub discarded: RecordByteCount,
}

#[derive(Debug)]
pub struct RecordingIngestFailure {
    pub error: WalError,
    pub result: RecordingIngestResult,
}

impl std::fmt::Display for RecordingIngestFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "recording ingest failed: {}", self.error)
    }
}

impl std::error::Error for RecordingIngestFailure {}

/// Application-owned bounded queue. WAL remains sole authority for physical capacity.
pub struct RecordingIngest {
    pub(crate) writer: GroupCommitWalWriter,
    queue: VecDeque<QueuedWalRecord>,
    state: IngestState,
    counters: RecordingCounters,
    written: RecordByteCount,
    written_records: BTreeMap<u64, RecordByteCount>,
    written_capture_timestamps: BTreeMap<u64, MonotonicTimestamp>,
    persisted_capture_timestamps: BTreeMap<ClockIdentity, MonotonicTimestamp>,
    terminal_discarded: BTreeMap<ClockIdentity, TerminalDiscardAccumulator>,
    terminal_discarded_without_time: RecordByteCount,
    pub(crate) terminal_wal_loss: Option<TerminalWalLossSummary>,
    last_valid_commit: Option<RecordingCommitBoundary>,
}

#[derive(Clone, Debug, Default)]
struct TerminalDiscardAccumulator {
    discarded: RecordByteCount,
    start: Option<MonotonicTimestamp>,
    end: Option<MonotonicTimestamp>,
}

impl RecordingIngest {
    pub fn new(writer: GroupCommitWalWriter) -> Self {
        let authority = writer.authority().clone();
        let counters = RecordingCounters {
            committed: RecordByteCount {
                records: authority.durable_record_count,
                bytes: authority.durable_payload_bytes,
            },
            ..RecordingCounters::default()
        };
        let last_valid_commit = authority
            .marker_sequence
            .zip(authority.durable_through_sequence)
            .zip(authority.segment_ordinal)
            .map(
                |((marker_sequence, durable_through_sequence), segment_ordinal)| {
                    RecordingCommitBoundary {
                        marker_sequence,
                        durable_through_sequence,
                        durable_record_count: authority.durable_record_count,
                        durable_payload_bytes: authority.durable_payload_bytes,
                        segment_ordinal,
                    }
                },
            );
        Self {
            writer,
            queue: VecDeque::with_capacity(INGEST_QUEUE_CAPACITY),
            state: IngestState::Accepting,
            counters,
            written: RecordByteCount::default(),
            written_records: BTreeMap::new(),
            written_capture_timestamps: BTreeMap::new(),
            persisted_capture_timestamps: BTreeMap::new(),
            terminal_discarded: BTreeMap::new(),
            terminal_discarded_without_time: RecordByteCount::default(),
            terminal_wal_loss: None,
            last_valid_commit,
        }
    }

    /// Returns the unqueued record on backpressure so callers can block and retry without loss.
    pub fn admit(&mut self, record: QueuedWalRecord) -> Result<IngestAdmission, QueuedWalRecord> {
        let bytes = record_bytes(&record);
        match self.state {
            IngestState::Accepting if self.queue.len() < INGEST_QUEUE_CAPACITY => {
                add_count(&mut self.counters.received, bytes);
                add_count(&mut self.counters.accepted_into_queue, bytes);
                self.queue.push_back(record);
                Ok(IngestAdmission::Accepted)
            }
            IngestState::Accepting => Err(record),
            IngestState::Stopping(_) | IngestState::Terminal => {
                add_count(&mut self.counters.rejected_after_stop, bytes);
                Ok(IngestAdmission::RejectedAfterStop)
            }
        }
    }

    pub fn record_quota_rejection(&mut self, bytes: u64) {
        add_count(&mut self.counters.rejected_due_to_quota, bytes);
    }

    pub fn record_kernel_or_backend_drop(&mut self, records: u64, bytes: u64) {
        self.counters.kernel_or_backend_dropped.records = self
            .counters
            .kernel_or_backend_dropped
            .records
            .saturating_add(records);
        self.counters.kernel_or_backend_dropped.bytes = self
            .counters
            .kernel_or_backend_dropped
            .bytes
            .saturating_add(bytes);
    }

    pub fn queue_len(&self) -> usize {
        self.queue.len()
    }

    pub fn counters(&self) -> &RecordingCounters {
        &self.counters
    }

    pub fn recording_id(&self) -> RecordingId {
        self.writer.recording_id()
    }

    pub fn next_sequence(&self) -> u64 {
        self.writer.next_sequence()
    }

    pub fn commit_boundary(&self) -> Option<RecordingCommitBoundary> {
        let authority = self.writer.authority();
        match (
            authority.marker_sequence,
            authority.durable_through_sequence,
            authority.segment_ordinal,
        ) {
            (Some(marker_sequence), Some(durable_through_sequence), Some(segment_ordinal)) => {
                Some(RecordingCommitBoundary {
                    marker_sequence,
                    durable_through_sequence,
                    durable_record_count: authority.durable_record_count,
                    durable_payload_bytes: authority.durable_payload_bytes,
                    segment_ordinal,
                })
            }
            _ => self.last_valid_commit.clone(),
        }
    }

    pub fn stop_reason(&self) -> Option<ShutdownReason> {
        match self.state {
            IngestState::Stopping(reason) => Some(reason),
            IngestState::Accepting | IngestState::Terminal => None,
        }
    }

    /// Writes FIFO until empty or WAL's physical-capacity check refuses the next record.
    pub fn drain(&mut self, now_millis: u64) -> Result<(), WalError> {
        while self.state == IngestState::Accepting {
            let Some(record) = self.queue.pop_front() else {
                break;
            };
            let bytes = record_bytes(&record);
            let QueuedWalRecord {
                kind,
                schema_version,
                flags,
                payload,
                capture_timestamp,
            } = record;
            // On a write/flush error WAL may have accepted the frame before reporting failure.
            // Count it as written-not-committed uncertainty; capacity rejection is preflight-only.
            add_count(&mut self.written, bytes);
            match self
                .writer
                .append(kind, schema_version, flags, payload, now_millis)
            {
                Ok(append) => {
                    self.written_records
                        .insert(append.sequence, RecordByteCount { records: 1, bytes });
                    if let Some(timestamp) = capture_timestamp {
                        self.written_capture_timestamps
                            .insert(append.sequence, timestamp);
                    }
                    self.refresh_durability();
                }
                Err(error) if error.is_capacity_exhausted() => {
                    subtract_count(&mut self.written, bytes);
                    self.record_wal_limit_discard(bytes, capture_timestamp);
                    while let Some(discarded) = self.queue.pop_front() {
                        let bytes = record_bytes(&discarded);
                        self.record_wal_limit_discard(bytes, discarded.capture_timestamp);
                    }
                    self.build_terminal_wal_loss_summary();
                    self.state = IngestState::Stopping(ShutdownReason::WalSizeLimit);
                    self.refresh_durability();
                    break;
                }
                Err(error) => {
                    self.state = IngestState::Stopping(ShutdownReason::WalFailure);
                    self.refresh_durability();
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    /// Seal current epoch and replace its writer without detaching capture ownership.
    pub fn rollover_to(
        &mut self,
        next_writer: GroupCommitWalWriter,
        now_millis: u64,
    ) -> Result<Option<RecordingCommitBoundary>, WalError> {
        self.drain(now_millis)?;
        self.writer.shutdown(now_millis)?;
        let boundary = self.commit_boundary();
        self.last_valid_commit.clone_from(&boundary);
        self.writer = next_writer;
        self.reset_after_rollover();
        Ok(boundary)
    }

    /// Seal current epoch and reopen same WAL ownership without constructing a second lock.
    pub fn rollover(
        &mut self,
        now_millis: u64,
    ) -> Result<Option<RecordingCommitBoundary>, WalError> {
        self.drain(now_millis)?;
        self.writer.shutdown(now_millis)?;
        let boundary = self.commit_boundary();
        self.last_valid_commit.clone_from(&boundary);
        let first_sequence = self.writer.next_sequence();
        self.writer.rollover(now_millis, first_sequence)?;
        self.reset_after_rollover();
        Ok(boundary)
    }

    /// Release the active epoch's WAL lock (call only after finish/shutdown).
    /// flock(2) treats a second fd for the same file in the same process as a
    /// conflicting owner, so the final in-process ETL must run after this.
    pub fn release_wal_lock(&mut self) {
        self.writer.release_recording_lock();
    }

    fn reset_after_rollover(&mut self) {
        self.queue.clear();
        self.written_records.clear();
        self.written_capture_timestamps.clear();
        self.terminal_discarded.clear();
        self.terminal_discarded_without_time = RecordByteCount::default();
        self.terminal_wal_loss = None;
        self.state = IngestState::Accepting;
    }

    /// WAL-limit detection wins over duration when both occur in one shutdown cycle.
    #[allow(clippy::result_large_err)] // Failure includes safe final counters needed by callers.
    pub fn finish(
        &mut self,
        requested_reason: ShutdownReason,
        now_millis: u64,
    ) -> Result<RecordingIngestResult, RecordingIngestFailure> {
        if let Err(error) = self.drain(now_millis) {
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        let shutdown_reason = match self.state {
            IngestState::Accepting => {
                self.state = IngestState::Stopping(requested_reason);
                requested_reason
            }
            IngestState::Stopping(reason) => reason,
            IngestState::Terminal => requested_reason,
        };
        if shutdown_reason == ShutdownReason::WalSizeLimit
            && let Err(error) = self.append_terminal_wal_loss(now_millis)
        {
            self.refresh_durability();
            self.mark_terminal_loss_not_persisted();
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        if let Err(error) = self.writer.shutdown(now_millis) {
            self.refresh_durability();
            self.mark_terminal_loss_not_persisted();
            self.state = IngestState::Terminal;
            return Err(RecordingIngestFailure {
                error,
                result: self.failure_result(),
            });
        }
        self.refresh_durability();
        self.mark_terminal_loss_persisted();
        self.state = IngestState::Terminal;
        Ok(RecordingIngestResult {
            status: RecordingStatus::Completed,
            shutdown_reason,
            counters: self.counters.clone(),
            terminal_wal_loss: self.terminal_wal_loss.clone(),
        })
    }

    fn failure_result(&self) -> RecordingIngestResult {
        RecordingIngestResult {
            status: RecordingStatus::Failed,
            shutdown_reason: ShutdownReason::WalFailure,
            counters: self.counters.clone(),
            terminal_wal_loss: self.terminal_wal_loss.clone(),
        }
    }

    fn record_wal_limit_discard(&mut self, bytes: u64, timestamp: Option<MonotonicTimestamp>) {
        add_count(
            &mut self.counters.discarded_from_queue_due_to_wal_limit,
            bytes,
        );
        let Some(timestamp) = timestamp.filter(valid_capture_timestamp) else {
            add_count(&mut self.terminal_discarded_without_time, bytes);
            return;
        };
        let accumulator = self
            .terminal_discarded
            .entry(timestamp.clock.clone())
            .or_default();
        add_count(&mut accumulator.discarded, bytes);
        if accumulator
            .start
            .as_ref()
            .is_none_or(|start| timestamp.nanoseconds < start.nanoseconds)
        {
            accumulator.start = Some(timestamp.clone());
        }
        if accumulator
            .end
            .as_ref()
            .is_none_or(|end| timestamp.nanoseconds > end.nanoseconds)
        {
            accumulator.end = Some(timestamp);
        }
    }

    fn build_terminal_wal_loss_summary(&mut self) {
        let mut entries: Vec<_> = self
            .terminal_discarded
            .iter()
            .map(|(clock, accumulator)| TerminalWalLossSummaryEntry {
                clock: Some(clock.clone()),
                start: accumulator.start.clone(),
                end: accumulator.end.clone(),
                discarded: accumulator.discarded.clone(),
                time_source: TerminalWalLossTimeSource::Observed,
                persistence: TerminalWalLossPersistence::PendingWal,
            })
            .collect();
        if self.terminal_discarded_without_time.records != 0 {
            let fallback = (self.persisted_capture_timestamps.len() == 1)
                .then(|| self.persisted_capture_timestamps.values().next().cloned())
                .flatten();
            entries.push(TerminalWalLossSummaryEntry {
                clock: fallback.as_ref().map(|timestamp| timestamp.clock.clone()),
                start: fallback,
                end: None,
                discarded: self.terminal_discarded_without_time.clone(),
                time_source: if self.persisted_capture_timestamps.len() == 1 {
                    TerminalWalLossTimeSource::FallbackLastPersisted
                } else {
                    TerminalWalLossTimeSource::TimestampUnavailable
                },
                persistence: TerminalWalLossPersistence::MetadataOnly,
            });
        }
        self.terminal_wal_loss = (!entries.is_empty()).then_some(TerminalWalLossSummary {
            entries,
            discarded: self.counters.discarded_from_queue_due_to_wal_limit.clone(),
        });
    }

    fn append_terminal_wal_loss(&mut self, now_millis: u64) -> Result<(), WalError> {
        let Some(summary) = &mut self.terminal_wal_loss else {
            return Ok(());
        };
        for entry in &mut summary.entries {
            if entry.persistence != TerminalWalLossPersistence::PendingWal {
                continue;
            }
            let (Some(start), Some(end)) = (entry.start.clone(), entry.end.clone()) else {
                entry.persistence = TerminalWalLossPersistence::MetadataOnly;
                continue;
            };
            let loss = TerminalWalLoss {
                interval: TerminalWalLossInterval { start, end },
                discarded_records: entry.discarded.records,
                discarded_payload_bytes: entry.discarded.bytes,
                reason: TerminalWalLossReason::WalHardLimit,
                ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects,
            };
            let payload = encode_terminal_wal_loss(&loss)?;
            match self.writer.append(
                RecordKind::TerminalWalLoss,
                TERMINAL_WAL_LOSS_SCHEMA_VERSION,
                0,
                payload,
                now_millis,
            ) {
                Ok(_) => {}
                Err(error) if error.is_capacity_exhausted() => {
                    entry.persistence = TerminalWalLossPersistence::MetadataOnly;
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    fn mark_terminal_loss_persisted(&mut self) {
        if let Some(summary) = &mut self.terminal_wal_loss {
            for entry in &mut summary.entries {
                if entry.persistence == TerminalWalLossPersistence::PendingWal {
                    entry.persistence = TerminalWalLossPersistence::PersistedWal;
                }
            }
        }
    }

    fn mark_terminal_loss_not_persisted(&mut self) {
        if let Some(summary) = &mut self.terminal_wal_loss {
            for entry in &mut summary.entries {
                if entry.persistence == TerminalWalLossPersistence::PendingWal {
                    entry.persistence = TerminalWalLossPersistence::NotPersistedDueToWalFailure;
                }
            }
        }
    }

    fn refresh_durability(&mut self) {
        let authority = self.writer.authority();
        if let Some(durable_through) = authority.durable_through_sequence {
            let durable_sequences: Vec<_> = self
                .written_capture_timestamps
                .range(..=durable_through)
                .map(|(sequence, timestamp)| (*sequence, timestamp.clone()))
                .collect();
            for (sequence, timestamp) in durable_sequences {
                self.written_capture_timestamps.remove(&sequence);
                self.persisted_capture_timestamps
                    .entry(timestamp.clock.clone())
                    .and_modify(|current| {
                        if current.nanoseconds < timestamp.nanoseconds {
                            *current = timestamp.clone();
                        }
                    })
                    .or_insert(timestamp);
            }
        }
        if let Some(durable_through) = authority.durable_through_sequence {
            let durable_sequences: Vec<_> = self
                .written_records
                .range(..=durable_through)
                .map(|(sequence, count)| (*sequence, count.clone()))
                .collect();
            for (sequence, count) in durable_sequences {
                self.written_records.remove(&sequence);
                self.counters.committed.records = self
                    .counters
                    .committed
                    .records
                    .saturating_add(count.records);
                self.counters.committed.bytes =
                    self.counters.committed.bytes.saturating_add(count.bytes);
            }
        }
        self.counters.written_not_committed.records = self
            .written
            .records
            .saturating_sub(self.counters.committed.records);
        self.counters.written_not_committed.bytes = self
            .written
            .bytes
            .saturating_sub(self.counters.committed.bytes);
    }
}

fn valid_capture_timestamp(timestamp: &MonotonicTimestamp) -> bool {
    !timestamp.clock.boot_id.is_empty()
}

fn record_bytes(record: &QueuedWalRecord) -> u64 {
    u64::try_from(record.payload.len()).unwrap_or(u64::MAX)
}

fn add_count(count: &mut RecordByteCount, bytes: u64) {
    count.records = count.records.saturating_add(1);
    count.bytes = count.bytes.saturating_add(bytes);
}

fn subtract_count(count: &mut RecordByteCount, bytes: u64) {
    count.records = count.records.saturating_sub(1);
    count.bytes = count.bytes.saturating_sub(bytes);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCommitBoundary {
    pub marker_sequence: u64,
    pub durable_through_sequence: u64,
    pub durable_record_count: u64,
    pub durable_payload_bytes: u64,
    pub segment_ordinal: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingBuildIdentity {
    pub chronicle_version: String,
    pub aya_version: String,
    pub aya_ebpf_version: String,
    pub ebpf_object_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingHostIdentity {
    pub kernel_release: String,
    pub architecture: String,
    pub boot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingSelectorScope {
    pub direct_tgid_count: usize,
    pub descendant_cgroup_count: usize,
    pub selected_subtree: bool,
    pub shared_scope_acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingBounds {
    pub duration_seconds: u64,
    pub max_wal_bytes: u64,
    pub segment_bytes: u64,
    pub ring_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingCaptureErrorCode {
    Unsupported,
    Capability,
    Object,
    Attach,
    Source,
    LossSample,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCaptureError {
    pub code: RecordingCaptureErrorCode,
}

/// Capture-specific fields appended to core metadata after feasibility validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingCaptureMetadata {
    pub version: u16,
    pub build: RecordingBuildIdentity,
    pub host: RecordingHostIdentity,
    pub scope: RecordingSelectorScope,
    pub capabilities: BTreeSet<String>,
    pub configured_bounds: RecordingBounds,
    pub effective_bounds: RecordingBounds,
    pub errors: Vec<RecordingCaptureError>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMetadata {
    pub version: u16,
    pub recording_id: RecordingId,
    pub selector: Option<RecordingSelectorIdentity>,
    pub status: RecordingStatus,
    pub shutdown_reason: Option<ShutdownReason>,
    pub last_valid_commit: Option<RecordingCommitBoundary>,
    pub counters: RecordingCounters,
    #[serde(default)]
    pub terminal_wal_loss: Option<TerminalWalLossSummary>,
    #[serde(default)]
    pub capture: Option<RecordingCaptureMetadata>,
}

impl RecordingMetadata {
    /// Persist feasibility-checked capture context before live eBPF work starts.
    pub fn transition_to_recording(
        &mut self,
        capture: RecordingCaptureMetadata,
    ) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Starting || self.shutdown_reason.is_some() {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a starting recording may transition to recording".into(),
            ));
        }
        self.capture = Some(capture);
        self.status = RecordingStatus::Recording;
        validate_recording_metadata(self)
    }

    pub fn fail_start(&mut self, reason: ShutdownReason) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Starting {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a starting recording may fail before attachment".into(),
            ));
        }
        self.status = RecordingStatus::Failed;
        self.shutdown_reason = Some(reason);
        validate_recording_metadata(self)
    }

    pub fn finalize(
        &mut self,
        status: RecordingStatus,
        reason: ShutdownReason,
    ) -> Result<(), ApplicationError> {
        if self.status != RecordingStatus::Recording
            || !matches!(status, RecordingStatus::Completed | RecordingStatus::Failed)
        {
            return Err(ApplicationError::RecordingMetadataValidation(
                "only a recording may finalize as completed or failed".into(),
            ));
        }
        self.status = status;
        self.shutdown_reason = Some(reason);
        validate_recording_metadata(self)
    }
}

#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecordingMetadataWriteFault {
    BeforeFileSync,
    BeforeRename,
    AfterRename,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub wal: WalConfig,
    pub capture: CaptureConfig,
    pub postgres: Option<PostgresConfig>,
    pub s3: Option<S3Config>,
    pub protocol_overrides: Vec<ProtocolOverride>,
    pub redaction: RedactionConfig,
    pub replay: ReplayConfig,
    /// Public data directory; overridden by `--data-dir` and `CHRONICLE_DATA_DIR`.
    pub data_dir: Option<PathBuf>,
    /// Optional explicit root for the shared exact `.chronicle-domain.lock` path.
    pub domain_lock_root: Option<PathBuf>,
}

impl AppConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WalConfig {
    pub directory: PathBuf,
    pub segment_size_bytes: u64,
    pub disk_limit_bytes: u64,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("./chronicle-wal"),
            segment_size_bytes: 64 * 1024 * 1024,
            disk_limit_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub process_ids: Vec<u32>,
    pub network_namespaces: Vec<u64>,
    pub ports: Vec<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub connection_url_env: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: Option<String>,
    pub bucket: String,
    pub region: Option<String>,
    pub path_style: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtocolOverride {
    pub port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RedactionConfig {
    pub redact_headers: Vec<String>,
    pub drop_payloads: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // Independent safety gates remain visible in config.
pub struct ReplayConfig {
    pub target_mappings: Vec<TargetMappingConfig>,
    pub dry_run: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub allow_publication: bool,
    pub authorization_env: Option<String>,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            target_mappings: Vec::new(),
            dry_run: true,
            allow_reads: false,
            allow_writes: false,
            allow_publication: false,
            authorization_env: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetMappingConfig {
    pub protocol: Option<String>,
    pub recorded_host: Option<String>,
    pub recorded_port: Option<u16>,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Clone, Debug)]
pub enum ApplicationCommand {
    Record,
    Etl,
    Replay {
        session_id: String,
        timing: ReplayTiming,
    },
    Inspect {
        session_id: String,
    },
    Doctor,
}

#[derive(Clone, Copy, Debug)]
pub enum ReplayTiming {
    Preserve,
    Asap,
}
