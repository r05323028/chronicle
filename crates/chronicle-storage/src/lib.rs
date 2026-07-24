//! Metadata and artifact repository boundaries.
//!
//! `PostgreSQL` and S3-compatible clients are deferred; in-memory adapters exercise contracts.

use chronicle_canonical::CanonicalSession;
use chronicle_common::SessionId;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use thiserror::Error;

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
