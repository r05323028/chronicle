use crate::{
    CheckpointLifecycle, CheckpointOwner, DECODER_IMPLEMENTATION_VERSION, DECODER_KIND,
    DECODER_SNAPSHOT_VERSION, DecoderReconstructionState, DeltaBatchReference, MarkerLineage,
    SegmentLineage, SourceStatus,
};
use chronicle_common::{EpochId, RecordingId};
use chronicle_session::{ReconstructionAssembler, ReconstructionLimits};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const EPOCH_CONTINUATION_CHECKPOINT_VERSION: u16 = 1;
pub const INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION: u16 = 2;
pub const EPOCH_CONTINUATION_IN_FILE: &str = "continuation-in.json";
pub const EPOCH_CONTINUATION_OUT_FILE: &str = "continuation-out.json";
pub const EPOCH_CONTINUATION_STATE_FILE: &str = "continuation-state.json";
/// Compatibility alias for the successor-facing continuation output.
pub const EPOCH_CONTINUATION_FILE: &str = EPOCH_CONTINUATION_OUT_FILE;
pub const MAX_CONTINUATION_SEGMENTS: usize = 256;
pub const MAX_CONTINUATION_STATE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuationState {
    Pending,
    Ready,
    Consumed,
    Unavailable,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationLimits {
    pub max_connections: usize,
    pub max_generation_entries: usize,
    pub max_partial_frame_bytes: usize,
    pub max_correlation_entries: usize,
    pub max_loss_windows: usize,
    pub max_issue_entries: usize,
    pub max_state_bytes: usize,
}

impl Default for ContinuationLimits {
    fn default() -> Self {
        Self {
            max_connections: 4_096,
            max_generation_entries: 8_192,
            max_partial_frame_bytes: 8 * 1024 * 1024,
            max_correlation_entries: 16_384,
            max_loss_windows: 4_096,
            max_issue_entries: 4_096,
            max_state_bytes: MAX_CONTINUATION_STATE_BYTES,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochContinuationCheckpointV1 {
    pub version: u16,
    pub parent_id: RecordingId,
    pub predecessor_epoch_id: EpochId,
    pub successor_epoch_id: EpochId,
    pub predecessor_final_marker: MarkerLineage,
    pub segment_lineage: Vec<SegmentLineage>,
    pub decoder_kind: String,
    pub decoder_schema_version: u32,
    pub pipeline_version: String,
    pub state: Vec<u8>,
    pub connection_count: usize,
    pub generation_count: usize,
    pub partial_frame_bytes: usize,
    pub correlation_count: usize,
    pub negotiation_count: usize,
    pub loss_window_count: usize,
    pub issue_count: usize,
    pub limits: ContinuationLimits,
    pub checksum: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ContinuationError {
    #[error("continuation checkpoint is invalid: {0}")]
    Invalid(&'static str),
    #[error("continuation checkpoint checksum mismatch")]
    ChecksumMismatch,
    #[error("continuation checkpoint I/O failed")]
    Io,
    #[error("continuation checkpoint serialization failed")]
    Serialization,
    #[error("continuation checkpoint could not be parsed")]
    Parse,
    #[error("continuation checkpoint lineage mismatch")]
    LineageMismatch,
    #[error("continuation checkpoint state transition is invalid")]
    InvalidTransition,
}

impl EpochContinuationCheckpointV1 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        parent_id: RecordingId,
        predecessor_epoch_id: EpochId,
        successor_epoch_id: EpochId,
        predecessor_final_marker: MarkerLineage,
        segment_lineage: Vec<SegmentLineage>,
        pipeline_version: impl Into<String>,
        state: Vec<u8>,
        limits: ContinuationLimits,
    ) -> Result<Self, ContinuationError> {
        let value = Self {
            version: EPOCH_CONTINUATION_CHECKPOINT_VERSION,
            parent_id,
            predecessor_epoch_id,
            successor_epoch_id,
            predecessor_final_marker,
            segment_lineage,
            decoder_kind: DECODER_KIND.into(),
            decoder_schema_version: DECODER_SNAPSHOT_VERSION,
            pipeline_version: pipeline_version.into(),
            state,
            connection_count: 0,
            generation_count: 0,
            partial_frame_bytes: 0,
            correlation_count: 0,
            negotiation_count: 0,
            loss_window_count: 0,
            issue_count: 0,
            limits,
            checksum: String::new(),
        };
        value.with_checksum()
    }

    pub fn with_counts(
        mut self,
        [
            connection_count,
            generation_count,
            partial_frame_bytes,
            correlation_count,
            negotiation_count,
            loss_window_count,
            issue_count,
        ]: [usize; 7],
    ) -> Result<Self, ContinuationError> {
        self.connection_count = connection_count;
        self.generation_count = generation_count;
        self.partial_frame_bytes = partial_frame_bytes;
        self.correlation_count = correlation_count;
        self.negotiation_count = negotiation_count;
        self.loss_window_count = loss_window_count;
        self.issue_count = issue_count;
        self.with_checksum()
    }

    pub fn validate(&self) -> Result<(), ContinuationError> {
        self.validate_without_checksum()?;
        if self.checksum != self.compute_checksum()? {
            return Err(ContinuationError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn validate_against(
        &self,
        parent_id: RecordingId,
        predecessor_epoch_id: EpochId,
        successor_epoch_id: EpochId,
        marker: &MarkerLineage,
        segment_lineage: &[SegmentLineage],
        pipeline_version: &str,
    ) -> Result<(), ContinuationError> {
        self.validate()?;
        if self.parent_id != parent_id
            || self.predecessor_epoch_id != predecessor_epoch_id
            || self.successor_epoch_id != successor_epoch_id
            || self.pipeline_version != pipeline_version
            || self.predecessor_final_marker != *marker
            || self.segment_lineage != segment_lineage
        {
            return Err(ContinuationError::LineageMismatch);
        }
        let restored =
            ReconstructionAssembler::restore(ReconstructionLimits::default(), &self.state)
                .map_err(|_| ContinuationError::Invalid("decoder state"))?;
        if restored.pending_connection_count() != self.connection_count {
            return Err(ContinuationError::Invalid("decoder connection count"));
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ContinuationError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum = value.compute_checksum()?;
        let mut bytes =
            serde_json::to_vec_pretty(&value).map_err(|_| ContinuationError::Serialization)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ContinuationError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ContinuationError::Parse)?;
        value.validate()?;
        Ok(value)
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<PathBuf, ContinuationError> {
        write_json_atomic(path.as_ref(), &self.encode()?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ContinuationError> {
        Self::decode(&fs::read(path).map_err(|_| ContinuationError::Io)?)
    }

    fn with_checksum(mut self) -> Result<Self, ContinuationError> {
        self.validate_without_checksum()?;
        self.checksum = self.compute_checksum()?;
        Ok(self)
    }

    fn validate_without_checksum(&self) -> Result<(), ContinuationError> {
        if self.version != EPOCH_CONTINUATION_CHECKPOINT_VERSION
            || self.parent_id.as_uuid().is_nil()
            || self.predecessor_epoch_id.as_uuid().is_nil()
            || self.successor_epoch_id.as_uuid().is_nil()
            || self.predecessor_epoch_id == self.successor_epoch_id
            || self.decoder_kind != DECODER_KIND
            || self.decoder_schema_version != DECODER_SNAPSHOT_VERSION
            || self.pipeline_version.is_empty()
            || self.pipeline_version.len() > 64
            || self.state.is_empty()
            || self.state.len() > self.limits.max_state_bytes
            || self.state.len() > MAX_CONTINUATION_STATE_BYTES
            || self.segment_lineage.len() > MAX_CONTINUATION_SEGMENTS
            || self.connection_count > self.limits.max_connections
            || self.generation_count > self.limits.max_generation_entries
            || self.partial_frame_bytes > self.limits.max_partial_frame_bytes
            || self.correlation_count > self.limits.max_correlation_entries
            || self.loss_window_count > self.limits.max_loss_windows
            || self.issue_count > self.limits.max_issue_entries
        {
            return Err(ContinuationError::Invalid("continuation fields"));
        }
        validate_marker(&self.predecessor_final_marker)?;
        for segment in &self.segment_lineage {
            if segment.first_sequence > segment.last_sequence
                || !is_digest(&segment.digest)
                || segment.last_sequence > self.predecessor_final_marker.sequence
            {
                return Err(ContinuationError::Invalid("continuation segment lineage"));
            }
        }
        for pair in self.segment_lineage.windows(2) {
            if pair[0].ordinal >= pair[1].ordinal || pair[0].last_sequence >= pair[1].first_sequence
            {
                return Err(ContinuationError::Invalid("continuation segment lineage"));
            }
        }
        Ok(())
    }

    fn compute_checksum(&self) -> Result<String, ContinuationError> {
        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        checksum_json(&unsigned)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContinuationDependencyV2 {
    #[serde(default)]
    pub parent_id: Option<RecordingId>,
    pub state: ContinuationState,
    pub predecessor_epoch_id: Option<EpochId>,
    pub successor_epoch_id: EpochId,
    pub input_digest: Option<String>,
    pub output_digest: Option<String>,
    pub failure_code: Option<String>,
}

impl ContinuationDependencyV2 {
    pub fn pending(predecessor_epoch_id: EpochId, successor_epoch_id: EpochId) -> Self {
        Self::pending_for_parent(None, predecessor_epoch_id, successor_epoch_id)
    }

    pub fn pending_for_parent(
        parent_id: Option<RecordingId>,
        predecessor_epoch_id: EpochId,
        successor_epoch_id: EpochId,
    ) -> Self {
        Self {
            parent_id,
            state: ContinuationState::Pending,
            predecessor_epoch_id: Some(predecessor_epoch_id),
            successor_epoch_id,
            input_digest: None,
            output_digest: None,
            failure_code: None,
        }
    }

    /// Verify one immutable predecessor checkpoint and enter `ready` with
    /// its exact content digest. This does not authorize WAL/topology changes.
    pub fn ready_from_checkpoint(
        parent_id: RecordingId,
        predecessor_epoch_id: EpochId,
        checkpoint: &EpochContinuationCheckpointV1,
        pipeline_version: &str,
    ) -> Result<Self, ContinuationError> {
        if checkpoint.parent_id != parent_id
            || checkpoint.predecessor_epoch_id != predecessor_epoch_id
            || checkpoint.pipeline_version != pipeline_version
        {
            return Err(ContinuationError::LineageMismatch);
        }
        checkpoint.validate()?;
        let bytes = checkpoint.encode()?;
        let digest = digest_bytes(&bytes);
        Self::pending_for_parent(
            Some(parent_id),
            predecessor_epoch_id,
            checkpoint.successor_epoch_id,
        )
        .transition(ContinuationState::Ready, Some(digest), None)
    }

    /// Consume ready artifact exactly once; successor input must match the
    /// checksummed continuation that reached `ready`.
    pub fn consume(self, input_digest: impl Into<String>) -> Result<Self, ContinuationError> {
        let input_digest = input_digest.into();
        if self.state != ContinuationState::Ready
            || self.output_digest.as_deref() != Some(input_digest.as_str())
        {
            return Err(ContinuationError::ChecksumMismatch);
        }
        self.transition(ContinuationState::Consumed, Some(input_digest), None)
    }

    pub fn transition(
        mut self,
        state: ContinuationState,
        digest: Option<String>,
        failure_code: Option<String>,
    ) -> Result<Self, ContinuationError> {
        let allowed = matches!(
            (self.state, state),
            (
                ContinuationState::Pending,
                ContinuationState::Ready
                    | ContinuationState::Unavailable
                    | ContinuationState::Failed
            ) | (ContinuationState::Ready, ContinuationState::Consumed)
        );
        if !allowed {
            return Err(ContinuationError::InvalidTransition);
        }
        match state {
            ContinuationState::Ready => self.output_digest = digest,
            ContinuationState::Consumed => self.input_digest = digest,
            ContinuationState::Unavailable | ContinuationState::Failed => {
                self.failure_code = failure_code;
            }
            ContinuationState::Pending => {}
        }
        self.state = state;
        self.validate()?;
        Ok(self)
    }

    pub fn encode(&self) -> Result<Vec<u8>, ContinuationError> {
        self.validate()?;
        let mut bytes =
            serde_json::to_vec_pretty(self).map_err(|_| ContinuationError::Serialization)?;
        bytes.push(10);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ContinuationError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ContinuationError::Parse)?;
        value.validate()?;
        Ok(value)
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<PathBuf, ContinuationError> {
        write_json_atomic(path.as_ref(), &self.encode()?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ContinuationError> {
        Self::decode(&fs::read(path).map_err(|_| ContinuationError::Io)?)
    }

    pub fn validate(&self) -> Result<(), ContinuationError> {
        if self.successor_epoch_id.as_uuid().is_nil()
            || self
                .predecessor_epoch_id
                .is_some_and(|id| id.as_uuid().is_nil())
            || self.predecessor_epoch_id == Some(self.successor_epoch_id)
        {
            return Err(ContinuationError::Invalid(
                "continuation dependency identity",
            ));
        }
        if self
            .input_digest
            .as_deref()
            .is_some_and(|digest| !is_digest(digest))
            || self
                .output_digest
                .as_deref()
                .is_some_and(|digest| !is_digest(digest))
        {
            return Err(ContinuationError::Invalid("continuation dependency digest"));
        }
        match self.state {
            ContinuationState::Pending => {
                if self.output_digest.is_some() || self.failure_code.is_some() {
                    return Err(ContinuationError::Invalid("pending continuation fields"));
                }
            }
            ContinuationState::Ready => {
                if self.output_digest.is_none() || self.failure_code.is_some() {
                    return Err(ContinuationError::Invalid("ready continuation fields"));
                }
            }
            ContinuationState::Consumed => {
                if self.input_digest.is_none() || self.output_digest.is_none() {
                    return Err(ContinuationError::Invalid("consumed continuation fields"));
                }
            }
            ContinuationState::Unavailable | ContinuationState::Failed => {
                if self.failure_code.as_deref().is_none_or(str::is_empty) {
                    return Err(ContinuationError::Invalid("failed continuation fields"));
                }
            }
        }
        Ok(())
    }
}

/// ETL-owned lifecycle for one verified predecessor/successor edge.
/// Capture may activate successor WAL independently; this value only gates
/// decoder restoration and retention proof.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinuationCoordinator {
    parent_id: RecordingId,
    predecessor_epoch_id: EpochId,
    successor_epoch_id: EpochId,
    pipeline_version: String,
    dependency: ContinuationDependencyV2,
}

impl ContinuationCoordinator {
    pub fn pending(
        parent_id: RecordingId,
        predecessor_epoch_id: EpochId,
        successor_epoch_id: EpochId,
        pipeline_version: impl Into<String>,
    ) -> Result<Self, ContinuationError> {
        let pipeline_version = pipeline_version.into();
        if pipeline_version.is_empty() || pipeline_version.len() > 64 {
            return Err(ContinuationError::Invalid("continuation pipeline version"));
        }
        let dependency =
            ContinuationDependencyV2::pending(predecessor_epoch_id, successor_epoch_id);
        dependency.validate()?;
        if parent_id.as_uuid().is_nil() {
            return Err(ContinuationError::Invalid("continuation parent identity"));
        }
        Ok(Self {
            parent_id,
            predecessor_epoch_id,
            successor_epoch_id,
            pipeline_version,
            dependency,
        })
    }

    pub fn restore_from_checkpoint(
        checkpoint: &IncrementalEtlCheckpointV2,
        parent_id: RecordingId,
        predecessor_epoch_id: EpochId,
        successor_epoch_id: EpochId,
        pipeline_version: &str,
    ) -> Result<Self, ContinuationError> {
        checkpoint.validate()?;
        if checkpoint.parent_id != parent_id
            || checkpoint.epoch_id != successor_epoch_id
            || checkpoint.pipeline_version != pipeline_version
            || checkpoint.continuation.predecessor_epoch_id != Some(predecessor_epoch_id)
            || checkpoint.continuation.successor_epoch_id != successor_epoch_id
        {
            return Err(ContinuationError::LineageMismatch);
        }
        Ok(Self {
            parent_id,
            predecessor_epoch_id,
            successor_epoch_id,
            pipeline_version: pipeline_version.into(),
            dependency: checkpoint.continuation.clone(),
        })
    }

    pub fn ready_from_checkpoint(
        &self,
        checkpoint: &EpochContinuationCheckpointV1,
    ) -> Result<Self, ContinuationError> {
        if checkpoint.successor_epoch_id != self.successor_epoch_id {
            return Err(ContinuationError::LineageMismatch);
        }
        Ok(Self {
            dependency: ContinuationDependencyV2::ready_from_checkpoint(
                self.parent_id,
                self.predecessor_epoch_id,
                checkpoint,
                &self.pipeline_version,
            )?,
            ..self.clone()
        })
    }

    /// Catch-up proof: successor checkpoint must be valid and name this edge
    /// before predecessor output can become ready.
    pub fn ready_from_incremental_checkpoint(
        &self,
        checkpoint: &IncrementalEtlCheckpointV2,
        handoff: &EpochContinuationCheckpointV1,
    ) -> Result<Self, ContinuationError> {
        checkpoint.validate()?;
        if checkpoint.parent_id != self.parent_id
            || checkpoint.epoch_id != self.successor_epoch_id
            || checkpoint.continuation.predecessor_epoch_id != Some(self.predecessor_epoch_id)
            || checkpoint.continuation.successor_epoch_id != self.successor_epoch_id
        {
            return Err(ContinuationError::LineageMismatch);
        }
        if checkpoint.continuation.state != ContinuationState::Pending {
            return Err(ContinuationError::InvalidTransition);
        }
        self.ready_from_checkpoint(handoff)
    }

    pub fn consume(self, input_digest: impl Into<String>) -> Result<Self, ContinuationError> {
        Ok(Self {
            dependency: self.dependency.consume(input_digest)?,
            ..self
        })
    }

    pub fn unavailable(self, reason: impl Into<String>) -> Result<Self, ContinuationError> {
        self.fail(ContinuationState::Unavailable, reason)
    }

    pub fn failed(self, reason: impl Into<String>) -> Result<Self, ContinuationError> {
        self.fail(ContinuationState::Failed, reason)
    }

    fn fail(
        self,
        state: ContinuationState,
        reason: impl Into<String>,
    ) -> Result<Self, ContinuationError> {
        Ok(Self {
            dependency: self
                .dependency
                .transition(state, None, Some(reason.into()))?,
            ..self
        })
    }

    pub const fn parent_id(&self) -> RecordingId {
        self.parent_id
    }

    pub const fn predecessor_epoch_id(&self) -> EpochId {
        self.predecessor_epoch_id
    }

    pub const fn successor_epoch_id(&self) -> EpochId {
        self.successor_epoch_id
    }

    pub fn pipeline_version(&self) -> &str {
        &self.pipeline_version
    }

    pub const fn state(&self) -> ContinuationState {
        self.dependency.state
    }

    pub const fn dependency(&self) -> &ContinuationDependencyV2 {
        &self.dependency
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IncrementalEtlCheckpointV2 {
    pub version: u16,
    pub owner: CheckpointOwner,
    pub lifecycle: CheckpointLifecycle,
    pub parent_id: RecordingId,
    pub epoch_id: EpochId,
    pub epoch_ordinal: u64,
    pub config_digest: String,
    pub pipeline_version: String,
    pub canonical_schema_version: u16,
    pub marker: MarkerLineage,
    pub segment_lineage: Vec<SegmentLineage>,
    pub continuation: ContinuationDependencyV2,
    pub decoder: DecoderReconstructionState,
    pub outputs: Vec<DeltaBatchReference>,
    pub published_operation_keys: Vec<String>,
    pub source_status: SourceStatus,
    pub checksum: String,
}

impl IncrementalEtlCheckpointV2 {
    pub fn validate(&self) -> Result<(), ContinuationError> {
        self.validate_without_checksum()?;
        if self.checksum != self.compute_checksum()? {
            return Err(ContinuationError::ChecksumMismatch);
        }
        Ok(())
    }

    pub fn with_checksum(mut self) -> Result<Self, ContinuationError> {
        self.validate_without_checksum()?;
        self.checksum = self.compute_checksum()?;
        Ok(self)
    }

    pub fn validate_against(
        &self,
        parent_id: RecordingId,
        epoch_id: EpochId,
        marker: &MarkerLineage,
        segment_lineage: &[SegmentLineage],
        pipeline_version: &str,
    ) -> Result<(), ContinuationError> {
        self.validate()?;
        if self.parent_id != parent_id
            || self.epoch_id != epoch_id
            || self.decoder.recording_id != epoch_id.as_uuid()
        {
            return Err(ContinuationError::LineageMismatch);
        }
        if self.marker.sequence > marker.sequence
            || self.marker.segment_ordinal > marker.segment_ordinal
            || self.segment_lineage.len() > segment_lineage.len()
            || self.pipeline_version != pipeline_version
        {
            return Err(ContinuationError::LineageMismatch);
        }
        if self.marker.sequence == marker.sequence {
            if self.marker != *marker || self.segment_lineage != segment_lineage {
                return Err(ContinuationError::LineageMismatch);
            }
        } else if !self
            .segment_lineage
            .iter()
            .zip(segment_lineage)
            .all(|(left, right)| left == right)
        {
            return Err(ContinuationError::LineageMismatch);
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ContinuationError> {
        self.validate_without_checksum()?;
        let mut value = self.clone();
        value.checksum = value.compute_checksum()?;
        let mut bytes =
            serde_json::to_vec_pretty(&value).map_err(|_| ContinuationError::Serialization)?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ContinuationError> {
        let value: Self = serde_json::from_slice(bytes).map_err(|_| ContinuationError::Parse)?;
        value.validate()?;
        Ok(value)
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> Result<PathBuf, ContinuationError> {
        write_json_atomic(path.as_ref(), &self.encode()?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ContinuationError> {
        Self::decode(&fs::read(path).map_err(|_| ContinuationError::Io)?)
    }

    fn validate_without_checksum(&self) -> Result<(), ContinuationError> {
        if self.version != INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION
            || self.owner != CheckpointOwner::Etl
            || self.parent_id.as_uuid().is_nil()
            || self.epoch_id.as_uuid().is_nil()
            || self.config_digest.len() != 64
            || !is_digest(&self.config_digest)
            || self.pipeline_version.is_empty()
            || self.pipeline_version.len() > 64
            || self.segment_lineage.len() > MAX_CONTINUATION_SEGMENTS
            || self.outputs.len() > 4_096
            || self.published_operation_keys.len() > 10_000
        {
            return Err(ContinuationError::Invalid("incremental checkpoint fields"));
        }
        validate_marker(&self.marker)?;
        self.continuation.validate()?;
        if self.continuation.successor_epoch_id != self.epoch_id {
            return Err(ContinuationError::LineageMismatch);
        }
        if self.decoder.kind != DECODER_KIND
            || self.decoder.implementation_version != DECODER_IMPLEMENTATION_VERSION
            || self.decoder.snapshot_version != DECODER_SNAPSHOT_VERSION
            || self.decoder.session_id.is_nil()
            || self.decoder.recording_id != self.epoch_id.as_uuid()
            || self.decoder.epoch_ordinal != self.epoch_ordinal
            || self.decoder.marker_sequence != self.marker.sequence
            || self.decoder.marker_digest != self.marker.marker_digest
            || self.decoder.state.is_empty()
            || self.decoder.serialized_bytes != self.decoder.state.len()
        {
            return Err(ContinuationError::Invalid("decoder state metadata"));
        }
        let restored =
            ReconstructionAssembler::restore(ReconstructionLimits::default(), &self.decoder.state)
                .map_err(|_| ContinuationError::Invalid("decoder state"))?;
        if restored.pending_connection_count() != self.decoder.pending_connection_count {
            return Err(ContinuationError::Invalid("decoder connection count"));
        }
        for segment in &self.segment_lineage {
            if segment.first_sequence > segment.last_sequence
                || segment.last_sequence > self.marker.sequence
                || !is_digest(&segment.digest)
            {
                return Err(ContinuationError::Invalid("segment lineage"));
            }
        }
        for output in &self.outputs {
            if output.key.is_empty() || output.key.len() > 512 || output.key.contains("..") {
                return Err(ContinuationError::Invalid("output reference"));
            }
            if !is_digest(&output.digest) || output.last_marker_sequence > self.marker.sequence {
                return Err(ContinuationError::Invalid("output reference"));
            }
        }
        Ok(())
    }

    fn compute_checksum(&self) -> Result<String, ContinuationError> {
        let mut unsigned = self.clone();
        unsigned.checksum.clear();
        checksum_json(&unsigned)
    }
}

fn write_json_atomic(path: &Path, bytes: &[u8]) -> Result<PathBuf, ContinuationError> {
    let parent = path.parent().ok_or(ContinuationError::Io)?;
    fs::create_dir_all(parent).map_err(|_| ContinuationError::Io)?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options
            .open(&temporary)
            .map_err(|_| ContinuationError::Io)?;
        file.write_all(bytes).map_err(|_| ContinuationError::Io)?;
        file.sync_all().map_err(|_| ContinuationError::Io)?;
        fs::rename(&temporary, path).map_err(|_| ContinuationError::Io)?;
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ContinuationError::Io)?;
        Ok(path.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_marker(marker: &MarkerLineage) -> Result<(), ContinuationError> {
    if !is_digest(&marker.marker_digest) || !is_digest(&marker.active_segment_digest) {
        return Err(ContinuationError::Invalid("marker lineage"));
    }
    Ok(())
}

fn is_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn digest_bytes(bytes: &[u8]) -> String {
    let mut checksum = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        use std::fmt::Write as _;
        let _ = write!(&mut checksum, "{byte:02x}");
    }
    checksum
}

fn checksum_json<T: Serialize>(value: &T) -> Result<String, ContinuationError> {
    use std::fmt::Write as _;

    let bytes = serde_json::to_vec(value).map_err(|_| ContinuationError::Serialization)?;
    let mut checksum = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut checksum, "{byte:02x}").map_err(|_| ContinuationError::Serialization)?;
    }
    Ok(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_session::ReconstructionAssembler;
    use uuid::Uuid;

    fn marker(sequence: u64) -> MarkerLineage {
        MarkerLineage {
            sequence,
            segment_ordinal: 0,
            marker_digest: "a".repeat(64),
            active_segment_digest: "b".repeat(64),
        }
    }

    fn segments(sequence: u64) -> Vec<SegmentLineage> {
        vec![SegmentLineage {
            ordinal: 0,
            digest: "c".repeat(64),
            first_sequence: 0,
            last_sequence: sequence,
        }]
    }

    #[test]
    fn continuation_round_trip_and_state_machine_are_bounded() {
        let root = std::env::temp_dir().join(format!("chronicle-continuation-{}", Uuid::new_v4()));
        let parent = RecordingId::new();
        let predecessor = EpochId::new();
        let successor = EpochId::new();
        let value = EpochContinuationCheckpointV1::new(
            parent,
            predecessor,
            successor,
            marker(9),
            segments(9),
            "pipeline-v2",
            vec![1, 2, 3],
            ContinuationLimits::default(),
        )
        .unwrap();
        let value = value.with_counts([2, 3, 4, 5, 6, 7, 8]).unwrap();
        value
            .write_atomic(root.join(EPOCH_CONTINUATION_FILE))
            .unwrap();
        assert_eq!(
            EpochContinuationCheckpointV1::load(root.join(EPOCH_CONTINUATION_FILE)).unwrap(),
            value
        );
        let mut corrupted = value.clone();
        corrupted.checksum.replace_range(..1, "0");
        assert_eq!(
            corrupted.validate(),
            Err(ContinuationError::ChecksumMismatch)
        );
        let limits = ContinuationLimits {
            max_partial_frame_bytes: 1,
            ..ContinuationLimits::default()
        };
        assert_eq!(
            EpochContinuationCheckpointV1::new(
                parent,
                predecessor,
                successor,
                marker(9),
                segments(9),
                "pipeline-v2",
                vec![1],
                limits,
            )
            .unwrap()
            .with_counts([0, 0, 2, 0, 0, 0, 0]),
            Err(ContinuationError::Invalid("continuation fields"))
        );
        let dependency = ContinuationDependencyV2::pending(predecessor, successor);
        let ready = dependency
            .transition(ContinuationState::Ready, Some("d".repeat(64)), None)
            .unwrap();
        assert!(matches!(
            ready.clone().consume("e".repeat(64)),
            Err(ContinuationError::ChecksumMismatch)
        ));
        let consumed = ready.consume("d".repeat(64)).unwrap();
        assert_eq!(consumed.state, ContinuationState::Consumed);
        assert!(matches!(
            consumed.transition(ContinuationState::Ready, None, None),
            Err(ContinuationError::InvalidTransition)
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn incremental_v2_binds_parent_epoch_and_decoder_state() {
        let parent = RecordingId::new();
        let predecessor = EpochId::new();
        let epoch = EpochId::new();
        let state = ReconstructionAssembler::default().snapshot().unwrap();
        let value = IncrementalEtlCheckpointV2 {
            version: INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION,
            owner: CheckpointOwner::Etl,
            lifecycle: CheckpointLifecycle::Active,
            parent_id: parent,
            epoch_id: epoch,
            epoch_ordinal: 2,
            config_digest: "f".repeat(64),
            pipeline_version: "pipeline-v2".into(),
            canonical_schema_version: 1,
            marker: marker(9),
            segment_lineage: segments(9),
            continuation: ContinuationDependencyV2::pending(predecessor, epoch),
            decoder: DecoderReconstructionState {
                kind: DECODER_KIND.into(),
                implementation_version: DECODER_IMPLEMENTATION_VERSION,
                snapshot_version: DECODER_SNAPSHOT_VERSION,
                session_id: Uuid::new_v4(),
                recording_id: epoch.as_uuid(),
                epoch_ordinal: 2,
                marker_sequence: 9,
                marker_digest: "a".repeat(64),
                state: state.clone(),
                pending_connection_count: 0,
                serialized_bytes: state.len(),
            },
            outputs: vec![DeltaBatchReference {
                key: "delta/2".into(),
                digest: "1".repeat(64),
                first_marker_sequence: 1,
                last_marker_sequence: 9,
                bytes: 10,
            }],
            published_operation_keys: Vec::new(),
            source_status: SourceStatus::Live,
            checksum: String::new(),
        };
        value.validate_without_checksum().unwrap();
        let value = IncrementalEtlCheckpointV2 {
            checksum: value.compute_checksum().unwrap(),
            ..value
        };
        assert!(value.validate().is_ok());
        let handoff = EpochContinuationCheckpointV1::new(
            parent,
            predecessor,
            epoch,
            marker(9),
            segments(9),
            "pipeline-v2",
            state,
            ContinuationLimits::default(),
        )
        .unwrap();
        let coordinator = ContinuationCoordinator::restore_from_checkpoint(
            &value,
            parent,
            predecessor,
            epoch,
            "pipeline-v2",
        )
        .unwrap();
        assert_eq!(coordinator.state(), ContinuationState::Pending);
        let mut interrupted_handoff = handoff.clone();
        interrupted_handoff.successor_epoch_id = EpochId::new();
        assert_eq!(
            coordinator.ready_from_incremental_checkpoint(&value, &interrupted_handoff),
            Err(ContinuationError::LineageMismatch)
        );
        assert_eq!(coordinator.state(), ContinuationState::Pending);
        let restarted = ContinuationCoordinator::restore_from_checkpoint(
            &value,
            parent,
            predecessor,
            epoch,
            "pipeline-v2",
        )
        .unwrap();
        assert_eq!(restarted.state(), ContinuationState::Pending);
        assert_eq!(
            restarted
                .ready_from_incremental_checkpoint(&value, &handoff)
                .unwrap()
                .state(),
            ContinuationState::Ready
        );
        assert_eq!(
            value.validate_against(parent, epoch, &marker(9), &segments(9), "pipeline-v2"),
            Ok(())
        );
        assert_eq!(
            value.validate_against(
                RecordingId::new(),
                epoch,
                &marker(9),
                &segments(9),
                "pipeline-v2"
            ),
            Err(ContinuationError::LineageMismatch)
        );
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn continuation_restore_validates_edge_and_starts_successor_cursor_fresh() {
        let parent = RecordingId::new();
        let predecessor = EpochId::new();
        let successor = EpochId::new();
        let state = ReconstructionAssembler::default().snapshot().unwrap();
        let checkpoint = EpochContinuationCheckpointV1::new(
            parent,
            predecessor,
            successor,
            marker(9),
            segments(9),
            "pipeline-v2",
            state,
            ContinuationLimits::default(),
        )
        .unwrap();
        checkpoint
            .validate_against(
                parent,
                predecessor,
                successor,
                &marker(9),
                &segments(9),
                "pipeline-v2",
            )
            .unwrap();
        let mut wrong_version = checkpoint.clone();
        wrong_version.version = 2;
        assert_eq!(
            wrong_version.validate(),
            Err(ContinuationError::Invalid("continuation fields"))
        );
        let mut wrong_checksum = checkpoint.clone();
        wrong_checksum.checksum = "0".repeat(64);
        assert_eq!(
            wrong_checksum.validate(),
            Err(ContinuationError::ChecksumMismatch)
        );
        assert_eq!(
            checkpoint.validate_against(
                parent,
                predecessor,
                successor,
                &marker(8),
                &segments(9),
                "pipeline-v2",
            ),
            Err(ContinuationError::LineageMismatch)
        );
        assert_eq!(
            checkpoint.validate_against(
                parent,
                predecessor,
                successor,
                &marker(9),
                &segments(9),
                "other-pipeline",
            ),
            Err(ContinuationError::LineageMismatch)
        );
        let coordinator =
            ContinuationCoordinator::pending(parent, predecessor, successor, "pipeline-v2")
                .unwrap();
        assert_eq!(coordinator.state(), ContinuationState::Pending);
        let coordinator = coordinator.ready_from_checkpoint(&checkpoint).unwrap();
        let coordinator_digest = coordinator.dependency().output_digest.clone().unwrap();
        let coordinator = coordinator.consume(coordinator_digest).unwrap();
        assert_eq!(coordinator.state(), ContinuationState::Consumed);
        assert_eq!(
            ContinuationCoordinator::pending(parent, predecessor, successor, "pipeline-v2")
                .unwrap()
                .unavailable("unsupported")
                .unwrap()
                .state(),
            ContinuationState::Unavailable
        );
        assert_eq!(
            ContinuationCoordinator::pending(parent, predecessor, successor, "pipeline-v2")
                .unwrap()
                .failed("invalid")
                .unwrap()
                .state(),
            ContinuationState::Failed
        );
        let dependency = ContinuationDependencyV2::ready_from_checkpoint(
            parent,
            predecessor,
            &checkpoint,
            "pipeline-v2",
        )
        .unwrap();
        assert_eq!(dependency.state, ContinuationState::Ready);
        let ready_digest = dependency.output_digest.clone().unwrap();
        assert_eq!(
            dependency.consume(ready_digest).unwrap().state,
            ContinuationState::Consumed
        );
        let mut processor =
            crate::IncrementalProcessor::new(chronicle_session::SessionLimits::default());
        processor
            .restore_continuation(&checkpoint, parent, predecessor, successor, "pipeline-v2")
            .unwrap();
        assert_eq!(processor.last_marker_sequence(), None);
        assert!(matches!(
            processor.restore_continuation(
                &checkpoint,
                RecordingId::new(),
                predecessor,
                successor,
                "pipeline-v2",
            ),
            Err(crate::EtlError::DecoderCheckpointMismatch)
        ));
    }

    #[test]
    fn repeated_continuation_edges_keep_state_bounded_and_independent() {
        let parent = RecordingId::new();
        let mut predecessor = EpochId::new();
        for sequence in 1..=8 {
            let successor = EpochId::new();
            let checkpoint = EpochContinuationCheckpointV1::new(
                parent,
                predecessor,
                successor,
                marker(sequence),
                segments(sequence),
                "pipeline-v2",
                vec![u8::try_from(sequence).unwrap()],
                ContinuationLimits::default(),
            )
            .unwrap();
            let coordinator =
                ContinuationCoordinator::pending(parent, predecessor, successor, "pipeline-v2")
                    .unwrap();
            assert_eq!(
                coordinator
                    .ready_from_checkpoint(&checkpoint)
                    .unwrap()
                    .state(),
                ContinuationState::Ready
            );
            assert!(checkpoint.state.len() <= checkpoint.limits.max_state_bytes);
            predecessor = successor;
        }
    }
}
