//! One-owner filesystem-domain reservation authority. Enforcement is opt-in.

use std::fs;
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};
use thiserror::Error;

pub const QUOTA_ENFORCEMENT_ENABLED: bool = true;

pub trait CapacityProvider: Send + Sync {
    fn available_bytes(&self, root: &Path) -> Result<u64, QuotaError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct StatvfsCapacityProvider;

impl CapacityProvider for StatvfsCapacityProvider {
    fn available_bytes(&self, root: &Path) -> Result<u64, QuotaError> {
        let stat = rustix::fs::statvfs(root).map_err(|_| QuotaError::FilesystemProbe)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DomainQuota {
    pub domain: String,
    pub quota_bytes: u64,
    pub minimum_free_bytes: u64,
    pub free_bytes: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReservationKind {
    Wal,
    Manifest,
    Checkpoint,
    RecordingStore,
    FinalSession,
    Staging,
    Trash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaReservation {
    pub kind: ReservationKind,
    pub bytes: u64,
}

pub struct QuotaReservationAuthority {
    quota: DomainQuota,
    free_bytes: AtomicU64,
    managed_bytes: AtomicU64,
    managed_roots: Vec<std::path::PathBuf>,
    capacity_provider: Arc<dyn CapacityProvider>,
    reservations: Mutex<Vec<QuotaReservation>>,
}

impl QuotaReservationAuthority {
    pub fn new(quota: DomainQuota) -> Result<Self, QuotaError> {
        if quota.quota_bytes < quota.minimum_free_bytes
            || quota.free_bytes < quota.minimum_free_bytes
        {
            return Err(QuotaError::InvalidQuota);
        }
        let free_bytes = quota.free_bytes;
        Ok(Self {
            quota,
            free_bytes: AtomicU64::new(free_bytes),
            managed_bytes: AtomicU64::new(0),
            managed_roots: Vec::new(),
            capacity_provider: Arc::new(StatvfsCapacityProvider),
            reservations: Mutex::new(Vec::new()),
        })
    }

    pub fn from_filesystem(
        root: impl AsRef<Path>,
        quota_bytes: u64,
        minimum_free_bytes: u64,
    ) -> Result<Self, QuotaError> {
        let root = root.as_ref().to_path_buf();
        Self::from_filesystem_with_roots(&root, &[root.as_path()], quota_bytes, minimum_free_bytes)
    }

    pub fn from_filesystem_with_provider(
        root: impl AsRef<Path>,
        managed_roots: &[&Path],
        quota_bytes: u64,
        minimum_free_bytes: u64,
        capacity_provider: Arc<dyn CapacityProvider>,
    ) -> Result<Self, QuotaError> {
        let root = root.as_ref();
        let managed_bytes = managed_roots
            .iter()
            .map(|path| directory_bytes(path))
            .try_fold(0u64, |total, bytes| {
                bytes.map(|bytes| total.saturating_add(bytes))
            })?;
        let mut authority = Self::new(DomainQuota {
            domain: root.display().to_string(),
            quota_bytes,
            minimum_free_bytes,
            free_bytes: quota_bytes,
        })?;
        authority.capacity_provider = capacity_provider;
        authority.managed_roots = managed_roots
            .iter()
            .map(|path| (*path).to_path_buf())
            .collect();
        authority
            .managed_bytes
            .store(managed_bytes, Ordering::Release);
        authority.refresh_from_filesystem()?;
        Ok(authority)
    }

    pub fn from_filesystem_with_roots(
        root: impl AsRef<Path>,
        managed_roots: &[&Path],
        quota_bytes: u64,
        minimum_free_bytes: u64,
    ) -> Result<Self, QuotaError> {
        Self::from_filesystem_with_provider(
            root,
            managed_roots,
            quota_bytes,
            minimum_free_bytes,
            Arc::new(StatvfsCapacityProvider),
        )
    }

    pub fn refresh_from_filesystem(&self) -> Result<u64, QuotaError> {
        let root = Path::new(&self.quota.domain);
        let filesystem_free = self.capacity_provider.available_bytes(root)?;
        let free_bytes = filesystem_free.min(
            self.quota
                .quota_bytes
                .saturating_sub(self.managed_bytes.load(Ordering::Acquire)),
        );
        self.free_bytes.store(free_bytes, Ordering::Release);
        Ok(free_bytes)
    }

    pub fn rebuild_managed_usage(&self) -> Result<u64, QuotaError> {
        let used = self
            .managed_roots
            .iter()
            .map(|path| directory_bytes(path))
            .try_fold(0u64, |total, bytes| {
                bytes.map(|bytes| total.saturating_add(bytes))
            })?;
        self.managed_bytes.store(used, Ordering::Release);
        self.refresh_from_filesystem()
    }

    pub fn available_bytes(&self) -> u64 {
        self.free_bytes.load(Ordering::Acquire)
    }

    pub fn reserve(&self, kind: ReservationKind, bytes: u64) -> Result<(), QuotaError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        let reserved: u64 = reservations
            .iter()
            .map(|reservation| reservation.bytes)
            .sum();
        if QUOTA_ENFORCEMENT_ENABLED
            && reserved.saturating_add(bytes)
                > self
                    .available_bytes()
                    .saturating_sub(self.quota.minimum_free_bytes)
        {
            return Err(QuotaError::InsufficientHeadroom);
        }
        reservations.push(QuotaReservation { kind, bytes });
        Ok(())
    }

    pub fn release(&self, kind: ReservationKind, bytes: u64) -> Result<(), QuotaError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        let Some(index) = reservations
            .iter()
            .position(|reservation| reservation.kind == kind && reservation.bytes == bytes)
        else {
            return Err(QuotaError::ReservationNotFound);
        };
        reservations.remove(index);
        Ok(())
    }

    pub fn reserved_bytes(&self) -> Result<u64, QuotaError> {
        Ok(self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?
            .iter()
            .map(|reservation| reservation.bytes)
            .sum())
    }

    pub fn quota(&self) -> &DomainQuota {
        &self.quota
    }
}

fn directory_bytes(root: &Path) -> Result<u64, QuotaError> {
    let mut pending = vec![root.to_path_buf()];
    let mut total = 0u64;
    while let Some(path) = pending.pop() {
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(QuotaError::FilesystemProbe),
        };
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|_| QuotaError::FilesystemProbe)? {
                pending.push(entry.map_err(|_| QuotaError::FilesystemProbe)?.path());
            }
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum QuotaError {
    #[error("quota configuration is invalid")]
    InvalidQuota,
    #[error("quota has insufficient headroom")]
    InsufficientHeadroom,
    #[error("filesystem capacity probe failed")]
    FilesystemProbe,
    #[error("quota reservation was not found")]
    ReservationNotFound,
    #[error("quota reservation lock was poisoned")]
    LockPoisoned,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct FixedCapacity(AtomicU64);

    impl CapacityProvider for FixedCapacity {
        fn available_bytes(&self, _root: &Path) -> Result<u64, QuotaError> {
            Ok(self.0.load(Ordering::Acquire))
        }
    }

    #[test]
    fn reservations_are_exact_and_releasable() {
        let authority = QuotaReservationAuthority::new(DomainQuota {
            domain: "domain".into(),
            quota_bytes: 100,
            minimum_free_bytes: 10,
            free_bytes: 100,
        })
        .unwrap();
        authority.reserve(ReservationKind::Wal, 20).unwrap();
        authority.reserve(ReservationKind::Checkpoint, 5).unwrap();
        assert_eq!(authority.reserved_bytes().unwrap(), 25);
        authority.release(ReservationKind::Wal, 20).unwrap();
        assert_eq!(authority.reserved_bytes().unwrap(), 5);
    }

    #[test]
    fn enforcement_preserves_minimum_free_headroom() {
        let authority = QuotaReservationAuthority::new(DomainQuota {
            domain: "domain".into(),
            quota_bytes: 100,
            minimum_free_bytes: 10,
            free_bytes: 100,
        })
        .unwrap();
        authority.reserve(ReservationKind::Wal, 90).unwrap();
        assert_eq!(
            authority.reserve(ReservationKind::Trash, 1),
            Err(QuotaError::InsufficientHeadroom)
        );
    }

    #[test]
    fn filesystem_probe_recomputes_quota_after_restart() {
        let root = std::env::temp_dir().join(format!("chronicle-quota-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let authority = QuotaReservationAuthority::from_filesystem(&root, 100, 1).unwrap();
        std::fs::write(root.join("committed.wal"), vec![0u8; 99]).unwrap();
        assert!(authority.rebuild_managed_usage().unwrap() <= 1);
        assert_eq!(
            authority.reserve(ReservationKind::Wal, 1),
            Err(QuotaError::InsufficientHeadroom)
        );
        std::fs::remove_file(root.join("committed.wal")).unwrap();
        authority.rebuild_managed_usage().unwrap();
        authority.reserve(ReservationKind::Wal, 1).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn injected_capacity_proves_exhaustion_and_recovery() {
        let root =
            std::env::temp_dir().join(format!("chronicle-capacity-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let capacity = std::sync::Arc::new(FixedCapacity(AtomicU64::new(100)));
        let authority = QuotaReservationAuthority::from_filesystem_with_provider(
            &root,
            &[root.as_path()],
            200,
            10,
            capacity.clone(),
        )
        .unwrap();
        authority.reserve(ReservationKind::Wal, 80).unwrap();
        assert_eq!(
            authority.reserve(ReservationKind::Staging, 11),
            Err(QuotaError::InsufficientHeadroom)
        );
        capacity.0.store(200, Ordering::Release);
        authority.refresh_from_filesystem().unwrap();
        authority.reserve(ReservationKind::Staging, 11).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn separate_domains_do_not_borrow_headroom() {
        let first = QuotaReservationAuthority::new(DomainQuota {
            domain: "first".into(),
            quota_bytes: 100,
            minimum_free_bytes: 10,
            free_bytes: 100,
        })
        .unwrap();
        let second = QuotaReservationAuthority::new(DomainQuota {
            domain: "second".into(),
            quota_bytes: 100,
            minimum_free_bytes: 10,
            free_bytes: 100,
        })
        .unwrap();

        first.reserve(ReservationKind::Wal, 90).unwrap();
        assert_eq!(
            first.reserve(ReservationKind::RecordingStore, 1),
            Err(QuotaError::InsufficientHeadroom)
        );
        assert!(second.reserve(ReservationKind::RecordingStore, 90).is_ok());
    }
}
