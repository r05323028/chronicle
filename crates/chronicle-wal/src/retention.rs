//! Proof-gated WAL segment lifecycle. Production deletion remains disabled.
#![allow(clippy::format_collect, clippy::struct_excessive_bools)]

use crate::SegmentLifecycle;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

pub const RETENTION_PRODUCTION_ENABLED: bool = false;
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
    pub digest: String,
}

pub fn cleanup_finalized_segment(
    source: impl AsRef<Path>,
    trash_root: impl AsRef<Path>,
    intent_path: impl AsRef<Path>,
    transaction_id: impl Into<String>,
) -> Result<CleanupOutcome, RetentionError> {
    if !RETENTION_PRODUCTION_ENABLED {
        return Err(RetentionError::ProductionDisabled);
    }
    let source = source.as_ref();
    let trash_root = trash_root.as_ref();
    let intent_path = intent_path.as_ref();
    if fs::symlink_metadata(source)
        .map_err(|_| RetentionError::Io)?
        .file_type()
        .is_symlink()
    {
        return Err(RetentionError::UnsafePath);
    }
    let bytes = fs::read(source).map_err(|_| RetentionError::Io)?;
    let digest = Sha256::digest(&bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let intent = CleanupIntent {
        version: 1,
        transaction_id: transaction_id.into(),
        source: source.display().to_string(),
        digest,
    };
    write_intent(intent_path, &intent)?;
    fs::create_dir_all(trash_root).map_err(|_| RetentionError::Io)?;
    let target = trash_root.join(source.file_name().ok_or(RetentionError::UnsafePath)?);
    fs::rename(source, &target).map_err(|_| RetentionError::Io)?;
    sync_directory(trash_root)?;
    let tombstone = target.with_extension("deleted");
    fs::write(
        &tombstone,
        serde_json::to_vec(&intent).map_err(|_| RetentionError::Io)?,
    )
    .map_err(|_| RetentionError::Io)?;
    File::open(trash_root)
        .map_err(|_| RetentionError::Io)?
        .sync_all()
        .map_err(|_| RetentionError::Io)?;
    fs::remove_file(&target).map_err(|_| RetentionError::Io)?;
    File::open(trash_root)
        .map_err(|_| RetentionError::Io)?
        .sync_all()
        .map_err(|_| RetentionError::Io)?;
    Ok(CleanupOutcome::Deleted)
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

fn sync_directory(path: &Path) -> Result<(), RetentionError> {
    File::open(path)
        .map_err(|_| RetentionError::Io)?
        .sync_all()
        .map_err(|_| RetentionError::Io)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    Deleted,
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
    fn lifecycle_is_monotonic_and_delete_is_disabled() {
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
        assert_eq!(
            segment.transition(
                SegmentLifecycle::Deleted,
                LifecycleProof {
                    cleanup: true,
                    ..LifecycleProof::default()
                }
            ),
            Err(RetentionError::TransitionDenied)
        );
    }
}
