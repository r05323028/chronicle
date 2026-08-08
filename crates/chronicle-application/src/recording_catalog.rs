//! Application-owned advisory recording catalog (v1) and bounded scans.
//!
//! `catalog.json` is additive, private, and advisory: authoritative WAL,
//! canonical-session, and session-manifest formats remain unchanged and win
//! over catalog facts. Public mutation requires the exact domain lock held by
//! the caller; this module performs no locking itself.

use crate::domain_lock::DomainLockGuard;
use crate::{
    ApplicationError, FilesystemSessionStore, RecordingStatus, load_recording_metadata,
    write_private_atomic_json,
};
use chronicle_common::{RecordingId, SessionId, Timestamp};
use chronicle_storage::{SESSION_SUMMARY_LIMIT, SessionSummary};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub const CATALOG_FILE: &str = "catalog.json";
pub const CATALOG_MAX_BYTES: u64 = 16 * 1024 * 1024;
pub const CATALOG_MAX_ENTRIES: usize = 10_000;
pub const RECORDING_INTENT_FILE: &str = "recording-intent.json";
pub const RECORDING_INTENT_MAX_BYTES: u64 = 4 * 1024;
pub const RECORDINGS_SUBDIR: &str = "recordings";

/// Stable catalog statuses (advisory lifecycle/name/child facts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingCatalogStatus {
    InProgress,
    Recoverable,
    Published,
    Failed,
    Inconsistent,
}

/// Structured child result of the recorded command (null when no child ran).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChildExitResult {
    ExitCode { code: i32 },
    Signal { signal: i32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogEntryV1 {
    pub recording_id: RecordingId,
    pub name: Option<String>,
    pub created_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub status: RecordingCatalogStatus,
    pub session_id: Option<SessionId>,
    pub child_exit: Option<ChildExitResult>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogV1 {
    pub version: u32,
    pub entries: Vec<CatalogEntryV1>,
}

impl CatalogV1 {
    pub fn empty() -> Self {
        Self {
            version: 1,
            entries: Vec::new(),
        }
    }
}

/// Validate recording-name rules: exact UTF-8, 1–128 bytes, no control
/// characters, not `latest`, no case-sensitive `rec_` prefix.
pub fn validate_recording_name(name: &str) -> Result<(), ApplicationError> {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return Err(ApplicationError::InvalidRecordingName(
            name.to_owned(),
            "must be 1-128 bytes".into(),
        ));
    }
    if bytes.iter().any(u8::is_ascii_control) {
        return Err(ApplicationError::InvalidRecordingName(
            name.to_owned(),
            "must not contain control characters".into(),
        ));
    }
    if name == "latest" {
        return Err(ApplicationError::InvalidRecordingName(
            name.to_owned(),
            "'latest' is reserved".into(),
        ));
    }
    if name.starts_with("rec_") {
        return Err(ApplicationError::InvalidRecordingName(
            name.to_owned(),
            "the 'rec_' prefix is reserved for recording IDs".into(),
        ));
    }
    Ok(())
}

/// Structural validation before persisting or trusting a catalog: version 1,
/// entry bounds, and unique recording IDs/names.
pub fn validate_catalog(catalog: &CatalogV1) -> Result<(), ApplicationError> {
    if catalog.version != 1 {
        return Err(ApplicationError::CatalogInvalid(format!(
            "unsupported catalog version {}",
            catalog.version
        )));
    }
    if catalog.entries.len() > CATALOG_MAX_ENTRIES {
        return Err(ApplicationError::CatalogEntryLimit {
            count: catalog.entries.len(),
            limit: CATALOG_MAX_ENTRIES,
        });
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut names = std::collections::BTreeSet::new();
    for entry in &catalog.entries {
        if !ids.insert(entry.recording_id) {
            return Err(ApplicationError::CatalogInvalid(format!(
                "duplicate recording id {}",
                entry.recording_id
            )));
        }
        if let Some(name) = &entry.name {
            if !names.insert(name.clone()) {
                return Err(ApplicationError::CatalogInvalid(format!(
                    "duplicate recording name '{name}'"
                )));
            }
            validate_recording_name(name)?;
        }
    }
    Ok(())
}

/// Resolve the effective status from canonical/WAL/catalog evidence.
///
/// Canonical evidence wins identity/start/end/operation facts and its
/// publication marks `Published`; recovery-authoritative WAL determines
/// recoverability (any un-published persisted WAL is retryable); catalog state
/// supplies advisory lifecycle facts. Direct contradictions surface as
/// `Inconsistent`, never silent overwrite.
pub fn reconcile_entry_status(
    canonical_published: bool,
    wal_incomplete: bool,
    wal_present: bool,
    catalog: Option<RecordingCatalogStatus>,
) -> RecordingCatalogStatus {
    if canonical_published {
        if wal_incomplete {
            RecordingCatalogStatus::Inconsistent
        } else {
            RecordingCatalogStatus::Published
        }
    } else if wal_present {
        // WAL determines recoverability; an advisory published claim without
        // canonical evidence contradicts the un-published WAL.
        if catalog == Some(RecordingCatalogStatus::Published) {
            RecordingCatalogStatus::Inconsistent
        } else {
            RecordingCatalogStatus::Recoverable
        }
    } else if let Some(status) = catalog {
        status
    } else {
        RecordingCatalogStatus::InProgress
    }
}

/// The recording a published session belongs to: `source_provenance.recording_id`
/// when present (ETL-published sessions carry it), else id equality (legacy
/// sessions).
fn recording_for_summary(summary: &SessionSummary) -> RecordingId {
    summary
        .recording_id
        .unwrap_or(RecordingId(summary.session_id.0))
}

/// Index sessions by the recording they belong to.
fn summary_by_recording<'a>(
    summaries: impl Iterator<Item = &'a SessionSummary>,
) -> BTreeMap<RecordingId, &'a SessionSummary> {
    let mut index = BTreeMap::new();
    for summary in summaries {
        index
            .entry(recording_for_summary(summary))
            .or_insert(summary);
    }
    index
}

pub fn catalog_path(data_dir: &Path) -> PathBuf {
    data_dir.join(CATALOG_FILE)
}

pub fn recordings_root(data_dir: &Path) -> PathBuf {
    data_dir.join(RECORDINGS_SUBDIR)
}

/// Resolve a recording reference deterministically: `latest` (newest
/// successfully published by canonical start timestamp, ties broken by the
/// lexicographically greatest recording ID), an exact `rec_<uuid>`/bare UUID
/// identity, or an exact name match. Unresolved references return a typed
/// not-found error (CLI exit 3). Identity resolution precedes name matching.
pub fn resolve_recording(
    data_dir: &Path,
    reference: &str,
) -> Result<RecordingId, ApplicationError> {
    if reference == "latest" {
        return resolve_latest(data_dir);
    }
    if let Ok(id) = RecordingId::parse_cli(reference) {
        let view = reconcile_catalog(data_dir)?;
        if view.entries.iter().any(|entry| entry.recording_id == id) {
            return Ok(id);
        }
        return Err(ApplicationError::RecordingNotFound(reference.to_owned()));
    }
    let view = reconcile_catalog(data_dir)?;
    let matches: Vec<RecordingId> = view
        .entries
        .iter()
        .filter(|entry| entry.name.as_deref() == Some(reference))
        .map(|entry| entry.recording_id)
        .collect();
    match matches.as_slice() {
        [id] => Ok(*id),
        _ => Err(ApplicationError::RecordingNotFound(reference.to_owned())),
    }
}

/// Newest successfully published recording by canonical start timestamp; ties
/// are broken by the lexicographically greatest recording ID. Only recordings
/// with persisted recording directories and published canonical sessions
/// qualify.
fn resolve_latest(data_dir: &Path) -> Result<RecordingId, ApplicationError> {
    let known: BTreeSet<RecordingId> = list_recording_ids(data_dir)?.into_iter().collect();
    let summaries = FilesystemSessionStore::new(data_dir)
        .list_summaries(SESSION_SUMMARY_LIMIT)
        .map_err(ApplicationError::from)?;
    let mut candidates: Vec<_> = summaries
        .iter()
        .filter(|summary| known.contains(&recording_for_summary(summary)))
        .collect();
    candidates.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.session_id.cmp(&right.session_id))
    });
    candidates
        .pop()
        .map(recording_for_summary)
        .ok_or_else(|| ApplicationError::RecordingNotFound("latest".to_owned()))
}

/// Private v1 recording-intent sidecar: ID, optional validated name, and
/// allocation timestamp. Persisted after preflight/scope creation but before
/// capture attachment, while the exact domain lock is held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingIntentV1 {
    pub version: u32,
    pub recording_id: RecordingId,
    pub name: Option<String>,
    pub created_at: Timestamp,
}

pub fn recording_intent_path(data_dir: &Path, id: RecordingId) -> PathBuf {
    recordings_root(data_dir)
        .join(id.to_string())
        .join(RECORDING_INTENT_FILE)
}

/// Persist the recording-intent sidecar under the exact domain lock.
///
/// Passing `guard` (the held [`DomainLockGuard`]) is the required proof of
/// domain ownership: this function cannot be called without a guard value, so
/// a write without the held lock is rejected at compile time. The recording
/// directory is derived from the ID (ID-directory match by construction) and
/// must not be a symlink; serialized size is capped at 4 KiB.
#[allow(unused_variables)]
pub fn write_recording_intent(
    guard: &DomainLockGuard,
    data_dir: &Path,
    intent: &RecordingIntentV1,
) -> Result<(), ApplicationError> {
    if intent.version != 1 {
        return Err(ApplicationError::CatalogInvalid(format!(
            "unsupported recording-intent version {}",
            intent.version
        )));
    }
    if let Some(name) = &intent.name {
        validate_recording_name(name)?;
    }
    let directory = recordings_root(data_dir).join(intent.recording_id.to_string());
    if fs::symlink_metadata(&directory).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err(ApplicationError::CatalogUnsafePath(directory));
    }
    let bytes = serde_json::to_vec(intent)?;
    if bytes.len() as u64 > RECORDING_INTENT_MAX_BYTES {
        return Err(ApplicationError::CatalogInvalid(format!(
            "recording-intent is {} bytes; limit is {RECORDING_INTENT_MAX_BYTES}",
            bytes.len()
        )));
    }
    write_private_atomic_json(&directory, RECORDING_INTENT_FILE, intent, None)
}

/// Read the sidecar for a recording. Missing sidecar yields `None`; symlinked
/// files or directories, oversized files, non-v1 content, ID mismatches, and
/// invalid names fail safely.
pub fn load_recording_intent(
    data_dir: &Path,
    id: RecordingId,
) -> Result<Option<RecordingIntentV1>, ApplicationError> {
    let path = recording_intent_path(data_dir, id);
    // The sidecar's parent directory is derived from the data directory and
    // the bare recording ID, so it always exists as a path.
    let directory = path.parent().unwrap_or(data_dir);
    // ID-directory confinement first: the sidecar's parent directory must be a
    // real directory (never a symlink pointing outside the data directory),
    // before any file lookup resolves through it. A missing parent simply
    // means no sidecar exists.
    match fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ApplicationError::CatalogUnsafePath(directory.to_path_buf()));
        }
        Ok(_) => {}
    }
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ApplicationError::CatalogUnsafePath(path));
            }
            if metadata.len() > RECORDING_INTENT_MAX_BYTES {
                return Err(ApplicationError::CatalogInvalid(format!(
                    "recording-intent is {} bytes; limit is {RECORDING_INTENT_MAX_BYTES}",
                    metadata.len()
                )));
            }
        }
    }
    let bytes = fs::read(&path)?;
    let intent: RecordingIntentV1 = serde_json::from_slice(&bytes).map_err(|error| {
        ApplicationError::CatalogInvalid(format!("malformed recording-intent: {error}"))
    })?;
    if intent.version != 1 {
        return Err(ApplicationError::CatalogInvalid(format!(
            "unsupported recording-intent version {}",
            intent.version
        )));
    }
    if intent.recording_id != id {
        return Err(ApplicationError::CatalogInvalid(format!(
            "recording-intent id {} does not match directory {}",
            intent.recording_id, id
        )));
    }
    if let Some(name) = &intent.name {
        validate_recording_name(name)?;
    }
    Ok(Some(intent))
}

/// Load the catalog. Missing file yields the empty catalog; oversized,
/// non-v1, malformed, or unsafe (symlinked) input yields a typed error that
/// callers may fall back from to bounded in-memory reconciliation.
pub fn load_catalog(data_dir: &Path) -> Result<CatalogV1, ApplicationError> {
    let path = catalog_path(data_dir);
    match fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CatalogV1::empty()),
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ApplicationError::CatalogUnsafePath(path));
            }
            if metadata.len() > CATALOG_MAX_BYTES {
                return Err(ApplicationError::CatalogInvalid(format!(
                    "catalog is {} bytes; limit is {CATALOG_MAX_BYTES}",
                    metadata.len()
                )));
            }
            let bytes = fs::read(&path)?;
            let catalog: CatalogV1 = serde_json::from_slice(&bytes).map_err(|error| {
                ApplicationError::CatalogInvalid(format!("malformed catalog: {error}"))
            })?;
            validate_catalog(&catalog)?;
            Ok(catalog)
        }
    }
}

/// Atomically persist the catalog (16 MiB / 10,000-entry bounds enforced).
/// The caller must hold the exact domain lock; this function performs no
/// locking and does not check ownership.
pub fn save_catalog(data_dir: &Path, catalog: &CatalogV1) -> Result<(), ApplicationError> {
    validate_catalog(catalog)?;
    let bytes = serde_json::to_vec(catalog)?;
    if bytes.len() as u64 > CATALOG_MAX_BYTES {
        return Err(ApplicationError::CatalogInvalid(format!(
            "catalog serializes to {} bytes; limit is {CATALOG_MAX_BYTES}",
            bytes.len()
        )));
    }
    write_private_atomic_json(data_dir, CATALOG_FILE, catalog, None)
}

/// Scan immediate children of `<data>/recordings/`: every entry must be a
/// real directory whose name is a bare UUID (ID-directory match). Symlinks,
/// non-directories, and non-UUID names fail safely with no follow and no
/// recursion. Missing recordings root yields an empty list.
pub fn list_recording_ids(data_dir: &Path) -> Result<Vec<RecordingId>, ApplicationError> {
    let root = recordings_root(data_dir);
    let entries = match fs::read_dir(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
        Ok(entries) => entries,
    };
    if fs::symlink_metadata(&root)?.file_type().is_symlink() {
        return Err(ApplicationError::CatalogUnsafePath(root));
    }
    let mut ids = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ApplicationError::CatalogUnsafePath(path));
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let uuid = Uuid::parse_str(&name)
            .map_err(|_| ApplicationError::CatalogUnsafePath(path.clone()))?;
        ids.push(RecordingId(uuid));
    }
    ids.sort();
    Ok(ids)
}

/// Bounded, read-only catalog reconciliation: build an in-memory view from
/// sidecars, recovery-authoritative WAL metadata, and bounded canonical session
/// summaries. Canonical facts win; WAL determines recoverability; contradictions
/// become `Inconsistent`. Never runs ETL, never re-publishes, and never mutates
/// any file (even when a live owner holds the exact domain lock). A symlinked or
/// otherwise unsafe catalog file fails safely; a malformed/oversized/non-v1
/// advisory catalog falls back to an artifact-only view.
pub fn reconcile_catalog(data_dir: &Path) -> Result<CatalogV1, ApplicationError> {
    let advisory = match load_catalog(data_dir) {
        Ok(catalog) => catalog,
        Err(ApplicationError::CatalogUnsafePath(_)) => {
            return Err(ApplicationError::CatalogUnsafePath(
                data_dir.join(CATALOG_FILE),
            ));
        }
        Err(_) => CatalogV1::empty(),
    };
    let advisory_by_id: BTreeMap<RecordingId, CatalogEntryV1> = advisory
        .entries
        .into_iter()
        .map(|entry| (entry.recording_id, entry))
        .collect();
    let ids = list_recording_ids(data_dir)?;
    let summaries = FilesystemSessionStore::new(data_dir)
        .list_summaries(SESSION_SUMMARY_LIMIT)
        .map_err(ApplicationError::from)?;
    let summary_index = summary_by_recording(summaries.iter());

    let mut entries = Vec::new();
    let mut seen = BTreeSet::new();
    for id in ids {
        seen.insert(id);
        let intent = load_recording_intent(data_dir, id)?;
        let wal_directory = recordings_root(data_dir).join(id.to_string());
        let wal = load_recording_metadata(&wal_directory)?;
        let canonical = summary_index.get(&id);
        let advisory_entry = advisory_by_id.get(&id);
        let canonical_published = canonical.is_some();
        let wal_present = wal.is_some();
        let wal_incomplete =
            wal.is_some_and(|metadata| metadata.status != RecordingStatus::Completed);
        let status = reconcile_entry_status(
            canonical_published,
            wal_incomplete,
            wal_present,
            advisory_entry.map(|entry| entry.status),
        );
        // CREATED always uses the persisted allocation timestamp: sidecar first,
        // then advisory catalog, then canonical start, then a deterministic
        // sentinel for artifact-only entries with no timestamp evidence.
        let created_at = intent
            .as_ref()
            .map(|intent| intent.created_at)
            .or_else(|| advisory_entry.map(|entry| entry.created_at))
            .or_else(|| canonical.map(|summary| summary.started_at))
            .unwrap_or(Timestamp::UNIX_EPOCH);
        let ended_at = canonical
            .and_then(|summary| summary.ended_at)
            .or_else(|| advisory_entry.and_then(|entry| entry.ended_at));
        let name = intent
            .as_ref()
            .and_then(|intent| intent.name.clone())
            .or_else(|| advisory_entry.and_then(|entry| entry.name.clone()));
        entries.push(CatalogEntryV1 {
            recording_id: id,
            name,
            created_at,
            ended_at,
            status,
            session_id: canonical.map(|summary| summary.session_id),
            child_exit: advisory_entry.and_then(|entry| entry.child_exit.clone()),
        });
    }
    // Catalog-only entries (artifacts removed by retention or crash cleanup)
    // survive as advisory facts.
    for (id, entry) in advisory_by_id {
        if !seen.contains(&id) {
            entries.push(entry);
        }
    }
    // Newest first by creation time, tie-broken by recording ID (descending).
    entries.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.recording_id.cmp(&left.recording_id))
    });
    Ok(CatalogV1 {
        version: 1,
        entries,
    })
}

/// Persist the reconciled catalog while the exact domain lock is held. Passing
/// `guard` is the compile-time proof of ownership; read-only reconciliation
/// (`reconcile_catalog`) never persists.
#[allow(unused_variables)]
pub fn persist_reconciled_catalog(
    guard: &DomainLockGuard,
    data_dir: &Path,
) -> Result<(), ApplicationError> {
    let view = reconcile_catalog(data_dir)?;
    save_catalog(data_dir, &view)
}

/// Reserve a recording name under the exact domain lock, before any durable
/// allocation: validates the name rules and rejects collisions against the
/// reconciled catalog. Reserved names (`latest`, `rec_*`) exit via
/// [`validate_recording_name`]; collisions exit 3.
#[allow(unused_variables)]
pub fn claim_recording_name(
    guard: &DomainLockGuard,
    data_dir: &Path,
    name: &str,
) -> Result<(), ApplicationError> {
    validate_recording_name(name)?;
    let view = reconcile_catalog(data_dir)?;
    if view
        .entries
        .iter()
        .any(|entry| entry.name.as_deref() == Some(name))
    {
        return Err(ApplicationError::RecordingNameCollision(name.to_owned()));
    }
    Ok(())
}

/// One row for `list`: recording identity plus canonical session counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedRecording {
    pub recording_id: RecordingId,
    pub name: Option<String>,
    /// Persisted allocation timestamp.
    pub created_at: Timestamp,
    /// Canonical session duration in milliseconds when published.
    pub duration_ms: Option<u64>,
    /// Number of published canonical sessions (one in the 0.1.x model).
    pub sessions: usize,
    /// Total canonical operation count.
    pub operations: usize,
    pub status: RecordingCatalogStatus,
}

/// Resolve the published session id for a recording, if one exists.
pub fn resolve_session(
    data_dir: &Path,
    recording_id: RecordingId,
) -> Result<Option<SessionId>, ApplicationError> {
    let view = reconcile_catalog(data_dir)?;
    Ok(view
        .entries
        .into_iter()
        .find(|entry| entry.recording_id == recording_id)
        .and_then(|entry| entry.session_id))
}

/// Bounded list of every catalog entry with canonical session counts, ordered
/// newest-first by creation time (tie-broken by recording ID descending).
/// Unpublished entries report zero sessions/operations and `None` duration.
pub fn list_recordings(data_dir: &Path) -> Result<Vec<ListedRecording>, ApplicationError> {
    let view = reconcile_catalog(data_dir)?;
    let summaries = FilesystemSessionStore::new(data_dir)
        .list_summaries(SESSION_SUMMARY_LIMIT)
        .map_err(ApplicationError::from)?;
    let summary_index = summary_by_recording(summaries.iter());
    let mut rows = Vec::with_capacity(view.entries.len());
    for entry in view.entries {
        let canonical = entry
            .session_id
            .as_ref()
            .and_then(|_| summary_index.get(&entry.recording_id));
        let duration_ms = match canonical {
            Some(summary) => summary.ended_at.map(|ended| {
                let millis = (ended - summary.started_at).whole_milliseconds().max(0);
                u64::try_from(millis).unwrap_or(u64::MAX)
            }),
            None => None,
        };
        let published = canonical.is_some();
        rows.push(ListedRecording {
            recording_id: entry.recording_id,
            name: entry.name,
            created_at: entry.created_at,
            duration_ms,
            sessions: usize::from(published),
            operations: canonical.map_or(0, |summary| summary.operation_count),
            status: entry.status,
        });
    }
    rows.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.recording_id.cmp(&left.recording_id))
    });
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RecordingMetadata, ShutdownReason};
    use chronicle_canonical::CanonicalSession;
    use time::OffsetDateTime;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("chronicle-catalog-{}", Uuid::new_v4()))
    }

    fn entry(id: &str, name: Option<&str>, status: RecordingCatalogStatus) -> CatalogEntryV1 {
        CatalogEntryV1 {
            recording_id: RecordingId(Uuid::parse_str(id).unwrap()),
            name: name.map(str::to_owned),
            created_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            status,
            session_id: None,
            child_exit: None,
        }
    }

    #[test]
    fn atomic_write_roundtrips() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let mut catalog = CatalogV1::empty();
        catalog.entries.push(entry(
            "00000000-0000-0000-0000-000000000001",
            Some("checkout"),
            RecordingCatalogStatus::Published,
        ));
        save_catalog(&root, &catalog).expect("saves");
        let loaded = load_catalog(&root).expect("loads");
        assert_eq!(loaded, catalog);
        // No temp files remain.
        let leftovers: Vec<_> = fs::read_dir(&root)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty());
        // Overwrite is atomic and single-valued.
        let mut second = CatalogV1::empty();
        second.entries.push(entry(
            "00000000-0000-0000-0000-000000000002",
            None,
            RecordingCatalogStatus::Failed,
        ));
        save_catalog(&root, &second).expect("overwrites");
        assert_eq!(load_catalog(&root).unwrap(), second);
    }

    #[test]
    fn missing_catalog_is_empty() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        assert_eq!(load_catalog(&root).unwrap(), CatalogV1::empty());
    }

    #[test]
    fn non_v1_catalog_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(catalog_path(&root), r#"{"version":2,"entries":[]}"#).unwrap();
        let err = load_catalog(&root).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogInvalid(_)));
        assert!(err.to_string().contains("version"));
    }

    #[test]
    fn oversized_catalog_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let mut catalog = CatalogV1::empty();
        for index in 0..=CATALOG_MAX_ENTRIES {
            let uuid = format!("00000000-0000-0000-0000-{index:012}");
            catalog
                .entries
                .push(entry(&uuid, None, RecordingCatalogStatus::InProgress));
        }
        let err = save_catalog(&root, &catalog).unwrap_err();
        assert!(matches!(
            err,
            ApplicationError::CatalogEntryLimit { count: _, limit: _ }
        ));
    }

    #[test]
    fn malformed_catalog_is_typed_error() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(catalog_path(&root), b"{not json").unwrap();
        let err = load_catalog(&root).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogInvalid(_)));
    }

    #[test]
    fn duplicate_ids_and_names_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let mut catalog = CatalogV1::empty();
        catalog.entries.push(entry(
            "00000000-0000-0000-0000-000000000001",
            Some("dup"),
            RecordingCatalogStatus::InProgress,
        ));
        catalog.entries.push(entry(
            "00000000-0000-0000-0000-000000000001",
            None,
            RecordingCatalogStatus::Failed,
        ));
        assert!(matches!(
            validate_catalog(&catalog),
            Err(ApplicationError::CatalogInvalid(_))
        ));

        let mut named = CatalogV1::empty();
        named.entries.push(entry(
            "00000000-0000-0000-0000-000000000001",
            Some("same"),
            RecordingCatalogStatus::InProgress,
        ));
        named.entries.push(entry(
            "00000000-0000-0000-0000-000000000002",
            Some("same"),
            RecordingCatalogStatus::InProgress,
        ));
        assert!(matches!(
            validate_catalog(&named),
            Err(ApplicationError::CatalogInvalid(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_catalog_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let real = root.join("real-catalog");
        fs::write(&real, r#"{"version":1,"entries":[]}"#).unwrap();
        std::os::unix::fs::symlink(&real, catalog_path(&root)).unwrap();
        let err = load_catalog(&root).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogUnsafePath(_)));
    }

    #[test]
    fn recording_ids_scan_is_immediate_no_follow() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let recordings = recordings_root(&root);
        fs::create_dir_all(recordings.join("00000000-0000-0000-0000-000000000001")).unwrap();
        fs::create_dir_all(recordings.join("00000000-0000-0000-0000-000000000002")).unwrap();
        let ids = list_recording_ids(&root).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&RecordingId(
            Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
        )));

        // Non-UUID directory fails safely.
        fs::create_dir_all(recordings.join("not-a-uuid")).unwrap();
        assert!(matches!(
            list_recording_ids(&root),
            Err(ApplicationError::CatalogUnsafePath(_))
        ));

        // Regular file entry fails safely.
        fs::remove_dir_all(recordings.join("not-a-uuid")).unwrap();
        fs::write(recordings.join("stray.txt"), b"x").unwrap();
        assert!(matches!(
            list_recording_ids(&root),
            Err(ApplicationError::CatalogUnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_recording_dir_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let recordings = recordings_root(&root);
        fs::create_dir_all(&recordings).unwrap();
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(
            &real,
            recordings.join("00000000-0000-0000-0000-000000000001"),
        )
        .unwrap();
        assert!(matches!(
            list_recording_ids(&root),
            Err(ApplicationError::CatalogUnsafePath(_))
        ));
    }

    #[test]
    fn missing_recordings_root_is_empty() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        assert!(list_recording_ids(&root).unwrap().is_empty());
    }

    #[test]
    fn effective_status_precedence() {
        use RecordingCatalogStatus as S;
        // Canonical publication wins.
        assert_eq!(
            reconcile_entry_status(true, false, true, Some(S::InProgress)),
            S::Published
        );
        assert_eq!(
            reconcile_entry_status(true, false, true, Some(S::Recoverable)),
            S::Published
        );
        assert_eq!(
            reconcile_entry_status(true, false, false, None),
            S::Published
        );
        // Canonical + incomplete WAL is a contradiction.
        assert_eq!(
            reconcile_entry_status(true, true, true, None),
            S::Inconsistent
        );
        assert_eq!(
            reconcile_entry_status(true, true, true, Some(S::Published)),
            S::Inconsistent
        );
        // WAL determines recoverability without canonical evidence.
        assert_eq!(
            reconcile_entry_status(false, true, true, None),
            S::Recoverable
        );
        assert_eq!(
            reconcile_entry_status(false, true, true, Some(S::InProgress)),
            S::Recoverable
        );
        assert_eq!(
            reconcile_entry_status(false, true, true, Some(S::Failed)),
            S::Recoverable
        );
        // Completed WAL without canonical evidence is still recoverable
        // (ETL/publication pending or failed).
        assert_eq!(
            reconcile_entry_status(false, false, true, None),
            S::Recoverable
        );
        // Advisory published claim contradicts an un-published WAL.
        assert_eq!(
            reconcile_entry_status(false, false, true, Some(S::Published)),
            S::Inconsistent
        );
        // Without canonical or WAL evidence, advisory state stands.
        assert_eq!(
            reconcile_entry_status(false, false, false, None),
            S::InProgress
        );
        assert_eq!(
            reconcile_entry_status(false, false, false, Some(S::Failed)),
            S::Failed
        );
        assert_eq!(
            reconcile_entry_status(false, false, false, Some(S::Published)),
            S::Published
        );
    }

    #[test]
    fn name_rules() {
        assert!(validate_recording_name("checkout").is_ok());
        assert!(validate_recording_name(&"a".repeat(128)).is_ok());
        assert!(validate_recording_name(&"a".repeat(129)).is_err());
        assert!(validate_recording_name("").is_err());
        assert!(validate_recording_name("line\nbreak").is_err());
        assert!(validate_recording_name("ansi\x1b[31m").is_err());
        assert!(validate_recording_name("latest").is_err());
        assert!(validate_recording_name("rec_abc").is_err());
        assert!(validate_recording_name("REC_abc").is_ok()); // case-sensitive prefix
    }

    fn intent(version: u32, id: &str, name: Option<&str>) -> RecordingIntentV1 {
        RecordingIntentV1 {
            version,
            recording_id: RecordingId(Uuid::parse_str(id).unwrap()),
            name: name.map(str::to_owned),
            created_at: OffsetDateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn intent_write_requires_guard_and_roundtrips() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        // Acquire the exact domain lock to obtain the guard token.
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).expect("lock");
        let id = "00000000-0000-0000-0000-000000000001";
        write_recording_intent(&guard, &root, &intent(1, id, Some("checkout"))).expect("writes");
        let loaded =
            load_recording_intent(&root, RecordingId(Uuid::parse_str(id).unwrap())).expect("loads");
        assert_eq!(loaded, Some(intent(1, id, Some("checkout"))));
        // Sidecar lives at recordings/<bare-uuid>/recording-intent.json.
        assert!(recording_intent_path(&root, RecordingId(Uuid::parse_str(id).unwrap())).is_file());
        // Missing sidecar for another id is None.
        let other = "00000000-0000-0000-0000-000000000099";
        assert_eq!(
            load_recording_intent(&root, RecordingId(Uuid::parse_str(other).unwrap())).unwrap(),
            None
        );
    }

    #[test]
    fn intent_non_v1_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).expect("lock");
        let id = "00000000-0000-0000-0000-000000000001";
        let bad = intent(2, id, None);
        assert!(matches!(
            write_recording_intent(&guard, &root, &bad),
            Err(ApplicationError::CatalogInvalid(_))
        ));
    }

    #[test]
    fn intent_oversize_rejected_on_read() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let dir = recordings_root(&root).join(id);
        fs::create_dir_all(&dir).unwrap();
        // Handcrafted oversized sidecar: exceeds the 4 KiB cap.
        let mut blob = String::from(
            "{\"version\":1,\"recording_id\":\"00000000-0000-0000-0000-000000000001\",\"name\":\"",
        );
        blob.push_str(&"a".repeat(usize::try_from(RECORDING_INTENT_MAX_BYTES).unwrap() + 8));
        blob.push_str("\"}");
        fs::write(dir.join(RECORDING_INTENT_FILE), blob).unwrap();
        let err =
            load_recording_intent(&root, RecordingId(Uuid::parse_str(id).unwrap())).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogInvalid(_)));
    }

    #[cfg(unix)]
    #[test]
    fn intent_symlinked_file_and_dir_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let recording_dir = recordings_root(&root).join(id);
        fs::create_dir_all(&recording_dir).unwrap();
        // Symlinked sidecar file.
        let real = root.join("real-intent");
        fs::write(&real, b"{}").unwrap();
        std::os::unix::fs::symlink(&real, recording_dir.join(RECORDING_INTENT_FILE)).unwrap();
        let err =
            load_recording_intent(&root, RecordingId(Uuid::parse_str(id).unwrap())).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogUnsafePath(_)));
        std::fs::remove_file(recording_dir.join(RECORDING_INTENT_FILE)).unwrap();
        // Symlinked recording directory (path escape).
        std::fs::remove_dir_all(&recording_dir).unwrap();
        let escape = root.join("escape");
        fs::create_dir_all(&escape).unwrap();
        std::os::unix::fs::symlink(&escape, &recording_dir).unwrap();
        let err =
            load_recording_intent(&root, RecordingId(Uuid::parse_str(id).unwrap())).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogUnsafePath(_)));
    }

    #[test]
    fn intent_id_mismatch_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let dir = recordings_root(&root).join(id);
        fs::create_dir_all(&dir).unwrap();
        let other = intent(1, "00000000-0000-0000-0000-000000000002", None);
        fs::write(
            dir.join(RECORDING_INTENT_FILE),
            serde_json::to_vec(&other).unwrap(),
        )
        .unwrap();
        let err =
            load_recording_intent(&root, RecordingId(Uuid::parse_str(id).unwrap())).unwrap_err();
        assert!(matches!(err, ApplicationError::CatalogInvalid(_)));
    }

    #[test]
    fn intent_invalid_name_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).expect("lock");
        let id = "00000000-0000-0000-0000-000000000001";
        let bad = intent(1, id, Some("latest"));
        assert!(matches!(
            write_recording_intent(&guard, &root, &bad),
            Err(ApplicationError::InvalidRecordingName(_, _))
        ));
    }

    fn wal_metadata(
        id: &str,
        status: RecordingStatus,
        reason: Option<ShutdownReason>,
    ) -> RecordingMetadata {
        RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id: RecordingId(Uuid::parse_str(id).unwrap()),
            selector: None,
            status,
            shutdown_reason: reason,
            last_valid_commit: None,
            counters: Default::default(),
            terminal_wal_loss: None,
            capture: None,
        }
    }

    fn minimal_session(id: &str, started_at: Timestamp) -> CanonicalSession {
        minimal_session_ended(id, started_at, None)
    }

    fn minimal_session_ended(
        id: &str,
        started_at: Timestamp,
        ended_at: Option<Timestamp>,
    ) -> CanonicalSession {
        use chronicle_canonical::{
            Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalOperation,
            Completeness, OperationEffect, OperationKind, PayloadRef, ProtocolData,
            RelativeTimeNanos, ReplayMetadata, SourceMetadata, TimelineEntry,
        };
        let connection_id = chronicle_common::ConnectionId::new();
        let operation_id = chronicle_common::OperationId::new();
        CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId(Uuid::parse_str(id).unwrap()),
            started_at,
            ended_at,
            source: SourceMetadata::default(),
            source_provenance: Default::default(),
            connections: vec![CanonicalConnection {
                id: connection_id,
                protocol: chronicle_common::ProtocolId::new("custom/1"),
                client: chronicle_common::Endpoint::new("127.0.0.1", 1),
                server: chronicle_common::Endpoint::new("127.0.0.1", 2),
                attributes: Attributes::new(),
                operations: vec![CanonicalOperation {
                    id: operation_id,
                    sequence: 1,
                    started_at_offset: RelativeTimeNanos(0),
                    completed_at_offset: None,
                    kind: OperationKind::Request,
                    effect: OperationEffect::Unknown,
                    request: PayloadRef::Inline {
                        content_type: Some("application/custom".into()),
                        bytes: b"body".to_vec(),
                    },
                    recorded_response: None,
                    attributes: Attributes::new(),
                    protocol_data: ProtocolData {
                        schema_version: 1,
                        media_type: None,
                        bytes: Vec::new(),
                    },
                    provenance: Default::default(),
                    redactions: Vec::new(),
                    warnings: Vec::new(),
                }],
            }],
            connection_completeness: [(connection_id, Completeness::Complete)].into(),
            operation_completeness: [(operation_id, Completeness::Complete)].into(),
            timeline: vec![TimelineEntry {
                connection_id,
                operation_id,
                offset: RelativeTimeNanos(0),
            }],
            replay: ReplayMetadata::default(),
            replay_attributes: Attributes::new(),
        }
    }

    fn publish_session(root: &Path, id: &str) {
        let store = FilesystemSessionStore::new(root);
        store
            .publish(chronicle_storage::PublishSession {
                session: minimal_session(id, Timestamp::UNIX_EPOCH),
                checkpoint: Some("checkpoint".into()),
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
    }

    fn publish_session_at(root: &Path, id: &str, started_at: Timestamp) {
        let store = FilesystemSessionStore::new(root);
        store
            .publish(chronicle_storage::PublishSession {
                session: minimal_session(id, started_at),
                checkpoint: Some("checkpoint".into()),
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
    }

    fn seed_recording(root: &Path, id: &str, name: Option<&str>, status: RecordingStatus) {
        let guard = crate::domain_lock::acquire_domain_lock(root, None).expect("lock");
        write_recording_intent(&guard, root, &intent(1, id, name)).expect("intent");
        let wal_dir = recordings_root(root).join(id);
        let reason = if matches!(
            status,
            RecordingStatus::Completed | RecordingStatus::Failed | RecordingStatus::Aborted
        ) {
            Some(ShutdownReason::SourceCompleted)
        } else {
            None
        };
        crate::write_recording_metadata(&wal_dir, &wal_metadata(id, status, reason)).expect("wal");
        drop(guard);
    }

    #[test]
    fn reconcile_adopts_empty_catalog_from_artifacts() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("checkout"), RecordingStatus::Completed);
        publish_session(&root, id);

        let view = reconcile_catalog(&root).expect("reconciles");
        assert_eq!(view.entries.len(), 1);
        let entry = &view.entries[0];
        assert_eq!(
            entry.recording_id,
            RecordingId(Uuid::parse_str(id).unwrap())
        );
        assert_eq!(entry.name.as_deref(), Some("checkout"));
        assert_eq!(entry.status, RecordingCatalogStatus::Published);
        assert_eq!(
            entry.session_id,
            Some(SessionId(Uuid::parse_str(id).unwrap()))
        );
        assert_eq!(entry.created_at, OffsetDateTime::UNIX_EPOCH);
        // Nothing was persisted by the read-only reconcile.
        assert!(!catalog_path(&root).exists());
    }

    #[test]
    fn reconcile_recoverable_from_wal_without_canonical() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("pending"), RecordingStatus::Recording);
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Recoverable);
        assert_eq!(view.entries[0].session_id, None);
    }

    #[test]
    fn reconcile_contradiction_canonical_plus_incomplete_wal() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, None, RecordingStatus::Recording);
        publish_session(&root, id);
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Inconsistent);
    }

    #[test]
    fn reconcile_advisory_published_without_canonical_is_inconsistent() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, None, RecordingStatus::Recording);
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).unwrap();
        let mut catalog = CatalogV1::empty();
        catalog
            .entries
            .push(entry(id, None, RecordingCatalogStatus::Published));
        save_catalog(&root, &catalog).unwrap();
        drop(guard);
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Inconsistent);
    }

    #[test]
    fn reconcile_completed_wal_without_canonical_is_recoverable() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        // WAL finalized as Completed but ETL/publication never ran.
        seed_recording(&root, id, None, RecordingStatus::Completed);
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Recoverable);
    }

    #[test]
    fn reconcile_catalog_only_entries_survive() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).unwrap();
        let mut catalog = CatalogV1::empty();
        catalog.entries.push(entry(
            "00000000-0000-0000-0000-000000000001",
            Some("ghost"),
            RecordingCatalogStatus::Failed,
        ));
        save_catalog(&root, &catalog).unwrap();
        drop(guard);
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Failed);
        assert_eq!(view.entries[0].name.as_deref(), Some("ghost"));
    }

    #[test]
    fn reconcile_malformed_catalog_falls_back_to_artifacts() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("name"), RecordingStatus::Recording);
        fs::write(catalog_path(&root), b"{not json").unwrap();
        let view = reconcile_catalog(&root).unwrap();
        assert_eq!(view.entries.len(), 1);
        assert_eq!(view.entries[0].name.as_deref(), Some("name"));
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Recoverable);
    }

    #[cfg(unix)]
    #[test]
    fn reconcile_hostile_symlink_recording_dir_fails_safely() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let recordings = recordings_root(&root);
        fs::create_dir_all(&recordings).unwrap();
        let escape = root.join("escape");
        fs::create_dir_all(&escape).unwrap();
        std::os::unix::fs::symlink(
            &escape,
            recordings.join("00000000-0000-0000-0000-000000000001"),
        )
        .unwrap();
        assert!(matches!(
            reconcile_catalog(&root),
            Err(ApplicationError::CatalogUnsafePath(_))
        ));
    }

    #[test]
    fn reconcile_owned_domain_read_is_non_mutating_and_no_publication() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("owned"), RecordingStatus::Completed);
        publish_session(&root, id);
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).unwrap();
        let sessions_before = list_sessions_snapshot(&root);
        let view = reconcile_catalog(&root).expect("reads while owned");
        assert_eq!(view.entries[0].status, RecordingCatalogStatus::Published);
        // No catalog persisted, no session mutated, no publication happened.
        assert!(!catalog_path(&root).exists());
        assert_eq!(list_sessions_snapshot(&root), sessions_before);
        drop(guard);
    }

    #[test]
    fn persist_reconciled_requires_guard_and_writes_atomic() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("persisted"), RecordingStatus::Completed);
        publish_session(&root, id);
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).unwrap();
        persist_reconciled_catalog(&guard, &root).expect("persists under lock");
        drop(guard);
        let on_disk = load_catalog(&root).expect("loads");
        assert_eq!(on_disk.entries.len(), 1);
        assert_eq!(on_disk.entries[0].name.as_deref(), Some("persisted"));
        assert_eq!(on_disk.entries[0].status, RecordingCatalogStatus::Published);
    }

    #[test]
    fn resolve_by_prefixed_and_bare_id() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("by-id"), RecordingStatus::Completed);
        publish_session(&root, id);
        assert_eq!(
            resolve_recording(&root, &format!("rec_{id}")).unwrap(),
            RecordingId(Uuid::parse_str(id).unwrap())
        );
        assert_eq!(
            resolve_recording(&root, id).unwrap(),
            RecordingId(Uuid::parse_str(id).unwrap())
        );
    }

    #[test]
    fn resolve_by_name_exact_match() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, Some("checkout"), RecordingStatus::Completed);
        publish_session(&root, id);
        assert_eq!(
            resolve_recording(&root, "checkout").unwrap(),
            RecordingId(Uuid::parse_str(id).unwrap())
        );
        // Case-sensitive: different case does not resolve.
        assert!(matches!(
            resolve_recording(&root, "Checkout"),
            Err(ApplicationError::RecordingNotFound(_))
        ));
    }

    #[test]
    fn resolve_latest_by_canonical_started_at() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let older = "00000000-0000-0000-0000-000000000001";
        let newer = "00000000-0000-0000-0000-000000000002";
        seed_recording(&root, older, None, RecordingStatus::Completed);
        seed_recording(&root, newer, None, RecordingStatus::Completed);
        publish_session_at(
            &root,
            older,
            Timestamp::UNIX_EPOCH + time::Duration::seconds(100),
        );
        publish_session_at(
            &root,
            newer,
            Timestamp::UNIX_EPOCH + time::Duration::seconds(200),
        );
        assert_eq!(
            resolve_recording(&root, "latest").unwrap(),
            RecordingId(Uuid::parse_str(newer).unwrap())
        );
    }

    #[test]
    fn resolve_latest_tie_break_lexicographic_id() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        // Identical canonical start times; the lexicographically greatest ID
        // wins the tie deterministically.
        let lower = "00000000-0000-0000-0000-000000000001";
        let higher = "00000000-0000-0000-0000-000000000002";
        seed_recording(&root, lower, None, RecordingStatus::Completed);
        seed_recording(&root, higher, None, RecordingStatus::Completed);
        publish_session(&root, lower);
        publish_session(&root, higher);
        assert_eq!(
            resolve_recording(&root, "latest").unwrap(),
            RecordingId(Uuid::parse_str(higher).unwrap())
        );
    }

    #[test]
    fn resolve_latest_ignores_unpublished_and_unknown_sessions() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        // A session with no recording directory is not a recording.
        publish_session(&root, "00000000-0000-0000-0000-000000000099");
        assert!(matches!(
            resolve_recording(&root, "latest"),
            Err(ApplicationError::RecordingNotFound(_))
        ));
        // A recording with only an incomplete WAL is not published.
        let id = "00000000-0000-0000-0000-000000000001";
        seed_recording(&root, id, None, RecordingStatus::Recording);
        assert!(matches!(
            resolve_recording(&root, "latest"),
            Err(ApplicationError::RecordingNotFound(_))
        ));
    }

    #[test]
    fn resolve_unresolved_errors() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        assert!(matches!(
            resolve_recording(&root, "rec_00000000-0000-0000-0000-00000000dead"),
            Err(ApplicationError::RecordingNotFound(_))
        ));
        assert!(matches!(
            resolve_recording(&root, "no-such-name"),
            Err(ApplicationError::RecordingNotFound(_))
        ));
    }

    #[test]
    fn claim_name_validates_and_rejects_collisions_under_lock() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).expect("lock");
        // Reserved names exit as usage errors (InvalidRecordingName).
        for reserved in ["latest", "rec_abc", "", "line\nbreak"] {
            assert!(matches!(
                claim_recording_name(&guard, &root, reserved),
                Err(ApplicationError::InvalidRecordingName(_, _))
            ));
        }
        // Fresh name claims cleanly.
        claim_recording_name(&guard, &root, "checkout").expect("claims");
        drop(guard);

        // Existing name collides: exit 3 before any durable allocation.
        seed_recording(
            &root,
            "00000000-0000-0000-0000-000000000001",
            Some("checkout"),
            RecordingStatus::Completed,
        );
        let guard = crate::domain_lock::acquire_domain_lock(&root, None).expect("lock");
        let err = claim_recording_name(&guard, &root, "checkout").unwrap_err();
        assert!(matches!(err, ApplicationError::RecordingNameCollision(_)));
        // Case-sensitive: a different-cased name is a different name.
        claim_recording_name(&guard, &root, "Checkout").expect("case-sensitive");
    }

    #[test]
    fn list_recordings_reports_counts_and_newest_first() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let older = "00000000-0000-0000-0000-000000000001";
        let newer = "00000000-0000-0000-0000-000000000002";
        // Newer recording published with a session; older only WAL (recoverable).
        seed_recording(&root, older, Some("old"), RecordingStatus::Recording);
        seed_recording(&root, newer, Some("new"), RecordingStatus::Completed);
        // Publish with an ended session so duration is measurable (62s).
        let started = Timestamp::UNIX_EPOCH;
        let ended = started + time::Duration::seconds(62);
        let store = FilesystemSessionStore::new(&root);
        store
            .publish(chronicle_storage::PublishSession {
                session: minimal_session_ended(newer, started, Some(ended)),
                checkpoint: Some("checkpoint".into()),
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
        let rows = list_recordings(&root).unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(
            rows[0].recording_id,
            RecordingId(Uuid::parse_str(newer).unwrap())
        );
        assert_eq!(
            rows[1].recording_id,
            RecordingId(Uuid::parse_str(older).unwrap())
        );
        // Published row has canonical counts; unpublished row has zeros and null duration.
        assert_eq!(rows[0].sessions, 1);
        assert_eq!(rows[0].operations, 1);
        assert_eq!(rows[0].duration_ms, Some(62_000));
        assert_eq!(rows[0].status, RecordingCatalogStatus::Published);
        assert_eq!(rows[1].sessions, 0);
        assert_eq!(rows[1].operations, 0);
        assert_eq!(rows[1].duration_ms, None);
        assert_eq!(rows[1].status, RecordingCatalogStatus::Recoverable);
    }

    #[test]
    fn list_recordings_empty_storage() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        assert!(list_recordings(&root).unwrap().is_empty());
    }

    fn list_sessions_snapshot(root: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        let sessions = root.join("sessions");
        if let Ok(entries) = fs::read_dir(&sessions) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files.push(path);
                }
            }
        }
        files.sort();
        files
    }
}
