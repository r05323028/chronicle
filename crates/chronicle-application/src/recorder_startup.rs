//! Ordered pre-ETL recorder startup.

use crate::{
    DomainQuota, NormalizedRecorderConfig, QuotaError, QuotaReservationAuthority, RecorderHealth,
    RecorderLease, RecorderLeaseError, RecorderLifecycle, RecorderLifecycleError,
    RecorderReadiness,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecorderStartupError {
    #[error(transparent)]
    Lease(#[from] RecorderLeaseError),
    #[error(transparent)]
    Lifecycle(#[from] RecorderLifecycleError),
    #[error("recorder startup recovery failed")]
    Recovery,
    #[error("recorder capture attachment failed")]
    CaptureAttach,
    #[error("recorder quota reservation failed")]
    Quota(#[from] QuotaError),
    #[error("recorder appendable epoch establishment failed")]
    Appendable,
    #[error("recorder incremental ETL resume failed")]
    EtlResume,
}

pub struct RecorderStartup {
    lease: RecorderLease,
    lifecycle: RecorderLifecycle,
    quotas: Vec<QuotaReservationAuthority>,
}

impl RecorderStartup {
    /// Acquires ownership before recovery and attachment. `recover_appendable`
    /// must establish a durable appendable epoch; no ETL correctness is required.
    pub fn start(
        config: &NormalizedRecorderConfig,
        recover_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        attach_capture: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        let lease = RecorderLease::acquire(config)?;
        let mut lifecycle = RecorderLifecycle::new();
        lifecycle.begin_recovery()?;
        if recover_appendable().is_err() {
            lifecycle.fail();
            return Err(RecorderStartupError::Recovery);
        }
        if attach_capture().is_err() {
            lifecycle.fail();
            return Err(RecorderStartupError::CaptureAttach);
        }
        lifecycle.running(RecorderReadiness::Unknown)?;
        Ok(Self {
            lease,
            lifecycle,
            quotas: Vec::new(),
        })
    }

    /// Completes startup after retention/quota activation. Recovery includes
    /// state, WAL, manifest, checkpoint, and cleanup reconciliation; ETL is
    /// resumed only after capture is attached and appendability is durable.
    pub fn start_post_foundation(
        config: &NormalizedRecorderConfig,
        recover_foundation: impl FnOnce() -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(&[QuotaReservationAuthority]) -> Result<(), RecorderStartupError>,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        attach_capture: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        let lease = RecorderLease::acquire(config)?;
        let mut lifecycle = RecorderLifecycle::new();
        lifecycle.begin_recovery()?;
        if let Err(error) = recover_foundation() {
            lifecycle.fail();
            return Err(error);
        }
        let quotas = config
            .domains
            .iter()
            .map(|domain| {
                QuotaReservationAuthority::new(DomainQuota {
                    domain: domain.identity.canonical_root.clone(),
                    quota_bytes: domain.quota_bytes,
                    minimum_free_bytes: domain.minimum_free_bytes,
                    free_bytes: domain.quota_bytes,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Err(error) = reserve_quota(&quotas) {
            lifecycle.fail();
            return Err(error);
        }
        if let Err(error) = establish_appendable() {
            lifecycle.fail();
            return Err(error);
        }
        if let Err(error) = attach_capture() {
            lifecycle.fail();
            return Err(error);
        }
        if let Err(error) = resume_etl() {
            lifecycle.fail();
            return Err(error);
        }
        lifecycle.running(RecorderReadiness::Ready)?;
        Ok(Self {
            lease,
            lifecycle,
            quotas,
        })
    }

    pub fn begin_draining(&mut self) -> Result<(), RecorderStartupError> {
        self.lifecycle.begin_draining()?;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), RecorderStartupError> {
        self.lifecycle.stop()?;
        Ok(())
    }

    pub fn quota_authorities(&self) -> &[QuotaReservationAuthority] {
        &self.quotas
    }

    pub fn lifecycle(&self) -> &RecorderLifecycle {
        &self.lifecycle
    }

    pub fn capture_ready(&self) -> bool {
        self.lifecycle.readiness.capture == RecorderReadiness::Ready
    }

    pub fn processing_ready(&self) -> RecorderReadiness {
        self.lifecycle.readiness.processing
    }

    pub fn health(&self) -> RecorderHealth {
        self.lifecycle.readiness.health
    }

    pub fn lease(&self) -> &RecorderLease {
        &self.lease
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
    use std::fs;
    use std::sync::{Arc, Mutex};

    fn config(root: &std::path::Path) -> crate::NormalizedRecorderConfig {
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
    fn post_foundation_startup_orders_recovery_quota_append_capture_etl() {
        let root =
            std::env::temp_dir().join(format!("chronicle-post-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let record = |name: &'static str, order: &Arc<Mutex<Vec<&'static str>>>| {
            order.lock().unwrap().push(name);
        };
        let recovery_order = Arc::clone(&order);
        let quota_order = Arc::clone(&order);
        let append_order = Arc::clone(&order);
        let attach_order = Arc::clone(&order);
        let etl_order = Arc::clone(&order);
        let startup = RecorderStartup::start_post_foundation(
            &config(&root),
            move || {
                record("recover", &recovery_order);
                Ok(())
            },
            move |quotas| {
                assert_eq!(quotas.len(), 1);
                record("quota", &quota_order);
                Ok(())
            },
            move || {
                record("appendable", &append_order);
                Ok(())
            },
            move || {
                record("attach", &attach_order);
                Ok(())
            },
            move || {
                record("etl", &etl_order);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(
            *order.lock().unwrap(),
            ["recover", "quota", "appendable", "attach", "etl"]
        );
        assert!(startup.capture_ready());
        assert_eq!(startup.processing_ready(), RecorderReadiness::Ready);
        assert_eq!(startup.health(), RecorderHealth::Healthy);
        assert_eq!(startup.quota_authorities().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_foundation_failure_stops_before_later_steps() {
        let expected = ["recover", "quota", "appendable", "attach", "etl"];
        for failed in expected {
            let root = std::env::temp_dir().join(format!(
                "chronicle-post-failure-{failed}-{}",
                uuid::Uuid::new_v4()
            ));
            fs::create_dir_all(&root).unwrap();
            let order = Arc::new(Mutex::new(Vec::new()));
            let make_step = |name: &'static str, order: &Arc<Mutex<Vec<&'static str>>>| {
                let order = Arc::clone(order);
                move || {
                    order.lock().unwrap().push(name);
                    if failed == name {
                        return Err(match name {
                            "recover" => RecorderStartupError::Recovery,
                            "appendable" => RecorderStartupError::Appendable,
                            "attach" => RecorderStartupError::CaptureAttach,
                            _ => RecorderStartupError::EtlResume,
                        });
                    }
                    Ok(())
                }
            };
            let recovery = make_step("recover", &order);
            let quota = Arc::clone(&order);
            let appendable = make_step("appendable", &order);
            let attach = make_step("attach", &order);
            let etl = make_step("etl", &order);
            let result = RecorderStartup::start_post_foundation(
                &config(&root),
                recovery,
                move |_| {
                    quota.lock().unwrap().push("quota");
                    if failed == "quota" {
                        Err(RecorderStartupError::Quota(
                            QuotaError::InsufficientHeadroom,
                        ))
                    } else {
                        Ok(())
                    }
                },
                appendable,
                attach,
                etl,
            );
            assert!(result.is_err());
            let steps = order.lock().unwrap().clone();
            let end = expected.iter().position(|step| *step == failed).unwrap() + 1;
            assert_eq!(steps.as_slice(), &expected[..end]);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn startup_orders_recovery_before_capture_and_splits_readiness() {
        let root = std::env::temp_dir().join(format!("chronicle-startup-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let order = Arc::new(Mutex::new(Vec::new()));
        let recovery_order = Arc::clone(&order);
        let attach_order = Arc::clone(&order);
        let startup = RecorderStartup::start(
            &config(&root),
            move || {
                recovery_order.lock().unwrap().push("recover");
                Ok(())
            },
            move || {
                attach_order.lock().unwrap().push("attach");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(*order.lock().unwrap(), ["recover", "attach"]);
        assert!(startup.capture_ready());
        assert_eq!(startup.processing_ready(), RecorderReadiness::Unknown);
        fs::remove_dir_all(root).unwrap();
    }
}
