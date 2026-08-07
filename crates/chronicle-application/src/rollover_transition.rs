//! Crash-recoverable successor rollover transition journal.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const ROLLOVER_TRANSITION_SCHEMA_VERSION: u16 = 1;
pub const ROLLOVER_TRANSITION_FILE: &str = "rollover-transition.json";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloverTransitionPhase {
    Prepared,
    SuccessorCreated,
    BoundaryCommitted,
    MetadataPublished,
}

impl RolloverTransitionPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Prepared => 0,
            Self::SuccessorCreated => 1,
            Self::BoundaryCommitted => 2,
            Self::MetadataPublished => 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloverTransitionV1 {
    pub version: u16,
    pub old_epoch_ordinal: u64,
    pub new_epoch_ordinal: u64,
    pub old_recording_id: uuid::Uuid,
    pub new_recording_id: uuid::Uuid,
    pub old_path: String,
    pub new_path: String,
    pub reservation_bytes: u64,
    pub phase: RolloverTransitionPhase,
    pub checksum: String,
}

#[derive(Debug, Error)]
pub enum RolloverTransitionError {
    #[error("rollover transition I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("rollover transition encoding failed: {0}")]
    Encoding(#[from] serde_json::Error),
    #[error("rollover transition validation failed: {0}")]
    Invalid(&'static str),
    #[error("rollover transition checksum mismatch")]
    ChecksumMismatch,
}

impl RolloverTransitionV1 {
    pub fn new(
        old_epoch_ordinal: u64,
        new_epoch_ordinal: u64,
        old_recording_id: uuid::Uuid,
        new_recording_id: uuid::Uuid,
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
        reservation_bytes: u64,
    ) -> Result<Self, RolloverTransitionError> {
        let transition = Self {
            version: ROLLOVER_TRANSITION_SCHEMA_VERSION,
            old_epoch_ordinal,
            new_epoch_ordinal,
            old_recording_id,
            new_recording_id,
            old_path: path_string(old_path.as_ref())?,
            new_path: path_string(new_path.as_ref())?,
            reservation_bytes,
            phase: RolloverTransitionPhase::Prepared,
            checksum: String::new(),
        };
        transition.validate_without_checksum()?;
        let mut transition = transition;
        transition.checksum = checksum(&transition)?;
        Ok(transition)
    }

    pub fn validate(&self) -> Result<(), RolloverTransitionError> {
        self.validate_without_checksum()?;
        if self.checksum != checksum(self)? {
            return Err(RolloverTransitionError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, RolloverTransitionError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum = checksum(&value)?;
        let mut bytes = serde_json::to_vec_pretty(&value)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RolloverTransitionError> {
        let value: Self = serde_json::from_slice(bytes)?;
        value.validate()?;
        Ok(value)
    }

    pub fn with_phase(
        mut self,
        phase: RolloverTransitionPhase,
    ) -> Result<Self, RolloverTransitionError> {
        if phase.rank() != self.phase.rank().saturating_add(1) {
            return Err(RolloverTransitionError::Invalid("non-monotonic phase"));
        }
        self.phase = phase;
        self.validate_without_checksum()?;
        self.checksum = checksum(&self)?;
        Ok(self)
    }

    fn validate_without_checksum(&self) -> Result<(), RolloverTransitionError> {
        if self.version != ROLLOVER_TRANSITION_SCHEMA_VERSION
            || self.old_recording_id.is_nil()
            || self.new_recording_id.is_nil()
            || self.old_recording_id == self.new_recording_id
            || self.old_epoch_ordinal.checked_add(1) != Some(self.new_epoch_ordinal)
            || self.reservation_bytes == 0
            || self.old_path.is_empty()
            || self.new_path.is_empty()
            || self.old_path == self.new_path
        {
            return Err(RolloverTransitionError::Invalid("transition fields"));
        }
        Ok(())
    }
}

pub fn load_transition(
    root: impl AsRef<Path>,
) -> Result<Option<RolloverTransitionV1>, RolloverTransitionError> {
    let path = root.as_ref().join(ROLLOVER_TRANSITION_FILE);
    match fs::read(path) {
        Ok(bytes) => RolloverTransitionV1::decode(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_transition_atomic(
    root: impl AsRef<Path>,
    transition: &RolloverTransitionV1,
) -> Result<PathBuf, RolloverTransitionError> {
    let root = root.as_ref();
    transition.validate_without_checksum()?;
    let bytes = transition.encode()?;
    let temporary = root.join(format!(
        ".{ROLLOVER_TRANSITION_FILE}.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, root.join(ROLLOVER_TRANSITION_FILE))?;
        File::open(root)?.sync_all()?;
        Ok(root.join(ROLLOVER_TRANSITION_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn remove_transition(root: impl AsRef<Path>) -> Result<(), RolloverTransitionError> {
    let root = root.as_ref();
    match fs::remove_file(root.join(ROLLOVER_TRANSITION_FILE)) {
        Ok(()) => File::open(root)?.sync_all().map_err(Into::into),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn path_string(path: &Path) -> Result<String, RolloverTransitionError> {
    let value = path
        .to_str()
        .ok_or(RolloverTransitionError::Invalid("path"))?;
    (!value.is_empty())
        .then_some(value.to_owned())
        .ok_or(RolloverTransitionError::Invalid("path"))
}

#[allow(clippy::format_collect)]
fn checksum(value: &RolloverTransitionV1) -> Result<String, RolloverTransitionError> {
    let mut unsigned = value.clone();
    unsigned.checksum.clear();
    let bytes = serde_json::to_vec(&unsigned)?;
    let digest = Sha256::digest(bytes);
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_round_trip_preserves_phase_and_identity() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-transition-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let old_id = uuid::Uuid::new_v4();
        let new_id = uuid::Uuid::new_v4();
        let transition = RolloverTransitionV1::new(
            3,
            4,
            old_id,
            new_id,
            root.join("old"),
            root.join("new"),
            1024,
        )
        .unwrap();
        write_transition_atomic(&root, &transition).unwrap();
        let loaded = load_transition(&root).unwrap().unwrap();
        assert_eq!(loaded, transition);
        assert!(matches!(
            loaded
                .clone()
                .with_phase(RolloverTransitionPhase::MetadataPublished),
            Err(RolloverTransitionError::Invalid("non-monotonic phase"))
        ));
        let mut published = loaded;
        for phase in [
            RolloverTransitionPhase::SuccessorCreated,
            RolloverTransitionPhase::BoundaryCommitted,
            RolloverTransitionPhase::MetadataPublished,
        ] {
            published = published.with_phase(phase).unwrap();
            write_transition_atomic(&root, &published).unwrap();
        }
        assert_eq!(
            load_transition(&root).unwrap().unwrap().phase,
            RolloverTransitionPhase::MetadataPublished
        );
        remove_transition(&root).unwrap();
        assert!(load_transition(&root).unwrap().is_none());
        assert_eq!(old_id, transition.old_recording_id);
        assert_eq!(new_id, transition.new_recording_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transition_rejects_tampered_bytes() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-transition-tamper-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let transition = RolloverTransitionV1::new(
            0,
            1,
            uuid::Uuid::new_v4(),
            uuid::Uuid::new_v4(),
            root.join("old"),
            root.join("new"),
            1,
        )
        .unwrap();
        write_transition_atomic(&root, &transition).unwrap();
        let path = root.join(ROLLOVER_TRANSITION_FILE);
        let mut bytes = fs::read(&path).unwrap();
        let marker = b"\"checksum\": \"";
        let checksum_byte = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .unwrap()
            + marker.len();
        bytes[checksum_byte] = if bytes[checksum_byte] == b'0' {
            b'1'
        } else {
            b'0'
        };
        fs::write(path, bytes).unwrap();
        assert!(matches!(
            load_transition(&root),
            Err(RolloverTransitionError::ChecksumMismatch)
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
