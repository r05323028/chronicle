//! Bounded incremental ETL checkpoint. Distinct from final-session checkpoint.
#![allow(clippy::format_collect)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;
use uuid::Uuid;

pub const INCREMENTAL_CHECKPOINT_SCHEMA_VERSION: u16 = 1;
const MAX_SEGMENTS: usize = 256;
const MAX_CONNECTIONS: usize = 4_096;
const MAX_OUTPUTS: usize = 4_096;
const MAX_STATE_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointOwner {
    Recorder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointLifecycle {
    Active,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerLineage {
    pub sequence: u64,
    pub segment_ordinal: u64,
    pub marker_digest: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentLineage {
    pub ordinal: u64,
    pub digest: String,
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecoderReconstructionState {
    pub connections: Vec<ConnectionState>,
    pub serialized_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionState {
    pub connection_id: Uuid,
    pub generation: u64,
    pub tcp: TcpDirectionState,
    pub http: HttpCorrelationState,
    pub lifecycle: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpDirectionState {
    pub client_to_server_offset: u64,
    pub server_to_client_offset: u64,
    pub client_to_server_buffer_bytes: u64,
    pub server_to_client_buffer_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HttpCorrelationState {
    pub request_queue_depth: u32,
    pub active_request_id: Option<Uuid>,
    pub body_bytes_pending: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaBatchReference {
    pub key: String,
    pub digest: String,
    pub first_marker_sequence: u64,
    pub last_marker_sequence: u64,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStatus {
    Live,
    Draining,
    Stopped,
    RecoveryRequired,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalEtlCheckpointV1 {
    pub version: u16,
    pub owner: CheckpointOwner,
    pub lifecycle: CheckpointLifecycle,
    pub recording_id: Uuid,
    pub epoch_ordinal: u64,
    pub config_digest: String,
    pub pipeline_version: String,
    pub canonical_schema_version: u16,
    pub marker: MarkerLineage,
    pub segment_lineage: Vec<SegmentLineage>,
    pub predecessor_digest: Option<String>,
    pub decoder: DecoderReconstructionState,
    pub outputs: Vec<DeltaBatchReference>,
    pub source_status: SourceStatus,
    pub checksum: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveryAuthoritativeSnapshot {
    pub recording_id: Uuid,
    pub epoch_ordinal: u64,
    pub marker: MarkerLineage,
    pub segment_lineage: Vec<SegmentLineage>,
    pub pipeline_version: String,
}

static CHECKPOINT_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn write_checkpoint_atomic(
    path: impl AsRef<Path>,
    checkpoint: &IncrementalEtlCheckpointV1,
) -> Result<PathBuf, CheckpointError> {
    let path = path.as_ref();
    let parent = path.parent().ok_or(CheckpointError::Io)?;
    fs::create_dir_all(parent).map_err(|_| CheckpointError::Io)?;
    let temporary = parent.join(format!(
        ".checkpoint.tmp-{}",
        CHECKPOINT_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = checkpoint.encode()?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary).map_err(|_| CheckpointError::Io)?;
        file.write_all(&bytes).map_err(|_| CheckpointError::Io)?;
        file.sync_all().map_err(|_| CheckpointError::Io)?;
        fs::rename(&temporary, path).map_err(|_| CheckpointError::Io)?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| CheckpointError::Io)?;
        Ok(path.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<IncrementalEtlCheckpointV1, CheckpointError> {
    let bytes = fs::read(path).map_err(|_| CheckpointError::Io)?;
    IncrementalEtlCheckpointV1::decode(&bytes)
}

impl IncrementalEtlCheckpointV1 {
    pub fn validate_against(
        &self,
        snapshot: &RecoveryAuthoritativeSnapshot,
    ) -> Result<(), CheckpointError> {
        self.validate()?;
        if self.recording_id != snapshot.recording_id
            || self.epoch_ordinal != snapshot.epoch_ordinal
        {
            return Err(CheckpointError::WrongEpoch);
        }
        if self.marker != snapshot.marker || self.segment_lineage != snapshot.segment_lineage {
            return Err(CheckpointError::LineageMismatch);
        }
        if self.pipeline_version != snapshot.pipeline_version {
            return Err(CheckpointError::PipelineMismatch);
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<(), CheckpointError> {
        if self.version != INCREMENTAL_CHECKPOINT_SCHEMA_VERSION
            || self.owner != CheckpointOwner::Recorder
            || self.config_digest.len() != 64
            || !is_digest(&self.config_digest)
            || self.pipeline_version.len() > 32
            || self.segment_lineage.len() > MAX_SEGMENTS
            || self.decoder.connections.len() > MAX_CONNECTIONS
            || self.outputs.len() > MAX_OUTPUTS
            || self.decoder.serialized_bytes > MAX_STATE_BYTES
        {
            return Err(CheckpointError::Invalid);
        }
        validate_marker(&self.marker)?;
        for pair in self.segment_lineage.windows(2) {
            if pair[0].ordinal >= pair[1].ordinal || pair[0].last_sequence >= pair[1].first_sequence
            {
                return Err(CheckpointError::ContradictoryLineage);
            }
            validate_digest(&pair[0].digest)?;
        }
        if let Some(segment) = self.segment_lineage.last() {
            validate_digest(&segment.digest)?;
            if segment.last_sequence > self.marker.sequence {
                return Err(CheckpointError::AheadOfMarker);
            }
        }
        if let Some(digest) = &self.predecessor_digest {
            validate_digest(digest)?;
        }
        for output in &self.outputs {
            if output.key.is_empty()
                || output.key.len() > 512
                || output.key.contains("..")
                || output.key.starts_with('/')
            {
                return Err(CheckpointError::Invalid);
            }
            validate_digest(&output.digest)?;
            if output.last_marker_sequence > self.marker.sequence
                || output.first_marker_sequence > output.last_marker_sequence
            {
                return Err(CheckpointError::AheadOfMarker);
            }
        }
        for connection in &self.decoder.connections {
            if connection.lifecycle.len() > 32
                || connection.tcp.client_to_server_buffer_bytes > MAX_STATE_BYTES as u64
                || connection.tcp.server_to_client_buffer_bytes > MAX_STATE_BYTES as u64
            {
                return Err(CheckpointError::Invalid);
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, CheckpointError> {
        self.validate()?;
        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        let bytes = serde_json::to_vec(&unsigned).map_err(|_| CheckpointError::Serialization)?;
        let mut signed = unsigned;
        signed.checksum = digest(&bytes);
        serde_json::to_vec_pretty(&signed).map_err(|_| CheckpointError::Serialization)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, CheckpointError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| CheckpointError::Parse)?;
        value.validate()?;
        let mut unsigned = value.clone();
        let checksum = unsigned.checksum.clone();
        unsigned.checksum.clear();
        let expected =
            digest(&serde_json::to_vec(&unsigned).map_err(|_| CheckpointError::Serialization)?);
        if checksum != expected {
            return Err(CheckpointError::ChecksumMismatch);
        }
        Ok(value)
    }

    pub fn summary(&self) -> CheckpointSummary {
        CheckpointSummary {
            marker_sequence: self.marker.sequence,
            segment_count: self.segment_lineage.len(),
            connection_count: self.decoder.connections.len(),
            output_count: self.outputs.len(),
            source_status: self.source_status,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CheckpointSummary {
    pub marker_sequence: u64,
    pub segment_count: usize,
    pub connection_count: usize,
    pub output_count: usize,
    pub source_status: SourceStatus,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CheckpointError {
    #[error("incremental checkpoint is invalid")]
    Invalid,
    #[error("incremental checkpoint lineage is contradictory")]
    ContradictoryLineage,
    #[error("incremental checkpoint is ahead of committed marker")]
    AheadOfMarker,
    #[error("incremental checkpoint belongs to a different epoch")]
    WrongEpoch,
    #[error("incremental checkpoint lineage does not match recovery authority")]
    LineageMismatch,
    #[error("incremental checkpoint pipeline does not match recovery authority")]
    PipelineMismatch,
    #[error("incremental checkpoint checksum mismatch")]
    ChecksumMismatch,
    #[error("incremental checkpoint could not be parsed")]
    Parse,
    #[error("incremental checkpoint serialization failed")]
    Serialization,
    #[error("incremental checkpoint I/O failed")]
    Io,
}

fn validate_marker(marker: &MarkerLineage) -> Result<(), CheckpointError> {
    validate_digest(&marker.marker_digest)
}

fn validate_digest(value: &str) -> Result<(), CheckpointError> {
    if is_digest(value) {
        Ok(())
    } else {
        Err(CheckpointError::Invalid)
    }
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

    fn checkpoint() -> IncrementalEtlCheckpointV1 {
        IncrementalEtlCheckpointV1 {
            version: 1,
            owner: CheckpointOwner::Recorder,
            lifecycle: CheckpointLifecycle::Active,
            recording_id: Uuid::new_v4(),
            epoch_ordinal: 1,
            config_digest: "a".repeat(64),
            pipeline_version: "p2".into(),
            canonical_schema_version: 1,
            marker: MarkerLineage {
                sequence: 4,
                segment_ordinal: 0,
                marker_digest: "b".repeat(64),
            },
            segment_lineage: vec![SegmentLineage {
                ordinal: 0,
                digest: "c".repeat(64),
                first_sequence: 0,
                last_sequence: 4,
            }],
            predecessor_digest: None,
            decoder: DecoderReconstructionState {
                connections: vec![ConnectionState {
                    connection_id: Uuid::new_v4(),
                    generation: 2,
                    tcp: TcpDirectionState {
                        client_to_server_offset: 10,
                        server_to_client_offset: 20,
                        client_to_server_buffer_bytes: 3,
                        server_to_client_buffer_bytes: 4,
                    },
                    http: HttpCorrelationState {
                        request_queue_depth: 1,
                        active_request_id: Some(Uuid::new_v4()),
                        body_bytes_pending: 5,
                    },
                    lifecycle: "open".into(),
                }],
                serialized_bytes: 128,
            },
            outputs: vec![DeltaBatchReference {
                key: "delta/1".into(),
                digest: "d".repeat(64),
                first_marker_sequence: 1,
                last_marker_sequence: 4,
                bytes: 8,
            }],
            source_status: SourceStatus::Live,
            checksum: String::new(),
        }
    }

    #[test]
    fn checkpoint_round_trip_is_deterministic_and_payload_free() {
        let value = checkpoint();
        let bytes = value.encode().unwrap();
        let decoded = IncrementalEtlCheckpointV1::decode(&bytes).unwrap();
        assert_eq!(decoded.summary().marker_sequence, 4);
        assert_eq!(bytes, decoded.encode().unwrap());
    }

    #[test]
    fn atomic_checkpoint_round_trip() {
        let root =
            std::env::temp_dir().join(format!("chronicle-checkpoint-{}", std::process::id()));
        let path = root.join("checkpoint.json");
        let value = checkpoint();
        write_checkpoint_atomic(&path, &value).unwrap();
        assert_eq!(
            read_checkpoint(&path).unwrap().encode().unwrap(),
            value.encode().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn lineage_validation_rejects_wrong_epoch_and_accepts_authority() {
        let value = checkpoint();
        let snapshot = RecoveryAuthoritativeSnapshot {
            recording_id: value.recording_id,
            epoch_ordinal: value.epoch_ordinal,
            marker: value.marker.clone(),
            segment_lineage: value.segment_lineage.clone(),
            pipeline_version: value.pipeline_version.clone(),
        };
        assert_eq!(value.validate_against(&snapshot), Ok(()));
        let mut wrong = snapshot;
        wrong.epoch_ordinal += 1;
        assert_eq!(
            value.validate_against(&wrong),
            Err(CheckpointError::WrongEpoch)
        );
    }

    #[test]
    fn checkpoint_rejects_non_v1_and_ahead_output() {
        let mut value = checkpoint();
        value.version = 2;
        assert_eq!(value.encode(), Err(CheckpointError::Invalid));
        let mut value = checkpoint();
        value.outputs[0].last_marker_sequence = 5;
        assert_eq!(value.encode(), Err(CheckpointError::AheadOfMarker));
    }
}
