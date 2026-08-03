//! Output-then-checkpoint publication transaction boundary.
#![allow(clippy::format_collect)]

use crate::{
    CanonicalDeltaBatchV1, CheckpointError, IncrementalEtlCheckpointV1, read_checkpoint,
    write_checkpoint_atomic,
};
use chronicle_storage::{
    FilesystemSessionStore, PublishSession, PutIfAbsent, RecordingArtifact, RecordingArtifactKind,
    RecordingStore, RecordingStoreError,
};
use std::path::Path;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconcileOutcome {
    AlreadyConsistent,
    AdoptedOutput,
    RepairedCheckpoint,
}

#[derive(Debug, Error)]
pub enum PublicationError {
    #[error(transparent)]
    Delta(#[from] crate::DeltaBatchError),
    #[error(transparent)]
    Store(#[from] RecordingStoreError),
    #[error(transparent)]
    Checkpoint(#[from] CheckpointError),
    #[error("published delta failed verification")]
    Verification,
    #[error("checkpoint does not reference published delta")]
    OutputLineage,
    #[error("final session publication failed: {0}")]
    FinalSession(String),
    #[error("checkpoint publication fault injected")]
    InjectedFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CheckpointFault {
    BeforeCheckpoint,
    AfterCheckpoint,
}

pub fn reconcile_delta_checkpoint<S: RecordingStore>(
    store: &S,
    key: &str,
    checkpoint: &IncrementalEtlCheckpointV1,
    checkpoint_path: impl AsRef<Path>,
) -> Result<ReconcileOutcome, PublicationError> {
    let head = store.head(key).map_err(PublicationError::Store)?;
    let referenced = checkpoint
        .outputs
        .iter()
        .find(|output| output.key == key)
        .ok_or(PublicationError::OutputLineage)?;
    if head.checksum != referenced.digest {
        return Err(PublicationError::Verification);
    }
    match read_checkpoint(&checkpoint_path) {
        Ok(existing) if existing.encode()? == checkpoint.encode()? => {
            Ok(ReconcileOutcome::AlreadyConsistent)
        }
        Ok(_) => Err(PublicationError::OutputLineage),
        Err(CheckpointError::Io) => {
            write_checkpoint_atomic(checkpoint_path, checkpoint)?;
            Ok(ReconcileOutcome::RepairedCheckpoint)
        }
        Err(error) => Err(PublicationError::Checkpoint(error)),
    }
}

pub fn verify_one_shot_equivalence(
    incremental: &chronicle_canonical::CanonicalSession,
    one_shot: &chronicle_canonical::CanonicalSession,
) -> Result<(), PublicationError> {
    let left = serde_json::to_vec(incremental).map_err(|_| {
        PublicationError::FinalSession("incremental session serialization failed".into())
    })?;
    let right = serde_json::to_vec(one_shot).map_err(|_| {
        PublicationError::FinalSession("one-shot session serialization failed".into())
    })?;
    if left == right {
        Ok(())
    } else {
        Err(PublicationError::FinalSession(
            "incremental session differs from one-shot session".into(),
        ))
    }
}

pub fn finalize_incremental_session(
    store: &FilesystemSessionStore,
    output: &crate::EtlOutput,
    checkpoint: Option<String>,
) -> Result<(), PublicationError> {
    store
        .publish(PublishSession {
            session: output.session.clone(),
            checkpoint,
            issues: output
                .issues
                .iter()
                .map(|issue| format!("{:?}", issue.kind))
                .collect(),
            replayability: Vec::new(),
            complete: output.issues.is_empty(),
        })
        .map_err(|error| PublicationError::FinalSession(error.to_string()))
}

pub fn publish_delta_then_checkpoint<S: RecordingStore>(
    store: &S,
    key: impl Into<String>,
    batch: &CanonicalDeltaBatchV1,
    checkpoint: &IncrementalEtlCheckpointV1,
    checkpoint_path: impl AsRef<Path>,
) -> Result<PutIfAbsent, PublicationError> {
    publish_delta_then_checkpoint_with_fault(store, key, batch, checkpoint, checkpoint_path, None)
}

fn publish_delta_then_checkpoint_with_fault<S: RecordingStore>(
    store: &S,
    key: impl Into<String>,
    batch: &CanonicalDeltaBatchV1,
    checkpoint: &IncrementalEtlCheckpointV1,
    checkpoint_path: impl AsRef<Path>,
    fault: Option<CheckpointFault>,
) -> Result<PutIfAbsent, PublicationError> {
    batch.validate()?;
    checkpoint.validate()?;
    let key = key.into();
    let artifact = RecordingArtifact::new(
        key.clone(),
        RecordingArtifactKind::CanonicalDeltaBatch,
        batch.encode()?,
    )?;
    let checksum = artifact.checksum.clone();
    let artifact_version = artifact.version;
    let artifact_kind = artifact.kind;
    let artifact_bytes = artifact.bytes.len() as u64;
    if !checkpoint
        .outputs
        .iter()
        .any(|output| output.key == key && output.digest == checksum)
    {
        return Err(PublicationError::OutputLineage);
    }
    let outcome = store.put_if_absent(artifact)?;
    let head = store.head(&key)?;
    if head.version != artifact_version
        || head.kind != artifact_kind
        || head.bytes != artifact_bytes
        || head.checksum != checksum
    {
        return Err(PublicationError::Verification);
    }
    if fault == Some(CheckpointFault::BeforeCheckpoint) {
        return Err(PublicationError::InjectedFault);
    }
    write_checkpoint_atomic(checkpoint_path, checkpoint)?;
    if fault == Some(CheckpointFault::AfterCheckpoint) {
        return Err(PublicationError::InjectedFault);
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CheckpointLifecycle, CheckpointOwner, DecoderReconstructionState, DeltaBatchReference,
        MarkerLineage, SegmentLineage, SourceStatus,
    };
    use chronicle_storage::InMemoryRecordingStore;
    use sha2::Digest;
    use uuid::Uuid;

    fn checkpoint() -> IncrementalEtlCheckpointV1 {
        let recording_id = Uuid::new_v4();
        let state = chronicle_session::ReconstructionAssembler::default()
            .snapshot()
            .unwrap();
        IncrementalEtlCheckpointV1 {
            version: 1,
            owner: CheckpointOwner::Recorder,
            lifecycle: CheckpointLifecycle::Active,
            recording_id,
            epoch_ordinal: 0,
            config_digest: "a".repeat(64),
            pipeline_version: "p".into(),
            canonical_schema_version: 1,
            marker: MarkerLineage {
                sequence: 0,
                segment_ordinal: 0,
                marker_digest: "b".repeat(64),
                active_segment_digest: "e".repeat(64),
            },
            segment_lineage: vec![SegmentLineage {
                ordinal: 0,
                digest: "c".repeat(64),
                first_sequence: 0,
                last_sequence: 0,
            }],
            predecessor_digest: None,
            decoder: DecoderReconstructionState {
                kind: crate::DECODER_KIND.into(),
                implementation_version: crate::DECODER_IMPLEMENTATION_VERSION,
                snapshot_version: crate::DECODER_SNAPSHOT_VERSION,
                session_id: Uuid::new_v4(),
                recording_id,
                epoch_ordinal: 0,
                marker_sequence: 0,
                marker_digest: "b".repeat(64),
                state: state.clone(),
                pending_connection_count: 0,
                serialized_bytes: state.len(),
            },
            outputs: Vec::new(),
            published_operation_keys: Vec::new(),
            source_status: SourceStatus::Live,
            checksum: String::new(),
        }
    }

    #[test]
    fn checkpoint_fault_windows_recover_without_duplicate_output() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key = "delta/fault-window";
        let checkpoint = checkpoint_with_output(key, &batch);
        let root =
            std::env::temp_dir().join(format!("chronicle-pub-fault-{}", uuid::Uuid::new_v4()));
        let checkpoint_path = root.join("checkpoint.json");
        assert!(matches!(
            publish_delta_then_checkpoint_with_fault(
                &store,
                key,
                &batch,
                &checkpoint,
                &checkpoint_path,
                Some(CheckpointFault::BeforeCheckpoint),
            ),
            Err(PublicationError::InjectedFault)
        ));
        assert!(!checkpoint_path.exists());
        publish_delta_then_checkpoint(&store, key, &batch, &checkpoint, &checkpoint_path).unwrap();
        assert!(matches!(
            publish_delta_then_checkpoint_with_fault(
                &store,
                key,
                &batch,
                &checkpoint,
                &checkpoint_path,
                Some(CheckpointFault::AfterCheckpoint),
            ),
            Err(PublicationError::InjectedFault)
        ));
        let persisted = read_checkpoint(&checkpoint_path).unwrap();
        assert_eq!(persisted.outputs, checkpoint.outputs);
        assert_eq!(persisted.recording_id, checkpoint.recording_id);
        publish_delta_then_checkpoint(&store, key, &batch, &checkpoint, &checkpoint_path).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn publication_is_idempotent() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key = format!(
            "delta/{}",
            sha2::Sha256::digest(batch.encode().unwrap())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
        let root = std::env::temp_dir().join(format!("chronicle-pub-{}", std::process::id()));
        let checkpoint_path = root.join("checkpoint.json");
        let value = checkpoint_with_output(&key, &batch);
        let first =
            publish_delta_then_checkpoint(&store, key.clone(), &batch, &value, &checkpoint_path);
        assert!(first.is_ok());
        assert!(
            publish_delta_then_checkpoint(&store, key, &batch, &value, &checkpoint_path).is_ok()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    fn checkpoint_with_output(
        key: &str,
        batch: &CanonicalDeltaBatchV1,
    ) -> IncrementalEtlCheckpointV1 {
        let artifact = RecordingArtifact::new(
            key,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        let mut value = checkpoint();
        value.outputs.push(DeltaBatchReference {
            key: key.into(),
            digest: artifact.checksum,
            first_marker_sequence: 0,
            last_marker_sequence: 0,
            bytes: batch.encode().unwrap().len() as u64,
        });
        value
    }

    #[test]
    fn reconcile_adopts_orphan_output_and_repairs_missing_checkpoint() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key = "delta/orphan";
        let artifact = RecordingArtifact::new(
            key,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact).unwrap();
        let root = std::env::temp_dir().join(format!("chronicle-reconcile-{}", std::process::id()));
        let checkpoint_path = root.join("checkpoint.json");
        let value = checkpoint_with_output(key, &batch);
        assert_eq!(
            reconcile_delta_checkpoint(&store, key, &value, &checkpoint_path).unwrap(),
            ReconcileOutcome::RepairedCheckpoint
        );
        assert_eq!(
            read_checkpoint(&checkpoint_path).unwrap().encode().unwrap(),
            value.encode().unwrap()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reconcile_rejects_corrupt_or_forked_checkpoint() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key = "delta/fork";
        let artifact = RecordingArtifact::new(
            key,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact).unwrap();
        let root =
            std::env::temp_dir().join(format!("chronicle-reconcile-fork-{}", std::process::id()));
        let checkpoint_path = root.join("checkpoint.json");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&checkpoint_path, b"not-json").unwrap();
        let value = checkpoint_with_output(key, &batch);
        assert!(matches!(
            reconcile_delta_checkpoint(&store, key, &value, &checkpoint_path),
            Err(PublicationError::Checkpoint(CheckpointError::Parse))
        ));
        let fork = checkpoint();
        std::fs::write(&checkpoint_path, fork.encode().unwrap()).unwrap();
        assert!(matches!(
            reconcile_delta_checkpoint(&store, key, &value, &checkpoint_path),
            Err(PublicationError::OutputLineage)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
