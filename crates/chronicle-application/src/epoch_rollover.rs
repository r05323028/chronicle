//! Bounded epoch-handoff accounting. WAL marker authority decides assignment;
#![allow(clippy::format_collect)]
//! this journal records only proven ranges or explicit metadata-only failure.

use chronicle_capture::MonotonicTimestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const EPOCH_OUTCOME_SCHEMA_VERSION: u16 = 1;
const MAX_ADMITTED_OBSERVATIONS: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochAdmittedObservation {
    pub ordinal: u64,
    pub payload_bytes: u64,
    pub capture_timestamp: Option<MonotonicTimestamp>,
    pub source_metadata: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RolloverFailureReason {
    NextEpochOpen,
    OutcomePersistence,
    MarkerUnproven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "assignment", rename_all = "snake_case", deny_unknown_fields)]
pub enum RolloverAssignment {
    OldEpoch { marker_sequence: u64 },
    NewEpoch { marker_sequence: u64 },
    EpochRolloverFailure { reason: RolloverFailureReason },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutcomeRange {
    pub start_ordinal: u64,
    pub end_ordinal: u64,
    pub records: u64,
    pub bytes: u64,
    pub start_timestamp: Option<MonotonicTimestamp>,
    pub end_timestamp: Option<MonotonicTimestamp>,
    pub assignment: RolloverAssignment,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochOutcomeJournalV1 {
    pub version: u16,
    pub epoch_ordinal: u64,
    pub predecessor_epoch: Option<uuid::Uuid>,
    pub admitted: Vec<EpochAdmittedObservation>,
    pub outcomes: Vec<OutcomeRange>,
    pub checksum: String,
}

impl EpochOutcomeJournalV1 {
    pub fn new(epoch_ordinal: u64, predecessor_epoch: Option<uuid::Uuid>) -> Self {
        Self {
            version: EPOCH_OUTCOME_SCHEMA_VERSION,
            epoch_ordinal,
            predecessor_epoch,
            admitted: Vec::new(),
            outcomes: Vec::new(),
            checksum: String::new(),
        }
    }

    pub fn admit(
        &mut self,
        payload_bytes: u64,
        capture_timestamp: Option<MonotonicTimestamp>,
        source_metadata: Option<String>,
    ) -> Result<u64, EpochOutcomeError> {
        if self.admitted.len() >= MAX_ADMITTED_OBSERVATIONS {
            return Err(EpochOutcomeError::BoundExceeded);
        }
        if source_metadata
            .as_ref()
            .is_some_and(|value| value.len() > 128)
        {
            return Err(EpochOutcomeError::InvalidMetadata);
        }
        let ordinal =
            u64::try_from(self.admitted.len()).map_err(|_| EpochOutcomeError::BoundExceeded)?;
        self.admitted.push(EpochAdmittedObservation {
            ordinal,
            payload_bytes,
            capture_timestamp,
            source_metadata,
        });
        Ok(ordinal)
    }

    pub fn record_outcome(
        &mut self,
        start_ordinal: u64,
        end_ordinal: u64,
        assignment: RolloverAssignment,
    ) -> Result<(), EpochOutcomeError> {
        if start_ordinal > end_ordinal
            || end_ordinal >= u64::try_from(self.admitted.len()).unwrap_or(u64::MAX)
            || self.outcomes.iter().any(|range| {
                start_ordinal <= range.end_ordinal && end_ordinal >= range.start_ordinal
            })
        {
            return Err(EpochOutcomeError::InvalidRange);
        }
        let observations = &self.admitted[usize::try_from(start_ordinal)
            .map_err(|_| EpochOutcomeError::InvalidRange)?
            ..=usize::try_from(end_ordinal).map_err(|_| EpochOutcomeError::InvalidRange)?];
        let bytes = observations
            .iter()
            .map(|observation| observation.payload_bytes)
            .sum();
        let start_timestamp = observations
            .iter()
            .filter_map(|observation| observation.capture_timestamp.clone())
            .min_by_key(|timestamp| timestamp.nanoseconds);
        let end_timestamp = observations
            .iter()
            .filter_map(|observation| observation.capture_timestamp.clone())
            .max_by_key(|timestamp| timestamp.nanoseconds);
        self.outcomes.push(OutcomeRange {
            start_ordinal,
            end_ordinal,
            records: end_ordinal - start_ordinal + 1,
            bytes,
            start_timestamp,
            end_timestamp,
            assignment,
        });
        self.outcomes.sort_by_key(|range| range.start_ordinal);
        Ok(())
    }

    pub fn ensure_complete(&self) -> Result<(), EpochOutcomeError> {
        self.validate()?;
        if self.outcomes.iter().map(|range| range.records).sum::<u64>()
            != u64::try_from(self.admitted.len()).unwrap_or(u64::MAX)
        {
            return Err(EpochOutcomeError::UnassignedObservation);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), EpochOutcomeError> {
        if self.version != EPOCH_OUTCOME_SCHEMA_VERSION
            || self.admitted.len() > MAX_ADMITTED_OBSERVATIONS
        {
            return Err(EpochOutcomeError::UnsupportedVersion);
        }
        let mut assigned = BTreeSet::new();
        for (index, observation) in self.admitted.iter().enumerate() {
            if observation.ordinal != u64::try_from(index).unwrap_or(u64::MAX) {
                return Err(EpochOutcomeError::InvalidRange);
            }
        }
        for range in &self.outcomes {
            if range.start_ordinal > range.end_ordinal
                || range.end_ordinal >= u64::try_from(self.admitted.len()).unwrap_or(u64::MAX)
                || range.records != range.end_ordinal - range.start_ordinal + 1
            {
                return Err(EpochOutcomeError::InvalidRange);
            }
            let mut bytes = 0_u64;
            for ordinal in range.start_ordinal..=range.end_ordinal {
                if !assigned.insert(ordinal) {
                    return Err(EpochOutcomeError::InvalidRange);
                }
                bytes = bytes.saturating_add(
                    self.admitted[usize::try_from(ordinal).unwrap_or(usize::MAX)].payload_bytes,
                );
            }
            if bytes != range.bytes {
                return Err(EpochOutcomeError::InvalidRange);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, EpochOutcomeError> {
        self.validate()?;
        let mut value = self.clone();
        value.checksum.clear();
        let unsigned = serde_json::to_vec(&value).map_err(|_| EpochOutcomeError::Serialization)?;
        value.checksum = digest(&unsigned);
        serde_json::to_vec_pretty(&value)
            .map_err(|_| EpochOutcomeError::Serialization)
            .map(|mut bytes| {
                bytes.push(b'\n');
                bytes
            })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, EpochOutcomeError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| EpochOutcomeError::Parse)?;
        value.validate()?;
        let mut unsigned = value.clone();
        unsigned.checksum.clear();
        let expected =
            digest(&serde_json::to_vec(&unsigned).map_err(|_| EpochOutcomeError::Serialization)?);
        if value.checksum != expected {
            return Err(EpochOutcomeError::ChecksumMismatch);
        }
        Ok(value)
    }
}

pub fn write_epoch_outcome_atomic(
    path: impl AsRef<Path>,
    journal: &EpochOutcomeJournalV1,
) -> Result<PathBuf, EpochOutcomeError> {
    let path = path.as_ref();
    let directory = path.parent().ok_or(EpochOutcomeError::Io)?;
    let bytes = journal.encode()?;
    let temporary = directory.join(format!(".epoch-outcomes.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| EpochOutcomeError::Io)?;
        file.write_all(&bytes).map_err(|_| EpochOutcomeError::Io)?;
        file.sync_all().map_err(|_| EpochOutcomeError::Io)?;
        fs::rename(&temporary, path).map_err(|_| EpochOutcomeError::Io)?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| EpochOutcomeError::Io)?;
        Ok(path.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum EpochOutcomeError {
    #[error("epoch outcome journal version is unsupported")]
    UnsupportedVersion,
    #[error("epoch outcome journal range is invalid")]
    InvalidRange,
    #[error("epoch outcome journal has an unassigned admitted observation")]
    UnassignedObservation,
    #[error("epoch outcome journal metadata is invalid")]
    InvalidMetadata,
    #[error("epoch outcome journal bound exceeded")]
    BoundExceeded,
    #[error("epoch outcome journal checksum mismatch")]
    ChecksumMismatch,
    #[error("epoch outcome journal could not be parsed")]
    Parse,
    #[error("epoch outcome journal serialization failed")]
    Serialization,
    #[error("epoch outcome journal I/O failed")]
    Io,
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admitted_ranges_preserve_exact_bytes_and_failure_reason() {
        let mut journal = EpochOutcomeJournalV1::new(2, None);
        journal.admit(3, None, Some("source".into())).unwrap();
        journal.admit(5, None, None).unwrap();
        journal
            .record_outcome(
                0,
                1,
                RolloverAssignment::EpochRolloverFailure {
                    reason: RolloverFailureReason::NextEpochOpen,
                },
            )
            .unwrap();
        assert_eq!(journal.outcomes[0].records, 2);
        assert_eq!(journal.outcomes[0].bytes, 8);
        let bytes = journal.encode().unwrap();
        assert_eq!(EpochOutcomeJournalV1::decode(&bytes).unwrap(), {
            let mut expected = journal.clone();
            expected.checksum = digest(
                &serde_json::to_vec(&{
                    let mut value = journal.clone();
                    value.checksum.clear();
                    value
                })
                .unwrap(),
            );
            expected
        });
    }

    #[test]
    fn overlapping_outcomes_fail_closed() {
        let mut journal = EpochOutcomeJournalV1::new(0, None);
        journal.admit(1, None, None).unwrap();
        journal
            .record_outcome(0, 0, RolloverAssignment::OldEpoch { marker_sequence: 1 })
            .unwrap();
        assert_eq!(
            journal.record_outcome(0, 0, RolloverAssignment::NewEpoch { marker_sequence: 2 }),
            Err(EpochOutcomeError::InvalidRange)
        );
    }
}
