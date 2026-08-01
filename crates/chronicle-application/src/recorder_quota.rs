//! One-owner filesystem-domain reservation authority. Enforcement is opt-in.

use std::sync::Mutex;
use thiserror::Error;

pub const QUOTA_ENFORCEMENT_ENABLED: bool = true;

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
    reservations: Mutex<Vec<QuotaReservation>>,
}

impl QuotaReservationAuthority {
    pub fn new(quota: DomainQuota) -> Result<Self, QuotaError> {
        if quota.quota_bytes < quota.minimum_free_bytes
            || quota.free_bytes < quota.minimum_free_bytes
        {
            return Err(QuotaError::InvalidQuota);
        }
        Ok(Self {
            quota,
            reservations: Mutex::new(Vec::new()),
        })
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
                    .quota
                    .free_bytes
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

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum QuotaError {
    #[error("quota configuration is invalid")]
    InvalidQuota,
    #[error("quota has insufficient headroom")]
    InsufficientHeadroom,
    #[error("quota reservation was not found")]
    ReservationNotFound,
    #[error("quota reservation lock was poisoned")]
    LockPoisoned,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
