//! OS-released recorder ownership leases.
//!
//! Domain ownership is acquired before state recovery or WAL mutation. The
//! subordinate state lease is intentionally separate from the WAL `record.lock`
//! and from the exact Chronicle data-domain lock (`domain_lock`);
//! `state_is_owned` is never evidence of domain-lock equivalence.

use crate::NormalizedRecorderConfig;
use crate::domain_lock::{DomainLockError, lock_is_held, open_lock};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const STATE_LOCK_FILE: &str = ".chronicle-recorder.lock";

#[derive(Debug, Error)]
pub enum RecorderLeaseError {
    #[error("recorder domain is already owned")]
    DomainOwned,
    #[error("recorder state root is already owned")]
    StateOwned,
    #[error("recorder lease is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("recorder lease path is unsafe")]
    UnsafePath,
    #[error("recorder lease I/O failed")]
    Io(#[source] std::io::Error),
}

/// Holds domain and subordinate state leases. Dropping this value releases both
/// OS locks, including after an abrupt process exit.
pub struct RecorderLease {
    domain_file: File,
    state_file: File,
    pub domain_lock_path: PathBuf,
    pub state_lock_path: PathBuf,
}

impl RecorderLease {
    pub fn acquire(config: &NormalizedRecorderConfig) -> Result<Self, RecorderLeaseError> {
        let domain_path = &config.domain_lock_path;
        let domain_file = open_lock(domain_path)
            .map_err(|error| map_domain_error(error, RecorderLeaseError::DomainOwned))?;

        if fs::create_dir_all(&config.state_root).is_err() {
            return Err(RecorderLeaseError::Io(std::io::Error::other(
                "state root unavailable",
            )));
        }
        #[cfg(unix)]
        fs::set_permissions(&config.state_root, fs::Permissions::from_mode(0o700))
            .map_err(RecorderLeaseError::Io)?;
        let state_path = config.state_root.join(STATE_LOCK_FILE);
        let state_file = open_lock(&state_path)
            .map_err(|error| map_domain_error(error, RecorderLeaseError::StateOwned))?;

        Ok(Self {
            domain_file,
            state_file,
            domain_lock_path: domain_path.clone(),
            state_lock_path: state_path,
        })
    }

    pub fn domain_is_owned(config: &NormalizedRecorderConfig) -> Result<bool, RecorderLeaseError> {
        lock_is_held(&config.domain_lock_path).map_err(map_domain_error_owned)
    }

    pub fn state_lock_name() -> &'static str {
        STATE_LOCK_FILE
    }

    pub fn state_is_owned(state_root: impl AsRef<Path>) -> Result<bool, RecorderLeaseError> {
        lock_is_held(&state_root.as_ref().join(STATE_LOCK_FILE)).map_err(map_domain_error_owned)
    }

    pub fn domain_file(&self) -> &File {
        &self.domain_file
    }

    pub fn state_file(&self) -> &File {
        &self.state_file
    }
}

fn map_domain_error_owned(error: DomainLockError) -> RecorderLeaseError {
    map_domain_error(error, RecorderLeaseError::DomainOwned)
}

fn map_domain_error(error: DomainLockError, held: RecorderLeaseError) -> RecorderLeaseError {
    match error {
        DomainLockError::DomainOwned => held,
        DomainLockError::UnsupportedPlatform => RecorderLeaseError::UnsupportedPlatform,
        DomainLockError::UnsafePath => RecorderLeaseError::UnsafePath,
        DomainLockError::Io(error) => RecorderLeaseError::Io(error),
    }
}

impl From<RecorderLeaseError> for crate::ApplicationError {
    fn from(error: RecorderLeaseError) -> Self {
        Self::InvalidConfig(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder_config::{
        EpochConfig, EtlConfig, FilesystemDomainConfig, LogLevel, LoggingConfig, RecorderConfigV1,
        RecorderScopeConfig, RetentionMode, SegmentConfig, ShutdownConfig, StoreBackend,
        StoreConfig,
    };

    fn lease_config(root: &std::path::Path) -> NormalizedRecorderConfig {
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
                max_age_seconds: 3600,
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
                retry_attempts: 1,
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
        .normalize()
        .unwrap()
    }

    #[test]
    fn domain_lease_is_exclusive_and_released_on_drop() {
        let root = std::env::temp_dir().join(format!("chronicle-lease-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let normalized = lease_config(&root);
        let lease = RecorderLease::acquire(&normalized).unwrap();
        assert!(RecorderLease::domain_is_owned(&normalized).unwrap());
        assert!(matches!(
            RecorderLease::acquire(&normalized),
            Err(RecorderLeaseError::DomainOwned)
        ));
        drop(lease);
        assert!(!RecorderLease::domain_is_owned(&normalized).unwrap());
        let _second = RecorderLease::acquire(&normalized).unwrap();
    }

    #[test]
    fn state_lease_reported_while_held_and_released_on_drop() {
        let root = std::env::temp_dir().join(format!("chronicle-lease-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let normalized = lease_config(&root);
        assert!(!RecorderLease::state_is_owned(&normalized.state_root).unwrap());
        let lease = RecorderLease::acquire(&normalized).unwrap();
        assert!(RecorderLease::state_is_owned(&normalized.state_root).unwrap());
        drop(lease);
        assert!(!RecorderLease::state_is_owned(&normalized.state_root).unwrap());
        fs::remove_dir_all(&root).unwrap();
    }
}
