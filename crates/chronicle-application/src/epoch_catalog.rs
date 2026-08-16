use chronicle_common::{EpochId, RecordingId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    #[error("epoch catalog checksum mismatch")]
    ChecksumMismatch,
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

    /// Loads catalog bytes without mutating catalog or epoch directories.
    /// Recovery requires recorder lease and quota ownership; callers must use
    /// [`Self::recover_prepared_tail`] only after those authorities are held.
    pub fn load(root: impl AsRef<Path>) -> Result<Self, EpochCatalogError> {
        let bytes = fs::read(root.as_ref().join(EPOCH_CATALOG_FILE))?;
        let catalog: Self = serde_json::from_slice(&bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Completes or rolls back a prepared tail after startup owns recovery
    /// authorities. No caller may invoke this before acquiring the recorder
    /// lease and quota reservations.
    pub fn recover_prepared_tail(
        &mut self,
        root: impl AsRef<Path>,
    ) -> Result<(), EpochCatalogError> {
        let root = root.as_ref();
        if !matches!(
            self.epochs.last().map(|entry| entry.state),
            Some(EpochCatalogState::Prepared)
        ) {
            return Ok(());
        }
        let Some(prepared) = self.epochs.last() else {
            return Err(EpochCatalogError::Invalid("epoch catalog is empty"));
        };
        let metadata_path = root.join(&prepared.path).join("recording.json");
        match fs::read(&metadata_path) {
            Ok(metadata_bytes) => {
                let metadata: crate::RecordingMetadata = serde_json::from_slice(&metadata_bytes)
                    .map_err(|_| {
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
                self.activate_latest()?;
                self.write_atomic(root)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // A writer header alone is not durable recording evidence. A
                // crash before successor metadata publication can therefore
                // roll back its prepared directory; committed evidence without
                // metadata remains fail-closed and available for diagnosis.
                let prepared_root = root.join(&prepared.path);
                if prepared_root.exists() {
                    let has_entries = fs::read_dir(&prepared_root)?.next().is_some();
                    if has_entries {
                        match chronicle_wal::read_committed_snapshot_with_records(
                            &prepared_root,
                            prepared.recording_id,
                            chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
                        ) {
                            Err(chronicle_wal::WalError::NoPublishedSegments) => {
                                fs::remove_dir_all(&prepared_root)?;
                            }
                            Ok(_) | Err(_) => {
                                return Err(EpochCatalogError::Invalid(
                                    "prepared epoch has committed or invalid evidence without metadata",
                                ));
                            }
                        }
                    } else {
                        fs::remove_dir_all(&prepared_root)?;
                    }
                }
                self.epochs.pop();
                self.validate()?;
                self.write_atomic(root)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(())
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

/// Schema version for the parent-aware topology authority in `epochs.json`.
pub const EPOCH_CATALOG_V2_VERSION: u16 = 2;

/// Bounded, payload-free lifecycle evidence retained in the authoritative epoch index.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogSummaryV2 {
    #[serde(default)]
    pub committed_through: Option<u64>,
    #[serde(default)]
    pub committed_bytes: u64,
    #[serde(default)]
    pub checkpoint_through: Option<u64>,
    #[serde(default)]
    pub published: bool,
    #[serde(default)]
    pub continuation_state: Option<String>,
    #[serde(default)]
    pub retention_state: Option<String>,
    #[serde(default)]
    pub warning_codes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogEntryV2 {
    pub ordinal: u64,
    pub epoch_id: EpochId,
    pub predecessor: Option<EpochId>,
    pub successor: Option<EpochId>,
    pub path: String,
    pub state: EpochCatalogState,
    #[serde(default)]
    pub summary: EpochCatalogSummaryV2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochCatalogV2 {
    pub version: u16,
    pub parent_id: RecordingId,
    pub epochs: Vec<EpochCatalogEntryV2>,
    pub checksum: String,
}

impl EpochCatalogV2 {
    pub fn new(
        parent_id: RecordingId,
        epoch_id: EpochId,
        path: impl AsRef<Path>,
    ) -> Result<Self, EpochCatalogError> {
        let mut catalog = Self {
            version: EPOCH_CATALOG_V2_VERSION,
            parent_id,
            epochs: vec![EpochCatalogEntryV2 {
                ordinal: 0,
                epoch_id,
                predecessor: None,
                successor: None,
                path: path_string(path.as_ref())?,
                state: EpochCatalogState::Active,
                summary: EpochCatalogSummaryV2::default(),
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

    /// Load parent-aware topology, explicitly mapping legacy V1 bytes once.
    /// V1 remains a read-only compatibility source; all new writes use V2.
    pub fn load_compatible(
        root: impl AsRef<Path>,
        parent_id: RecordingId,
    ) -> Result<Self, EpochCatalogError> {
        let path = root.as_ref().join(EPOCH_CATALOG_FILE);
        let bytes = fs::read(path)?;
        let version = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|value| value.get("version").and_then(serde_json::Value::as_u64));
        match version {
            Some(value) if value == u64::from(EPOCH_CATALOG_V2_VERSION) => Self::decode(&bytes),
            Some(1) => {
                let legacy: EpochCatalogV1 = serde_json::from_slice(&bytes)?;
                Self::from_legacy(parent_id, &legacy)
            }
            _ => Err(EpochCatalogError::Invalid(
                "unsupported epoch catalog version",
            )),
        }
    }

    /// Explicit compatibility conversion; no mtime or directory ordering is used.
    pub fn from_legacy(
        parent_id: RecordingId,
        legacy: &EpochCatalogV1,
    ) -> Result<Self, EpochCatalogError> {
        if legacy.epochs.is_empty() {
            return Err(EpochCatalogError::Invalid("legacy epoch catalog is empty"));
        }
        let epochs = legacy
            .epochs
            .iter()
            .enumerate()
            .map(|(index, entry)| EpochCatalogEntryV2 {
                ordinal: entry.ordinal,
                epoch_id: EpochId::from_uuid(entry.recording_id.0),
                predecessor: entry.predecessor.map(|id| EpochId::from_uuid(id.0)),
                successor: legacy
                    .epochs
                    .get(index + 1)
                    .map(|next| EpochId::from_uuid(next.recording_id.0)),
                path: entry.path.clone(),
                state: entry.state,
                summary: EpochCatalogSummaryV2::default(),
            })
            .collect();
        let mut catalog = Self {
            version: EPOCH_CATALOG_V2_VERSION,
            parent_id,
            epochs,
            checksum: String::new(),
        };
        catalog.validate_without_checksum()?;
        catalog.checksum = catalog.compute_checksum()?;
        Ok(catalog)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EpochCatalogError> {
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Return a read-only V1 compatibility view for legacy helpers.
    pub fn to_legacy(&self) -> Result<EpochCatalogV1, EpochCatalogError> {
        self.validate()?;
        let legacy = EpochCatalogV1 {
            version: EPOCH_CATALOG_VERSION,
            epochs: self
                .epochs
                .iter()
                .map(|entry| EpochCatalogEntry {
                    ordinal: entry.ordinal,
                    recording_id: RecordingId::from_uuid(entry.epoch_id.as_uuid()),
                    predecessor: entry
                        .predecessor
                        .map(|id| RecordingId::from_uuid(id.as_uuid())),
                    path: entry.path.clone(),
                    state: entry.state,
                })
                .collect(),
        };
        legacy.validate()?;
        Ok(legacy)
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

    pub fn active(&self) -> Option<&EpochCatalogEntryV2> {
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

    pub fn epoch(&self, epoch_id: EpochId) -> Option<&EpochCatalogEntryV2> {
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
        self.epochs.push(EpochCatalogEntryV2 {
            ordinal,
            epoch_id,
            predecessor: Some(active_epoch_id),
            successor: None,
            path,
            state: EpochCatalogState::Prepared,
            summary: EpochCatalogSummaryV2::default(),
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
        summary: EpochCatalogSummaryV2,
    ) -> Result<(), EpochCatalogError> {
        if summary.warning_codes.len() > 32
            || summary
                .warning_codes
                .iter()
                .any(|warning| warning.len() > 256)
        {
            return Err(EpochCatalogError::Invalid("epoch summary exceeds bound"));
        }
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
        if self.version != EPOCH_CATALOG_V2_VERSION {
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

/// Explicit compatibility mapping for a legacy UUID that served both roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LegacyOneEpochMapping {
    pub legacy_uuid: uuid::Uuid,
    pub parent_id: RecordingId,
    pub epoch_id: EpochId,
}

impl LegacyOneEpochMapping {
    pub const fn from_uuid(legacy_uuid: uuid::Uuid) -> Self {
        Self {
            legacy_uuid,
            parent_id: RecordingId::from_uuid(legacy_uuid),
            epoch_id: EpochId::from_uuid(legacy_uuid),
        }
    }

    pub fn validate(self) -> bool {
        self.parent_id.as_uuid() == self.legacy_uuid && self.epoch_id.as_uuid() == self.legacy_uuid
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
    LegacyOneEpochMapping,
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
    catalog: &EpochCatalogV2,
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
        let before = fs::read(root.join(EPOCH_CATALOG_FILE)).unwrap();
        let mut loaded = EpochCatalogV1::load(&root).unwrap();
        assert_eq!(loaded.epochs.len(), 2);
        assert_eq!(loaded.active().unwrap().recording_id, first);
        assert_eq!(fs::read(root.join(EPOCH_CATALOG_FILE)).unwrap(), before);
        loaded.recover_prepared_tail(&root).unwrap();
        assert_eq!(loaded.epochs.len(), 1);
        assert_eq!(loaded.active().unwrap().recording_id, first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_tail_with_only_wal_header_rolls_back() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-catalog-header-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let prepared_root = root.join("epochs/1");
        fs::create_dir_all(&prepared_root).unwrap();
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
        let writer = chronicle_wal::GroupCommitWalWriter::create_with_total_limit_and_age(
            &prepared_root,
            second,
            chronicle_wal::DEFAULT_SEGMENT_BYTES,
            chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            1,
            1,
            300_000,
        )
        .unwrap();
        drop(writer);
        let mut loaded = EpochCatalogV1::load(&root).unwrap();
        loaded.recover_prepared_tail(&root).unwrap();
        assert!(!prepared_root.exists());
        assert_eq!(loaded.active().unwrap().recording_id, first);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_tail_with_metadata_activates_without_mutating_predecessor() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-catalog-forward-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let prepared_root = root.join("epochs/1");
        fs::create_dir_all(&prepared_root).unwrap();
        let first = RecordingId::new();
        let second = RecordingId::new();
        let predecessor = crate::RecordingMetadata {
            version: crate::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id: first,
            selector: None,
            status: crate::RecordingStatus::Recording,
            shutdown_reason: None,
            last_valid_commit: None,
            counters: crate::RecordingCounters::default(),
            terminal_wal_loss: None,
            capture: None,
        };
        crate::write_recording_metadata(&root, &predecessor).unwrap();
        let predecessor_bytes = fs::read(root.join("recording.json")).unwrap();
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
        let successor = crate::RecordingMetadata {
            recording_id: second,
            ..predecessor
        };
        crate::write_recording_metadata(&prepared_root, &successor).unwrap();
        let mut loaded = EpochCatalogV1::load(&root).unwrap();
        loaded.recover_prepared_tail(&root).unwrap();
        assert_eq!(loaded.active().unwrap().recording_id, second);
        assert_eq!(
            loaded
                .epochs
                .iter()
                .filter(|entry| entry.state == EpochCatalogState::Active)
                .count(),
            1
        );
        assert_eq!(loaded.epochs[0].state, EpochCatalogState::Finalized);
        assert_eq!(loaded.epochs[0].recording_id, first);
        assert_eq!(loaded.epochs[0].predecessor, None);
        assert_eq!(
            fs::read(root.join("recording.json")).unwrap(),
            predecessor_bytes
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_tail_with_invalid_evidence_fails_closed() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-catalog-invalid-recovery-{}",
            uuid::Uuid::new_v4()
        ));
        let prepared_root = root.join("epochs/1");
        fs::create_dir_all(prepared_root.join("segments")).unwrap();
        fs::write(prepared_root.join("segments/garbage.chwal"), b"invalid").unwrap();
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
        let mut loaded = EpochCatalogV1::load(&root).unwrap();
        assert!(matches!(
            loaded.recover_prepared_tail(&root),
            Err(EpochCatalogError::Invalid(
                "prepared epoch has committed or invalid evidence without metadata"
            ))
        ));
        assert!(prepared_root.join("segments/garbage.chwal").exists());
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
        let mut loaded = EpochCatalogV1::load(&root).unwrap();
        assert_eq!(loaded.epochs.len(), 2);
        assert_eq!(loaded.resolve_active_path(&root).unwrap(), root);
        loaded.recover_prepared_tail(&root).unwrap();
        assert_eq!(loaded.resolve_active_path(&root).unwrap(), root);
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

    #[test]
    fn v2_catalog_round_trips_and_activates_one_successor() {
        let root =
            std::env::temp_dir().join(format!("chronicle-catalog-v2-{}", uuid::Uuid::new_v4()));
        let parent = RecordingId::new();
        let first = EpochId::new();
        let second = EpochId::new();
        let mut catalog = EpochCatalogV2::new(parent, first, ".").unwrap();
        let first_digest = catalog.topology_digest().to_owned();
        catalog.append_successor(second, "epochs/1").unwrap();
        assert_eq!(catalog.parent_id, parent);
        assert_eq!(catalog.epochs[0].successor, Some(second));
        assert_eq!(catalog.epochs[1].predecessor, Some(first));
        assert_eq!(catalog.active().unwrap().epoch_id, first);
        assert_ne!(catalog.topology_digest(), first_digest);
        catalog.write_atomic(&root).unwrap();

        let mut loaded = EpochCatalogV2::load(&root).unwrap();
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
        let mut catalog = EpochCatalogV2::new(parent, first, ".").unwrap();
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
    fn legacy_mapping_keeps_parent_and_epoch_types_distinct() {
        let uuid = uuid::Uuid::new_v4();
        let mapping = LegacyOneEpochMapping::from_uuid(uuid);
        assert!(mapping.validate());
        assert_eq!(mapping.parent_id.as_uuid(), uuid);
        assert_eq!(mapping.epoch_id.as_uuid(), uuid);
        assert_ne!(
            mapping.parent_id.to_cli_string(),
            mapping.epoch_id.to_cli_string()
        );
    }

    #[test]
    fn derived_evidence_rebuilds_without_promoting_topology() {
        let parent = RecordingId::new();
        let epoch = EpochId::new();
        let catalog = EpochCatalogV2::new(parent, epoch, ".").unwrap();
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
        let catalog = EpochCatalogV2::new(parent, epoch, ".").unwrap();
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
        let catalog = EpochCatalogV2::new(parent, epoch, ".").unwrap();
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
