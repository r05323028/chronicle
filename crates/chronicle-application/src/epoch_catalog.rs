use chronicle_common::{EpochId, RecordingId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const EPOCH_CATALOG_FILE: &str = "epochs.json";
const MAX_EPOCHS: usize = 4096;

#[derive(Debug, Error)]
pub enum EpochCatalogError {
    #[error("epoch catalog I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("epoch catalog encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("epoch catalog validation failed: {0}")]
    Invalid(&'static str),
    #[error("epoch catalog checksum mismatch")]
    ChecksumMismatch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EpochCatalogState {
    Prepared,
    #[default]
    Active,
    Finalized,
}

/// Schema version for the parent-aware topology authority in `epochs.json`.
pub const EPOCH_CATALOG_SCHEMA_VERSION: u16 = 2;

/// Derived/cache-only publication flag retained in the epoch index.
///
/// epochs.json is the topology authority: recovery, checksum, and lineage
/// validation never consult this flag, and it never gates ETL, continuation,
/// or retention. It is written during finalization so public list views can
/// report published epochs without scanning the session store.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogSummary {
    #[serde(default)]
    pub published: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogEntry {
    pub ordinal: u64,
    pub epoch_id: EpochId,
    pub predecessor: Option<EpochId>,
    pub successor: Option<EpochId>,
    pub path: String,
    pub state: EpochCatalogState,
    #[serde(default)]
    pub summary: EpochCatalogSummary,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalog {
    pub version: u16,
    pub parent_id: RecordingId,
    pub epochs: Vec<EpochCatalogEntry>,
    pub checksum: String,
}

impl EpochCatalog {
    pub fn new(
        parent_id: RecordingId,
        epoch_id: EpochId,
        path: impl AsRef<Path>,
    ) -> Result<Self, EpochCatalogError> {
        let mut catalog = Self {
            version: EPOCH_CATALOG_SCHEMA_VERSION,
            parent_id,
            epochs: vec![EpochCatalogEntry {
                ordinal: 0,
                epoch_id,
                predecessor: None,
                successor: None,
                path: path_string(path.as_ref())?,
                state: EpochCatalogState::Active,
                summary: EpochCatalogSummary::default(),
            }],
            checksum: String::new(),
        };
        catalog.validate_without_checksum()?;
        catalog.checksum = catalog.compute_checksum()?;
        Ok(catalog)
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self, EpochCatalogError> {
        let path = root.as_ref().join(EPOCH_CATALOG_FILE);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_file() {
            return Err(EpochCatalogError::Invalid(
                "epoch catalog is not a regular file",
            ));
        }
        Self::decode(&fs::read(path)?)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EpochCatalogError> {
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn encode(&self) -> Result<Vec<u8>, EpochCatalogError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum = value.compute_checksum()?;
        let mut bytes = serde_json::to_vec_pretty(&value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn write_atomic(&self, root: impl AsRef<Path>) -> Result<(), EpochCatalogError> {
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let bytes = self.encode()?;
        let temporary = root.join(format!(
            ".{EPOCH_CATALOG_FILE}.v2-tmp-{}",
            uuid::Uuid::new_v4()
        ));
        let result = (|| {
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            let mut file = options.open(&temporary)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            fs::rename(&temporary, root.join(EPOCH_CATALOG_FILE))?;
            fs::File::open(root)?.sync_all()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn topology_digest(&self) -> &str {
        &self.checksum
    }

    pub fn active(&self) -> Option<&EpochCatalogEntry> {
        self.epochs
            .iter()
            .find(|entry| matches!(entry.state, EpochCatalogState::Active))
    }

    pub fn resolve_active_path(
        &self,
        root: impl AsRef<Path>,
    ) -> Result<PathBuf, EpochCatalogError> {
        let entry = self
            .active()
            .ok_or(EpochCatalogError::Invalid("epoch catalog is empty"))?;
        Ok(root.as_ref().join(entry.path.as_str()))
    }

    pub fn epoch(&self, epoch_id: EpochId) -> Option<&EpochCatalogEntry> {
        self.epochs.iter().find(|entry| entry.epoch_id == epoch_id)
    }

    pub fn append_successor(
        &mut self,
        epoch_id: EpochId,
        path: impl AsRef<Path>,
    ) -> Result<(), EpochCatalogError> {
        let previous = self.clone();
        let active_epoch_id = self
            .active()
            .ok_or(EpochCatalogError::Invalid("catalog has no active epoch"))?
            .epoch_id;
        let active_ordinal = self
            .active()
            .ok_or(EpochCatalogError::Invalid("catalog has no active epoch"))?
            .ordinal;
        if self.epochs.len() >= MAX_EPOCHS {
            return Err(EpochCatalogError::Invalid("epoch catalog is full"));
        }
        let ordinal = active_ordinal
            .checked_add(1)
            .ok_or(EpochCatalogError::Invalid("epoch ordinal exhausted"))?;
        let path = path_string(path.as_ref())?;
        if self.epoch(epoch_id).is_some() {
            return Err(EpochCatalogError::Invalid("duplicate epoch identity"));
        }
        if self.epochs.iter().any(|entry| entry.path == path) {
            return Err(EpochCatalogError::Invalid("duplicate epoch path"));
        }
        self.checksum.clear();
        let active_index = self
            .epochs
            .iter()
            .position(|entry| entry.state == EpochCatalogState::Active)
            .ok_or(EpochCatalogError::Invalid("catalog has no active epoch"))?;
        self.epochs[active_index].successor = Some(epoch_id);
        self.epochs.push(EpochCatalogEntry {
            ordinal,
            epoch_id,
            predecessor: Some(active_epoch_id),
            successor: None,
            path,
            state: EpochCatalogState::Prepared,
            summary: EpochCatalogSummary::default(),
        });
        if let Err(error) = self.validate_without_checksum() {
            *self = previous;
            return Err(error);
        }
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn rollback_prepared_successor(
        &mut self,
        epoch_id: EpochId,
    ) -> Result<(), EpochCatalogError> {
        let snapshot = self.clone();
        let Some(last) = self.epochs.last() else {
            return Err(EpochCatalogError::Invalid("catalog is empty"));
        };
        if last.epoch_id != epoch_id || last.state != EpochCatalogState::Prepared {
            return Err(EpochCatalogError::Invalid("successor is not prepared tail"));
        }
        self.checksum.clear();
        self.epochs.pop();
        if let Some(previous) = self.epochs.last_mut() {
            previous.successor = None;
            if previous.state == EpochCatalogState::Finalized {
                previous.state = EpochCatalogState::Active;
            }
        }
        if let Err(error) = self.validate_without_checksum() {
            *self = snapshot;
            return Err(error);
        }
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn activate_successor(&mut self, epoch_id: EpochId) -> Result<(), EpochCatalogError> {
        let previous = self.clone();
        let last = self
            .epochs
            .last()
            .ok_or(EpochCatalogError::Invalid("catalog is empty"))?;
        if last.epoch_id != epoch_id || last.state != EpochCatalogState::Prepared {
            return Err(EpochCatalogError::Invalid("successor is not prepared tail"));
        }
        self.checksum.clear();
        let previous_index = self.epochs.len().saturating_sub(2);
        if self.epochs.len() > 1 {
            self.epochs[previous_index].state = EpochCatalogState::Finalized;
        }
        self.epochs
            .last_mut()
            .ok_or(EpochCatalogError::Invalid("catalog is empty"))?
            .state = EpochCatalogState::Active;
        if let Err(error) = self.validate_without_checksum() {
            *self = previous;
            return Err(error);
        }
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn update_summary(
        &mut self,
        epoch_id: EpochId,
        summary: EpochCatalogSummary,
    ) -> Result<(), EpochCatalogError> {
        let previous = self.clone();
        self.checksum.clear();
        let Some(entry) = self
            .epochs
            .iter_mut()
            .find(|entry| entry.epoch_id == epoch_id)
        else {
            return Err(EpochCatalogError::Invalid("unknown epoch identity"));
        };
        entry.summary = summary;
        if let Err(error) = self.validate_without_checksum() {
            *self = previous;
            return Err(error);
        }
        self.checksum = self.compute_checksum()?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), EpochCatalogError> {
        self.validate_without_checksum()?;
        if self.checksum != self.compute_checksum()? {
            return Err(EpochCatalogError::ChecksumMismatch);
        }
        Ok(())
    }

    fn validate_without_checksum(&self) -> Result<(), EpochCatalogError> {
        if self.version != EPOCH_CATALOG_SCHEMA_VERSION {
            return Err(EpochCatalogError::Invalid(
                "unsupported epoch catalog version",
            ));
        }
        if self.parent_id.as_uuid().is_nil() {
            return Err(EpochCatalogError::Invalid(
                "parent recording identity is nil",
            ));
        }
        if self.epochs.is_empty() || self.epochs.len() > MAX_EPOCHS {
            return Err(EpochCatalogError::Invalid("invalid epoch count"));
        }
        let active_count = self
            .epochs
            .iter()
            .filter(|entry| entry.state == EpochCatalogState::Active)
            .count();
        if active_count != 1 {
            return Err(EpochCatalogError::Invalid(
                "catalog must contain one active epoch",
            ));
        }
        let prepared_count = self
            .epochs
            .iter()
            .filter(|entry| entry.state == EpochCatalogState::Prepared)
            .count();
        if prepared_count > 1
            || self.epochs.iter().enumerate().any(|(index, entry)| {
                entry.state == EpochCatalogState::Prepared && index + 1 != self.epochs.len()
            })
        {
            return Err(EpochCatalogError::Invalid(
                "prepared epoch must be unique tail",
            ));
        }
        for (index, entry) in self.epochs.iter().enumerate() {
            validate_relative_path(&entry.path)?;
            if entry.epoch_id.as_uuid().is_nil()
                || self.epochs[..index].iter().any(|previous| {
                    previous.epoch_id == entry.epoch_id || previous.path == entry.path
                })
            {
                return Err(EpochCatalogError::Invalid(
                    "duplicate epoch identity or path",
                ));
            }
            if index == 0 {
                if entry.ordinal != 0 || entry.predecessor.is_some() {
                    return Err(EpochCatalogError::Invalid("invalid initial epoch lineage"));
                }
            } else {
                let previous = &self.epochs[index - 1];
                if entry.ordinal != previous.ordinal.saturating_add(1)
                    || entry.predecessor != Some(previous.epoch_id)
                    || previous.successor != Some(entry.epoch_id)
                {
                    return Err(EpochCatalogError::Invalid("broken epoch lineage"));
                }
            }
            let expected_successor = self.epochs.get(index + 1).map(|next| next.epoch_id);
            if entry.successor != expected_successor {
                return Err(EpochCatalogError::Invalid("broken successor lineage"));
            }
            if index + 1 < self.epochs.len()
                && entry.state != EpochCatalogState::Finalized
                && !(entry.state == EpochCatalogState::Active
                    && self.epochs[index + 1].state == EpochCatalogState::Prepared)
            {
                return Err(EpochCatalogError::Invalid("prior epoch is not finalized"));
            }
        }
        Ok(())
    }

    fn compute_checksum(&self) -> Result<String, EpochCatalogError> {
        use std::fmt::Write as _;

        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        let digest = Sha256::digest(serde_json::to_vec(&unsigned)?);
        let mut checksum = String::with_capacity(64);
        for byte in digest {
            write!(&mut checksum, "{byte:02x}")
                .map_err(|_| EpochCatalogError::Invalid("checksum formatting failed"))?;
        }
        Ok(checksum)
    }
}

/// Sources accepted during deterministic startup reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochEvidenceAuthority {
    EpochCatalog,
    WalCommitMarkers,
    RolloverTransitionJournal,
    RunSummary,
    Checkpoint,
    FilesystemMtime,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EpochRecoveryDecision {
    Authoritative,
    RebuildDerived,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpochRecoveryEvidence {
    pub authority: EpochEvidenceAuthority,
    pub parent_id: RecordingId,
    pub epoch_id: EpochId,
    pub ordinal: u64,
    pub path: String,
    pub topology_digest: String,
}

impl EpochRecoveryEvidence {
    pub fn new(
        authority: EpochEvidenceAuthority,
        parent_id: RecordingId,
        epoch_id: EpochId,
        ordinal: u64,
        path: impl Into<String>,
        topology_digest: impl Into<String>,
    ) -> Self {
        Self {
            authority,
            parent_id,
            epoch_id,
            ordinal,
            path: path.into(),
            topology_digest: topology_digest.into(),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EpochAuthorityError {
    #[error("{authority:?} conflicts with parent recording identity")]
    ParentIdentityConflict { authority: EpochEvidenceAuthority },
    #[error("{authority:?} proves a forked epoch lineage")]
    LineageFork { authority: EpochEvidenceAuthority },
    #[error("{authority:?} conflicts with authoritative epochs.json topology")]
    AuthorityConflict { authority: EpochEvidenceAuthority },
    #[error("filesystem mtime/newest-file selection is forbidden")]
    MtimeSelectionForbidden,
}

/// Reconcile derived evidence without allowing it to promote topology.
///
/// `epochs.json` owns parent/epoch topology. Run summaries and checkpoints may
/// be regenerated when identity and lineage match; WAL markers and transition
/// journals cannot override a conflicting catalog digest.
pub fn reconcile_epoch_evidence(
    catalog: &EpochCatalog,
    evidence: &[EpochRecoveryEvidence],
) -> Result<EpochRecoveryDecision, EpochAuthorityError> {
    let mut rebuild_derived = false;
    for item in evidence {
        if item.authority == EpochEvidenceAuthority::FilesystemMtime {
            return Err(EpochAuthorityError::MtimeSelectionForbidden);
        }
        if item.parent_id != catalog.parent_id {
            return Err(EpochAuthorityError::ParentIdentityConflict {
                authority: item.authority,
            });
        }
        let Some(entry) = catalog.epoch(item.epoch_id) else {
            return Err(EpochAuthorityError::LineageFork {
                authority: item.authority,
            });
        };
        if entry.ordinal != item.ordinal || entry.path != item.path {
            return Err(EpochAuthorityError::LineageFork {
                authority: item.authority,
            });
        }
        if item.topology_digest != catalog.topology_digest() {
            match item.authority {
                EpochEvidenceAuthority::RunSummary | EpochEvidenceAuthority::Checkpoint => {
                    rebuild_derived = true;
                }
                _ => {
                    return Err(EpochAuthorityError::AuthorityConflict {
                        authority: item.authority,
                    });
                }
            }
        }
    }
    Ok(if rebuild_derived {
        EpochRecoveryDecision::RebuildDerived
    } else {
        EpochRecoveryDecision::Authoritative
    })
}

fn path_string(path: &Path) -> Result<String, EpochCatalogError> {
    let value = path
        .to_str()
        .ok_or(EpochCatalogError::Invalid("epoch path is not UTF-8"))?;
    validate_relative_path(value)?;
    Ok(value.to_owned())
}

fn validate_relative_path(path: &str) -> Result<(), EpochCatalogError> {
    let path = Path::new(path);
    if path.is_absolute() || path.as_os_str().is_empty() {
        return Err(EpochCatalogError::Invalid("epoch path must be relative"));
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(EpochCatalogError::Invalid(
            "epoch path escapes catalog root",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_catalog_round_trips_and_activates_one_successor() {
        let root =
            std::env::temp_dir().join(format!("chronicle-catalog-v2-{}", uuid::Uuid::new_v4()));
        let parent = RecordingId::new();
        let first = EpochId::new();
        let second = EpochId::new();
        let mut catalog = EpochCatalog::new(parent, first, ".").unwrap();
        let first_digest = catalog.topology_digest().to_owned();
        catalog.append_successor(second, "epochs/1").unwrap();
        assert_eq!(catalog.parent_id, parent);
        assert_eq!(catalog.epochs[0].successor, Some(second));
        assert_eq!(catalog.epochs[1].predecessor, Some(first));
        assert_eq!(catalog.active().unwrap().epoch_id, first);
        assert_ne!(catalog.topology_digest(), first_digest);
        catalog.write_atomic(&root).unwrap();

        let mut loaded = EpochCatalog::load(&root).unwrap();
        assert_eq!(loaded.topology_digest(), catalog.topology_digest());
        loaded.activate_successor(second).unwrap();
        assert_eq!(loaded.epochs[0].state, EpochCatalogState::Finalized);
        assert_eq!(loaded.active().unwrap().epoch_id, second);
        loaded.validate().unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v2_catalog_checksum_and_lineage_conflicts_fail_closed() {
        let parent = RecordingId::new();
        let first = EpochId::new();
        let second = EpochId::new();
        let mut catalog = EpochCatalog::new(parent, first, ".").unwrap();
        catalog.append_successor(second, "epochs/1").unwrap();
        let mut tampered = catalog.clone();
        tampered.parent_id = RecordingId::new();
        assert!(matches!(
            tampered.validate(),
            Err(EpochCatalogError::ChecksumMismatch)
        ));
        let mut forked = catalog.clone();
        forked.epochs[1].predecessor = Some(EpochId::new());
        assert!(matches!(
            forked.encode(),
            Err(EpochCatalogError::Invalid("broken epoch lineage"))
        ));
        let mut duplicate = catalog;
        duplicate.checksum.clear();
        duplicate.epochs[1].epoch_id = first;
        assert!(duplicate.encode().is_err());
    }

    #[test]
    fn derived_evidence_rebuilds_without_promoting_topology() {
        let parent = RecordingId::new();
        let epoch = EpochId::new();
        let catalog = EpochCatalog::new(parent, epoch, ".").unwrap();
        let evidence = EpochRecoveryEvidence::new(
            EpochEvidenceAuthority::RunSummary,
            parent,
            epoch,
            0,
            ".",
            "stale-summary-digest",
        );
        assert_eq!(
            reconcile_epoch_evidence(&catalog, &[evidence]).unwrap(),
            EpochRecoveryDecision::RebuildDerived
        );
    }

    #[test]
    fn stronger_authority_conflicts_fail_closed_and_mtime_is_rejected() {
        let parent = RecordingId::new();
        let epoch = EpochId::new();
        let catalog = EpochCatalog::new(parent, epoch, ".").unwrap();
        let wal_evidence = EpochRecoveryEvidence::new(
            EpochEvidenceAuthority::WalCommitMarkers,
            parent,
            epoch,
            0,
            ".",
            "stale-wal-topology",
        );
        assert!(matches!(
            reconcile_epoch_evidence(&catalog, &[wal_evidence]),
            Err(EpochAuthorityError::AuthorityConflict {
                authority: EpochEvidenceAuthority::WalCommitMarkers
            })
        ));
        let mtime_evidence = EpochRecoveryEvidence::new(
            EpochEvidenceAuthority::FilesystemMtime,
            parent,
            epoch,
            0,
            ".",
            catalog.topology_digest(),
        );
        assert_eq!(
            reconcile_epoch_evidence(&catalog, &[mtime_evidence]),
            Err(EpochAuthorityError::MtimeSelectionForbidden)
        );
    }

    #[test]
    fn identity_or_lineage_conflicts_are_not_repaired_by_derived_state() {
        let parent = RecordingId::new();
        let epoch = EpochId::new();
        let catalog = EpochCatalog::new(parent, epoch, ".").unwrap();
        let evidence = EpochRecoveryEvidence::new(
            EpochEvidenceAuthority::RunSummary,
            RecordingId::new(),
            epoch,
            0,
            ".",
            catalog.topology_digest(),
        );
        assert!(matches!(
            reconcile_epoch_evidence(&catalog, &[evidence]),
            Err(EpochAuthorityError::ParentIdentityConflict {
                authority: EpochEvidenceAuthority::RunSummary
            })
        ));
        let fork = EpochRecoveryEvidence::new(
            EpochEvidenceAuthority::Checkpoint,
            parent,
            EpochId::new(),
            0,
            ".",
            catalog.topology_digest(),
        );
        assert!(matches!(
            reconcile_epoch_evidence(&catalog, &[fork]),
            Err(EpochAuthorityError::LineageFork {
                authority: EpochEvidenceAuthority::Checkpoint
            })
        ));
    }
}
