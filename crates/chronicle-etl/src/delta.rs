//! Deterministic immutable canonical delta batch.
#![allow(clippy::format_collect)]

use chronicle_canonical::CanonicalOperation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const CANONICAL_DELTA_SCHEMA_VERSION: u16 = 1;
const MAX_OPERATIONS: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonicalDeltaBatchV1 {
    pub version: u16,
    pub first_marker_sequence: u64,
    pub last_marker_sequence: u64,
    pub operations: Vec<CanonicalOperation>,
    pub checksum: String,
}

impl CanonicalDeltaBatchV1 {
    pub fn new(
        first_marker_sequence: u64,
        last_marker_sequence: u64,
        mut operations: Vec<CanonicalOperation>,
    ) -> Result<Self, DeltaBatchError> {
        if first_marker_sequence > last_marker_sequence || operations.len() > MAX_OPERATIONS {
            return Err(DeltaBatchError::Invalid);
        }
        operations.sort_by_key(|operation| (operation.sequence, operation.id));
        let value = Self {
            version: CANONICAL_DELTA_SCHEMA_VERSION,
            first_marker_sequence,
            last_marker_sequence,
            operations,
            checksum: String::new(),
        };
        let mut signed = value.clone();
        signed.checksum =
            digest(&serde_json::to_vec(&value).map_err(|_| DeltaBatchError::Serialization)?);
        Ok(signed)
    }

    pub fn validate(&self) -> Result<(), DeltaBatchError> {
        if self.version != CANONICAL_DELTA_SCHEMA_VERSION
            || self.first_marker_sequence > self.last_marker_sequence
            || self.operations.len() > MAX_OPERATIONS
            || self.checksum.len() != 64
            || !self.checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DeltaBatchError::Invalid);
        }
        for pair in self.operations.windows(2) {
            if (pair[0].sequence, pair[0].id) > (pair[1].sequence, pair[1].id) {
                return Err(DeltaBatchError::Unordered);
            }
        }
        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        if digest(&serde_json::to_vec(&unsigned).map_err(|_| DeltaBatchError::Serialization)?)
            != self.checksum
        {
            return Err(DeltaBatchError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, DeltaBatchError> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|_| DeltaBatchError::Serialization)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DeltaBatchError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| DeltaBatchError::Parse)?;
        value.validate()?;
        Ok(value)
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeltaBatchError {
    #[error("canonical delta batch is invalid")]
    Invalid,
    #[error("canonical delta batch operations are unordered")]
    Unordered,
    #[error("canonical delta batch checksum mismatch")]
    ChecksumMismatch,
    #[error("canonical delta batch could not be parsed")]
    Parse,
    #[error("canonical delta batch serialization failed")]
    Serialization,
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
    fn empty_delta_has_deterministic_digest_and_rejects_range_inversion() {
        let batch = CanonicalDeltaBatchV1::new(1, 2, Vec::new()).unwrap();
        assert_eq!(
            batch,
            CanonicalDeltaBatchV1::decode(&batch.encode().unwrap()).unwrap()
        );
        assert_eq!(
            CanonicalDeltaBatchV1::new(2, 1, Vec::new()),
            Err(DeltaBatchError::Invalid)
        );
    }
}
