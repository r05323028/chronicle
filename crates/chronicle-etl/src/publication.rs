//! Output-then-checkpoint publication transaction boundary.
#![allow(clippy::format_collect)]

use std::fs::OpenOptions;
use std::sync::atomic::{AtomicU64, Ordering};

use chronicle_common::SessionId;

use crate::{
    CanonicalDeltaBatchV1, CheckpointError, ContinuationError, IncrementalEtlCheckpointV1,
    IncrementalEtlCheckpointV2, read_checkpoint, write_checkpoint_atomic,
};
use chronicle_storage::{
    FilesystemSessionStore, PublishSession, PutIfAbsent, RecordingArtifact, RecordingArtifactKind,
    RecordingStore, RecordingStoreError,
};
use std::fs;
use std::path::{Path, PathBuf};
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
    #[error(transparent)]
    Continuation(#[from] ContinuationError),
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
pub enum CheckpointFault {
    BeforeCheckpoint,
    AfterCheckpoint,
}

pub fn recover_pending_checkpoint<S: RecordingStore>(
    store: &S,
    checkpoint_path: impl AsRef<Path>,
) -> Result<Option<ReconcileOutcome>, PublicationError> {
    let checkpoint_path = checkpoint_path.as_ref();
    let pending_path = pending_checkpoint_path(checkpoint_path);
    if !pending_path.exists() {
        return Ok(None);
    }
    let candidate = read_checkpoint(&pending_path)?;
    let existing = match read_checkpoint(checkpoint_path) {
        Ok(existing) => Some(existing),
        Err(CheckpointError::Io) => None,
        Err(error) => return Err(PublicationError::Checkpoint(error)),
    };
    let existing_len = existing.as_ref().map_or(0, |value| value.outputs.len());
    // The pending candidate must be a strict successor of the durable
    // checkpoint: same output prefix and predecessor digest. Otherwise the
    // interrupted transaction is ambiguous evidence and recovery fails closed.
    if candidate.outputs.len() < existing_len
        || existing.as_ref().is_some_and(|value| {
            candidate.predecessor_digest.as_ref() != Some(&value.checksum)
                || candidate.outputs[..existing_len] != value.outputs
        })
    {
        return Err(PublicationError::OutputLineage);
    }
    let mut present = 0usize;
    for (index, output) in candidate.outputs.iter().enumerate() {
        let head = match store.head(&output.key) {
            Ok(head)
                if head.kind == RecordingArtifactKind::CanonicalDeltaBatch
                    && head.checksum == output.digest
                    && head.bytes == output.bytes =>
            {
                Some(())
            }
            Ok(_) => return Err(PublicationError::Verification),
            Err(RecordingStoreError::NotFound) => None,
            Err(error) => return Err(PublicationError::Store(error)),
        };
        if head.is_some() {
            present += 1;
        }
        // Outputs already referenced by the durable checkpoint must still be
        // durable; a missing interior output is evidence loss and fails closed.
        if index < existing_len && head.is_none() {
            return Err(PublicationError::OutputLineage);
        }
    }
    if present != candidate.outputs.len() {
        // Some new suffix outputs are not durable, so the transaction did not
        // complete. Every existing output is intact; discard only private
        // staging and let the retry reprocess the suffix to convergence.
        remove_pending_checkpoint(&pending_path)?;
        return Ok(None);
    }
    let outcome = match existing {
        Some(existing) if existing.encode()? == candidate.encode()? => {
            ReconcileOutcome::AlreadyConsistent
        }
        Some(_) => {
            write_checkpoint_atomic(checkpoint_path, &candidate)?;
            ReconcileOutcome::AdoptedOutput
        }
        None => {
            write_checkpoint_atomic(checkpoint_path, &candidate)?;
            ReconcileOutcome::RepairedCheckpoint
        }
    };
    remove_pending_checkpoint(&pending_path)?;
    Ok(Some(outcome))
}

/// Recover an interrupted parent-aware checkpoint publication. V2 uses the
/// same output-first contract as V1, but owns its pending file and checksum.
pub fn recover_pending_checkpoint_v2<S: RecordingStore>(
    store: &S,
    checkpoint_path: impl AsRef<Path>,
) -> Result<Option<ReconcileOutcome>, PublicationError> {
    let checkpoint_path = checkpoint_path.as_ref();
    let pending_path = pending_checkpoint_path(checkpoint_path);
    if !pending_path.exists() {
        return Ok(None);
    }
    let candidate = IncrementalEtlCheckpointV2::load(&pending_path)?;
    let existing = match IncrementalEtlCheckpointV2::load(checkpoint_path) {
        Ok(existing) => Some(existing),
        Err(ContinuationError::Io) if !checkpoint_path.exists() => None,
        Err(error) => return Err(PublicationError::Continuation(error)),
    };
    let existing_len = existing.as_ref().map_or(0, |value| value.outputs.len());
    if candidate.outputs.len() < existing_len
        || existing.as_ref().is_some_and(|value| {
            candidate.parent_id != value.parent_id
                || candidate.epoch_id != value.epoch_id
                || candidate.outputs[..existing_len] != value.outputs
        })
    {
        return Err(PublicationError::OutputLineage);
    }
    let mut present = 0usize;
    for (index, output) in candidate.outputs.iter().enumerate() {
        let head = match store.head(&output.key) {
            Ok(head)
                if head.kind == RecordingArtifactKind::CanonicalDeltaBatch
                    && head.checksum == output.digest
                    && head.bytes == output.bytes =>
            {
                Some(())
            }
            Ok(_) => return Err(PublicationError::Verification),
            Err(RecordingStoreError::NotFound) => None,
            Err(error) => return Err(PublicationError::Store(error)),
        };
        if head.is_some() {
            present += 1;
        }
        if index < existing_len && head.is_none() {
            return Err(PublicationError::OutputLineage);
        }
    }
    if present != candidate.outputs.len() {
        remove_pending_v2(&pending_path)?;
        return Ok(None);
    }
    let outcome = match existing {
        Some(existing) if existing.encode()? == candidate.encode()? => {
            ReconcileOutcome::AlreadyConsistent
        }
        Some(_) => {
            candidate.write_atomic(checkpoint_path)?;
            ReconcileOutcome::AdoptedOutput
        }
        None => {
            candidate.write_atomic(checkpoint_path)?;
            ReconcileOutcome::RepairedCheckpoint
        }
    };
    remove_pending_v2(&pending_path)?;
    Ok(Some(outcome))
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
            replayability: output
                .issues
                .iter()
                .map(|issue| format!("{:?}", issue.kind))
                .collect(),
            complete: output.issues.is_empty(),
        })
        .map_err(|error| PublicationError::FinalSession(error.to_string()))
}

/// Outcome of an ETL-owned one-shot final session publication.
pub struct OneShotPublicationOutcome {
    pub session_id: SessionId,
    pub already_published: bool,
    pub manifest_checksum: String,
}

#[derive(Debug, Error)]
pub enum OneShotPublicationError {
    #[error("one-shot session publication failed: {0}")]
    Store(String),
    #[error("one-shot published session differs from expected session {session_id}")]
    Mismatch { session_id: SessionId },
    #[error("one-shot manifest verification failed: {0}")]
    Verify(String),
    #[error("one-shot checkpoint reservation failed: {0}")]
    Reserve(String),
    #[error("one-shot checkpoint write failed: {0}")]
    Checkpoint(String),
}

static ONESHOT_TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// ETL-owned Load transaction for one-shot final session publication. Publishes
/// the canonical session (no-replace), verifies the published manifest, then
/// advances the recording-local checkpoint in that order. The checkpoint payload
/// schema and quota reservation stay application-owned; this API owns the
/// concrete storage publication, verification, and checkpoint advancement order
/// so application never calls the concrete session publisher or advances ETL
/// checkpoints directly.
pub fn publish_final_session(
    store: &FilesystemSessionStore,
    expected: &PublishSession,
    checkpoint_bytes: impl FnOnce(
        &OneShotPublicationOutcome,
    ) -> Result<Vec<u8>, OneShotPublicationError>,
    checkpoint_path: &Path,
    checkpoint_reserve: impl FnOnce(u64) -> Result<(), OneShotPublicationError>,
    matches_existing: impl Fn(
        &FilesystemSessionStore,
        &PublishSession,
    ) -> Result<bool, OneShotPublicationError>,
) -> Result<OneShotPublicationOutcome, OneShotPublicationError> {
    let session_id = expected.session.id;
    let already_published = match store.publish(expected.clone()) {
        Ok(()) => false,
        Err(error) => match matches_existing(store, expected) {
            Ok(true) => true,
            Ok(false) => {
                return Err(OneShotPublicationError::Mismatch { session_id });
            }
            Err(_) => return Err(OneShotPublicationError::Store(error.to_string())),
        },
    };
    let inspection = store
        .verify_existing_manifest(session_id)
        .map_err(|error| OneShotPublicationError::Verify(error.to_string()))?;
    let outcome = OneShotPublicationOutcome {
        session_id,
        already_published,
        manifest_checksum: inspection.session_checksum,
    };
    let bytes = checkpoint_bytes(&outcome)?;
    checkpoint_reserve(bytes.len() as u64)?;
    write_atomic_bytes(checkpoint_path, &bytes)
        .map_err(|error| OneShotPublicationError::Checkpoint(error.to_string()))?;
    Ok(outcome)
}

fn write_atomic_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "checkpoint path has no parent",
        )
    })?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    let mut permissions = fs::metadata(parent)?.permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o700);
    fs::set_permissions(parent, permissions)?;
    let temporary = parent.join(format!(
        ".oneshot.tmp-{}",
        ONESHOT_TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = (|| {
        use std::io::Write;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        // Sync the parent directory so the rename is durable, matching the
        // application-side atomic writer it replaces.
        #[cfg(unix)]
        if let Ok(directory) = fs::File::open(parent) {
            directory.sync_all()?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
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

pub fn publish_delta_then_checkpoint_with_fault<S: RecordingStore>(
    store: &S,
    key: impl Into<String>,
    batch: &CanonicalDeltaBatchV1,
    checkpoint: &IncrementalEtlCheckpointV1,
    checkpoint_path: impl AsRef<Path>,
    fault: Option<CheckpointFault>,
) -> Result<PutIfAbsent, PublicationError> {
    batch.validate()?;
    checkpoint.validate()?;
    let checkpoint_path = checkpoint_path.as_ref();
    let pending_path = pending_checkpoint_path(checkpoint_path);
    write_checkpoint_atomic(&pending_path, checkpoint)?;
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
    wait_for_checkpoint_commit()?;
    write_checkpoint_atomic(checkpoint_path, checkpoint)?;
    if fault == Some(CheckpointFault::AfterCheckpoint) {
        return Err(PublicationError::InjectedFault);
    }
    remove_pending_checkpoint(&pending_path)?;
    Ok(outcome)
}

/// V2 equivalent of [`publish_delta_then_checkpoint`]. The immutable output
/// is verified before the parent-aware checkpoint advances, so a crash leaves
/// either a retryable pending suffix or one adopted matching artifact.
pub fn publish_delta_then_checkpoint_v2<S: RecordingStore>(
    store: &S,
    key: impl Into<String>,
    batch: &CanonicalDeltaBatchV1,
    checkpoint: &IncrementalEtlCheckpointV2,
    checkpoint_path: impl AsRef<Path>,
) -> Result<PutIfAbsent, PublicationError> {
    batch.validate()?;
    checkpoint.validate()?;
    let checkpoint_path = checkpoint_path.as_ref();
    let pending_path = pending_checkpoint_path(checkpoint_path);
    checkpoint.write_atomic(&pending_path)?;
    let key = key.into();
    let artifact = RecordingArtifact::new(
        key.clone(),
        RecordingArtifactKind::CanonicalDeltaBatch,
        batch.encode()?,
    )?;
    let checksum = artifact.checksum.clone();
    let head = artifact.head();
    if !checkpoint
        .outputs
        .iter()
        .any(|output| output.key == key && output.digest == checksum)
    {
        return Err(PublicationError::OutputLineage);
    }
    let outcome = store.put_if_absent(artifact)?;
    let stored = store.head(&key)?;
    if stored.version != head.version
        || stored.kind != head.kind
        || stored.bytes != head.bytes
        || stored.checksum != head.checksum
    {
        return Err(PublicationError::Verification);
    }
    wait_for_checkpoint_commit()?;
    checkpoint.write_atomic(checkpoint_path)?;
    remove_pending_v2(&pending_path)?;
    Ok(outcome)
}

/// Publish one immutable continuation-out artifact before advancing a v2
/// checkpoint. Matching retries adopt the existing digest; conflicts fail.
pub fn publish_continuation_then_checkpoint_v2<S: RecordingStore>(
    store: &S,
    key: impl Into<String>,
    continuation: &crate::EpochContinuationCheckpointV1,
    checkpoint: &IncrementalEtlCheckpointV2,
    checkpoint_path: impl AsRef<Path>,
) -> Result<PutIfAbsent, PublicationError> {
    continuation.validate()?;
    checkpoint.validate()?;
    if checkpoint.continuation.state != crate::ContinuationState::Ready
        || checkpoint.continuation.predecessor_epoch_id != Some(continuation.predecessor_epoch_id)
        || checkpoint.continuation.successor_epoch_id != continuation.successor_epoch_id
        || checkpoint.parent_id != continuation.parent_id
        || checkpoint.pipeline_version != continuation.pipeline_version
    {
        return Err(PublicationError::OutputLineage);
    }
    let bytes = continuation.encode()?;
    let artifact =
        RecordingArtifact::new(key, RecordingArtifactKind::ContinuationCheckpoint, bytes)?
            .with_lineage(
                checkpoint.parent_id,
                checkpoint.epoch_id,
                checkpoint.epoch_ordinal,
            )?;
    if checkpoint.continuation.output_digest.as_deref() != Some(artifact.checksum.as_str()) {
        return Err(PublicationError::OutputLineage);
    }
    let checkpoint_path = checkpoint_path.as_ref();
    let pending_path = pending_checkpoint_path(checkpoint_path);
    checkpoint.write_atomic(&pending_path)?;
    let key = artifact.key.clone();
    let expected = artifact.head();
    let outcome = store.put_if_absent(artifact)?;
    let stored = store.head(&key)?;
    if stored.version != expected.version
        || stored.kind != expected.kind
        || stored.parent_id != expected.parent_id
        || stored.epoch_id != expected.epoch_id
        || stored.epoch_ordinal != expected.epoch_ordinal
        || stored.bytes != expected.bytes
        || stored.checksum != expected.checksum
    {
        return Err(PublicationError::Verification);
    }
    checkpoint.write_atomic(checkpoint_path)?;
    remove_pending_v2(&pending_path)?;
    Ok(outcome)
}

fn remove_pending_v2(path: &Path) -> Result<(), PublicationError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(PublicationError::Continuation(ContinuationError::Io)),
    }
}

fn wait_for_checkpoint_commit() -> Result<(), PublicationError> {
    let Ok(control_path) = std::env::var("CHRONICLE_CHECKPOINT_PAUSE_FILE") else {
        return Ok(());
    };
    let control_path = PathBuf::from(control_path);
    if !control_path.exists() {
        return Ok(());
    }
    let ready_path = control_path.with_extension("ready");
    fs::write(&ready_path, std::process::id().to_string())
        .map_err(|_| PublicationError::Checkpoint(CheckpointError::Io))?;
    while control_path.exists() {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let _ = fs::remove_file(ready_path);
    Ok(())
}

fn pending_checkpoint_path(checkpoint_path: &Path) -> PathBuf {
    let name = checkpoint_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("checkpoint.json");
    checkpoint_path.with_file_name(format!(".{name}.pending"))
}

fn remove_pending_checkpoint(path: &Path) -> Result<(), CheckpointError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(CheckpointError::Io),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CheckpointLifecycle, CheckpointOwner, ContinuationDependencyV2, ContinuationState,
        DecoderReconstructionState, DeltaBatchReference, INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION,
        MarkerLineage, SegmentLineage, SourceStatus,
    };
    use chronicle_common::{EpochId, RecordingId};
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
            pipeline_version: "1".into(),
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
        assert!(
            recover_pending_checkpoint(&store, &checkpoint_path)
                .unwrap()
                .is_some()
        );
        assert!(checkpoint_path.exists());
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
    fn pending_partial_suffix_discards_staging_and_converges() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key_a = "delta/partial-a";
        let key_b = "delta/partial-b";
        let artifact_a = RecordingArtifact::new(
            key_a,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact_a).unwrap();
        let root =
            std::env::temp_dir().join(format!("chronicle-pub-partial-{}", std::process::id()));
        let checkpoint_path = root.join("checkpoint.json");
        std::fs::create_dir_all(&root).unwrap();
        let existing = checkpoint_with_output(key_a, &batch);
        write_checkpoint_atomic(&checkpoint_path, &existing).unwrap();
        let existing = read_checkpoint(&checkpoint_path).unwrap();
        let mut candidate = checkpoint_with_output(key_b, &batch);
        let suffix = candidate.outputs.clone();
        let mut outputs = existing.outputs.clone();
        outputs.extend(suffix);
        candidate.outputs = outputs;
        candidate.predecessor_digest = Some(existing.checksum.clone());
        let pending = pending_checkpoint_path(&checkpoint_path);
        write_checkpoint_atomic(&pending, &candidate).unwrap();
        // Crash after pending-checkpoint write but before the new delta is
        // durable: only private staging is discarded and the retry converges.
        assert_eq!(
            recover_pending_checkpoint(&store, &checkpoint_path).unwrap(),
            None
        );
        assert!(!pending.exists());
        assert_eq!(
            read_checkpoint(&checkpoint_path).unwrap().outputs,
            existing.outputs
        );
        // Once the missing suffix delta is durable, the retry adopts it.
        let artifact_b = RecordingArtifact::new(
            key_b,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact_b).unwrap();
        write_checkpoint_atomic(&pending, &candidate).unwrap();
        assert_eq!(
            recover_pending_checkpoint(&store, &checkpoint_path).unwrap(),
            Some(ReconcileOutcome::AdoptedOutput)
        );
        assert!(!pending.exists());
        assert_eq!(
            read_checkpoint(&checkpoint_path).unwrap().outputs,
            candidate.outputs
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_interior_missing_output_fails_closed() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key_a = "delta/interior-a";
        let key_b = "delta/interior-b";
        // Only the suffix output is durable; an existing-prefix output vanished.
        let artifact_b = RecordingArtifact::new(
            key_b,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact_b).unwrap();
        let root =
            std::env::temp_dir().join(format!("chronicle-pub-interior-{}", std::process::id()));
        let checkpoint_path = root.join("checkpoint.json");
        std::fs::create_dir_all(&root).unwrap();
        let existing = checkpoint_with_output(key_a, &batch);
        write_checkpoint_atomic(&checkpoint_path, &existing).unwrap();
        let existing = read_checkpoint(&checkpoint_path).unwrap();
        let mut candidate = checkpoint_with_output(key_b, &batch);
        let suffix = candidate.outputs.clone();
        let mut outputs = existing.outputs.clone();
        outputs.extend(suffix);
        candidate.outputs = outputs;
        candidate.predecessor_digest = Some(existing.checksum.clone());
        write_checkpoint_atomic(pending_checkpoint_path(&checkpoint_path), &candidate).unwrap();
        assert!(matches!(
            recover_pending_checkpoint(&store, &checkpoint_path),
            Err(PublicationError::OutputLineage)
        ));
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

    #[test]
    fn v2_pending_checkpoint_adopts_durable_output() {
        let store = InMemoryRecordingStore::default();
        let batch = CanonicalDeltaBatchV1::new(0, 0, Vec::new()).unwrap();
        let key = "delta/v2-recovery";
        let artifact = RecordingArtifact::new(
            key,
            RecordingArtifactKind::CanonicalDeltaBatch,
            batch.encode().unwrap(),
        )
        .unwrap();
        store.put_if_absent(artifact).unwrap();

        let base = checkpoint_with_output(key, &batch);
        let epoch_id = EpochId::new();
        let mut decoder = base.decoder.clone();
        decoder.recording_id = epoch_id.as_uuid();
        let candidate = IncrementalEtlCheckpointV2 {
            version: INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION,
            owner: CheckpointOwner::Etl,
            lifecycle: CheckpointLifecycle::Active,
            parent_id: RecordingId::from_uuid(base.recording_id),
            epoch_id,
            epoch_ordinal: 0,
            config_digest: base.config_digest,
            pipeline_version: base.pipeline_version,
            canonical_schema_version: base.canonical_schema_version,
            marker: base.marker,
            segment_lineage: base.segment_lineage,
            continuation: ContinuationDependencyV2 {
                parent_id: Some(RecordingId::from_uuid(base.recording_id)),
                state: ContinuationState::Unavailable,
                predecessor_epoch_id: None,
                successor_epoch_id: epoch_id,
                input_digest: None,
                output_digest: None,
                failure_code: Some("no_predecessor".into()),
            },
            decoder,
            outputs: base.outputs,
            published_operation_keys: Vec::new(),
            source_status: SourceStatus::Live,
            checksum: String::new(),
        }
        .with_checksum()
        .unwrap();
        let root =
            std::env::temp_dir().join(format!("chronicle-v2-recovery-{}", std::process::id()));
        let checkpoint_path = root.join("incremental-etl-checkpoint-v2.json");
        let pending_path = pending_checkpoint_path(&checkpoint_path);
        candidate.write_atomic(&pending_path).unwrap();
        assert_eq!(
            recover_pending_checkpoint_v2(&store, &checkpoint_path).unwrap(),
            Some(ReconcileOutcome::RepairedCheckpoint)
        );
        assert!(!pending_path.exists());
        assert_eq!(
            IncrementalEtlCheckpointV2::load(&checkpoint_path).unwrap(),
            candidate
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
