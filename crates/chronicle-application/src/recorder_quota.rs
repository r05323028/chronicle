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
    managed_roots: Mutex<Vec<std::path::PathBuf>>,
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
            managed_roots: Mutex::new(Vec::new()),
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
        let managed_roots = normalize_managed_roots(managed_roots);
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
        authority.managed_roots = Mutex::new(managed_roots);
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
        let roots = self
            .managed_roots
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?
            .clone();
        let used = roots
            .iter()
            .map(|path| directory_bytes(path))
            .try_fold(0u64, |total, bytes| {
                bytes.map(|bytes| total.saturating_add(bytes))
            })?;
        self.managed_bytes.store(used, Ordering::Release);
        self.refresh_from_filesystem()
    }

    /// Add durable storage root to filesystem accounting, avoiding nested-root
    /// double counting. Root must belong to this quota domain.
    pub fn register_managed_root(&self, root: impl AsRef<Path>) -> Result<(), QuotaError> {
        let root = root.as_ref().to_path_buf();
        if !root.starts_with(Path::new(&self.quota.domain)) {
            return Err(QuotaError::InvalidQuota);
        }
        let mut roots = self
            .managed_roots
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        if roots.iter().any(|existing| root.starts_with(existing)) {
            drop(roots);
            return self.rebuild_managed_usage().map(|_| ());
        }
        roots.retain(|existing| !existing.starts_with(&root));
        roots.push(root);
        drop(roots);
        self.rebuild_managed_usage().map(|_| ())
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

    /// Convert reserved future capacity into durable managed usage.
    pub fn consume(&self, kind: ReservationKind, bytes: u64) -> Result<(), QuotaError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        let Some(reservation) = reservations
            .iter_mut()
            .find(|reservation| reservation.kind == kind)
        else {
            return Ok(());
        };
        let consumed = reservation.bytes.min(bytes);
        reservation.bytes -= consumed;
        if reservation.bytes == 0 {
            reservations.retain(|item| item.bytes != 0);
        }
        Ok(())
    }

    /// Release oldest reservation for kind after its durable owner is
    /// finalized. Partial release preserves successor reservations.
    pub fn release_prefix(&self, kind: ReservationKind, bytes: u64) -> Result<(), QuotaError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        let Some(index) = reservations
            .iter()
            .position(|reservation| reservation.kind == kind)
        else {
            return Ok(());
        };
        let released = reservations[index].bytes.min(bytes);
        reservations[index].bytes -= released;
        if reservations[index].bytes == 0 {
            reservations.remove(index);
        }
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

    pub fn release_all(&self, kind: ReservationKind) -> Result<u64, QuotaError> {
        let mut reservations = self
            .reservations
            .lock()
            .map_err(|_| QuotaError::LockPoisoned)?;
        let mut released = 0u64;
        reservations.retain(|reservation| {
            if reservation.kind == kind {
                released = released.saturating_add(reservation.bytes);
                false
            } else {
                true
            }
        });
        Ok(released)
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

    pub fn managed_bytes(&self) -> u64 {
        self.managed_bytes.load(Ordering::Acquire)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaPressureKind {
    None,
    Recoverable,
    Terminal,
}

/// Classify headroom pressure: recoverable when external space or retention
/// cleanup can restore the minimum-free headroom; terminal only when managed
/// durable usage alone already exceeds the configured quota ceiling, so
/// headroom can only be restored by deleting protected or corrupt evidence
/// or by raising the quota.
pub fn classify_quota_pressure(
    free_bytes: u64,
    reserved_bytes: u64,
    quota_bytes: u64,
    minimum_free_bytes: u64,
    managed_bytes: u64,
) -> QuotaPressureKind {
    let pressured = free_bytes < minimum_free_bytes
        || reserved_bytes > free_bytes.saturating_sub(minimum_free_bytes);
    if !pressured {
        return QuotaPressureKind::None;
    }
    if managed_bytes > quota_bytes {
        QuotaPressureKind::Terminal
    } else {
        QuotaPressureKind::Recoverable
    }
}

fn normalize_managed_roots(managed_roots: &[&Path]) -> Vec<std::path::PathBuf> {
    let mut normalized: Vec<std::path::PathBuf> = Vec::new();
    for root in managed_roots {
        if normalized.iter().any(|existing| root.starts_with(existing)) {
            continue;
        }
        normalized.retain(|existing| !existing.starts_with(root));
        normalized.push((*root).to_path_buf());
    }
    normalized
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
        authority.consume(ReservationKind::Wal, 10).unwrap();
        assert_eq!(authority.reserved_bytes().unwrap(), 15);
        authority.release(ReservationKind::Wal, 10).unwrap();
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
    fn minimum_free_blocks_before_quota_is_exhausted_and_recovers() {
        let root =
            std::env::temp_dir().join(format!("chronicle-min-free-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let capacity = std::sync::Arc::new(FixedCapacity(AtomicU64::new(50)));
        let authority = QuotaReservationAuthority::from_filesystem_with_provider(
            &root,
            &[root.as_path()],
            100,
            40,
            capacity.clone(),
        )
        .unwrap();
        assert_eq!(authority.quota().quota_bytes, 100);
        assert_eq!(
            authority.reserve(ReservationKind::Wal, 11),
            Err(QuotaError::InsufficientHeadroom)
        );
        capacity.0.store(80, Ordering::Release);
        authority.refresh_from_filesystem().unwrap();
        authority.reserve(ReservationKind::Wal, 11).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn registering_wal_root_matches_restart_accounting() {
        let root =
            std::env::temp_dir().join(format!("chronicle-quota-roots-{}", uuid::Uuid::new_v4()));
        let store = root.join("store");
        let wal = root.join("wal");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&wal).unwrap();
        std::fs::write(store.join("manifest"), vec![0_u8; 40]).unwrap();
        std::fs::write(wal.join("segment"), vec![0_u8; 60]).unwrap();
        let capacity = std::sync::Arc::new(FixedCapacity(AtomicU64::new(1_000)));
        let authority = QuotaReservationAuthority::from_filesystem_with_provider(
            &root,
            &[store.as_path()],
            200,
            1,
            capacity.clone(),
        )
        .unwrap();
        authority.register_managed_root(&wal).unwrap();
        let restarted = QuotaReservationAuthority::from_filesystem_with_provider(
            &root,
            &[store.as_path(), wal.as_path()],
            200,
            1,
            capacity,
        )
        .unwrap();
        assert_eq!(authority.available_bytes(), 100);
        assert_eq!(authority.available_bytes(), restarted.available_bytes());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nested_managed_roots_are_not_double_counted_and_outside_is_rejected() {
        let root =
            std::env::temp_dir().join(format!("chronicle-quota-nested-{}", uuid::Uuid::new_v4()));
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(root.join("root-file"), vec![0_u8; 10]).unwrap();
        std::fs::write(nested.join("nested-file"), vec![0_u8; 20]).unwrap();
        let capacity = std::sync::Arc::new(FixedCapacity(AtomicU64::new(1_000)));
        let authority = QuotaReservationAuthority::from_filesystem_with_provider(
            &root,
            &[nested.as_path()],
            100,
            1,
            capacity,
        )
        .unwrap();
        authority.register_managed_root(&root).unwrap();
        assert_eq!(authority.available_bytes(), 70);
        let outside = root
            .parent()
            .unwrap()
            .join(format!("chronicle-quota-outside-{}", uuid::Uuid::new_v4()));
        assert_eq!(
            authority.register_managed_root(&outside),
            Err(QuotaError::InvalidQuota)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn classify_quota_pressure_distinguishes_recoverable_and_terminal() {
        // No pressure.
        assert_eq!(
            classify_quota_pressure(500, 0, 1_000, 100, 400),
            QuotaPressureKind::None
        );
        // Filesystem-side pressure with managed usage inside quota headroom is
        // recoverable: external space or retention cleanup restores headroom.
        assert_eq!(
            classify_quota_pressure(50, 0, 1_000, 100, 400),
            QuotaPressureKind::Recoverable
        );
        // Reserved-bytes pressure with managed usage inside quota headroom is
        // recoverable once the transient reservation is released.
        assert_eq!(
            classify_quota_pressure(500, 450, 1_000, 100, 400),
            QuotaPressureKind::Recoverable
        );
        // Managed durable usage alone exceeds the configured quota ceiling:
        // only deleting protected or corrupt evidence (or raising the quota)
        // could restore headroom (terminal).
        assert_eq!(
            classify_quota_pressure(0, 0, 1_000, 100, 1_001),
            QuotaPressureKind::Terminal
        );
        // Usage at the ceiling is still recoverable while external space can
        // be freed.
        assert_eq!(
            classify_quota_pressure(0, 0, 1_000, 100, 1_000),
            QuotaPressureKind::Recoverable
        );
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
