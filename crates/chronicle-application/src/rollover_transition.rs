//! Crash-recoverable successor rollover transition journal.

use chronicle_common::{EpochId, RecordingId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const ROLLOVER_TRANSITION_FILE: &str = "rollover-transition.json";

pub const ROLLOVER_TRANSITION_SCHEMA_VERSION: u16 = 2;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloverTransitionPhase {
    Prepared,
    SuccessorCreated,
    BoundaryCommitted,
    TopologyActivated,
    Complete,
}

impl RolloverTransitionPhase {
    const fn rank(self) -> u8 {
        match self {
            Self::Prepared => 0,
            Self::SuccessorCreated => 1,
            Self::BoundaryCommitted => 2,
            Self::TopologyActivated => 3,
            Self::Complete => 4,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RolloverTransition {
    pub version: u16,
    pub parent_id: RecordingId,
    pub old_epoch_id: EpochId,
    pub new_epoch_id: EpochId,
    pub old_epoch_ordinal: u64,
    pub new_epoch_ordinal: u64,
    pub old_path: String,
    pub new_path: String,
    pub predecessor_boundary_digest: String,
    pub admitted_observation_start: Option<u64>,
    pub admitted_observation_end: Option<u64>,
    pub outcome_digest: String,
    pub reservation_id: String,
    pub reservation_bytes: u64,
    pub phase: RolloverTransitionPhase,
    pub checksum: String,
}

impl RolloverTransition {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent_id: RecordingId,
        old_epoch_id: EpochId,
        new_epoch_id: EpochId,
        old_epoch_ordinal: u64,
        new_epoch_ordinal: u64,
        old_path: impl AsRef<Path>,
        new_path: impl AsRef<Path>,
        predecessor_boundary_digest: impl Into<String>,
        outcome_digest: impl Into<String>,
        reservation_id: impl Into<String>,
        reservation_bytes: u64,
    ) -> Result<Self, RolloverTransitionError> {
        let mut transition = Self {
            version: ROLLOVER_TRANSITION_SCHEMA_VERSION,
            parent_id,
            old_epoch_id,
            new_epoch_id,
            old_epoch_ordinal,
            new_epoch_ordinal,
            old_path: path_string(old_path.as_ref())?,
            new_path: path_string(new_path.as_ref())?,
            predecessor_boundary_digest: predecessor_boundary_digest.into(),
            admitted_observation_start: None,
            admitted_observation_end: None,
            outcome_digest: outcome_digest.into(),
            reservation_id: reservation_id.into(),
            reservation_bytes,
            phase: RolloverTransitionPhase::Prepared,
            checksum: String::new(),
        };
        transition.validate_without_checksum()?;
        transition.checksum = checksum(&transition)?;
        Ok(transition)
    }

    pub fn with_boundary_evidence(
        mut self,
        predecessor_boundary_digest: impl Into<String>,
        outcome_digest: impl Into<String>,
    ) -> Result<Self, RolloverTransitionError> {
        self.predecessor_boundary_digest = predecessor_boundary_digest.into();
        self.outcome_digest = outcome_digest.into();
        self.validate_without_checksum()?;
        self.checksum = checksum(&self)?;
        Ok(self)
    }

    pub fn with_observation_range(
        mut self,
        start: u64,
        end: u64,
    ) -> Result<Self, RolloverTransitionError> {
        if start > end {
            return Err(RolloverTransitionError::Invalid("observation range"));
        }
        self.admitted_observation_start = Some(start);
        self.admitted_observation_end = Some(end);
        self.validate_without_checksum()?;
        self.checksum = checksum(&self)?;
        Ok(self)
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

    fn validate_without_checksum(&self) -> Result<(), RolloverTransitionError> {
        if self.version != ROLLOVER_TRANSITION_SCHEMA_VERSION
            || self.parent_id.as_uuid().is_nil()
            || self.old_epoch_id.as_uuid().is_nil()
            || self.new_epoch_id.as_uuid().is_nil()
            || self.old_epoch_id == self.new_epoch_id
            || self.old_epoch_ordinal.checked_add(1) != Some(self.new_epoch_ordinal)
            || self.old_path.is_empty()
            || self.new_path.is_empty()
            || self.old_path == self.new_path
            || self.predecessor_boundary_digest.is_empty()
            || self.predecessor_boundary_digest.len() > 128
            || self.outcome_digest.is_empty()
            || self.outcome_digest.len() > 128
            || self.reservation_id.is_empty()
            || self.reservation_id.len() > 128
            || self.reservation_bytes == 0
        {
            return Err(RolloverTransitionError::Invalid("transition fields"));
        }
        match (
            self.admitted_observation_start,
            self.admitted_observation_end,
        ) {
            (Some(start), Some(end)) if start <= end => {}
            (None, None) => {}
            _ => return Err(RolloverTransitionError::Invalid("observation range")),
        }
        Ok(())
    }
}

pub fn load_transition(
    root: impl AsRef<Path>,
) -> Result<Option<RolloverTransition>, RolloverTransitionError> {
    let path = root.as_ref().join(ROLLOVER_TRANSITION_FILE);
    match fs::read(path) {
        Ok(bytes) => RolloverTransition::decode(&bytes).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_transition_atomic(
    root: impl AsRef<Path>,
    transition: &RolloverTransition,
) -> Result<PathBuf, RolloverTransitionError> {
    let root = root.as_ref();
    transition.validate_without_checksum()?;
    let bytes = transition.encode()?;
    let temporary = root.join(format!(
        ".{ROLLOVER_TRANSITION_FILE}.v2-tmp-{}",
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

#[allow(clippy::format_collect)]
fn checksum(value: &RolloverTransition) -> Result<String, RolloverTransitionError> {
    let mut unsigned = value.clone();
    unsigned.checksum.clear();
    let digest = Sha256::digest(serde_json::to_vec(&unsigned)?);
    Ok(digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_transition_round_trip_tracks_parent_boundary_and_reservation() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-transition-v2-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let parent_id = RecordingId::new();
        let old_epoch_id = EpochId::new();
        let new_epoch_id = EpochId::new();
        let mut transition = RolloverTransition::new(
            parent_id,
            old_epoch_id,
            new_epoch_id,
            4,
            5,
            root.join("epochs/4"),
            root.join("epochs/5"),
            "predecessor-boundary",
            "outcome-digest",
            "reservation-5",
            4096,
        )
        .unwrap()
        .with_observation_range(10, 20)
        .unwrap();
        for phase in [
            RolloverTransitionPhase::SuccessorCreated,
            RolloverTransitionPhase::BoundaryCommitted,
            RolloverTransitionPhase::TopologyActivated,
            RolloverTransitionPhase::Complete,
        ] {
            transition = transition.with_phase(phase).unwrap();
        }
        write_transition_atomic(&root, &transition).unwrap();
        let bytes = fs::read(root.join(ROLLOVER_TRANSITION_FILE)).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains("etl"));
        assert_eq!(load_transition(&root).unwrap(), Some(transition.clone()));

        let mut tampered = transition;
        tampered.outcome_digest = "different-outcome".into();
        assert!(matches!(
            tampered.validate(),
            Err(RolloverTransitionError::ChecksumMismatch)
        ));
        assert!(matches!(
            tampered.with_phase(RolloverTransitionPhase::Complete),
            Err(RolloverTransitionError::Invalid("non-monotonic phase"))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn v2_transition_rejects_lineage_and_reservation_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-rollover-transition-v2-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        let parent_id = RecordingId::new();
        let old_epoch_id = EpochId::new();
        let new_epoch_id = EpochId::new();
        assert!(matches!(
            RolloverTransition::new(
                parent_id,
                old_epoch_id,
                new_epoch_id,
                1,
                3,
                root.join("old"),
                root.join("new"),
                "boundary",
                "outcome",
                "reservation",
                1,
            ),
            Err(RolloverTransitionError::Invalid("transition fields"))
        ));
        assert!(matches!(
            RolloverTransition::new(
                parent_id,
                old_epoch_id,
                new_epoch_id,
                1,
                2,
                root.join("old"),
                root.join("new"),
                "boundary",
                "outcome",
                "reservation",
                0,
            ),
            Err(RolloverTransitionError::Invalid("transition fields"))
        ));
    }

    #[test]
    fn v2_phase_fault_matrix_is_capture_only_and_fail_closed() {
        let phases = [
            RolloverTransitionPhase::Prepared,
            RolloverTransitionPhase::SuccessorCreated,
            RolloverTransitionPhase::BoundaryCommitted,
            RolloverTransitionPhase::TopologyActivated,
            RolloverTransitionPhase::Complete,
        ];
        for (index, target) in phases.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "chronicle-rollover-fault-matrix-{}-{index}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&root).unwrap();
            let parent = RecordingId::new();
            let old = EpochId::new();
            let new = EpochId::new();
            let mut transition = RolloverTransition::new(
                parent,
                old,
                new,
                0,
                1,
                "epochs/0",
                "epochs/1",
                "boundary",
                "outcome",
                "reservation",
                1024,
            )
            .unwrap();
            for phase in phases.iter().copied().skip(1) {
                if phase.rank() <= target.rank() {
                    transition = transition.with_phase(phase).unwrap();
                }
            }
            write_transition_atomic(&root, &transition).unwrap();
            assert_eq!(load_transition(&root).unwrap(), Some(transition.clone()));
            assert!(matches!(
                transition.clone().with_phase(target),
                Err(RolloverTransitionError::Invalid("non-monotonic phase"))
            ));
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
}
