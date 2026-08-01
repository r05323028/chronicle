//! Ordered pre-ETL recorder startup.

use crate::{
    NormalizedRecorderConfig, RecorderHealth, RecorderLease, RecorderLeaseError, RecorderLifecycle,
    RecorderLifecycleError, RecorderReadiness,
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
}

pub struct RecorderStartup {
    lease: RecorderLease,
    lifecycle: RecorderLifecycle,
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
        Ok(Self { lease, lifecycle })
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
