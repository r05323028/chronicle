//! Versioned, bounded active-recorder metadata.
#![allow(clippy::format_collect, clippy::bool_assert_comparison)]

use crate::{NormalizedScopeConfig, RecorderHealth, RecorderLifecycleState, RecorderReadiness};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const RECORDER_METADATA_SCHEMA_VERSION: u16 = 1;
pub const RECORDER_METADATA_FILE: &str = "recorder.json";
const MAX_QUOTA_DOMAINS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderMetadataV1 {
    pub version: u16,
    pub recorder_id: uuid::Uuid,
    pub attempt_id: uuid::Uuid,
    pub config_digest: String,
    pub scope: NormalizedScopeConfig,
    pub boot_clock_identity: String,
    pub lifecycle: RecorderLifecycleState,
    pub capture_readiness: RecorderReadiness,
    pub processing_readiness: RecorderReadiness,
    pub health: RecorderHealth,
    pub current_epoch: Option<EpochMetadata>,
    pub previous_epoch: Option<EpochMetadata>,
    pub active_segment: Option<u64>,
    pub commit: Option<CommitWatermark>,
    pub incremental_checkpoint: Option<IncrementalCheckpointSummaryV1>,
    pub lag: LagSummary,
    pub quota: Vec<QuotaStatus>,
    pub counters: RecorderCounters,
    pub recovery: Option<RecoverySummary>,
    pub shutdown: Option<MetadataCode>,
    pub failure: Option<MetadataCode>,
    pub updated_at_unix_seconds: u64,
    pub checksum: String,
}

impl RecorderMetadataV1 {
    pub fn validate(&self) -> Result<(), RecorderMetadataError> {
        if self.version != RECORDER_METADATA_SCHEMA_VERSION {
            return Err(RecorderMetadataError::UnsupportedVersion);
        }
        validate_digest(&self.config_digest)?;
        if self.boot_clock_identity.is_empty() || self.boot_clock_identity.len() > 128 {
            return Err(RecorderMetadataError::InvalidField("boot_clock_identity"));
        }
        if self.quota.len() > MAX_QUOTA_DOMAINS {
            return Err(RecorderMetadataError::InvalidField("quota"));
        }
        for quota in &self.quota {
            quota.validate()?;
        }
        if self.lifecycle == RecorderLifecycleState::Running
            && self.capture_readiness != RecorderReadiness::Ready
        {
            return Err(RecorderMetadataError::Contradiction);
        }
        if self.capture_readiness != RecorderReadiness::Ready
            && self.processing_readiness == RecorderReadiness::Ready
        {
            return Err(RecorderMetadataError::Contradiction);
        }
        if self.lifecycle == RecorderLifecycleState::Failed && self.health != RecorderHealth::Failed
        {
            return Err(RecorderMetadataError::Contradiction);
        }
        if matches!(
            self.lifecycle,
            RecorderLifecycleState::Stopped | RecorderLifecycleState::Failed
        ) && self.capture_readiness == RecorderReadiness::Ready
        {
            return Err(RecorderMetadataError::Contradiction);
        }
        if let (Some(commit), Some(checkpoint)) = (&self.commit, &self.incremental_checkpoint) {
            commit.validate()?;
            checkpoint.validate()?;
            if checkpoint.marker_sequence > commit.sequence {
                return Err(RecorderMetadataError::Contradiction);
            }
        } else if let Some(commit) = &self.commit {
            commit.validate()?;
        } else if let Some(checkpoint) = &self.incremental_checkpoint {
            checkpoint.validate()?;
        }
        self.lag.validate(self.incremental_checkpoint.is_some())?;
        if self.checksum.len() != 64 || !self.checksum.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(RecorderMetadataError::InvalidField("checksum"));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, RecorderMetadataError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum.clear();
        let unsigned =
            serde_json::to_vec(&value).map_err(|_| RecorderMetadataError::Serialization)?;
        value.checksum = sha256_hex(&unsigned);
        serde_json::to_vec_pretty(&value)
            .map(|mut bytes| {
                bytes.push(b'\n');
                bytes
            })
            .map_err(|_| RecorderMetadataError::Serialization)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, RecorderMetadataError> {
        let value: Self =
            serde_json::from_slice(bytes).map_err(|_| RecorderMetadataError::Parse)?;
        value.validate_without_checksum()?;
        let expected = {
            let mut unsigned = value.clone();
            unsigned.checksum.clear();
            let bytes =
                serde_json::to_vec(&unsigned).map_err(|_| RecorderMetadataError::Serialization)?;
            sha256_hex(&bytes)
        };
        if value.checksum != expected {
            return Err(RecorderMetadataError::ChecksumMismatch);
        }
        value.validate()?;
        Ok(value)
    }

    fn validate_without_checksum(&self) -> Result<(), RecorderMetadataError> {
        let mut value = self.clone();
        value.checksum = sha256_hex(b"");
        value.validate()
    }
}

pub fn write_recorder_metadata(
    state_root: impl AsRef<Path>,
    metadata: &RecorderMetadataV1,
) -> Result<PathBuf, RecorderMetadataError> {
    metadata.validate_without_checksum()?;
    let root = state_root.as_ref();
    if !root.is_dir() {
        return Err(RecorderMetadataError::StateRootUnavailable);
    }
    let bytes = metadata.encode()?;
    let temporary = root.join(format!(
        ".{RECORDER_METADATA_FILE}.tmp-{}",
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| RecorderMetadataError::Io)?;
        file.write_all(&bytes)
            .map_err(|_| RecorderMetadataError::Io)?;
        file.sync_all().map_err(|_| RecorderMetadataError::Io)?;
        fs::rename(&temporary, root.join(RECORDER_METADATA_FILE))
            .map_err(|_| RecorderMetadataError::Io)?;
        File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| RecorderMetadataError::Io)?;
        Ok(root.join(RECORDER_METADATA_FILE))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn load_recorder_metadata(
    state_root: impl AsRef<Path>,
) -> Result<RecorderMetadataV1, RecorderMetadataError> {
    let bytes = fs::read(state_root.as_ref().join(RECORDER_METADATA_FILE))
        .map_err(|_| RecorderMetadataError::Io)?;
    RecorderMetadataV1::decode(&bytes)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochMetadata {
    pub ordinal: u64,
    pub recording_id: uuid::Uuid,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommitWatermark {
    pub sequence: u64,
    pub segment_ordinal: u64,
    pub marker_digest: String,
}

impl CommitWatermark {
    fn validate(&self) -> Result<(), RecorderMetadataError> {
        validate_digest(&self.marker_digest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalCheckpointSummaryV1 {
    pub marker_sequence: u64,
    pub checkpoint_digest: String,
    pub output_count: u64,
}

impl IncrementalCheckpointSummaryV1 {
    fn validate(&self) -> Result<(), RecorderMetadataError> {
        validate_digest(&self.checkpoint_digest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LagSummary {
    pub records: u64,
    pub bytes: u64,
    pub age_seconds: u64,
}

impl LagSummary {
    fn validate(&self, checkpoint_exists: bool) -> Result<(), RecorderMetadataError> {
        if !checkpoint_exists && (self.records != 0 || self.bytes != 0 || self.age_seconds != 0) {
            return Err(RecorderMetadataError::Contradiction);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuotaStatus {
    pub domain: String,
    pub quota_bytes: u64,
    pub free_bytes: u64,
    pub reserved_bytes: u64,
}

impl QuotaStatus {
    fn validate(&self) -> Result<(), RecorderMetadataError> {
        if self.domain.is_empty()
            || self.domain.len() > 256
            || self.reserved_bytes > self.quota_bytes
        {
            return Err(RecorderMetadataError::InvalidField("quota"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderCounters {
    pub captured_records: u64,
    pub captured_bytes: u64,
    pub committed_records: u64,
    pub committed_bytes: u64,
    pub rollover_failure_records: u64,
    pub rollover_failure_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoverySummary {
    pub code: MetadataCode,
    pub repaired_tail: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetadataCode {
    None,
    CleanStart,
    CrashRecovered,
    ForcedTermination,
    CorruptWal,
    CheckpointConflict,
    QuotaPressure,
    StorageFailure,
    ShutdownTimeout,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RecorderMetadataError {
    #[error("recorder metadata version is unsupported")]
    UnsupportedVersion,
    #[error("recorder metadata contains invalid field: {0}")]
    InvalidField(&'static str),
    #[error("recorder metadata contains contradictory state")]
    Contradiction,
    #[error("recorder metadata checksum mismatch")]
    ChecksumMismatch,
    #[error("recorder metadata could not be parsed")]
    Parse,
    #[error("recorder metadata serialization failed")]
    Serialization,
    #[error("recorder state root is unavailable")]
    StateRootUnavailable,
    #[error("recorder metadata I/O failed")]
    Io,
}

fn validate_digest(value: &str) -> Result<(), RecorderMetadataError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(RecorderMetadataError::InvalidField("digest"));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl From<RecorderMetadataError> for crate::ApplicationError {
    fn from(error: RecorderMetadataError) -> Self {
        Self::InvalidConfig(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> RecorderMetadataV1 {
        RecorderMetadataV1 {
            version: 1,
            recorder_id: uuid::Uuid::new_v4(),
            attempt_id: uuid::Uuid::new_v4(),
            config_digest: "a".repeat(64),
            scope: NormalizedScopeConfig {
                cgroup_path: PathBuf::from("/sys/fs/cgroup/workload"),
                cgroup_id: 7,
                shared_scope_acknowledged: true,
            },
            boot_clock_identity: "boot-1".into(),
            lifecycle: RecorderLifecycleState::Running,
            capture_readiness: RecorderReadiness::Ready,
            processing_readiness: RecorderReadiness::Ready,
            health: RecorderHealth::Healthy,
            current_epoch: Some(EpochMetadata {
                ordinal: 1,
                recording_id: uuid::Uuid::new_v4(),
            }),
            previous_epoch: None,
            active_segment: Some(0),
            commit: Some(CommitWatermark {
                sequence: 3,
                segment_ordinal: 0,
                marker_digest: "b".repeat(64),
            }),
            incremental_checkpoint: Some(IncrementalCheckpointSummaryV1 {
                marker_sequence: 3,
                checkpoint_digest: "c".repeat(64),
                output_count: 1,
            }),
            lag: LagSummary {
                records: 0,
                bytes: 0,
                age_seconds: 0,
            },
            quota: vec![QuotaStatus {
                domain: "domain".into(),
                quota_bytes: 100,
                free_bytes: 50,
                reserved_bytes: 10,
            }],
            counters: RecorderCounters::default(),
            recovery: Some(RecoverySummary {
                code: MetadataCode::CleanStart,
                repaired_tail: false,
            }),
            shutdown: None,
            failure: None,
            updated_at_unix_seconds: 1,
            checksum: String::new(),
        }
    }

    #[test]
    fn round_trip_and_checksum() {
        let value = metadata();
        let encoded = value.encode().unwrap();
        let decoded = RecorderMetadataV1::decode(&encoded).unwrap();
        assert_eq!(decoded.config_digest, value.config_digest);
        assert_ne!(decoded.checksum, "");
    }

    #[test]
    fn leading_checkpoint_is_rejected() {
        let mut value = metadata();
        value
            .incremental_checkpoint
            .as_mut()
            .unwrap()
            .marker_sequence = 4;
        assert_eq!(value.encode(), Err(RecorderMetadataError::Contradiction));
    }

    #[test]
    fn atomic_write_is_private() {
        let root =
            std::env::temp_dir().join(format!("chronicle-metadata-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = write_recorder_metadata(&root, &metadata()).unwrap();
        assert_eq!(path, root.join(RECORDER_METADATA_FILE));
        assert_eq!(load_recorder_metadata(&root).unwrap().version, 1);
        assert!(!root.join(".recorder.json.tmp").exists());
    }
}
