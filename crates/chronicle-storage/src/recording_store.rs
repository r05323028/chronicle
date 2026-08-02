//! Immutable recorder artifact store contract.
#![allow(clippy::format_collect, clippy::needless_pass_by_value)]
//!
//! This store owns sealed copies, canonical delta batches, recovery reports,
//! and provenance indexes only. WAL commit/repair, ETL checkpoints, and final
//! session publication remain owned by their existing components.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

pub const RECORDING_STORE_SCHEMA_VERSION: u16 = 1;
const MAX_KEY_BYTES: usize = 512;
const MAX_ARTIFACT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingArtifactKind {
    SealedWalCopy,
    CanonicalDeltaBatch,
    RecoveryReport,
    ProvenanceIndex,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactLifecycle {
    Published,
    Verified,
    RetentionEligible,
    Deleting,
    Deleted,
    Quarantined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingArtifact {
    pub version: u16,
    pub key: String,
    pub kind: RecordingArtifactKind,
    pub checksum: String,
    pub lifecycle: ArtifactLifecycle,
    pub bytes: Vec<u8>,
}

impl RecordingArtifact {
    pub fn new(
        key: impl Into<String>,
        kind: RecordingArtifactKind,
        bytes: Vec<u8>,
    ) -> Result<Self, RecordingStoreError> {
        let key = key.into();
        validate_key(&key)?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(RecordingStoreError::ArtifactTooLarge);
        }
        let checksum = digest(&bytes);
        Ok(Self {
            version: RECORDING_STORE_SCHEMA_VERSION,
            key,
            kind,
            checksum,
            lifecycle: ArtifactLifecycle::Published,
            bytes,
        })
    }

    pub fn validate(&self) -> Result<(), RecordingStoreError> {
        if self.version != RECORDING_STORE_SCHEMA_VERSION || self.bytes.len() > MAX_ARTIFACT_BYTES {
            return Err(RecordingStoreError::InvalidArtifact);
        }
        validate_key(&self.key)?;
        if self.checksum != digest(&self.bytes) {
            return Err(RecordingStoreError::ChecksumMismatch);
        }
        if matches!(self.lifecycle, ArtifactLifecycle::Deleted) {
            return Err(RecordingStoreError::InvalidArtifact);
        }
        Ok(())
    }

    pub fn transition(&mut self, next: ArtifactLifecycle) -> Result<(), RecordingStoreError> {
        let valid = matches!(
            (self.lifecycle, next),
            (ArtifactLifecycle::Published, ArtifactLifecycle::Verified)
                | (
                    ArtifactLifecycle::Verified,
                    ArtifactLifecycle::RetentionEligible
                )
                | (
                    ArtifactLifecycle::RetentionEligible,
                    ArtifactLifecycle::Deleting
                )
                | (ArtifactLifecycle::Deleting, ArtifactLifecycle::Deleted)
                | (_, ArtifactLifecycle::Quarantined)
        );
        if !valid {
            return Err(RecordingStoreError::InvalidLifecycle);
        }
        self.lifecycle = next;
        Ok(())
    }

    pub fn head(&self) -> RecordingArtifactHead {
        RecordingArtifactHead {
            version: self.version,
            key: self.key.clone(),
            kind: self.kind,
            checksum: self.checksum.clone(),
            lifecycle: self.lifecycle,
            bytes: self.bytes.len() as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingArtifactHead {
    pub version: u16,
    pub key: String,
    pub kind: RecordingArtifactKind,
    pub checksum: String,
    pub lifecycle: ArtifactLifecycle,
    pub bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PutIfAbsent {
    Created,
    AlreadyExistsMatching,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeleteIfDigest {
    Deleted,
    NotFound,
    DigestMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingTombstone {
    pub version: u16,
    pub key: String,
    pub checksum: String,
    pub lifecycle: ArtifactLifecycle,
}

pub trait RecordingStore: Send + Sync {
    fn put_if_absent(
        &self,
        artifact: RecordingArtifact,
    ) -> Result<PutIfAbsent, RecordingStoreError>;
    fn head(&self, key: &str) -> Result<RecordingArtifactHead, RecordingStoreError>;
    fn get(&self, key: &str) -> Result<RecordingArtifact, RecordingStoreError>;
    fn list(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<RecordingArtifactHead>, RecordingStoreError>;
    fn delete_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError>;
    fn tombstone_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError>;
}

#[derive(Default)]
pub struct InMemoryRecordingStore {
    artifacts: Mutex<BTreeMap<String, RecordingArtifact>>,
    tombstones: Mutex<BTreeMap<String, RecordingTombstone>>,
}

impl RecordingStore for InMemoryRecordingStore {
    fn put_if_absent(
        &self,
        artifact: RecordingArtifact,
    ) -> Result<PutIfAbsent, RecordingStoreError> {
        artifact.validate()?;
        let mut artifacts = self
            .artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?;
        if self
            .tombstones
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?
            .contains_key(&artifact.key)
        {
            return Err(RecordingStoreError::Conflict);
        }
        match artifacts.get(&artifact.key) {
            Some(existing) if existing.checksum == artifact.checksum => {
                Ok(PutIfAbsent::AlreadyExistsMatching)
            }
            Some(_) => Err(RecordingStoreError::Conflict),
            None => {
                artifacts.insert(artifact.key.clone(), artifact);
                Ok(PutIfAbsent::Created)
            }
        }
    }

    fn head(&self, key: &str) -> Result<RecordingArtifactHead, RecordingStoreError> {
        validate_key(key)?;
        self.artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?
            .get(key)
            .map(RecordingArtifact::head)
            .ok_or(RecordingStoreError::NotFound)
    }

    fn get(&self, key: &str) -> Result<RecordingArtifact, RecordingStoreError> {
        validate_key(key)?;
        self.artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?
            .get(key)
            .cloned()
            .ok_or(RecordingStoreError::NotFound)
    }

    fn list(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<RecordingArtifactHead>, RecordingStoreError> {
        validate_prefix(prefix)?;
        Ok(self
            .artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?
            .iter()
            .filter(|(key, _)| key.starts_with(prefix))
            .take(limit.min(256))
            .map(|(_, artifact)| artifact.head())
            .collect())
    }

    fn delete_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError> {
        validate_key(key)?;
        let mut artifacts = self
            .artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?;
        let Some(artifact) = artifacts.get(key) else {
            return Ok(DeleteIfDigest::NotFound);
        };
        if artifact.checksum != expected_digest {
            return Ok(DeleteIfDigest::DigestMismatch);
        }
        artifacts.remove(key);
        Ok(DeleteIfDigest::Deleted)
    }

    fn tombstone_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError> {
        validate_key(key)?;
        let mut artifacts = self
            .artifacts
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?;
        let Some(artifact) = artifacts.get(key) else {
            return Ok(DeleteIfDigest::NotFound);
        };
        if artifact.checksum != expected_digest {
            return Ok(DeleteIfDigest::DigestMismatch);
        }
        let tombstone = RecordingTombstone {
            version: RECORDING_STORE_SCHEMA_VERSION,
            key: key.into(),
            checksum: expected_digest.into(),
            lifecycle: ArtifactLifecycle::Deleted,
        };
        artifacts.remove(key);
        self.tombstones
            .lock()
            .map_err(|_| RecordingStoreError::LockPoisoned)?
            .insert(key.into(), tombstone);
        Ok(DeleteIfDigest::Deleted)
    }
}

pub struct FilesystemRecordingStore {
    root: PathBuf,
    #[cfg(test)]
    fault: Mutex<Option<FilesystemStoreFault>>,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilesystemStoreFault {
    BeforeTempOpen,
    AfterTempWrite,
    BeforePublish,
    BeforePublishDirectorySync,
    BeforeDelete,
    BeforeDeleteDirectorySync,
    BeforeTombstoneOpen,
    AfterTombstoneSync,
    BeforeTombstoneDirectorySync,
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

impl FilesystemRecordingStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(test)]
            fault: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn inject_fault(&self, fault: FilesystemStoreFault) {
        let Ok(mut injected) = self.fault.lock() else {
            panic!("filesystem fault mutex poisoned");
        };
        *injected = Some(fault);
    }

    #[cfg(test)]
    fn fail_if_injected(&self, fault: FilesystemStoreFault) -> Result<(), RecordingStoreError> {
        let Ok(mut injected) = self.fault.lock() else {
            return Err(RecordingStoreError::LockPoisoned);
        };
        if *injected == Some(fault) {
            *injected = None;
            return Err(RecordingStoreError::Backend(format!(
                "injected ENOSPC at {fault:?}"
            )));
        }
        Ok(())
    }

    pub fn recover_staging(&self) -> Result<usize, RecordingStoreError> {
        let artifacts = self.artifacts_root();
        if !artifacts.exists() {
            return Ok(0);
        }
        let mut removed = 0;
        for path in walk_files(&artifacts)? {
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".tmp-"))
            {
                fs::remove_file(path).map_err(io_error)?;
                removed += 1;
            }
        }
        Ok(removed)
    }

    fn artifacts_root(&self) -> PathBuf {
        self.root.join("artifacts")
    }

    fn path_for(&self, key: &str) -> Result<PathBuf, RecordingStoreError> {
        validate_key(key)?;
        if self.root.exists() {
            ensure_no_symlink(&self.root)?;
        }
        let mut path = self.artifacts_root();
        for component in key.split('/') {
            path.push(component);
            if path.exists() {
                ensure_no_symlink(&path)?;
            }
        }
        Ok(path)
    }

    fn tombstone_path(&self, key: &str) -> Result<PathBuf, RecordingStoreError> {
        let path = self.path_for(key)?;
        Ok(path.with_extension(
            format!(
                "{} tombstone",
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or("artifact")
            )
            .replace(' ', "-"),
        ))
    }

    fn ensure_parent(&self, path: &Path) -> Result<(), RecordingStoreError> {
        let parent = path.parent().ok_or(RecordingStoreError::UnsafeKey)?;
        fs::create_dir_all(parent).map_err(io_error)?;
        ensure_no_symlink(&self.root)?;
        for ancestor in parent.ancestors() {
            if (ancestor == self.root || ancestor.starts_with(&self.root)) && ancestor.exists() {
                ensure_no_symlink(ancestor)?;
            }
            if ancestor == self.root {
                break;
            }
        }
        Ok(())
    }
}

impl RecordingStore for FilesystemRecordingStore {
    fn put_if_absent(
        &self,
        artifact: RecordingArtifact,
    ) -> Result<PutIfAbsent, RecordingStoreError> {
        artifact.validate()?;
        let destination = self.path_for(&artifact.key)?;
        let tombstone = self.tombstone_path(&artifact.key)?;
        if tombstone.exists() {
            return Err(RecordingStoreError::Conflict);
        }
        self.ensure_parent(&destination)?;
        let parent = destination.parent().ok_or(RecordingStoreError::UnsafeKey)?;
        let temporary = parent.join(format!(
            ".{}.tmp-{}",
            destination
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("artifact"),
            STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let bytes = serde_json::to_vec(&artifact)
            .map_err(|_| RecordingStoreError::Backend("artifact serialization failed".into()))?;
        {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            #[cfg(test)]
            self.fail_if_injected(FilesystemStoreFault::BeforeTempOpen)?;
            let mut file = options.open(&temporary).map_err(io_error)?;
            file.write_all(&bytes).map_err(io_error)?;
            #[cfg(test)]
            self.fail_if_injected(FilesystemStoreFault::AfterTempWrite)?;
            file.sync_all().map_err(io_error)?;
        }
        #[cfg(test)]
        self.fail_if_injected(FilesystemStoreFault::BeforePublish)?;
        let result = match fs::hard_link(&temporary, &destination) {
            Ok(()) => {
                #[cfg(test)]
                self.fail_if_injected(FilesystemStoreFault::BeforePublishDirectorySync)?;
                let result = sync_directory(parent).map(|()| PutIfAbsent::Created);
                let _ = fs::remove_file(&temporary);
                result
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temporary);
                let existing = self.get(&artifact.key)?;
                if existing.checksum == artifact.checksum {
                    Ok(PutIfAbsent::AlreadyExistsMatching)
                } else {
                    Err(RecordingStoreError::Conflict)
                }
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                Err(io_error(error))
            }
        }?;
        Ok(result)
    }

    fn head(&self, key: &str) -> Result<RecordingArtifactHead, RecordingStoreError> {
        Ok(self.get(key)?.head())
    }

    fn get(&self, key: &str) -> Result<RecordingArtifact, RecordingStoreError> {
        let path = self.path_for(key)?;
        let bytes = fs::read(path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RecordingStoreError::NotFound
            } else {
                io_error(error)
            }
        })?;
        let artifact: RecordingArtifact =
            serde_json::from_slice(&bytes).map_err(|_| RecordingStoreError::InvalidArtifact)?;
        artifact.validate()?;
        if artifact.key != key {
            return Err(RecordingStoreError::InvalidArtifact);
        }
        Ok(artifact)
    }

    fn list(
        &self,
        prefix: &str,
        limit: usize,
    ) -> Result<Vec<RecordingArtifactHead>, RecordingStoreError> {
        validate_prefix(prefix)?;
        let mut result = Vec::new();
        for path in walk_files(&self.artifacts_root())? {
            if result.len() >= limit.min(256)
                || path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.contains(".tmp-"))
            {
                continue;
            }
            let Ok(relative) = path.strip_prefix(self.artifacts_root()) else {
                continue;
            };
            let key = relative
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if key.starts_with(prefix) {
                result.push(self.head(&key)?);
            }
        }
        result.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(result)
    }

    fn delete_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError> {
        let path = self.path_for(key)?;
        let Ok(existing) = self.get(key) else {
            return Ok(DeleteIfDigest::NotFound);
        };
        if existing.checksum != expected_digest {
            return Ok(DeleteIfDigest::DigestMismatch);
        }
        #[cfg(test)]
        self.fail_if_injected(FilesystemStoreFault::BeforeDelete)?;
        fs::remove_file(path).map_err(io_error)?;
        if let Some(parent) = self.path_for(key)?.parent() {
            #[cfg(test)]
            self.fail_if_injected(FilesystemStoreFault::BeforeDeleteDirectorySync)?;
            sync_directory(parent)?;
        }
        Ok(DeleteIfDigest::Deleted)
    }

    fn tombstone_if_digest(
        &self,
        key: &str,
        expected_digest: &str,
    ) -> Result<DeleteIfDigest, RecordingStoreError> {
        let path = self.path_for(key)?;
        let Ok(existing) = self.get(key) else {
            return Ok(DeleteIfDigest::NotFound);
        };
        if existing.checksum != expected_digest {
            return Ok(DeleteIfDigest::DigestMismatch);
        }
        let tombstone = self.tombstone_path(key)?;
        self.ensure_parent(&tombstone)?;
        let bytes = serde_json::to_vec(&RecordingTombstone {
            version: RECORDING_STORE_SCHEMA_VERSION,
            key: key.into(),
            checksum: expected_digest.into(),
            lifecycle: ArtifactLifecycle::Deleted,
        })
        .map_err(|_| RecordingStoreError::Backend("tombstone serialization failed".into()))?;
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(test)]
        self.fail_if_injected(FilesystemStoreFault::BeforeTombstoneOpen)?;
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&tombstone).map_err(io_error)?;
        file.write_all(&bytes).map_err(io_error)?;
        file.sync_all().map_err(io_error)?;
        #[cfg(test)]
        self.fail_if_injected(FilesystemStoreFault::AfterTombstoneSync)?;
        fs::remove_file(path).map_err(io_error)?;
        #[cfg(test)]
        self.fail_if_injected(FilesystemStoreFault::BeforeTombstoneDirectorySync)?;
        sync_directory(tombstone.parent().ok_or(RecordingStoreError::UnsafeKey)?)?;
        Ok(DeleteIfDigest::Deleted)
    }
}

fn walk_files(root: &Path) -> Result<Vec<PathBuf>, RecordingStoreError> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    ensure_no_symlink(root)?;
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let path = entry.path();
            ensure_no_symlink(&path)?;
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    Ok(files)
}

fn ensure_no_symlink(path: &Path) -> Result<(), RecordingStoreError> {
    if fs::symlink_metadata(path)
        .map_err(io_error)?
        .file_type()
        .is_symlink()
    {
        return Err(RecordingStoreError::UnsafeKey);
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), RecordingStoreError> {
    File::open(path)
        .map_err(io_error)?
        .sync_all()
        .map_err(io_error)
}

fn io_error(error: std::io::Error) -> RecordingStoreError {
    RecordingStoreError::Backend(error.to_string())
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RecordingStoreError {
    #[error("recording artifact key is unsafe")]
    UnsafeKey,
    #[error("recording artifact is invalid")]
    InvalidArtifact,
    #[error("recording artifact is too large")]
    ArtifactTooLarge,
    #[error("recording artifact checksum mismatch")]
    ChecksumMismatch,
    #[error("recording artifact lifecycle transition is invalid")]
    InvalidLifecycle,
    #[error("recording artifact was not found")]
    NotFound,
    #[error("recording artifact conflicts with existing immutable object")]
    Conflict,
    #[error("recording store lock was poisoned")]
    LockPoisoned,
    #[error("recording store backend failed: {0}")]
    Backend(String),
}

fn validate_prefix(prefix: &str) -> Result<(), RecordingStoreError> {
    if prefix.len() > MAX_KEY_BYTES || prefix.starts_with('/') || prefix.contains("..") {
        return Err(RecordingStoreError::UnsafeKey);
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), RecordingStoreError> {
    validate_prefix(key)?;
    if key.is_empty() || key.ends_with('/') || key.split('/').any(str::is_empty) {
        return Err(RecordingStoreError::UnsafeKey);
    }
    Ok(())
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
    fn put_is_idempotent_but_conflict_is_rejected() {
        let store = InMemoryRecordingStore::default();
        let artifact = RecordingArtifact::new(
            "delta/0001",
            RecordingArtifactKind::CanonicalDeltaBatch,
            b"x".to_vec(),
        )
        .unwrap();
        assert_eq!(
            store.put_if_absent(artifact.clone()).unwrap(),
            PutIfAbsent::Created
        );
        assert_eq!(
            store.put_if_absent(artifact).unwrap(),
            PutIfAbsent::AlreadyExistsMatching
        );
        let conflict = RecordingArtifact::new(
            "delta/0001",
            RecordingArtifactKind::CanonicalDeltaBatch,
            b"y".to_vec(),
        )
        .unwrap();
        assert_eq!(
            store.put_if_absent(conflict),
            Err(RecordingStoreError::Conflict)
        );
    }

    #[test]
    fn lifecycle_transitions_are_monotonic_and_quarantine_is_terminal() {
        let mut artifact =
            RecordingArtifact::new("recovery/1", RecordingArtifactKind::RecoveryReport, vec![])
                .unwrap();
        assert_eq!(
            artifact.transition(ArtifactLifecycle::RetentionEligible),
            Err(RecordingStoreError::InvalidLifecycle)
        );
        artifact.transition(ArtifactLifecycle::Verified).unwrap();
        artifact
            .transition(ArtifactLifecycle::RetentionEligible)
            .unwrap();
        artifact.transition(ArtifactLifecycle::Deleting).unwrap();
        artifact.transition(ArtifactLifecycle::Deleted).unwrap();
        assert_eq!(
            artifact.validate(),
            Err(RecordingStoreError::InvalidArtifact)
        );
    }

    fn recording_store_conformance(store: &dyn RecordingStore) {
        let artifact = RecordingArtifact::new(
            "conformance/delta-0001",
            RecordingArtifactKind::CanonicalDeltaBatch,
            b"bytes".to_vec(),
        )
        .unwrap();
        let digest = artifact.checksum.clone();
        assert_eq!(
            store.put_if_absent(artifact.clone()).unwrap(),
            PutIfAbsent::Created
        );
        assert_eq!(
            store.put_if_absent(artifact.clone()).unwrap(),
            PutIfAbsent::AlreadyExistsMatching
        );
        assert_eq!(store.head(&artifact.key).unwrap().checksum, digest);
        assert_eq!(store.get(&artifact.key).unwrap(), artifact);
        assert_eq!(store.list("conformance/", 10).unwrap().len(), 1);
        assert_eq!(
            store.delete_if_digest(&artifact.key, "wrong").unwrap(),
            DeleteIfDigest::DigestMismatch
        );
        assert_eq!(
            store.tombstone_if_digest(&artifact.key, &digest).unwrap(),
            DeleteIfDigest::Deleted
        );
        assert_eq!(store.get(&artifact.key), Err(RecordingStoreError::NotFound));
        assert_eq!(
            store.put_if_absent(
                RecordingArtifact::new(
                    artifact.key,
                    RecordingArtifactKind::CanonicalDeltaBatch,
                    b"new".to_vec(),
                )
                .unwrap()
            ),
            Err(RecordingStoreError::Conflict)
        );
    }

    #[test]
    fn in_memory_store_matches_recording_store_contract() {
        recording_store_conformance(&InMemoryRecordingStore::default());
    }

    #[test]
    fn filesystem_store_matches_recording_store_contract() {
        let root =
            std::env::temp_dir().join(format!("chronicle-recording-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = FilesystemRecordingStore::new(&root);
        recording_store_conformance(&store);
        assert_eq!(store.recover_staging().unwrap(), 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn filesystem_faults_cover_disk_full_persistence_boundaries() {
        let put_faults = [
            FilesystemStoreFault::BeforeTempOpen,
            FilesystemStoreFault::AfterTempWrite,
            FilesystemStoreFault::BeforePublish,
            FilesystemStoreFault::BeforePublishDirectorySync,
        ];
        for (index, fault) in put_faults.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "chronicle-recording-store-enospc-put-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let store = FilesystemRecordingStore::new(&root);
            let artifact = RecordingArtifact::new(
                "faults/put",
                RecordingArtifactKind::CanonicalDeltaBatch,
                b"bytes".to_vec(),
            )
            .unwrap();
            store.inject_fault(fault);
            let result = store.put_if_absent(artifact.clone());
            assert!(result.is_err(), "fault {fault:?} returned {result:?}");
            let error = result.unwrap_err();
            assert!(matches!(
                error,
                RecordingStoreError::Backend(message) if message.contains("ENOSPC")
            ));
            store.recover_staging().unwrap();
            assert!(matches!(
                store.put_if_absent(artifact),
                Ok(PutIfAbsent::Created | PutIfAbsent::AlreadyExistsMatching)
            ));
            let _ = std::fs::remove_dir_all(root);
        }

        let delete_faults = [
            FilesystemStoreFault::BeforeDelete,
            FilesystemStoreFault::BeforeDeleteDirectorySync,
        ];
        for (index, fault) in delete_faults.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "chronicle-recording-store-enospc-delete-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let store = FilesystemRecordingStore::new(&root);
            let artifact = RecordingArtifact::new(
                "faults/delete",
                RecordingArtifactKind::CanonicalDeltaBatch,
                b"bytes".to_vec(),
            )
            .unwrap();
            let digest = artifact.checksum.clone();
            store.put_if_absent(artifact).unwrap();
            store.inject_fault(fault);
            let error = store
                .delete_if_digest("faults/delete", &digest)
                .unwrap_err();
            assert!(matches!(
                error,
                RecordingStoreError::Backend(message) if message.contains("ENOSPC")
            ));
            let _ = std::fs::remove_dir_all(root);
        }

        let tombstone_faults = [
            FilesystemStoreFault::BeforeTombstoneOpen,
            FilesystemStoreFault::AfterTombstoneSync,
            FilesystemStoreFault::BeforeTombstoneDirectorySync,
        ];
        for (index, fault) in tombstone_faults.into_iter().enumerate() {
            let root = std::env::temp_dir().join(format!(
                "chronicle-recording-store-enospc-tombstone-{}-{index}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let store = FilesystemRecordingStore::new(&root);
            let artifact = RecordingArtifact::new(
                "faults/tombstone",
                RecordingArtifactKind::CanonicalDeltaBatch,
                b"bytes".to_vec(),
            )
            .unwrap();
            let digest = artifact.checksum.clone();
            store.put_if_absent(artifact).unwrap();
            store.inject_fault(fault);
            let error = store
                .tombstone_if_digest("faults/tombstone", &digest)
                .unwrap_err();
            assert!(matches!(
                error,
                RecordingStoreError::Backend(message) if message.contains("ENOSPC")
            ));
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn keys_and_digest_delete_are_safe() {
        assert_eq!(
            RecordingArtifact::new("../escape", RecordingArtifactKind::RecoveryReport, vec![])
                .unwrap_err(),
            RecordingStoreError::UnsafeKey
        );
        let store = InMemoryRecordingStore::default();
        let artifact = RecordingArtifact::new(
            "recovery/1",
            RecordingArtifactKind::RecoveryReport,
            b"safe".to_vec(),
        )
        .unwrap();
        let digest = artifact.checksum.clone();
        store.put_if_absent(artifact).unwrap();
        assert_eq!(
            store.delete_if_digest("recovery/1", "bad").unwrap(),
            DeleteIfDigest::DigestMismatch
        );
        assert_eq!(
            store.delete_if_digest("recovery/1", &digest).unwrap(),
            DeleteIfDigest::Deleted
        );
        let tombstone = RecordingArtifact::new(
            "recovery/2",
            RecordingArtifactKind::RecoveryReport,
            b"safe".to_vec(),
        )
        .unwrap();
        let tombstone_digest = tombstone.checksum.clone();
        store.put_if_absent(tombstone).unwrap();
        assert_eq!(
            store
                .tombstone_if_digest("recovery/2", &tombstone_digest)
                .unwrap(),
            DeleteIfDigest::Deleted
        );
        assert_eq!(
            store.put_if_absent(
                RecordingArtifact::new(
                    "recovery/2",
                    RecordingArtifactKind::RecoveryReport,
                    b"new".to_vec()
                )
                .unwrap()
            ),
            Err(RecordingStoreError::Conflict)
        );
    }
}
