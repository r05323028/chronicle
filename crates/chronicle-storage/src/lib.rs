//! Metadata and artifact repository boundaries.
//!
//! `PostgreSQL` and S3-compatible clients are deferred; in-memory adapters exercise contracts.

use chronicle_canonical::{CanonicalSession, PayloadRef};
use chronicle_common::SessionId;
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{CWD, RenameFlags, renameat_with};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

pub const MANIFEST_SCHEMA_V1: u8 = 1;
pub const MANIFEST_SCHEMA_V2: u8 = 2;
pub const MANIFEST_SCHEMA_VERSION: u8 = MANIFEST_SCHEMA_V1;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PostgresMetadataConfig {
    pub connection_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct S3ArtifactConfig {
    pub endpoint: Option<String>,
    pub bucket: String,
    pub region: Option<String>,
    pub path_style: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactObject {
    pub key: String,
    pub checksum: String,
    pub bytes: Vec<u8>,
    pub content_type: Option<String>,
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("metadata session {0} was not found")]
    NotFound(SessionId),
    #[error("storage lock was poisoned")]
    LockPoisoned,
    #[error("storage backend failed: {0}")]
    Backend(String),
    #[error("storage data failed validation: {0}")]
    Validation(String),
}

pub trait MetadataRepository: Send + Sync {
    fn save_session<'a>(
        &'a self,
        session: &'a CanonicalSession,
    ) -> BoxFuture<'a, Result<(), StorageError>>;
    fn load_session(&self, id: SessionId) -> BoxFuture<'_, Result<CanonicalSession, StorageError>>;
}

pub trait ArtifactStore: Send + Sync {
    fn put(&self, object: ArtifactObject) -> BoxFuture<'_, Result<(), StorageError>>;
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ArtifactObject, StorageError>>;
}

#[derive(Default)]
pub struct InMemoryMetadataRepository {
    sessions: Mutex<BTreeMap<SessionId, CanonicalSession>>,
}

impl MetadataRepository for InMemoryMetadataRepository {
    fn save_session<'a>(
        &'a self,
        session: &'a CanonicalSession,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            self.sessions
                .lock()
                .map_err(|_| StorageError::LockPoisoned)?
                .insert(session.id, session.clone());
            Ok(())
        })
    }

    fn load_session(&self, id: SessionId) -> BoxFuture<'_, Result<CanonicalSession, StorageError>> {
        Box::pin(async move {
            self.sessions
                .lock()
                .map_err(|_| StorageError::LockPoisoned)?
                .get(&id)
                .cloned()
                .ok_or(StorageError::NotFound(id))
        })
    }
}

#[derive(Default)]
pub struct InMemoryArtifactStore {
    objects: Mutex<BTreeMap<String, ArtifactObject>>,
}

impl ArtifactStore for InMemoryArtifactStore {
    fn put(&self, object: ArtifactObject) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            self.objects
                .lock()
                .map_err(|_| StorageError::LockPoisoned)?
                .insert(object.key.clone(), object);
            Ok(())
        })
    }

    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ArtifactObject, StorageError>> {
        Box::pin(async move {
            self.objects
                .lock()
                .map_err(|_| StorageError::LockPoisoned)?
                .get(key)
                .cloned()
                .ok_or_else(|| StorageError::Backend(format!("artifact {key} was not found")))
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishSession {
    pub session: CanonicalSession,
    pub checkpoint: Option<String>,
    pub issues: Vec<String>,
    pub replayability: Vec<String>,
    pub complete: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredSessionInspection {
    pub session: CanonicalSession,
    pub session_checksum: String,
    pub canonical_schema_version: u16,
    pub payload_count: usize,
    pub payload_bytes: u64,
    pub checkpoint: Option<String>,
    pub issues: Vec<String>,
    pub replayability: Vec<String>,
    pub complete: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ManifestSchemaVersion {
    V1 = MANIFEST_SCHEMA_V1,
    V2 = MANIFEST_SCHEMA_V2,
}

impl TryFrom<u8> for ManifestSchemaVersion {
    type Error = u8;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            MANIFEST_SCHEMA_V1 => Ok(Self::V1),
            MANIFEST_SCHEMA_V2 => Ok(Self::V2),
            other => Err(other),
        }
    }
}

#[derive(serde::Deserialize)]
struct ManifestDiscriminator {
    version: u8,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ManifestV1 {
    version: u8,
    session_id: SessionId,
    canonical_schema_version: u16,
    session_file: String,
    session_checksum: String,
    payload_count: usize,
    payload_bytes: u64,
    checkpoint: Option<String>,
    issues: Vec<String>,
    replayability: Vec<String>,
    #[serde(default)]
    complete: bool,
}

fn decode_manifest(bytes: &[u8]) -> Result<ManifestV1, StorageError> {
    let discriminator: ManifestDiscriminator = serde_json::from_slice(bytes)
        .map_err(|error| StorageError::Validation(error.to_string()))?;
    match ManifestSchemaVersion::try_from(discriminator.version) {
        Ok(ManifestSchemaVersion::V1) => serde_json::from_slice(bytes)
            .map_err(|error| StorageError::Validation(error.to_string())),
        Ok(ManifestSchemaVersion::V2) | Err(_) => Err(StorageError::Validation(format!(
            "unsupported session manifest version {}",
            discriminator.version
        ))),
    }
}

pub struct FilesystemSessionStore {
    root: PathBuf,
    #[cfg(test)]
    fault: Mutex<Option<PublishFault>>,
}

#[cfg(test)]
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PublishFault {
    BeforePayloadDirectorySync,
    BeforeManifest,
    BeforeStagingSync,
    BeforeRename,
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

impl FilesystemSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            #[cfg(test)]
            fault: Mutex::new(None),
        }
    }

    #[cfg(test)]
    fn inject_failure(&self, fault: PublishFault) {
        *self.fault.lock().unwrap() = Some(fault);
    }

    #[cfg(test)]
    fn fail_if_injected(&self, fault: PublishFault) -> Result<(), StorageError> {
        let mut injected = self.fault.lock().unwrap();
        if *injected == Some(fault) {
            *injected = None;
            return Err(StorageError::Backend("injected publish failure".into()));
        }
        Ok(())
    }
    pub fn publish(&self, publish: PublishSession) -> Result<(), StorageError> {
        #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
        {
            let _ = publish;
            return Err(StorageError::Backend(
                "private filesystem ACL guarantees unavailable".into(),
            ));
        }
        #[cfg(any(target_os = "linux", target_vendor = "apple"))]
        self.publish_unix(publish)
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    fn publish_unix(&self, publish: PublishSession) -> Result<(), StorageError> {
        let id = publish.session.id;
        create_private_dir_all(&self.root)?;
        let sessions = self.root.join("sessions");
        create_private_dir_all(&sessions)?;
        let directory = sessions.join(id.to_string());
        if directory.exists() {
            return Err(StorageError::Backend("session already exists".into()));
        }
        let staging = sessions.join(format!(
            ".{}.tmp-{}",
            id,
            STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        create_private_dir(&staging)?;
        let result = self.write_staged(&staging, id, publish).and_then(|()| {
            #[cfg(test)]
            self.fail_if_injected(PublishFault::BeforeStagingSync)?;
            sync_dir(&staging)?;
            if directory.exists() {
                return Err(StorageError::Backend("session already exists".into()));
            }
            #[cfg(test)]
            self.fail_if_injected(PublishFault::BeforeRename)?;
            renameat_with(CWD, &staging, CWD, &directory, RenameFlags::NOREPLACE)
                .map_err(|error| StorageError::Backend(error.to_string()))?;
            sync_dir(&sessions)
        });
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[allow(clippy::unused_self)]
    fn write_staged(
        &self,
        directory: &Path,
        id: SessionId,
        publish: PublishSession,
    ) -> Result<(), StorageError> {
        create_private_dir(&directory.join("payloads"))?;
        let mut session = publish.session;
        let mut count = 0;
        let mut total = 0;
        let mut payloads = HashSet::new();
        for operation in session
            .connections
            .iter_mut()
            .flat_map(|c| c.operations.iter_mut())
        {
            Self::externalize(
                directory,
                id,
                &mut operation.request,
                &mut count,
                &mut total,
                &mut payloads,
            )?;
            if let Some(response) = &mut operation.recorded_response {
                Self::externalize(
                    directory,
                    id,
                    response,
                    &mut count,
                    &mut total,
                    &mut payloads,
                )?;
            }
        }
        #[cfg(test)]
        self.fail_if_injected(PublishFault::BeforePayloadDirectorySync)?;
        sync_dir(&directory.join("payloads"))?;
        let bytes =
            serde_json::to_vec(&session).map_err(|e| StorageError::Backend(e.to_string()))?;
        let checksum = digest(&bytes);
        write_private_new(&directory.join("session.json"), &bytes)?;
        let manifest = ManifestV1 {
            version: MANIFEST_SCHEMA_VERSION,
            session_id: id,
            canonical_schema_version: session.schema_version,
            session_file: "session.json".into(),
            session_checksum: checksum,
            payload_count: count,
            payload_bytes: total,
            checkpoint: publish.checkpoint,
            issues: publish.issues,
            replayability: publish.replayability,
            complete: publish.complete,
        };
        // Manifest is last: its presence marks a complete staged session.
        #[cfg(test)]
        self.fail_if_injected(PublishFault::BeforeManifest)?;
        write_private_new(
            &directory.join("manifest.json"),
            &serde_json::to_vec(&manifest).map_err(|e| StorageError::Backend(e.to_string()))?,
        )?;
        Ok(())
    }
    fn externalize(
        directory: &Path,
        id: SessionId,
        payload: &mut PayloadRef,
        count: &mut usize,
        total: &mut u64,
        payloads: &mut HashSet<String>,
    ) -> Result<(), StorageError> {
        let PayloadRef::Inline {
            content_type,
            bytes,
        } = payload
        else {
            return Ok(());
        };
        if bytes.is_empty() {
            return Ok(());
        }
        let checksum = digest(bytes);
        let name = checksum.strip_prefix("sha256:").unwrap();
        let size = bytes.len() as u64;
        if payloads.insert(checksum.clone()) {
            write_private_new(&directory.join("payloads").join(name), bytes)?;
            *count += 1;
            *total += size;
        }
        *payload = PayloadRef::Artifact {
            key: format!("sessions/{id}/payloads/{name}"),
            checksum,
            size,
            content_type: content_type.clone(),
        };
        Ok(())
    }
    fn session_dir(&self, id: SessionId) -> PathBuf {
        self.root.join("sessions").join(id.to_string())
    }

    /// Loads metadata and verifies Artifact file existence and declared sizes only.
    pub fn inspect(&self, id: SessionId) -> Result<CanonicalSession, StorageError> {
        let session = self.load(id)?;
        for operation in session
            .connections
            .iter()
            .flat_map(|connection| &connection.operations)
        {
            self.inspect_payload(&operation.request)?;
            if let Some(response) = &operation.recorded_response {
                self.inspect_payload(response)?;
            }
        }
        Ok(session)
    }

    /// Verifies an existing manifest and its canonical session checksum.
    pub fn verify_existing_manifest(
        &self,
        id: SessionId,
    ) -> Result<StoredSessionInspection, StorageError> {
        let (session, manifest) = self.load_with_manifest(id)?;
        Ok(StoredSessionInspection {
            session,
            session_checksum: manifest.session_checksum,
            canonical_schema_version: manifest.canonical_schema_version,
            payload_count: manifest.payload_count,
            payload_bytes: manifest.payload_bytes,
            checkpoint: manifest.checkpoint,
            issues: manifest.issues,
            replayability: manifest.replayability,
            complete: manifest.complete,
        })
    }

    /// Loads session data plus manifest processing metadata, without body reads.
    pub fn inspect_with_metadata(
        &self,
        id: SessionId,
    ) -> Result<StoredSessionInspection, StorageError> {
        let verified = self.verify_existing_manifest(id)?;
        for operation in verified
            .session
            .connections
            .iter()
            .flat_map(|connection| &connection.operations)
        {
            self.inspect_payload(&operation.request)?;
            if let Some(response) = &operation.recorded_response {
                self.inspect_payload(response)?;
            }
        }
        Ok(verified)
    }

    /// Loads a session with Artifact payloads verified and restored to Inline bytes.
    pub fn hydrate(&self, id: SessionId) -> Result<CanonicalSession, StorageError> {
        let mut session = self.load(id)?;
        for operation in session
            .connections
            .iter_mut()
            .flat_map(|connection| connection.operations.iter_mut())
        {
            self.hydrate_payload(&mut operation.request)?;
            if let Some(response) = &mut operation.recorded_response {
                self.hydrate_payload(response)?;
            }
        }
        Ok(session)
    }

    fn inspect_payload(&self, payload: &PayloadRef) -> Result<(), StorageError> {
        let PayloadRef::Artifact { key, size, .. } = payload else {
            return Ok(());
        };
        let metadata = std::fs::metadata(self.artifact_path(key)?).map_err(io)?;
        if metadata.len() != *size {
            return Err(StorageError::Validation("artifact size mismatch".into()));
        }
        Ok(())
    }

    fn hydrate_payload(&self, payload: &mut PayloadRef) -> Result<(), StorageError> {
        let PayloadRef::Artifact {
            key,
            checksum,
            size,
            content_type,
        } = payload
        else {
            return Ok(());
        };
        let bytes = std::fs::read(self.artifact_path(key)?).map_err(io)?;
        if bytes.len() as u64 != *size || digest(&bytes) != *checksum {
            return Err(StorageError::Validation(
                "artifact integrity mismatch".into(),
            ));
        }
        *payload = PayloadRef::Inline {
            content_type: content_type.clone(),
            bytes,
        };
        Ok(())
    }

    fn load(&self, id: SessionId) -> Result<CanonicalSession, StorageError> {
        self.load_with_manifest(id).map(|(session, _)| session)
    }

    fn load_with_manifest(
        &self,
        id: SessionId,
    ) -> Result<(CanonicalSession, ManifestV1), StorageError> {
        let directory = self.session_dir(id);
        let manifest = decode_manifest(
            &std::fs::read(directory.join("manifest.json"))
                .map_err(|_| StorageError::NotFound(id))?,
        )?;
        if manifest.version != MANIFEST_SCHEMA_VERSION
            || manifest.session_id != id
            || manifest.session_file != "session.json"
        {
            return Err(StorageError::Validation("invalid session manifest".into()));
        }
        let bytes = std::fs::read(directory.join(&manifest.session_file))
            .map_err(|_| StorageError::NotFound(id))?;
        if digest(&bytes) != manifest.session_checksum {
            return Err(StorageError::Validation("session checksum mismatch".into()));
        }
        let session: CanonicalSession = serde_json::from_slice(&bytes)
            .map_err(|error| StorageError::Validation(error.to_string()))?;
        if session.schema_version != manifest.canonical_schema_version {
            return Err(StorageError::Validation(
                "canonical schema version does not match manifest".into(),
            ));
        }
        Ok((session, manifest))
    }
    fn artifact_path(&self, key: &str) -> Result<PathBuf, StorageError> {
        let parts: Vec<_> = key.split('/').collect();
        if parts.len() != 4
            || parts[0] != "sessions"
            || parts[2] != "payloads"
            || parts
                .iter()
                .any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains('\\'))
        {
            return Err(StorageError::Validation("invalid artifact key".into()));
        }
        Ok(self
            .root
            .join(parts[0])
            .join(parts[1])
            .join(parts[2])
            .join(parts[3]))
    }
}
#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn create_private_dir(path: &Path) -> Result<(), StorageError> {
    use std::os::unix::fs::DirBuilderExt;
    let mut builder = std::fs::DirBuilder::new();
    builder.mode(0o700);
    builder.create(path).map_err(io)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
#[allow(clippy::items_after_statements)]
fn create_private_dir_all(path: &Path) -> Result<(), StorageError> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(io)?;
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn write_private_new(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn sync_dir(path: &Path) -> Result<(), StorageError> {
    File::open(path).map_err(io)?.sync_all().map_err(io)
}

#[allow(clippy::needless_pass_by_value)]
fn io(error: std::io::Error) -> StorageError {
    StorageError::Backend(error.to_string())
}
fn digest(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(71);
    output.push_str("sha256:");
    for byte in Sha256::digest(bytes) {
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}

impl MetadataRepository for FilesystemSessionStore {
    fn save_session<'a>(
        &'a self,
        session: &'a CanonicalSession,
    ) -> BoxFuture<'a, Result<(), StorageError>> {
        Box::pin(async move {
            self.publish(PublishSession {
                session: session.clone(),
                checkpoint: None,
                issues: Vec::new(),
                replayability: Vec::new(),
                complete: true,
            })
        })
    }
    fn load_session(&self, id: SessionId) -> BoxFuture<'_, Result<CanonicalSession, StorageError>> {
        Box::pin(async move { self.load(id) })
    }
}
impl ArtifactStore for FilesystemSessionStore {
    fn put(&self, object: ArtifactObject) -> BoxFuture<'_, Result<(), StorageError>> {
        Box::pin(async move {
            let path = self.artifact_path(&object.key)?;
            if digest(&object.bytes) != object.checksum {
                return Err(StorageError::Validation("checksum mismatch".into()));
            }
            std::fs::create_dir_all(path.parent().unwrap()).map_err(io)?;
            std::fs::write(path, object.bytes).map_err(io)
        })
    }
    fn get<'a>(&'a self, key: &'a str) -> BoxFuture<'a, Result<ArtifactObject, StorageError>> {
        Box::pin(async move {
            let bytes = std::fs::read(self.artifact_path(key)?)
                .map_err(|e| StorageError::Backend(e.to_string()))?;
            let checksum = digest(&bytes);
            let expected = key.rsplit('/').next().unwrap_or_default();
            if checksum.strip_prefix("sha256:") != Some(expected) {
                return Err(StorageError::Validation(
                    "artifact checksum mismatch".into(),
                ));
            }
            Ok(ArtifactObject {
                key: key.into(),
                checksum,
                bytes,
                content_type: None,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{
        Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalOperation,
        OperationEffect, OperationKind, ProtocolData, RelativeTimeNanos, ReplayMetadata,
        SourceMetadata,
    };
    use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId};
    use time::OffsetDateTime;

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!("chronicle-storage-{}", SessionId::new()))
    }

    fn session(body: Vec<u8>) -> CanonicalSession {
        CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            source: SourceMetadata::default(),
            connections: vec![CanonicalConnection {
                id: ConnectionId::new(),
                protocol: ProtocolId::new("custom/1"),
                client: Endpoint::new("127.0.0.1", 1),
                server: Endpoint::new("127.0.0.1", 2),
                attributes: Attributes::new(),
                incomplete: false,
                truncated: false,
                operations: vec![CanonicalOperation {
                    id: OperationId::new(),
                    sequence: 1,
                    started_at_offset: RelativeTimeNanos(0),
                    completed_at_offset: None,
                    kind: OperationKind::Request,
                    effect: OperationEffect::Unknown,
                    request: PayloadRef::Inline {
                        content_type: Some("application/custom".into()),
                        bytes: body,
                    },
                    recorded_response: None,
                    attributes: Attributes::new(),
                    protocol_data: ProtocolData {
                        schema_version: 1,
                        media_type: None,
                        bytes: Vec::new(),
                    },
                    incomplete: false,
                    truncated: false,
                    redactions: Vec::new(),
                    warnings: Vec::new(),
                }],
            }],
            timeline: Vec::new(),
            replay: ReplayMetadata::default(),
        }
    }

    #[tokio::test]
    async fn publish_load_and_direct_artifact_lookup_externalize_protocol_neutral_payload() {
        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let original = session(b"custom-body".to_vec());
        let id = original.id;
        store
            .publish(PublishSession {
                session: original,
                checkpoint: Some("checkpoint".into()),
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
        let loaded = store.load_session(id).await.unwrap();
        let PayloadRef::Artifact {
            key,
            size,
            content_type,
            ..
        } = &loaded.connections[0].operations[0].request
        else {
            panic!("inline payload was not externalized")
        };
        assert_eq!(*size, 11);
        assert_eq!(content_type.as_deref(), Some("application/custom"));
        assert_eq!(store.get(key).await.unwrap().bytes, b"custom-body");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn publish_reuses_duplicate_payload_digest() {
        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let mut original = session(b"same-body".to_vec());
        original.connections[0].operations[0].recorded_response = Some(PayloadRef::Inline {
            content_type: None,
            bytes: b"same-body".to_vec(),
        });
        let id = original.id;
        store
            .publish(PublishSession {
                session: original,
                checkpoint: None,
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
        let loaded = store.load_session(id).await.unwrap();
        let operation = &loaded.connections[0].operations[0];
        let PayloadRef::Artifact {
            key: request_key, ..
        } = &operation.request
        else {
            panic!("request was not externalized")
        };
        let Some(PayloadRef::Artifact {
            key: response_key, ..
        }) = &operation.recorded_response
        else {
            panic!("response was not externalized")
        };
        assert_eq!(request_key, response_key);
        assert_eq!(
            std::fs::read_dir(root.join("sessions").join(id.to_string()).join("payloads"))
                .unwrap()
                .count(),
            1
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn publish_makes_existing_root_and_sessions_private() {
        use std::os::unix::fs::PermissionsExt;

        let root = root();
        let sessions = root.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o755)).unwrap();
        let store = FilesystemSessionStore::new(&root);
        store
            .publish(PublishSession {
                session: session(b"payload".to_vec()),
                checkpoint: None,
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&sessions).unwrap().permissions().mode() & 0o777,
            0o700
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_uses_payload_metadata_while_hydration_verifies_checksum() {
        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let original = session(b"payload".to_vec());
        let id = original.id;
        store
            .publish(PublishSession {
                session: original,
                checkpoint: None,
                issues: Vec::new(),
                replayability: Vec::new(),
                complete: true,
            })
            .unwrap();
        let inspected = store.inspect(id).unwrap();
        let PayloadRef::Artifact { key, .. } = &inspected.connections[0].operations[0].request
        else {
            panic!("missing artifact")
        };
        std::fs::write(store.artifact_path(key).unwrap(), b"corrupt").unwrap();
        assert!(store.inspect(id).is_ok());
        assert!(matches!(
            store.hydrate(id),
            Err(StorageError::Validation(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn rejects_manifest_session_and_artifact_corruption_and_not_found() {
        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let original = session(b"payload".to_vec());
        let id = original.id;
        store
            .publish(PublishSession {
                session: original,
                checkpoint: None,
                issues: vec![],
                replayability: vec![],
                complete: true,
            })
            .unwrap();
        let loaded = store.load_session(id).await.unwrap();
        let PayloadRef::Artifact { key, .. } = &loaded.connections[0].operations[0].request else {
            panic!("missing artifact")
        };
        let directory = root.join("sessions").join(id.to_string());
        std::fs::write(
            store.artifact_path(key).unwrap(),
            b"larger corrupted payload",
        )
        .unwrap();
        assert!(matches!(
            store.get(key).await,
            Err(StorageError::Validation(_))
        ));
        std::fs::write(directory.join("session.json"), b"corrupt").unwrap();
        assert!(matches!(
            store.load_session(id).await,
            Err(StorageError::Validation(_))
        ));
        assert!(matches!(
            store.load_session(SessionId::new()).await,
            Err(StorageError::NotFound(_))
        ));
        assert!(matches!(
            store
                .get("sessions/00000000-0000-0000-0000-000000000000/payloads/deadbeef")
                .await,
            Err(StorageError::Backend(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn manifest_dispatch_preserves_v1_and_rejects_reserved_or_unknown_versions() {
        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let original = session(Vec::new());
        let id = original.id;
        store
            .publish(PublishSession {
                session: original,
                checkpoint: None,
                issues: Vec::new(),
                replayability: Vec::new(),
                complete: true,
            })
            .unwrap();
        let manifest_path = store.session_dir(id).join("manifest.json");
        let original_bytes = std::fs::read(&manifest_path).unwrap();
        let manifest: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
        assert_eq!(manifest["version"], MANIFEST_SCHEMA_V1);
        assert!(store.load(id).is_ok());

        for version in [0, MANIFEST_SCHEMA_V2, MANIFEST_SCHEMA_V2 + 1] {
            let mut manifest = manifest.clone();
            manifest["version"] = version.into();
            std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
            assert!(matches!(store.load(id), Err(StorageError::Validation(_))));
        }

        let mut mismatched = manifest;
        mismatched["canonical_schema_version"] = (CANONICAL_SCHEMA_VERSION + 1).into();
        std::fs::write(&manifest_path, serde_json::to_vec(&mismatched).unwrap()).unwrap();
        assert!(matches!(
            store.verify_existing_manifest(id),
            Err(StorageError::Validation(_))
        ));

        std::fs::write(manifest_path, original_bytes).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn staged_publish_is_private_and_refuses_same_session() {
        use std::os::unix::fs::PermissionsExt;

        let root = root();
        let store = FilesystemSessionStore::new(&root);
        let original = session(b"payload".to_vec());
        let id = original.id;
        store
            .publish(PublishSession {
                session: original.clone(),
                checkpoint: None,
                issues: Vec::new(),
                replayability: Vec::new(),
                complete: true,
            })
            .unwrap();
        let directory = root.join("sessions").join(id.to_string());
        let manifest_path = directory.join("manifest.json");
        let session_path = directory.join("session.json");
        let payload_directory = directory.join("payloads");
        let payload_path = std::fs::read_dir(&payload_directory)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let manifest_before = std::fs::read(&manifest_path).unwrap();
        let session_before = std::fs::read(&session_path).unwrap();
        assert!(manifest_path.is_file());
        assert_eq!(
            directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            payload_directory.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
        for file in [&manifest_path, &session_path, &payload_path] {
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        let verified = store.verify_existing_manifest(id).unwrap();
        assert_eq!(verified.session.id, id);
        assert_eq!(
            verified.canonical_schema_version,
            verified.session.schema_version
        );
        assert_eq!(verified.payload_count, 1);
        assert_eq!(verified.payload_bytes, 7);
        assert!(verified.session_checksum.starts_with("sha256:"));
        assert!(
            store
                .publish(PublishSession {
                    session: original,
                    checkpoint: None,
                    issues: Vec::new(),
                    replayability: Vec::new(),
                    complete: true,
                })
                .is_err()
        );
        assert_eq!(std::fs::read(&manifest_path).unwrap(), manifest_before);
        assert_eq!(std::fs::read(&session_path).unwrap(), session_before);
        assert!(
            std::fs::read_dir(root.join("sessions"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.'))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_publishers_same_session_have_one_winner() {
        use std::sync::{Arc, Barrier};

        let root = root();
        let original = session(b"payload".to_vec());
        let id = original.id;
        let barrier = Arc::new(Barrier::new(2));
        let threads: Vec<_> = (0..2)
            .map(|_| {
                let root = root.clone();
                let session = original.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    FilesystemSessionStore::new(root).publish(PublishSession {
                        session,
                        checkpoint: None,
                        issues: Vec::new(),
                        replayability: Vec::new(),
                        complete: true,
                    })
                })
            })
            .collect();
        let results: Vec<_> = threads
            .into_iter()
            .map(|thread| thread.join().unwrap())
            .collect();
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        let store = FilesystemSessionStore::new(&root);
        assert_eq!(store.verify_existing_manifest(id).unwrap().session.id, id);
        assert!(
            std::fs::read_dir(root.join("sessions"))
                .unwrap()
                .all(|entry| !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.'))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn injected_failures_cleanup_staging_and_never_publish_final() {
        for fault in [
            PublishFault::BeforePayloadDirectorySync,
            PublishFault::BeforeManifest,
            PublishFault::BeforeStagingSync,
            PublishFault::BeforeRename,
        ] {
            let root = root();
            let store = FilesystemSessionStore::new(&root);
            let original = session(b"secret-body".to_vec());
            let id = original.id;
            store.inject_failure(fault);
            assert!(
                store
                    .publish(PublishSession {
                        session: original,
                        checkpoint: None,
                        issues: Vec::new(),
                        replayability: Vec::new(),
                        complete: true,
                    })
                    .is_err(),
                "fault {fault:?} did not fail publication"
            );
            let sessions = root.join("sessions");
            assert!(!sessions.join(id.to_string()).exists());
            assert!(std::fs::read_dir(&sessions).unwrap().next().is_none());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn create_new_refuses_existing_staging_file() {
        let root = root();
        create_private_dir_all(&root).unwrap();
        let file = root.join("existing");
        write_private_new(&file, b"first").unwrap();
        assert!(write_private_new(&file, b"secret").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"first");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filesystem_store_rejects_uuid_path_traversal() {
        let store = FilesystemSessionStore::new(std::env::temp_dir());
        for key in [
            "sessions/x/payloads/../secret",
            "sessions/../payloads/deadbeef",
            "/sessions/x/payloads/deadbeef",
        ] {
            assert!(matches!(
                store.artifact_path(key),
                Err(StorageError::Validation(_))
            ));
        }
    }

    #[test]
    fn sha256_checksum_is_stable() {
        assert_eq!(
            digest(b"abc"),
            "sha256:ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
