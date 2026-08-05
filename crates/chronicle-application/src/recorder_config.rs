//! Strict, versioned configuration for the continuous recorder.
#![allow(clippy::format_collect)]
//!
//! This module deliberately does not create directories or acquire leases. It only
//! resolves existing filesystem identity and validates the requested bounds.

use crate::ApplicationError;
use chronicle_wal::{
    COMMIT_MARKER_FRAME_LEN, MAX_SEGMENT_BYTES, MIN_SEGMENT_BYTES, SEGMENT_HEADER_LEN,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

pub const RECORDER_CONFIG_SCHEMA_VERSION: u16 = 1;
const MAX_EPOCH_AGE_SECONDS: u64 = 3_600;
const MAX_EPOCH_BYTES: u64 = chronicle_wal::DEFAULT_MAX_WAL_BYTES;
const MAX_ETL_BATCH_RECORDS: u64 = 4_096;
const MAX_RETRY_ATTEMPTS: u32 = 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderConfigV1 {
    pub version: u16,
    pub scope: RecorderScopeConfig,
    pub state_root: PathBuf,
    pub store_root: PathBuf,
    pub domain_lock_root: PathBuf,
    pub epoch: EpochConfig,
    pub segment: SegmentConfig,
    pub domains: Vec<FilesystemDomainConfig>,
    pub retention: Option<RetentionMode>,
    pub etl: EtlConfig,
    pub store: StoreConfig,
    pub shutdown: ShutdownConfig,
    pub logging: LoggingConfig,
}

impl RecorderConfigV1 {
    pub fn from_toml(input: &str) -> Result<Self, RecorderConfigError> {
        toml::from_str(input).map_err(|_| RecorderConfigError::Parse)
    }

    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, RecorderConfigError> {
        let contents = fs::read_to_string(path).map_err(|_| RecorderConfigError::ReadFailed)?;
        Self::from_toml(&contents)
    }

    pub fn validate(&self) -> Result<(), RecorderConfigError> {
        self.normalize().map(|_| ())
    }

    pub fn normalize(&self) -> Result<NormalizedRecorderConfig, RecorderConfigError> {
        if self.version != RECORDER_CONFIG_SCHEMA_VERSION {
            return Err(RecorderConfigError::UnsupportedVersion);
        }
        self.validate_bounds()?;
        let state_root = resolve_path(&self.state_root, false)?;
        let store_root = resolve_path(&self.store_root, false)?;
        let domain_lock_root = resolve_path(&self.domain_lock_root, true)?;
        let mut domains = self
            .domains
            .iter()
            .map(ResolvedFilesystemDomain::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        if domains.is_empty() {
            return Err(RecorderConfigError::MissingDomain);
        }
        domains.sort_by(|left, right| {
            left.identity
                .canonical_root
                .cmp(&right.identity.canonical_root)
        });
        for pair in domains.windows(2) {
            if pair[0].identity.device == pair[1].identity.device {
                return Err(RecorderConfigError::DuplicateDomain);
            }
        }

        let state_domain = find_domain(&domains, &state_root)?.identity.clone();
        let store_domain = find_domain(&domains, &store_root)?.identity.clone();
        let lock_domain = find_domain(&domains, &domain_lock_root)?;
        if state_domain.device != lock_domain.identity.device {
            return Err(RecorderConfigError::IdentityMismatch);
        }

        let lock_path = domain_lock_root.join(".chronicle-domain.lock");
        let required_reserve = required_reserve(self);
        if domains.iter().any(|domain| {
            domain.quota_bytes < required_reserve.saturating_add(domain.minimum_free_bytes)
        }) {
            return Err(RecorderConfigError::InsufficientReserve);
        }

        let normalized = NormalizedRecorderConfig {
            version: self.version,
            scope: self.scope.clone().normalize()?,
            state_root,
            store_root,
            domain_lock_root,
            domain_lock_path: lock_path,
            epoch: self.epoch.clone(),
            segment: self.segment.clone(),
            domains,
            state_domain,
            store_domain,
            retention: self
                .retention
                .clone()
                .ok_or(RecorderConfigError::MissingRetention)?,
            etl: self.etl.clone(),
            store: self.store.clone(),
            shutdown: self.shutdown.clone(),
            logging: self.logging.clone(),
        };
        Ok(normalized)
    }

    pub fn stable_digest(&self) -> Result<String, RecorderConfigError> {
        self.normalize()?.stable_digest()
    }

    fn validate_bounds(&self) -> Result<(), RecorderConfigError> {
        if !(1..=MAX_EPOCH_AGE_SECONDS).contains(&self.epoch.max_age_seconds)
            || !(MIN_SEGMENT_BYTES..=MAX_EPOCH_BYTES).contains(&self.epoch.max_bytes)
            || !(1..=MAX_EPOCH_AGE_SECONDS).contains(&self.segment.max_age_seconds)
            || !(MIN_SEGMENT_BYTES..=MAX_SEGMENT_BYTES).contains(&self.segment.max_bytes)
            || self.segment.max_bytes > self.epoch.max_bytes
        {
            return Err(RecorderConfigError::InvalidBounds);
        }
        if self.etl.batch_records == 0
            || self.etl.batch_records > MAX_ETL_BATCH_RECORDS
            || self.etl.max_lag_records == 0
            || self.etl.max_lag_records < self.etl.batch_records
            || self.etl.retry_attempts > MAX_RETRY_ATTEMPTS
            || self.store.max_batch_bytes == 0
            || self.store.max_staging_bytes < self.store.max_batch_bytes
            || self.shutdown.timeout_seconds == 0
        {
            return Err(RecorderConfigError::InvalidBounds);
        }
        if !matches!(self.store.backend, StoreBackend::Filesystem) {
            return Err(RecorderConfigError::UnsupportedBackend);
        }
        match self.retention.as_ref() {
            Some(RetentionMode::Retain) => {}
            Some(RetentionMode::DeleteAfter {
                min_age_seconds,
                max_retained_bytes,
            }) => {
                if *min_age_seconds == 0 || max_retained_bytes.is_some_and(|bytes| bytes == 0) {
                    return Err(RecorderConfigError::InvalidRetention);
                }
            }
            None => return Err(RecorderConfigError::MissingRetention),
        }
        Ok(())
    }
}

impl TryFrom<&FilesystemDomainConfig> for ResolvedFilesystemDomain {
    type Error = RecorderConfigError;

    fn try_from(config: &FilesystemDomainConfig) -> Result<Self, Self::Error> {
        if config.quota_bytes == 0 || config.minimum_free_bytes == 0 {
            return Err(RecorderConfigError::InvalidBounds);
        }
        let root = resolve_path(&config.root, true)?;
        Ok(Self {
            identity: FilesystemDomainIdentity {
                device: filesystem_device(&root)?,
                canonical_root: root.display().to_string(),
            },
            quota_bytes: config.quota_bytes,
            minimum_free_bytes: config.minimum_free_bytes,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderScopeConfig {
    pub cgroup_path: PathBuf,
    pub cgroup_id: u64,
    pub shared_scope_acknowledged: bool,
}

impl RecorderScopeConfig {
    fn normalize(&self) -> Result<NormalizedScopeConfig, RecorderConfigError> {
        if self.cgroup_id == 0 {
            return Err(RecorderConfigError::InvalidScope);
        }
        let path = resolve_path(&self.cgroup_path, true)?;
        Ok(NormalizedScopeConfig {
            cgroup_path: path,
            cgroup_id: self.cgroup_id,
            shared_scope_acknowledged: self.shared_scope_acknowledged,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EpochConfig {
    pub max_age_seconds: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentConfig {
    pub max_age_seconds: u64,
    pub max_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemDomainConfig {
    pub root: PathBuf,
    pub quota_bytes: u64,
    pub minimum_free_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetentionMode {
    Retain,
    DeleteAfter {
        min_age_seconds: u64,
        #[serde(default)]
        max_retained_bytes: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EtlConfig {
    pub batch_records: u64,
    pub max_lag_records: u64,
    pub retry_attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoreConfig {
    pub backend: StoreBackend,
    pub max_batch_bytes: u64,
    pub max_staging_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StoreBackend {
    Filesystem,
    S3,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShutdownConfig {
    pub timeout_seconds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    pub level: LogLevel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedRecorderConfig {
    pub version: u16,
    pub scope: NormalizedScopeConfig,
    pub state_root: PathBuf,
    pub store_root: PathBuf,
    pub domain_lock_root: PathBuf,
    pub domain_lock_path: PathBuf,
    pub epoch: EpochConfig,
    pub segment: SegmentConfig,
    pub domains: Vec<ResolvedFilesystemDomain>,
    pub state_domain: FilesystemDomainIdentity,
    pub store_domain: FilesystemDomainIdentity,
    pub retention: RetentionMode,
    pub etl: EtlConfig,
    pub store: StoreConfig,
    pub shutdown: ShutdownConfig,
    pub logging: LoggingConfig,
}

impl NormalizedRecorderConfig {
    pub fn stable_digest(&self) -> Result<String, RecorderConfigError> {
        let mut value =
            serde_json::to_value(self).map_err(|_| RecorderConfigError::DigestFailed)?;
        if let serde_json::Value::Object(object) = &mut value {
            if let Some(serde_json::Value::Object(identity)) = object.get_mut("state_domain") {
                identity.remove("device");
            }
            if let Some(serde_json::Value::Object(identity)) = object.get_mut("store_domain") {
                identity.remove("device");
            }
            if let Some(serde_json::Value::Array(domains)) = object.get_mut("domains") {
                for domain in domains {
                    if let serde_json::Value::Object(domain) = domain
                        && let Some(serde_json::Value::Object(identity)) =
                            domain.get_mut("identity")
                    {
                        identity.remove("device");
                    }
                }
            }
        }
        let mut digest = Sha256::new();
        digest.update(b"chronicle/recorder-config/v1\0");
        digest.update(serde_json::to_vec(&value).map_err(|_| RecorderConfigError::DigestFailed)?);
        let bytes = digest.finalize();
        Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn validate_against(&self, other: &Self) -> Result<(), RecorderConfigError> {
        if self.scope != other.scope
            || self.state_root != other.state_root
            || self.store_root != other.store_root
            || self.domain_lock_root != other.domain_lock_root
            || self.domains != other.domains
        {
            return Err(RecorderConfigError::IdentityMismatch);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NormalizedScopeConfig {
    pub cgroup_path: PathBuf,
    pub cgroup_id: u64,
    pub shared_scope_acknowledged: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemDomainIdentity {
    pub device: u64,
    pub canonical_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResolvedFilesystemDomain {
    pub identity: FilesystemDomainIdentity,
    pub quota_bytes: u64,
    pub minimum_free_bytes: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RecorderConfigError {
    #[error("recorder configuration is not valid TOML")]
    Parse,
    #[error("recorder configuration file could not be read")]
    ReadFailed,
    #[error("recorder configuration version is unsupported")]
    UnsupportedVersion,
    #[error("recorder configuration contains invalid bounds")]
    InvalidBounds,
    #[error("recorder configuration contains invalid retention policy")]
    InvalidRetention,
    #[error("recorder configuration requires explicit retention policy")]
    MissingRetention,
    #[error("recorder configuration contains an unsupported store backend")]
    UnsupportedBackend,
    #[error("recorder configuration contains an unsafe path")]
    UnsafePath,
    #[error("recorder configuration contains an invalid scope")]
    InvalidScope,
    #[error("recorder configuration requires one filesystem domain")]
    MissingDomain,
    #[error("recorder configuration contains duplicate filesystem domains")]
    DuplicateDomain,
    #[error("recorder configuration filesystem identity mismatch")]
    IdentityMismatch,
    #[error("recorder configuration cannot reserve required filesystem space")]
    InsufficientReserve,
    #[error("recorder configuration filesystem identity is unavailable")]
    FilesystemIdentityUnavailable,
    #[error("recorder configuration digest could not be computed")]
    DigestFailed,
}

impl From<RecorderConfigError> for ApplicationError {
    fn from(error: RecorderConfigError) -> Self {
        Self::InvalidConfig(error.to_string())
    }
}

fn required_reserve(config: &RecorderConfigV1) -> u64 {
    config
        .segment
        .max_bytes
        .saturating_add(COMMIT_MARKER_FRAME_LEN as u64)
        .saturating_add(SEGMENT_HEADER_LEN as u64)
        .saturating_add(config.store.max_staging_bytes)
}

fn find_domain<'a>(
    domains: &'a [ResolvedFilesystemDomain],
    path: &Path,
) -> Result<&'a ResolvedFilesystemDomain, RecorderConfigError> {
    let device = filesystem_device(path)?;
    domains
        .iter()
        .find(|domain| {
            domain.identity.device == device
                && path.starts_with(Path::new(&domain.identity.canonical_root))
        })
        .ok_or(RecorderConfigError::IdentityMismatch)
}

fn resolve_path(path: &Path, must_exist_directory: bool) -> Result<PathBuf, RecorderConfigError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(RecorderConfigError::UnsafePath);
    }
    if must_exist_directory && !path.is_dir() {
        return Err(RecorderConfigError::UnsafePath);
    }
    let mut current = PathBuf::from(Path::new("/"));
    let mut missing = Vec::new();
    for component in path.components().skip(1) {
        let Component::Normal(part) = component else {
            return Err(RecorderConfigError::UnsafePath);
        };
        current.push(part);
        match fs::metadata(&current) {
            Ok(metadata) if !metadata.is_dir() => return Err(RecorderConfigError::UnsafePath),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(part.to_owned());
            }
            Err(_) => return Err(RecorderConfigError::UnsafePath),
        }
    }
    let existing = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or(RecorderConfigError::UnsafePath)?;
    if must_exist_directory
        && !fs::metadata(existing)
            .map_err(|_| RecorderConfigError::UnsafePath)?
            .is_dir()
    {
        return Err(RecorderConfigError::UnsafePath);
    }
    let canonical_existing =
        fs::canonicalize(existing).map_err(|_| RecorderConfigError::UnsafePath)?;
    let suffix = path
        .strip_prefix(existing)
        .map_err(|_| RecorderConfigError::UnsafePath)?;
    let resolved = canonical_existing.join(suffix);
    if resolved == Path::new("/") {
        return Err(RecorderConfigError::UnsafePath);
    }
    let _ = missing;
    Ok(resolved.components().collect())
}

fn filesystem_device(path: &Path) -> Result<u64, RecorderConfigError> {
    let probe = path
        .ancestors()
        .find(|candidate| candidate.exists())
        .ok_or(RecorderConfigError::FilesystemIdentityUnavailable)?;
    let metadata =
        fs::metadata(probe).map_err(|_| RecorderConfigError::FilesystemIdentityUnavailable)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(metadata.dev())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path) -> RecorderConfigV1 {
        RecorderConfigV1 {
            version: 1,
            scope: RecorderScopeConfig {
                cgroup_path: root.to_path_buf(),
                cgroup_id: 1,
                shared_scope_acknowledged: true,
            },
            state_root: root.join("state"),
            store_root: root.join("store"),
            domain_lock_root: root.to_path_buf(),
            epoch: EpochConfig {
                max_age_seconds: 3_600,
                max_bytes: chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            },
            segment: SegmentConfig {
                max_age_seconds: 300,
                max_bytes: chronicle_wal::DEFAULT_SEGMENT_BYTES,
            },
            domains: vec![FilesystemDomainConfig {
                root: root.to_path_buf(),
                quota_bytes: 8 * 1024 * 1024 * 1024,
                minimum_free_bytes: 1024 * 1024,
            }],
            retention: Some(RetentionMode::Retain),
            etl: EtlConfig {
                batch_records: 4096,
                max_lag_records: 4096,
                retry_attempts: 3,
            },
            store: StoreConfig {
                backend: StoreBackend::Filesystem,
                max_batch_bytes: 1024,
                max_staging_bytes: 4096,
            },
            shutdown: ShutdownConfig {
                timeout_seconds: 30,
            },
            logging: LoggingConfig {
                level: LogLevel::Info,
            },
        }
    }

    #[test]
    fn normalized_digest_is_stable() {
        let root = tempfile_root();
        let config = config(&root);
        let first = config.stable_digest().unwrap();
        fs::create_dir_all(&config.state_root).unwrap();
        fs::create_dir_all(&config.store_root).unwrap();
        let second = config.stable_digest().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn toml_round_trip_and_strict_schema() {
        let root = tempfile_root();
        let config = config(&root);
        let text = toml::to_string(&config).unwrap();
        let decoded = RecorderConfigV1::from_toml(&text).unwrap();
        assert_eq!(decoded, config);
        assert!(RecorderConfigV1::from_toml(&format!("{text}\\nunknown = true")).is_err());
    }

    #[test]
    fn unsupported_version_and_invalid_retention_fail_closed() {
        let root = tempfile_root();
        let mut config = config(&root);
        config.version = 2;
        assert_eq!(
            config.validate(),
            Err(RecorderConfigError::UnsupportedVersion)
        );
        config.version = 1;
        config.retention = None;
        assert_eq!(
            config.validate(),
            Err(RecorderConfigError::MissingRetention)
        );
    }

    #[test]
    fn digest_changes_with_effective_policy_and_lock_path_is_deterministic() {
        let root = tempfile_root();
        let config = config(&root);
        let normalized = config.normalize().unwrap();
        assert_eq!(
            normalized.domain_lock_path,
            normalized.domain_lock_root.join(".chronicle-domain.lock")
        );
        let mut changed = config.clone();
        changed.segment.max_age_seconds += 1;
        assert_ne!(
            config.stable_digest().unwrap(),
            changed.stable_digest().unwrap()
        );
    }

    #[test]
    fn unsafe_paths_are_rejected_without_mutation() {
        let root = tempfile_root();
        let mut config = config(&root);
        config.state_root = PathBuf::from("relative");
        assert_eq!(config.validate(), Err(RecorderConfigError::UnsafePath));
    }

    fn tempfile_root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("chronicle-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }
}
