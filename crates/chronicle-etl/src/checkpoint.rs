//! Shared incremental ETL checkpoint lineage and decoder-state types.
//! The checkpoint model itself lives in `continuation.rs` (parent-aware
//! `IncrementalEtlCheckpoint`); this module owns the versioned building
//! blocks both the checkpoint and continuation artifacts are built from.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DECODER_SNAPSHOT_VERSION: u32 = 1;
pub const DECODER_IMPLEMENTATION_VERSION: u32 = 1;
pub const DECODER_KIND: &str = "chronicle-reconstruction";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointOwner {
    Recorder,
    Etl,
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
    pub active_segment_digest: String,
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
    pub kind: String,
    pub implementation_version: u32,
    pub snapshot_version: u32,
    pub session_id: Uuid,
    pub recording_id: Uuid,
    pub epoch_ordinal: u64,
    pub marker_sequence: u64,
    pub marker_digest: String,
    /// Authoritative versioned snapshot of all mutable reconstruction state.
    pub state: Vec<u8>,
    pub pending_connection_count: usize,
    pub serialized_bytes: usize,
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
