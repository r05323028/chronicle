//! Proof-gated WAL segment lifecycle. Callers must supply retention proof.
#![allow(clippy::format_collect, clippy::struct_excessive_bools)]

use crate::SegmentLifecycle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

pub const RETENTION_PRODUCTION_ENABLED: bool = true;
const LIFECYCLE_INDEX_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleEpochEntry {
    pub epoch_ordinal: u64,
    pub predecessor_epoch: Option<u64>,
    pub digest: String,
    pub cleanup_complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleIndexV1 {
    pub version: u16,
    pub generation: u64,
    pub transaction_id: String,
    pub source_revision: u64,
    pub source_digest: String,
    pub first_retained_epoch: u64,
    pub last_compacted_epoch: Option<u64>,
    pub entries: Vec<LifecycleEpochEntry>,
    pub max_entries: usize,
    pub max_bytes: usize,
}

impl LifecycleIndexV1 {
    pub fn validate(&self) -> Result<(), RetentionError> {
        if self.version != LIFECYCLE_INDEX_VERSION
            || self.max_entries == 0
            || self.max_bytes == 0
            || self.entries.len() > self.max_entries
        {
            return Err(RetentionError::InvalidIndex);
        }
        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 && self.entries[index - 1].epoch_ordinal >= entry.epoch_ordinal {
                return Err(RetentionError::InvalidIndex);
            }
            if entry.digest.len() != 64
                || !entry.digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(RetentionError::InvalidIndex);
            }
        }
        let bytes = serde_json::to_vec(self)
            .map_err(|_| RetentionError::InvalidIndex)?
            .len();
        if bytes > self.max_bytes {
            return Err(RetentionError::MetadataLimit);
        }
        Ok(())
    }

    pub fn compact(&mut self, retain_from_epoch: u64) -> Result<(), RetentionError> {
        self.validate_without_size()?;
        let before = self.entries.len();
        self.entries
            .retain(|entry| entry.epoch_ordinal >= retain_from_epoch || !entry.cleanup_complete);
        if self
            .entries
            .iter()
            .any(|entry| entry.epoch_ordinal < retain_from_epoch)
        {
            return Err(RetentionError::MetadataLimit);
        }
        if before != self.entries.len() {
            self.last_compacted_epoch = Some(retain_from_epoch.saturating_sub(1));
            self.first_retained_epoch = self
                .entries
                .first()
                .map_or(retain_from_epoch, |entry| entry.epoch_ordinal);
            self.generation = self.generation.saturating_add(1);
        }
        self.validate()
    }

    fn validate_without_size(&self) -> Result<(), RetentionError> {
        if self.version != LIFECYCLE_INDEX_VERSION
            || self.max_entries == 0
            || self.max_bytes == 0
            || self.entries.len() > self.max_entries
        {
            return Err(RetentionError::InvalidIndex);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CleanupIntent {
    pub version: u16,
    pub transaction_id: String,
    pub source: String,
    pub trash_root: String,
    pub digest: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupFault {
    AfterIntent,
    AfterRename,
    AfterTrashSync,
    AfterTombstoneSync,
    AfterUnlink,
}

pub fn cleanup_finalized_segment(
    source: impl AsRef<Path>,
    trash_root: impl AsRef<Path>,
    intent_path: impl AsRef<Path>,
    transaction_id: impl Into<String>,
) -> Result<CleanupOutcome, RetentionError> {
    cleanup_finalized_segment_with_policy(
        source,
        trash_root,
        intent_path,
        transaction_id,
        None,
        RETENTION_PRODUCTION_ENABLED,
        None,
    )
}

pub fn cleanup_finalized_segment_verified(
    source: impl AsRef<Path>,
    expected_digest: &str,
    trash_root: impl AsRef<Path>,
    intent_path: impl AsRef<Path>,
    transaction_id: impl Into<String>,
) -> Result<CleanupOutcome, RetentionError> {
    cleanup_finalized_segment_with_policy(
        source,
        trash_root,
        intent_path,
        transaction_id,
        Some(expected_digest),
        RETENTION_PRODUCTION_ENABLED,
        None,
    )
}

pub fn recover_cleanup(intent_path: impl AsRef<Path>) -> Result<CleanupOutcome, RetentionError> {
    recover_cleanup_with_policy(intent_path, RETENTION_PRODUCTION_ENABLED)
}

pub fn cleanup_finalized_segment_with_policy(
    source: impl AsRef<Path>,
    trash_root: impl AsRef<Path>,
    intent_path: impl AsRef<Path>,
    transaction_id: impl Into<String>,
    expected_digest: Option<&str>,
    production_enabled: bool,
    fault: Option<CleanupFault>,
) -> Result<CleanupOutcome, RetentionError> {
    if !production_enabled {
        return Err(RetentionError::ProductionDisabled);
    }
    let source = source.as_ref();
    let trash_root = trash_root.as_ref();
    let intent_path = intent_path.as_ref();
    let metadata = fs::symlink_metadata(source).map_err(|_| RetentionError::Io)?;
    if metadata.file_type().is_symlink() {
        return Err(RetentionError::UnsafePath);
    }
    let bytes = fs::read(source).map_err(|_| RetentionError::Io)?;
    let digest = digest_bytes(&bytes);
    if expected_digest.is_some_and(|expected| expected != digest) {
        return Err(RetentionError::DigestMismatch);
    }
    let intent = CleanupIntent {
        version: 1,
        transaction_id: transaction_id.into(),
        source: source.display().to_string(),
        trash_root: trash_root.display().to_string(),
        digest,
    };
    write_intent(intent_path, &intent)?;
    if fault == Some(CleanupFault::AfterIntent) {
        return Err(RetentionError::Interrupted);
    }
    fs::create_dir_all(trash_root).map_err(|_| RetentionError::Io)?;
    let target = cleanup_target(trash_root, source, &intent.transaction_id)?;
    fs::rename(source, &target).map_err(|_| RetentionError::Io)?;
    if fault == Some(CleanupFault::AfterRename) {
        return Err(RetentionError::Interrupted);
    }
    sync_directory(trash_root)?;
    if fault == Some(CleanupFault::AfterTrashSync) {
        return Err(RetentionError::Interrupted);
    }
    wait_for_cleanup_commit()?;
    write_tombstone(&target, &intent)?;
    if fault == Some(CleanupFault::AfterTombstoneSync) {
        return Err(RetentionError::Interrupted);
    }
    fs::remove_file(&target).map_err(|_| RetentionError::Io)?;
    if fault == Some(CleanupFault::AfterUnlink) {
        return Err(RetentionError::Interrupted);
    }
    sync_directory(trash_root)?;
    Ok(CleanupOutcome::Deleted)
}

pub fn recover_cleanup_with_policy(
    intent_path: impl AsRef<Path>,
    production_enabled: bool,
) -> Result<CleanupOutcome, RetentionError> {
    if !production_enabled {
        return Err(RetentionError::ProductionDisabled);
    }
    let intent_path = intent_path.as_ref();
    let bytes = fs::read(intent_path).map_err(|_| RetentionError::MissingIntent)?;
    let intent: CleanupIntent =
        serde_json::from_slice(&bytes).map_err(|_| RetentionError::InvalidIntent)?;
    if intent.version != 1
        || !valid_transaction_id(&intent.transaction_id)
        || !valid_digest(&intent.digest)
    {
        return Err(RetentionError::InvalidIntent);
    }
    let source = Path::new(&intent.source);
    let trash_root = Path::new(&intent.trash_root);
    if !source.is_absolute()
        || !trash_root.is_absolute()
        || source
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
        || trash_root
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(RetentionError::UnsafePath);
    }
    fs::create_dir_all(trash_root).map_err(|_| RetentionError::Io)?;
    let target = cleanup_target(trash_root, source, &intent.transaction_id)?;
    if source.exists() && target.exists() {
        // Both authorities exist: choosing one would silently lose evidence.
        return Err(RetentionError::InvalidIntent);
    }
    if source.exists() && !target.exists() {
        if digest_bytes(&fs::read(source).map_err(|_| RetentionError::Io)?) != intent.digest {
            return Err(RetentionError::DigestMismatch);
        }
        fs::rename(source, &target).map_err(|_| RetentionError::Io)?;
        sync_directory(trash_root)?;
    }
    if !target.exists() {
        let tombstone = target.with_extension("deleted");
        return if tombstone.exists() {
            let tombstone_bytes = fs::read(&tombstone).map_err(|_| RetentionError::Io)?;
            let recorded: CleanupIntent = serde_json::from_slice(&tombstone_bytes)
                .map_err(|_| RetentionError::InvalidIntent)?;
            if recorded.transaction_id == intent.transaction_id && recorded.digest == intent.digest
            {
                let outcome = CleanupOutcome::Recovered;
                remove_intent(intent_path)?;
                Ok(outcome)
            } else {
                Err(RetentionError::InvalidIntent)
            }
        } else {
            Err(RetentionError::MissingArtifact)
        };
    }
    if fs::symlink_metadata(&target)
        .map_err(|_| RetentionError::Io)?
        .file_type()
        .is_symlink()
    {
        return Err(RetentionError::UnsafePath);
    }
    if digest_bytes(&fs::read(&target).map_err(|_| RetentionError::Io)?) != intent.digest {
        return Err(RetentionError::DigestMismatch);
    }
    write_tombstone(&target, &intent)?;
    fs::remove_file(&target).map_err(|_| RetentionError::Io)?;
    sync_directory(trash_root)?;
    let outcome = CleanupOutcome::Recovered;
    remove_intent(intent_path)?;
    Ok(outcome)
}

fn cleanup_target(
    trash_root: &Path,
    source: &Path,
    transaction_id: &str,
) -> Result<std::path::PathBuf, RetentionError> {
    let file_name = source.file_name().ok_or(RetentionError::UnsafePath)?;
    Ok(trash_root.join(format!(
        "{}.{}",
        file_name.to_string_lossy(),
        transaction_id
    )))
}

fn valid_transaction_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn write_tombstone(target: &Path, intent: &CleanupIntent) -> Result<(), RetentionError> {
    let tombstone = target.with_extension("deleted");
    fs::write(
        &tombstone,
        serde_json::to_vec(intent).map_err(|_| RetentionError::Io)?,
    )
    .map_err(|_| RetentionError::Io)?;
    sync_directory(target.parent().ok_or(RetentionError::UnsafePath)?)
}

fn remove_intent(intent_path: &Path) -> Result<(), RetentionError> {
    fs::remove_file(intent_path).map_err(|_| RetentionError::Io)?;
    if let Some(parent) = intent_path.parent() {
        sync_directory(parent).map_err(|_| RetentionError::Io)?;
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn write_intent(path: &Path, intent: &CleanupIntent) -> Result<(), RetentionError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| RetentionError::Io)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path).map_err(|_| RetentionError::Io)?;
    file.write_all(&serde_json::to_vec(intent).map_err(|_| RetentionError::Io)?)
        .map_err(|_| RetentionError::Io)?;
    file.sync_all().map_err(|_| RetentionError::Io)
}

fn wait_for_cleanup_commit() -> Result<(), RetentionError> {
    let Ok(control_path) = std::env::var("CHRONICLE_CLEANUP_PAUSE_FILE") else {
        return Ok(());
    };
    let control_path = Path::new(&control_path);
    let ready_path = control_path.with_extension("ready");
    fs::write(&ready_path, b"ready").map_err(|_| RetentionError::Io)?;
    while control_path.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = fs::remove_file(ready_path);
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), RetentionError> {
    File::open(path)
        .map_err(|_| RetentionError::Io)?
        .sync_all()
        .map_err(|_| RetentionError::Io)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    Deleted,
    Recovered,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentLifecycleRecord {
    pub ordinal: u64,
    pub digest: String,
    pub state: SegmentLifecycle,
    pub tombstone_generation: Option<u64>,
}

impl SegmentLifecycleRecord {
    pub fn transition(
        &mut self,
        next: SegmentLifecycle,
        proof: LifecycleProof,
    ) -> Result<(), RetentionError> {
        let allowed = match (self.state, next) {
            (SegmentLifecycle::Active, SegmentLifecycle::Sealed) => proof.sealed,
            (SegmentLifecycle::Sealed, SegmentLifecycle::Processed) => proof.processed,
            (SegmentLifecycle::Processed, SegmentLifecycle::RetentionEligible) => proof.retention,
            (SegmentLifecycle::RetentionEligible, SegmentLifecycle::Deleting) => {
                proof.retention && proof.cleanup
            }
            (SegmentLifecycle::Deleting, SegmentLifecycle::Deleted) => {
                proof.cleanup && RETENTION_PRODUCTION_ENABLED
            }
            (_, SegmentLifecycle::Quarantined) => proof.corruption,
            _ => false,
        };
        if !allowed {
            return Err(RetentionError::TransitionDenied);
        }
        self.state = next;
        if next == SegmentLifecycle::Deleted {
            self.tombstone_generation =
                Some(self.tombstone_generation.unwrap_or(0).saturating_add(1));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LifecycleProof {
    pub sealed: bool,
    pub processed: bool,
    pub retention: bool,
    pub cleanup: bool,
    pub corruption: bool,
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RetentionError {
    #[error("WAL segment lifecycle transition denied")]
    TransitionDenied,
    #[error("retention production deletion is disabled")]
    ProductionDisabled,
    #[error("retention cleanup path is unsafe")]
    UnsafePath,
    #[error("retention cleanup I/O failed")]
    Io,
    #[error("retention lifecycle index is invalid")]
    InvalidIndex,
    #[error("retention lifecycle metadata limit exhausted")]
    MetadataLimit,
    #[error("retention cleanup digest precondition mismatched")]
    DigestMismatch,
    #[error("retention cleanup intent is missing")]
    MissingIntent,
    #[error("retention cleanup intent is invalid")]
    InvalidIntent,
    #[error("retention cleanup artifact is missing")]
    MissingArtifact,
    #[error("retention cleanup interrupted after durable step")]
    Interrupted,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_index_compacts_only_cleanup_complete_epochs() {
        let mut index = LifecycleIndexV1 {
            version: 1,
            generation: 0,
            transaction_id: "tx".into(),
            source_revision: 1,
            source_digest: "a".repeat(64),
            first_retained_epoch: 0,
            last_compacted_epoch: None,
            entries: (0..3)
                .map(|epoch| LifecycleEpochEntry {
                    epoch_ordinal: epoch,
                    predecessor_epoch: epoch.checked_sub(1),
                    digest: "b".repeat(64),
                    cleanup_complete: epoch < 2,
                })
                .collect(),
            max_entries: 8,
            max_bytes: 4096,
        };
        index.compact(2).unwrap();
        assert_eq!(
            index
                .entries
                .iter()
                .map(|entry| entry.epoch_ordinal)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(index.last_compacted_epoch, Some(1));
    }

    #[test]
    fn cleanup_recovers_after_each_durable_step() {
        let faults = [
            CleanupFault::AfterIntent,
            CleanupFault::AfterRename,
            CleanupFault::AfterTrashSync,
            CleanupFault::AfterTombstoneSync,
            CleanupFault::AfterUnlink,
        ];
        for (index, fault) in faults.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "chronicle-retention-{index}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&root);
            let trash = root.join("trash");
            let source = root.join("segment.chwal");
            let intent = root.join("delete.intent");
            fs::create_dir_all(&root).unwrap();
            fs::write(&source, b"sealed-segment").unwrap();
            let digest = digest_bytes(b"sealed-segment");
            assert_eq!(
                cleanup_finalized_segment_with_policy(
                    &source,
                    &trash,
                    &intent,
                    "tx",
                    Some(&digest),
                    true,
                    Some(fault)
                ),
                Err(RetentionError::Interrupted)
            );
            assert_eq!(
                recover_cleanup_with_policy(&intent, true),
                Ok(CleanupOutcome::Recovered)
            );
            assert!(!source.exists());
            assert!(!trash.join("segment.chwal.tx").exists());
            assert!(trash.join("segment.chwal.deleted").exists());
            let _ = fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn cleanup_rejects_digest_mismatch_and_missing_intent() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-retention-mismatch-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let source = root.join("segment.chwal");
        let intent = root.join("delete.intent");
        let trash = root.join("trash");
        fs::write(&source, b"sealed-segment").unwrap();
        assert_eq!(
            cleanup_finalized_segment_with_policy(
                &source,
                &trash,
                &intent,
                "tx",
                Some("0".repeat(64).as_str()),
                true,
                None
            ),
            Err(RetentionError::DigestMismatch)
        );
        assert!(!intent.exists());
        assert_eq!(
            recover_cleanup_with_policy(&intent, true),
            Err(RetentionError::MissingIntent)
        );
        assert_eq!(
            cleanup_finalized_segment(&source, &trash, &intent, "tx"),
            Ok(CleanupOutcome::Deleted)
        );
        assert!(!source.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corruption_evidence_preservation_is_terminal_and_not_deletable() {
        let mut segment = SegmentLifecycleRecord {
            ordinal: 1,
            digest: "a".repeat(64),
            state: SegmentLifecycle::Sealed,
            tombstone_generation: None,
        };
        assert!(
            segment
                .transition(
                    SegmentLifecycle::Quarantined,
                    LifecycleProof {
                        corruption: true,
                        ..LifecycleProof::default()
                    },
                )
                .is_ok()
        );
        assert_eq!(
            segment.transition(
                SegmentLifecycle::Deleted,
                LifecycleProof {
                    cleanup: true,
                    ..LifecycleProof::default()
                },
            ),
            Err(RetentionError::TransitionDenied)
        );
    }

    #[test]
    fn lifecycle_is_monotonic_and_delete_is_enabled_after_proof() {
        let mut segment = SegmentLifecycleRecord {
            ordinal: 1,
            digest: "a".repeat(64),
            state: SegmentLifecycle::Active,
            tombstone_generation: None,
        };
        assert_eq!(
            segment.transition(SegmentLifecycle::Processed, LifecycleProof::default()),
            Err(RetentionError::TransitionDenied)
        );
        segment
            .transition(
                SegmentLifecycle::Sealed,
                LifecycleProof {
                    sealed: true,
                    ..LifecycleProof::default()
                },
            )
            .unwrap();
        segment
            .transition(
                SegmentLifecycle::Processed,
                LifecycleProof {
                    processed: true,
                    ..LifecycleProof::default()
                },
            )
            .unwrap();
        segment
            .transition(
                SegmentLifecycle::RetentionEligible,
                LifecycleProof {
                    retention: true,
                    ..LifecycleProof::default()
                },
            )
            .unwrap();
        segment
            .transition(
                SegmentLifecycle::Deleting,
                LifecycleProof {
                    retention: true,
                    cleanup: true,
                    ..LifecycleProof::default()
                },
            )
            .unwrap();
        segment
            .transition(
                SegmentLifecycle::Deleted,
                LifecycleProof {
                    cleanup: true,
                    ..LifecycleProof::default()
                },
            )
            .unwrap();
        assert_eq!(segment.state, SegmentLifecycle::Deleted);
        assert_eq!(segment.tombstone_generation, Some(1));
    }
}
