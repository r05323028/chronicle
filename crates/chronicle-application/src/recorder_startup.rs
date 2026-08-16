//! Ordered pre-ETL recorder startup.

use crate::{
    MetadataCode, NormalizedRecorderConfig, QuotaError, QuotaReservationAuthority, RecorderHealth,
    RecorderLease, RecorderLeaseError, RecorderLifecycle, RecorderLifecycleError,
    RecorderLifecycleState, RecorderReadiness, load_recorder_metadata, write_recorder_metadata,
};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum RecorderStartupError {
    #[error(transparent)]
    Lease(#[from] RecorderLeaseError),
    #[error(transparent)]
    Lifecycle(#[from] RecorderLifecycleError),
    #[error("recorder startup recovery failed")]
    Recovery,
    #[error("recorder startup recovery failed: {0}")]
    RecoveryDetail(String),
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
    state_root: std::path::PathBuf,
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
            state_root: config.state_root.clone(),
        })
    }

    /// Acquires lease and quota authority before recovery. Recovery includes
    /// state, WAL, manifest, checkpoint, and cleanup reconciliation; ETL is
    /// resumed only after capture is attached and appendability is durable.
    pub(crate) fn prepare_foundation(
        config: &NormalizedRecorderConfig,
        recover_foundation: impl FnOnce(
            &[QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(&[QuotaReservationAuthority]) -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        Self::prepare_foundation_with_metadata(
            config,
            |_, _| Ok(()),
            recover_foundation,
            reserve_quota,
        )
    }

    pub(crate) fn prepare_foundation_with_metadata(
        config: &NormalizedRecorderConfig,
        mut persist_metadata: impl FnMut(
            &[QuotaReservationAuthority],
            RecorderLifecycleState,
        ) -> Result<(), RecorderStartupError>,
        recover_foundation: impl FnOnce(
            &[QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(&[QuotaReservationAuthority]) -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        let lease = RecorderLease::acquire(config)?;
        let mut lifecycle = RecorderLifecycle::new();
        let quotas = build_quota_authorities(config)?;
        if let Err(error) = persist_metadata(&quotas, RecorderLifecycleState::Starting) {
            lifecycle.fail();
            persist_startup_failure_at(&config.state_root, failure_code(&error));
            return Err(error);
        }
        lifecycle.begin_recovery()?;
        if let Err(error) = persist_metadata(&quotas, RecorderLifecycleState::Recovering) {
            lifecycle.fail();
            persist_startup_failure_at(&config.state_root, failure_code(&error));
            return Err(error);
        }
        if quotas
            .iter()
            .any(|quota| quota.available_bytes() < quota.quota().minimum_free_bytes)
        {
            lifecycle.fail();
            persist_startup_failure_at(&config.state_root, MetadataCode::QuotaPressure);
            return Err(RecorderStartupError::Quota(
                QuotaError::InsufficientHeadroom,
            ));
        }
        if let Err(error) = reserve_quota(&quotas) {
            lifecycle.fail();
            persist_startup_failure_at(&config.state_root, failure_code(&error));
            return Err(error);
        }
        if let Err(error) = recover_foundation(&quotas) {
            lifecycle.fail();
            persist_startup_failure_at(&config.state_root, failure_code(&error));
            return Err(error);
        }
        Ok(Self {
            lease,
            lifecycle,
            quotas,
            state_root: config.state_root.clone(),
        })
    }

    pub(crate) fn finish(
        mut self,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        attach_capture: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        if let Err(error) = establish_appendable() {
            self.lifecycle.fail();
            persist_startup_failure_at(&self.state_root, failure_code(&error));
            return Err(error);
        }
        if let Err(error) = attach_capture() {
            self.lifecycle.fail();
            persist_startup_failure_at(&self.state_root, failure_code(&error));
            return Err(error);
        }
        if let Err(error) = resume_etl() {
            self.lifecycle.fail();
            persist_startup_failure_at(&self.state_root, failure_code(&error));
            return Err(error);
        }
        self.lifecycle.running(RecorderReadiness::Ready)?;
        Ok(self)
    }

    pub fn start_post_foundation(
        config: &NormalizedRecorderConfig,
        recover_foundation: impl FnOnce(
            &[QuotaReservationAuthority],
        ) -> Result<(), RecorderStartupError>,
        reserve_quota: impl FnOnce(&[QuotaReservationAuthority]) -> Result<(), RecorderStartupError>,
        establish_appendable: impl FnOnce() -> Result<(), RecorderStartupError>,
        attach_capture: impl FnOnce() -> Result<(), RecorderStartupError>,
        resume_etl: impl FnOnce() -> Result<(), RecorderStartupError>,
    ) -> Result<Self, RecorderStartupError> {
        Self::prepare_foundation(config, recover_foundation, reserve_quota)?.finish(
            establish_appendable,
            attach_capture,
            resume_etl,
        )
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

pub(crate) fn build_quota_authorities(
    config: &NormalizedRecorderConfig,
) -> Result<Vec<QuotaReservationAuthority>, QuotaError> {
    config
        .domains
        .iter()
        .map(|domain| {
            let domain_root = Path::new(&domain.identity.canonical_root);
            let managed_roots = [config.state_root.as_path(), config.store_root.as_path()]
                .into_iter()
                .filter(|root| root.starts_with(domain_root))
                .collect::<Vec<_>>();
            QuotaReservationAuthority::from_filesystem_with_roots(
                &domain.identity.canonical_root,
                &managed_roots,
                domain.quota_bytes,
                domain.minimum_free_bytes,
            )
        })
        .collect()
}

fn failure_code(error: &RecorderStartupError) -> MetadataCode {
    match error {
        RecorderStartupError::Recovery | RecorderStartupError::RecoveryDetail(_) => {
            MetadataCode::RecoveryFailure
        }
        RecorderStartupError::Quota(_) => MetadataCode::QuotaPressure,
        RecorderStartupError::Appendable => MetadataCode::AppendableFailure,
        RecorderStartupError::CaptureAttach => MetadataCode::CaptureAttachFailure,
        RecorderStartupError::EtlResume => MetadataCode::EtlResumeFailure,
        RecorderStartupError::Lease(_) | RecorderStartupError::Lifecycle(_) => {
            MetadataCode::StorageFailure
        }
    }
}

fn persist_startup_failure_at(state_root: &Path, code: MetadataCode) {
    let Ok(mut metadata) = load_recorder_metadata(state_root) else {
        return;
    };
    metadata.lifecycle = RecorderLifecycleState::Failed;
    metadata.capture_readiness = RecorderReadiness::NotReady;
    metadata.processing_readiness = RecorderReadiness::NotReady;
    metadata.health = RecorderHealth::Failed;
    metadata.failure = Some(code);
    metadata.updated_at_unix_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let _ = write_recorder_metadata(state_root, &metadata);
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
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    };

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
    fn competing_lease_blocks_wal_establishment_before_callback() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-startup-lease-boundary-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let wal = root.join("wal");
        let wal_for_callback = wal.clone();
        let _owner = RecorderLease::acquire(&config).unwrap();
        let attempted = Arc::new(AtomicBool::new(false));
        let attempted_callback = Arc::clone(&attempted);
        let result = RecorderStartup::start(
            &config,
            move || {
                attempted_callback.store(true, Ordering::SeqCst);
                chronicle_wal::GroupCommitWalWriter::create(
                    &wal_for_callback,
                    chronicle_common::RecordingId::new(),
                    chronicle_wal::DEFAULT_SEGMENT_BYTES,
                    1,
                    0,
                )
                .map(|_| ())
                .map_err(|_| RecorderStartupError::Appendable)
            },
            || Ok(()),
        );
        assert!(matches!(result, Err(RecorderStartupError::Lease(_))));
        assert!(!attempted.load(Ordering::SeqCst));
        assert!(!wal.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_foundation_startup_orders_quota_recovery_append_capture_etl() {
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
            move |_quotas: &[QuotaReservationAuthority]| {
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
            ["quota", "recover", "appendable", "attach", "etl"]
        );
        assert!(startup.capture_ready());
        assert_eq!(startup.processing_ready(), RecorderReadiness::Ready);
        assert_eq!(startup.health(), RecorderHealth::Healthy);
        assert_eq!(startup.quota_authorities().len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn post_foundation_failure_stops_before_later_steps() {
        let expected = ["quota", "recover", "appendable", "attach", "etl"];
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
            let recovery = {
                let order = Arc::clone(&order);
                move |_quotas: &[QuotaReservationAuthority]| {
                    order.lock().unwrap().push("recover");
                    if failed == "recover" {
                        Err(RecorderStartupError::Recovery)
                    } else {
                        Ok(())
                    }
                }
            };
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
    fn startup_errors_preserve_typed_failure_codes() {
        assert_eq!(
            failure_code(&RecorderStartupError::Recovery),
            MetadataCode::RecoveryFailure
        );
        assert_eq!(
            failure_code(&RecorderStartupError::Appendable),
            MetadataCode::AppendableFailure
        );
        assert_eq!(
            failure_code(&RecorderStartupError::CaptureAttach),
            MetadataCode::CaptureAttachFailure
        );
        assert_eq!(
            failure_code(&RecorderStartupError::EtlResume),
            MetadataCode::EtlResumeFailure
        );
        assert_eq!(
            failure_code(&RecorderStartupError::Quota(
                QuotaError::InsufficientHeadroom
            )),
            MetadataCode::QuotaPressure
        );
    }

    #[test]
    fn startup_persists_starting_and_recovering_before_recovery_failure() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-startup-metadata-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let config = config(&root);
        let states = Arc::new(Mutex::new(Vec::new()));
        let state_order = Arc::clone(&states);
        let state_root = config.state_root.clone();
        let scope = config.scope.clone();
        let config_digest = config.stable_digest().unwrap();
        let recorder_id = uuid::Uuid::new_v4();
        let attempt_id = uuid::Uuid::new_v4();
        let result = RecorderStartup::prepare_foundation_with_metadata(
            &config,
            move |quotas, lifecycle| {
                state_order.lock().unwrap().push(lifecycle);
                crate::write_recorder_metadata(
                    &state_root,
                    &crate::RecorderMetadataV1 {
                        version: crate::RECORDER_METADATA_SCHEMA_VERSION,
                        recorder_id,
                        parent_id: None,
                        current_epoch_id: None,
                        finalized_epoch_count: 0,
                        continuation_in: None,
                        continuation_out: None,
                        retention_state: None,
                        last_rollover: None,
                        attempt_id,
                        config_digest: config_digest.clone(),
                        scope: scope.clone(),
                        boot_clock_identity: "startup-test".into(),
                        lifecycle,
                        capture_readiness: RecorderReadiness::NotReady,
                        processing_readiness: RecorderReadiness::Unknown,
                        health: RecorderHealth::Degraded,
                        current_epoch: None,
                        previous_epoch: None,
                        active_segment: None,
                        commit: None,
                        incremental_checkpoint: None,
                        lag: crate::LagSummary {
                            records: 0,
                            bytes: 0,
                            age_seconds: 0,
                        },
                        quota: quotas
                            .iter()
                            .map(|quota| crate::QuotaStatus {
                                domain: quota.quota().domain.clone(),
                                quota_bytes: quota.quota().quota_bytes,
                                free_bytes: quota.available_bytes(),
                                reserved_bytes: quota.reserved_bytes().unwrap(),
                            })
                            .collect(),
                        counters: crate::RecorderCounters::default(),
                        recovery: Some(crate::RecoverySummary {
                            code: MetadataCode::CrashRecovered,
                            repaired_tail: false,
                        }),
                        shutdown: None,
                        failure: None,
                        updated_at_unix_seconds: 0,
                        checksum: String::new(),
                    },
                )
                .map(|_| ())
                .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))
            },
            move |_| Err(RecorderStartupError::Recovery),
            |_| Ok(()),
        );
        assert!(matches!(result, Err(RecorderStartupError::Recovery)));
        assert_eq!(
            *states.lock().unwrap(),
            [
                RecorderLifecycleState::Starting,
                RecorderLifecycleState::Recovering
            ]
        );
        let metadata = crate::load_recorder_metadata(&config.state_root).unwrap();
        assert_eq!(metadata.lifecycle, RecorderLifecycleState::Failed);
        assert_eq!(metadata.recorder_id, recorder_id);
        assert_eq!(metadata.attempt_id, attempt_id);
        assert_eq!(metadata.capture_readiness, RecorderReadiness::NotReady);
        assert_eq!(metadata.processing_readiness, RecorderReadiness::NotReady);
        assert_eq!(metadata.health, RecorderHealth::Failed);
        assert_eq!(metadata.failure, Some(MetadataCode::RecoveryFailure));
        fs::remove_dir_all(root).unwrap();
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
