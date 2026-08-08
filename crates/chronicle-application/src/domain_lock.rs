//! One exact Chronicle data-domain lock: opener and resolver shared by public
//! intent commands and the daemon `RecorderLease`.
//!
//! The lock file is always `<domain-lock-root>/.chronicle-domain.lock`; the
//! public default lock root is the resolved data directory. Filesystem device
//! identity alone is never treated as lock equivalence, and the subordinate
//! `state_is_owned` state lock is never treated as domain exclusion.

use crate::ApplicationError;
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use thiserror::Error;

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
use rustix::fs::{FlockOperation, flock};
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

pub const DOMAIN_LOCK_FILE: &str = ".chronicle-domain.lock";

#[derive(Debug, Error)]
pub enum DomainLockError {
    #[error("chronicle data domain is already owned by another process")]
    DomainOwned,
    #[error("domain lock is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("domain lock path is unsafe")]
    UnsafePath,
    #[error("domain lock I/O failed")]
    Io(#[source] std::io::Error),
}

impl From<DomainLockError> for ApplicationError {
    fn from(error: DomainLockError) -> Self {
        match error {
            DomainLockError::DomainOwned => Self::DomainOwned,
            DomainLockError::UnsupportedPlatform => Self::UnsupportedDomainLock,
            DomainLockError::UnsafePath => Self::DomainLockUnsafePath,
            DomainLockError::Io(error) => Self::Io(error),
        }
    }
}

/// Held exact-domain lock. Dropping (or process death) releases the OS lock.
#[derive(Debug)]
pub struct DomainLockGuard {
    pub file: File,
    pub path: PathBuf,
}

/// Resolve the exact domain-lock path for a data directory.
///
/// Lock root = configured `domain_lock_root` when supplied (validated), else
/// the data directory. A configured root that is a different path on the same
/// filesystem as the data directory is rejected: multiple differently locked
/// Chronicle domains on one physical filesystem are unsupported, and device
/// equality is never reported as exclusion.
pub fn resolve_domain_lock_path(
    data_dir: &Path,
    configured_root: Option<&Path>,
) -> Result<PathBuf, ApplicationError> {
    let root = if let Some(configured) = configured_root {
        validate_lock_root(configured)?;
        if configured != data_dir && same_filesystem_device(configured, data_dir)? {
            return Err(ApplicationError::IncompatibleDomainLockMapping {
                data_dir: data_dir.to_path_buf(),
                lock_root: configured.to_path_buf(),
            });
        }
        configured
    } else {
        validate_lock_root(data_dir)?;
        data_dir
    };
    Ok(root.join(DOMAIN_LOCK_FILE))
}

/// Acquire the exact domain lock, creating the lock root lazily (0o700).
/// The returned guard must be held for every catalog/sidecar/WAL mutation.
pub fn acquire_domain_lock(
    data_dir: &Path,
    configured_root: Option<&Path>,
) -> Result<DomainLockGuard, ApplicationError> {
    let root = match configured_root {
        Some(configured) => {
            validate_lock_root(configured)?;
            if configured != data_dir && same_filesystem_device(configured, data_dir)? {
                return Err(ApplicationError::IncompatibleDomainLockMapping {
                    data_dir: data_dir.to_path_buf(),
                    lock_root: configured.to_path_buf(),
                });
            }
            configured.to_path_buf()
        }
        None => data_dir.to_path_buf(),
    };
    fs::create_dir_all(&root)?;
    #[cfg(unix)]
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let path = root.join(DOMAIN_LOCK_FILE);
    let file = open_lock(&path)?;
    Ok(DomainLockGuard { file, path })
}

/// Read-only probe: is the exact domain lock at `data_dir`/configured root
/// currently held? Never consults device identity or subordinate state locks.
pub fn domain_lock_held(
    data_dir: &Path,
    configured_root: Option<&Path>,
) -> Result<bool, ApplicationError> {
    let path = resolve_domain_lock_path(data_dir, configured_root)?;
    lock_is_held(&path).map_err(Into::into)
}

/// Shared exact-path opener used by public record and daemon `RecorderLease`.
pub(crate) fn open_lock(path: &Path) -> Result<File, DomainLockError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let _ = path;
        return Err(DomainLockError::UnsupportedPlatform);
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    {
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(DomainLockError::UnsafePath);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options.open(path).map_err(DomainLockError::Io)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(DomainLockError::Io)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(file),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => {
                Err(DomainLockError::DomainOwned)
            }
            Err(error) => Err(DomainLockError::Io(std::io::Error::from_raw_os_error(
                error.raw_os_error(),
            ))),
        }
    }
}

/// Shared exact-path held-probe used by public commands and `RecorderLease`.
pub(crate) fn lock_is_held(path: &Path) -> Result<bool, DomainLockError> {
    #[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
    {
        let _ = path;
        return Err(DomainLockError::UnsupportedPlatform);
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
            return Err(DomainLockError::UnsafePath);
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true);
        let file = options.open(path).map_err(DomainLockError::Io)?;
        match flock(&file, FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(false),
            Err(error) if error == rustix::io::Errno::WOULDBLOCK => Ok(true),
            Err(error) => Err(DomainLockError::Io(std::io::Error::from_raw_os_error(
                error.raw_os_error(),
            ))),
        }
    }
}

fn validate_lock_root(root: &Path) -> Result<(), ApplicationError> {
    if !root.is_absolute()
        || root
            .components()
            .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ApplicationError::DomainLockUnsafePath);
    }
    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(ApplicationError::DomainLockUnsafePath)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(ApplicationError::DomainLockUnsafePath)
        }
        Err(_) => Err(ApplicationError::DomainLockUnsafePath),
    }
}

/// Device identity of the nearest existing ancestor, or an error when none
/// exists. Used only to reject incompatible lock mappings, never to claim
/// exclusion between different lock files.
fn same_filesystem_device(left: &Path, right: &Path) -> Result<bool, ApplicationError> {
    #[cfg(unix)]
    {
        let left_dev = nearest_existing_ancestor(left)?.metadata()?.dev();
        let right_dev = nearest_existing_ancestor(right)?.metadata()?.dev();
        Ok(left_dev == right_dev)
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        Ok(true)
    }
}

fn nearest_existing_ancestor(path: &Path) -> Result<PathBuf, ApplicationError> {
    path.ancestors()
        .find(|candidate| candidate.exists())
        .map(Path::to_path_buf)
        .ok_or_else(|| ApplicationError::Io(std::io::Error::other("no existing ancestor")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("chronicle-domain-lock-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn default_lock_path_is_data_dir_lock_file() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let path = resolve_domain_lock_path(&root, None).unwrap();
        assert_eq!(path, root.join(DOMAIN_LOCK_FILE));
    }

    #[test]
    fn configured_root_equal_to_data_dir_is_same_path() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let path = resolve_domain_lock_path(&root, Some(&root)).unwrap();
        assert_eq!(path, root.join(DOMAIN_LOCK_FILE));
    }

    #[test]
    fn same_exact_path_conflicts_across_acquirers() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let first = acquire_domain_lock(&root, None).expect("first acquires");
        assert!(domain_lock_held(&root, None).unwrap());
        // Second acquirer on the same exact path fails cleanly.
        let err = acquire_domain_lock(&root, None).unwrap_err();
        assert!(matches!(err, ApplicationError::DomainOwned));
        drop(first);
        assert!(!domain_lock_held(&root, None).unwrap());
        let _reacquired = acquire_domain_lock(&root, None).expect("reacquires after drop");
    }

    #[cfg(unix)]
    #[test]
    fn different_paths_never_claimed_equivalent() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let other = temp_root();
        fs::create_dir_all(&other).unwrap();
        // Holding the lock at `root` must not report the lock at `other` as
        // held, even though both live on the same filesystem.
        let _guard = acquire_domain_lock(&root, None).unwrap();
        assert!(domain_lock_held(&root, None).unwrap());
        assert!(!domain_lock_held(&other, None).unwrap());
        let _other_guard = acquire_domain_lock(&other, None).expect("other path is free");
    }

    #[cfg(unix)]
    #[test]
    fn mismatched_lock_mapping_on_same_device_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let other = root.join("other-root");
        fs::create_dir_all(&other).unwrap();
        // Same data root/domain with a different lock path on the same
        // filesystem: preflight rejects rather than claiming exclusion.
        let err = resolve_domain_lock_path(&root, Some(&other)).unwrap_err();
        assert!(matches!(
            err,
            ApplicationError::IncompatibleDomainLockMapping { .. }
        ));
        let err = acquire_domain_lock(&root, Some(&other)).unwrap_err();
        assert!(matches!(
            err,
            ApplicationError::IncompatibleDomainLockMapping { .. }
        ));
    }

    #[test]
    fn relative_lock_root_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let err = resolve_domain_lock_path(&root, Some(Path::new("relative"))).unwrap_err();
        assert!(matches!(err, ApplicationError::DomainLockUnsafePath));
    }

    #[test]
    fn nonexistent_lock_root_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let missing = root.join("missing");
        let err = resolve_domain_lock_path(&root, Some(&missing)).unwrap_err();
        assert!(matches!(err, ApplicationError::DomainLockUnsafePath));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_root_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let real = root.join("real");
        fs::create_dir_all(&real).unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = resolve_domain_lock_path(&root, Some(&link)).unwrap_err();
        assert!(matches!(err, ApplicationError::DomainLockUnsafePath));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_lock_file_rejected() {
        let root = temp_root();
        fs::create_dir_all(&root).unwrap();
        let real = root.join("real-target");
        fs::write(&real, b"x").unwrap();
        std::os::unix::fs::symlink(&real, root.join(DOMAIN_LOCK_FILE)).unwrap();
        // A pre-existing symlink at the exact lock path is unsafe.
        let result = acquire_domain_lock(&root, None);
        assert!(matches!(
            result,
            Err(ApplicationError::DomainLockUnsafePath)
        ));
    }
}
