//! Shared continuous-recorder runtime around startup, capture, rollover, and drain.
#![allow(clippy::result_large_err)]

/// Parent-aware incremental ETL checkpoint written by the publication
/// transaction and validated at startup.
const INCREMENTAL_ETL_CHECKPOINT_FILE: &str = "incremental-etl-checkpoint-v2.json";

use crate::{
    EpochMetadata, IncrementalWorker, IncrementalWorkerPolicy, LagSummary, MetadataCode,
    NormalizedRecorderConfig, QuotaStatus, RecorderCounters, RecorderEpochBoundary, RecorderHealth,
    RecorderLifecycleError, RecorderMetadataV1, RecorderOrchestrationError, RecorderOrchestrator,
    RecorderPoll, RecorderReadiness, RecorderStartup, RecorderStartupError,
    RecordingCaptureMetadata, RecordingMetadata, RecordingStatus, ReservationKind,
    RolloverTransition, RolloverTransitionPhase, load_transition, write_recorder_metadata,
    write_recording_metadata, write_transition_atomic,
};
use chronicle_capture::{CaptureError, CaptureSource};
use chronicle_common::{EpochId, RecordingId, SessionId};
use chronicle_etl::{
    CanonicalDeltaBatchV1, CheckpointFault, CheckpointLifecycle, CheckpointOwner,
    CommittedWalSnapshot, ContinuationDependency, ContinuationLimits, ContinuationState,
    DeltaBatchReference, EPOCH_CONTINUATION_IN_FILE, EPOCH_CONTINUATION_OUT_FILE,
    EPOCH_CONTINUATION_STATE_FILE, ETL_PIPELINE_VERSION, EpochContinuationCheckpoint,
    INCREMENTAL_CHECKPOINT_SCHEMA_VERSION, IncrementalEtlCheckpoint, IncrementalProcessor,
    IncrementalResult, MarkerLineage, SegmentLineage, SourceStatus, publish_delta_then_checkpoint,
    publish_delta_then_checkpoint_with_fault, recover_pending_checkpoint,
    stamp_epoch_operation_provenance,
};
use chronicle_protocol::ProtocolRegistry;
use chronicle_protocol_builtins::registry;
use chronicle_storage::{
    FilesystemRecordingStore, FilesystemSessionStore, RecordingArtifact, RecordingArtifactKind,
    RecordingStore,
};
use chronicle_wal::{GroupCommitWalWriter, WalError};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Clone, Debug)]
struct PendingContinuation {
    old_wal_directory: std::path::PathBuf,
    old_epoch_id: EpochId,
    successor_epoch_id: EpochId,
    old_epoch_ordinal: u64,
}

fn recover_pending_continuation(
    wal_directory: &Path,
    current_epoch_id: EpochId,
    continuation_in: Option<&EpochContinuationCheckpoint>,
) -> Result<Option<PendingContinuation>, ContinuousRecorderError> {
    let state_path = wal_directory.join(EPOCH_CONTINUATION_STATE_FILE);
    match fs::metadata(&state_path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ContinuousRecorderError::Incremental(error.to_string()));
        }
    }
    let dependency = ContinuationDependency::load(&state_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if dependency.successor_epoch_id != current_epoch_id {
        return Err(ContinuousRecorderError::Incremental(
            "continuation dependency successor mismatch".into(),
        ));
    }
    let Some(predecessor_epoch_id) = dependency.predecessor_epoch_id else {
        return Err(ContinuousRecorderError::Incremental(
            "pending continuation has no predecessor".into(),
        ));
    };
    if let Some(checkpoint) = continuation_in {
        if checkpoint.predecessor_epoch_id != predecessor_epoch_id
            || checkpoint.successor_epoch_id != current_epoch_id
        {
            return Err(ContinuousRecorderError::Incremental(
                "continuation dependency/input lineage mismatch".into(),
            ));
        }
        if dependency.state == ContinuationState::Pending {
            let parent_id = dependency.parent_id.unwrap_or(checkpoint.parent_id);
            ContinuationDependency::ready_from_checkpoint(
                parent_id,
                predecessor_epoch_id,
                checkpoint,
                &ETL_PIPELINE_VERSION.to_string(),
            )
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?
            .write_atomic(&state_path)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        }
        return Ok(None);
    }
    if dependency.state == ContinuationState::Ready {
        return Err(ContinuousRecorderError::Incremental(
            "ready continuation has no successor handoff".into(),
        ));
    }
    if dependency.state != ContinuationState::Pending {
        return Ok(None);
    }
    let epochs_directory = wal_directory
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "epochs"))
        .ok_or_else(|| {
            ContinuousRecorderError::Incremental(
                "pending continuation active path is outside epoch catalog".into(),
            )
        })?;
    let root = epochs_directory.parent().ok_or_else(|| {
        ContinuousRecorderError::Incremental("pending continuation catalog root missing".into())
    })?;
    let catalog = crate::EpochCatalog::load(root)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let predecessor = catalog.epoch(predecessor_epoch_id).ok_or_else(|| {
        ContinuousRecorderError::Incremental("pending continuation predecessor missing".into())
    })?;
    Ok(Some(PendingContinuation {
        old_wal_directory: root.join(&predecessor.path),
        old_epoch_id: predecessor_epoch_id,
        successor_epoch_id: current_epoch_id,
        old_epoch_ordinal: predecessor.ordinal,
    }))
}

#[derive(Debug, Error)]
pub enum ContinuousRecorderError {
    #[error(transparent)]
    Startup(#[from] RecorderStartupError),
    #[error(transparent)]
    Orchestration(#[from] RecorderOrchestrationError),
    #[error(transparent)]
    Lifecycle(#[from] RecorderLifecycleError),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("recorder metadata persistence failed: {0}")]
    Metadata(String),
    #[error("incremental ETL failed: {0}")]
    Incremental(String),
    #[error(transparent)]
    Quota(#[from] crate::QuotaError),
}

/// Foreground runtime. Owns startup lease and capture source for entire epoch sequence.
pub struct ContinuousRecorderService<S> {
    startup: RecorderStartup,
    recorder: RecorderOrchestrator<S>,
    wal_directory: std::path::PathBuf,
    metadata: RecordingMetadata,
    active_metadata: RecorderMetadataV1,
    state_root: std::path::PathBuf,
    store_root: std::path::PathBuf,
    incremental_worker: IncrementalWorker,
    incremental_processor: IncrementalProcessor,
    incremental_registry: ProtocolRegistry,
    incremental_session_id: SessionId,
    incremental_checkpoint: Option<IncrementalEtlCheckpoint>,
    /// Verified predecessor handoff for current epoch ETL. Continuation state
    /// is separate and immutable.
    continuation_in: Option<EpochContinuationCheckpoint>,
    continuation_out: Option<EpochContinuationCheckpoint>,
    pending_continuation: Option<PendingContinuation>,
    etl_batch_records: u64,
    next_incremental_millis: u64,
    checkpoint_fault: Option<CheckpointFault>,
    last_wal_bytes: u64,
    epoch_max_bytes: u64,
}

mod continuation;
mod finalization;
mod incremental;
mod metadata;
mod rollover;
mod startup;
#[cfg(test)]
mod tests;

#[allow(clippy::too_many_lines)]
fn validate_checkpoint_before_capture(
    config: &NormalizedRecorderConfig,
    wal_directory: &Path,
    recording_id: RecordingId,
    _epoch_ordinal: u64,
) -> Result<(), ContinuousRecorderError> {
    let checkpoint_path = wal_directory.join(INCREMENTAL_ETL_CHECKPOINT_FILE);
    let store = FilesystemRecordingStore::new(config.store_root.clone());
    recover_pending_checkpoint(&store, &checkpoint_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if !checkpoint_path.exists() {
        return Ok(());
    }
    let checkpoint = IncrementalEtlCheckpoint::load(&checkpoint_path)
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let config_digest = config
        .stable_digest()
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint.config_digest != config_digest
        || checkpoint.canonical_schema_version != chronicle_canonical::CANONICAL_SCHEMA_VERSION
        || checkpoint.pipeline_version != ETL_PIPELINE_VERSION.to_string()
    {
        return Err(ContinuousRecorderError::Incremental(format!(
            "checkpoint configuration or pipeline contradiction: config={} expected={} schema={} pipeline={}",
            checkpoint.config_digest,
            config_digest,
            checkpoint.canonical_schema_version,
            checkpoint.pipeline_version,
        )));
    }
    let committed = chronicle_wal::read_committed_snapshot_with_records(
        wal_directory,
        recording_id,
        chronicle_wal::DEFAULT_MAX_RECORD_BYTES,
    )
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    let active_segment_digest = chronicle_wal::verified_segment_prefix_sha256(
        wal_directory,
        recording_id,
        &committed.envelopes,
        committed.snapshot.marker_segment_ordinal,
        committed.snapshot.marker_sequence,
    )
    .map(|digest| digest_hex(&digest))
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint.marker.sequence < committed.snapshot.marker_sequence {
        let marker_digest = committed
            .envelopes
            .iter()
            .find(|envelope| envelope.sequence == checkpoint.marker.sequence)
            .and_then(|envelope| chronicle_wal::encode_envelope(envelope).ok())
            .map(|bytes| {
                let digest: [u8; 32] = Sha256::digest(bytes).into();
                digest_hex(&digest)
            });
        if marker_digest.as_deref() != Some(checkpoint.marker.marker_digest.as_str()) {
            return Err(ContinuousRecorderError::Incremental(
                "checkpoint historical marker digest mismatch".into(),
            ));
        }
    }
    let checkpoint_active_digest = chronicle_wal::verified_segment_prefix_sha256(
        wal_directory,
        recording_id,
        &committed.envelopes,
        checkpoint.marker.segment_ordinal,
        checkpoint.marker.sequence,
    )
    .map(|digest| digest_hex(&digest))
    .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    if checkpoint_active_digest != checkpoint.marker.active_segment_digest {
        return Err(ContinuousRecorderError::Incremental(
            "checkpoint active segment digest mismatch".into(),
        ));
    }
    let marker = MarkerLineage {
        sequence: committed.snapshot.marker_sequence,
        segment_ordinal: committed.snapshot.marker_segment_ordinal,
        marker_digest: digest_hex(&committed.snapshot.marker_digest),
        active_segment_digest,
    };
    let segment_lineage = committed
        .snapshot
        .segment_lineage
        .iter()
        .map(|segment| SegmentLineage {
            ordinal: segment.ordinal,
            digest: digest_hex(&segment.digest),
            first_sequence: segment.first_sequence,
            last_sequence: segment.last_sequence,
        })
        .collect::<Vec<_>>();
    checkpoint
        .validate_against(
            checkpoint.parent_id,
            EpochId::from_uuid(recording_id.0),
            &marker,
            &segment_lineage,
            &ETL_PIPELINE_VERSION.to_string(),
        )
        .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
    for output in &checkpoint.outputs {
        let head = store
            .head(&output.key)
            .map_err(|error| ContinuousRecorderError::Incremental(error.to_string()))?;
        if head.key != output.key
            || head.kind != RecordingArtifactKind::CanonicalDeltaBatch
            || head.checksum != output.digest
            || head.bytes != output.bytes
        {
            return Err(ContinuousRecorderError::Incremental(
                "checkpoint output artifact verification failed".into(),
            ));
        }
    }
    Ok(())
}

fn digest_hex(value: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
