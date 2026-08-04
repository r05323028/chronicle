use chronicle_common::RecordingId;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const EPOCH_CATALOG_FILE: &str = "epochs.json";
const EPOCH_CATALOG_VERSION: u16 = 1;
const MAX_EPOCHS: usize = 4096;

#[derive(Debug, Error)]
pub enum EpochCatalogError {
    #[error("epoch catalog I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("epoch catalog encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("epoch catalog validation failed: {0}")]
    Invalid(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogV1 {
    pub version: u16,
    pub epochs: Vec<EpochCatalogEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogEntry {
    pub ordinal: u64,
    pub recording_id: RecordingId,
    pub predecessor: Option<RecordingId>,
    pub path: String,
    #[serde(default)]
    pub state: EpochCatalogState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum EpochCatalogState {
    Prepared,
    #[default]
    Active,
    Finalized,
}

impl EpochCatalogV1 {
    pub fn new(entry: EpochCatalogEntry) -> Result<Self, EpochCatalogError> {
        let catalog = Self {
            version: EPOCH_CATALOG_VERSION,
            epochs: vec![entry],
        };
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn load(root: impl AsRef<Path>) -> Result<Self, EpochCatalogError> {
        let root = root.as_ref();
        let bytes = fs::read(root.join(EPOCH_CATALOG_FILE))?;
        let mut catalog: Self = serde_json::from_slice(&bytes)?;
        catalog.validate()?;
        if matches!(
            catalog.epochs.last().map(|entry| entry.state),
            Some(EpochCatalogState::Prepared)
        ) {
            let Some(prepared) = catalog.epochs.last() else {
                return Err(EpochCatalogError::Invalid("epoch catalog is empty"));
            };
            let metadata_path = root.join(&prepared.path).join("recording.json");
            match fs::read(&metadata_path) {
                Ok(metadata_bytes) => {
                    let metadata: crate::RecordingMetadata =
                        serde_json::from_slice(&metadata_bytes).map_err(|_| {
                            EpochCatalogError::Invalid("prepared epoch metadata is invalid")
                        })?;
                    if metadata.recording_id != prepared.recording_id {
                        return Err(EpochCatalogError::Invalid(
                            "prepared epoch metadata identity mismatch",
                        ));
                    }
                    // Metadata publication is the durable successor boundary. A
                    // crash after it and before catalog activation is forward
                    // recoverable; activation remains monotonic.
                    catalog.activate_latest()?;
                    catalog.write_atomic(root)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Preparation without successor metadata never acquired
                    // authority. Remove only an empty prepared directory;
                    // non-empty evidence is preserved and recovery fails
                    // closed instead of letting ETL silently ignore it.
                    let prepared_root = root.join(&prepared.path);
                    if prepared_root.exists() {
                        let mut entries = fs::read_dir(&prepared_root)?;
                        if entries.next().is_some() {
                            return Err(EpochCatalogError::Invalid(
                                "prepared epoch has uncommitted evidence",
                            ));
                        }
                        fs::remove_dir(&prepared_root)?;
                    }
                    catalog.epochs.pop();
                    catalog.validate()?;
                    catalog.write_atomic(root)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(catalog)
    }

    pub fn append(&mut self, entry: EpochCatalogEntry) -> Result<(), EpochCatalogError> {
        if self.epochs.len() >= MAX_EPOCHS {
            return Err(EpochCatalogError::Invalid("epoch catalog is full"));
        }
        self.epochs.push(entry);
        if let Err(error) = self.validate() {
            self.epochs.pop();
            return Err(error);
        }
        Ok(())
    }

    pub fn active(&self) -> Option<&EpochCatalogEntry> {
        self.epochs
            .iter()
            .rev()
            .find(|entry| matches!(entry.state, EpochCatalogState::Active))
    }

    pub fn activate_latest(&mut self) -> Result<(), EpochCatalogError> {
        let latest_index = self
            .epochs
            .len()
            .checked_sub(1)
            .ok_or(EpochCatalogError::Invalid("epoch catalog is empty"))?;
        if !matches!(self.epochs[latest_index].state, EpochCatalogState::Prepared) {
            return Err(EpochCatalogError::Invalid("no prepared successor"));
        }
        self.epochs[latest_index].state = EpochCatalogState::Active;
        if latest_index > 0 {
            self.epochs[latest_index - 1].state = EpochCatalogState::Finalized;
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<(), EpochCatalogError> {
        if self.version != EPOCH_CATALOG_VERSION {
            return Err(EpochCatalogError::Invalid("unsupported version"));
        }
        if self.epochs.is_empty() || self.epochs.len() > MAX_EPOCHS {
            return Err(EpochCatalogError::Invalid("invalid epoch count"));
        }
        let active_count = self
            .epochs
            .iter()
            .filter(|entry| matches!(entry.state, EpochCatalogState::Active))
            .count();
        if active_count != 1 {
            return Err(EpochCatalogError::Invalid(
                "catalog must contain one active epoch",
            ));
        }
        if self
            .epochs
            .iter()
            .take(self.epochs.len().saturating_sub(1))
            .any(|entry| matches!(entry.state, EpochCatalogState::Prepared))
        {
            return Err(EpochCatalogError::Invalid(
                "prepared epoch must be catalog tail",
            ));
        }
        for (index, entry) in self.epochs.iter().enumerate() {
            validate_relative_path(&entry.path)?;
            if self.epochs[..index].iter().any(|previous| {
                previous.recording_id == entry.recording_id || previous.path == entry.path
            }) {
                return Err(EpochCatalogError::Invalid(
                    "duplicate epoch identity or path",
                ));
            }
            if index == 0 {
                if entry.ordinal != 0
                    || entry.predecessor.is_some()
                    || matches!(entry.state, EpochCatalogState::Prepared)
                {
                    return Err(EpochCatalogError::Invalid("invalid initial epoch"));
                }
            } else {
                let previous = &self.epochs[index - 1];
                if entry.ordinal != previous.ordinal.saturating_add(1)
                    || entry.predecessor != Some(previous.recording_id)
                {
                    return Err(EpochCatalogError::Invalid("broken epoch lineage"));
                }
                if matches!(previous.state, EpochCatalogState::Active)
                    && !matches!(entry.state, EpochCatalogState::Prepared)
                {
                    return Err(EpochCatalogError::Invalid(
                        "active epoch must precede only prepared successor",
                    ));
                }
                if matches!(entry.state, EpochCatalogState::Finalized)
                    && index == self.epochs.len() - 1
                {
                    return Err(EpochCatalogError::Invalid(
                        "latest epoch cannot be finalized",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn write_atomic(&self, root: impl AsRef<Path>) -> Result<(), EpochCatalogError> {
        self.validate()?;
        let root = root.as_ref();
        fs::create_dir_all(root)?;
        let bytes = serde_json::to_vec_pretty(self)?;
        let temporary = root.join(format!(
            ".{EPOCH_CATALOG_FILE}.tmp-{}",
            uuid::Uuid::new_v4()
        ));
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, root.join(EPOCH_CATALOG_FILE))?;
        let directory = fs::File::open(root)?;
        directory.sync_all()?;
        Ok(())
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
    fn prepared_tail_without_metadata_rolls_back_to_prior_authority() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-catalog-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(root.join("epochs/1")).unwrap();
        let first = RecordingId::new();
        let second = RecordingId::new();
        let mut catalog = EpochCatalogV1::new(EpochCatalogEntry {
            ordinal: 0,
            recording_id: first,
            predecessor: None,
            path: ".".into(),
            state: EpochCatalogState::Active,
        })
        .unwrap();
        catalog
            .append(EpochCatalogEntry {
                ordinal: 1,
                recording_id: second,
                predecessor: Some(first),
                path: "epochs/1".into(),
                state: EpochCatalogState::Prepared,
            })
            .unwrap();
        catalog.write_atomic(&root).unwrap();
        let recovered = EpochCatalogV1::load(&root).unwrap();
        assert_eq!(recovered.epochs.len(), 1);
        assert_eq!(recovered.active().unwrap().recording_id, first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn catalog_rejects_multiple_active_epochs() {
        let first = RecordingId::new();
        let mut catalog = EpochCatalogV1 {
            version: EPOCH_CATALOG_VERSION,
            epochs: vec![
                EpochCatalogEntry {
                    ordinal: 0,
                    recording_id: first,
                    predecessor: None,
                    path: ".".into(),
                    state: EpochCatalogState::Active,
                },
                EpochCatalogEntry {
                    ordinal: 1,
                    recording_id: RecordingId::new(),
                    predecessor: Some(first),
                    path: "epochs/1".into(),
                    state: EpochCatalogState::Active,
                },
            ],
        };
        assert!(matches!(
            catalog.validate(),
            Err(EpochCatalogError::Invalid(
                "catalog must contain one active epoch"
            ))
        ));
        catalog.epochs[1].state = EpochCatalogState::Prepared;
        assert!(catalog.validate().is_ok());
    }

    #[test]
    fn catalog_round_trips_and_rejects_broken_lineage() {
        let root = std::env::temp_dir().join(format!("chronicle-catalog-{}", uuid::Uuid::new_v4()));
        let first = RecordingId::new();
        let second = RecordingId::new();
        let mut catalog = EpochCatalogV1::new(EpochCatalogEntry {
            ordinal: 0,
            recording_id: first,
            predecessor: None,
            path: ".".into(),
            state: EpochCatalogState::Active,
        })
        .unwrap();
        catalog
            .append(EpochCatalogEntry {
                ordinal: 1,
                recording_id: second,
                predecessor: Some(first),
                path: "epochs/1".into(),
                state: EpochCatalogState::Prepared,
            })
            .unwrap();
        catalog.write_atomic(&root).unwrap();
        let recovered = EpochCatalogV1::load(&root).unwrap();
        assert_eq!(recovered.epochs.len(), 1);
        assert_eq!(catalog.resolve_active_path(&root).unwrap(), root);
        catalog.activate_latest().unwrap();
        assert_eq!(
            catalog.resolve_active_path(&root).unwrap(),
            root.join("epochs/1")
        );
        catalog.epochs[1].predecessor = Some(RecordingId::new());
        assert!(matches!(
            catalog.validate(),
            Err(EpochCatalogError::Invalid("broken epoch lineage"))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
