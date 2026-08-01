//! OS-released recorder ownership leases.
//!
//! Domain ownership is acquired before state recovery or WAL mutation. The
//! subordinate state lease is intentionally separate from the WAL `record.lock`.

use crate::NormalizedRecorderConfig;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use thiserror::Error;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{FlockOperation, flock};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

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
        let domain_file = open_lock(domain_path, RecorderLeaseError::DomainOwned)?;

        if fs::create_dir_all(&config.state_root).is_err() {
            return Err(RecorderLeaseError::Io(std::io::Error::other(
                "state root unavailable",
            )));
        }
        #[cfg(unix)]
        fs::set_permissions(&config.state_root, fs::Permissions::from_mode(0o700))
            .map_err(RecorderLeaseError::Io)?;
        let state_path = config.state_root.join(STATE_LOCK_FILE);
        let state_file = open_lock(&state_path, RecorderLeaseError::StateOwned)?;

        Ok(Self {
            domain_file,
            state_file,
            domain_lock_path: domain_path.clone(),
            state_lock_path: state_path,
        })
    }

    pub fn domain_is_owned(config: &NormalizedRecorderConfig) -> Result<bool, RecorderLeaseError> {
        lock_is_held(&config.domain_lock_path)
    }

    pub fn state_lock_name() -> &'static str {
        STATE_LOCK_FILE
    }

    pub fn domain_file(&self) -> &File {
        &self.domain_file
    }

    pub fn state_file(&self) -> &File {
        &self.state_file
    }
}

fn open_lock(path: &Path, held: RecorderLeaseError) -> Result<File, RecorderLeaseError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let _ = (path, held);
        return Err(RecorderLeaseError::UnsupportedPlatform);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(RecorderLeaseError::UnsafePath);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path).map_err(RecorderLeaseError::Io)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(RecorderLeaseError::Io)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(file),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => Err(held),
            Err(error) => Err(RecorderLeaseError::Io(std::io::Error::from_raw_os_error(
                error.raw_os_error(),
            ))),
        }
    }
}

fn lock_is_held(path: &Path) -> Result<bool, RecorderLeaseError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let _ = path;
        return Err(RecorderLeaseError::UnsupportedPlatform);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        if !path.exists() {
            return Ok(false);
        }
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(RecorderLeaseError::UnsafePath);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        let file = options.open(path).map_err(RecorderLeaseError::Io)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(false),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => Ok(true),
            Err(error) => Err(RecorderLeaseError::Io(std::io::Error::from_raw_os_error(
                error.raw_os_error(),
            ))),
        }
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

    #[test]
    fn domain_lease_is_exclusive_and_released_on_drop() {
        let root = std::env::temp_dir().join(format!("chronicle-lease-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let config = RecorderConfigV1 {
            version: 1,
            scope: RecorderScopeConfig {
                cgroup_path: root.clone(),
                cgroup_id: 1,
                shared_scope_acknowledged: true,
            },
            state_root: root.join("state"),
            store_root: root.join("store"),
            domain_lock_root: root.clone(),
            epoch: EpochConfig {
                max_age_seconds: 3600,
                max_bytes: chronicle_wal::DEFAULT_MAX_WAL_BYTES,
            },
            segment: SegmentConfig {
                max_age_seconds: 300,
                max_bytes: chronicle_wal::DEFAULT_SEGMENT_BYTES,
            },
            domains: vec![FilesystemDomainConfig {
                root: root.clone(),
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
        };
        let normalized = config.normalize().unwrap();
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
}
