use super::*;
use crate::{
    EpochConfig, EtlConfig, FilesystemDomainConfig, LogLevel, LoggingConfig, RecorderConfigV1,
    RecorderScopeConfig, RetentionMode, SegmentConfig, ShutdownConfig, StoreBackend, StoreConfig,
    process_and_publish_recording_wal,
};
use chronicle_capture::FixtureCaptureSource;
use chronicle_common::{EpochId, RecordingId};
use chronicle_protocol_builtins::registry;
use chronicle_wal::{DEFAULT_SEGMENT_BYTES, GroupCommitWalWriter};
use std::fs;
use std::sync::{
    Arc,
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
            max_bytes: DEFAULT_SEGMENT_BYTES,
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

fn pressure_config(root: &std::path::Path) -> crate::NormalizedRecorderConfig {
    let mut config = config(root);
    config.domains[0].quota_bytes = 1_000_000;
    config.domains[0].minimum_free_bytes = 100_000;
    config
}

fn start_pressure_service(
    config: &crate::NormalizedRecorderConfig,
) -> ContinuousRecorderService<FixtureCaptureSource> {
    let wal = Path::new(&config.domains[0].identity.canonical_root).join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    ContinuousRecorderService::start(
        config,
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap()
}

#[test]
fn quota_pressure_is_terminal_without_cleanup_candidates_and_latches() {
    let root =
        std::env::temp_dir().join(format!("chronicle-quota-terminal-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let config = pressure_config(&root);
    let mut service = start_pressure_service(&config);
    fs::create_dir_all(root.join("store")).unwrap();
    // Durable managed usage alone exceeds the 1 MiB quota ceiling.
    fs::write(root.join("store/fill"), vec![0_u8; 1_050_000]).unwrap();
    service.poll(0).unwrap();
    assert_eq!(
        service.active_metadata.failure,
        Some(crate::MetadataCode::TerminalQuotaPressure)
    );
    assert_eq!(
        service.active_metadata.health,
        crate::RecorderHealth::Failed
    );
    assert_eq!(
        service.active_metadata.capture_readiness,
        crate::RecorderReadiness::NotReady
    );
    assert!(!service.recorder().admission_enabled());
    fs::remove_file(root.join("store/fill")).unwrap();
    service.poll(1).unwrap();
    assert_eq!(
        service.active_metadata.failure,
        Some(crate::MetadataCode::TerminalQuotaPressure)
    );
    assert_eq!(
        service.active_metadata.health,
        crate::RecorderHealth::Failed
    );
    assert!(!service.recorder().admission_enabled());
    fs::remove_dir_all(root).unwrap();
}

fn persist_test_startup_metadata(
    config: &crate::NormalizedRecorderConfig,
    quotas: &[crate::QuotaReservationAuthority],
    lifecycle: crate::RecorderLifecycleState,
    recorder_id: uuid::Uuid,
    attempt_id: uuid::Uuid,
) -> Result<(), RecorderStartupError> {
    let config_digest = config
        .stable_digest()
        .map_err(|error| RecorderStartupError::RecoveryDetail(error.to_string()))?;
    crate::write_recorder_metadata(
        &config.state_root,
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
            config_digest,
            scope: config.scope.clone(),
            boot_clock_identity: "continuous-recorder-test".into(),
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
                code: crate::MetadataCode::CrashRecovered,
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
}

struct StartProbe(Arc<AtomicBool>);

impl CaptureSource for StartProbe {
    fn start(&mut self) -> Result<(), CaptureError> {
        self.0.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn next_event(&mut self) -> Result<Option<chronicle_capture::CaptureEvent>, CaptureError> {
        Ok(None)
    }
}

#[test]
fn pending_v2_continuation_recovers_predecessor_from_catalog() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-pending-continuation-recovery-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(root.join("epochs/1")).unwrap();
    let parent_id = RecordingId::new();
    let predecessor = EpochId::new();
    let successor = EpochId::new();
    let mut catalog = crate::EpochCatalog::new(parent_id, predecessor, "epochs/0").unwrap();
    catalog.append_successor(successor, "epochs/1").unwrap();
    catalog.activate_successor(successor).unwrap();
    catalog.write_atomic(&root).unwrap();
    ContinuationDependency::pending_for_parent(Some(parent_id), predecessor, successor)
        .write_atomic(root.join("epochs/1/continuation-state.json"))
        .unwrap();

    let pending = recover_pending_continuation(&root.join("epochs/1"), successor, None)
        .unwrap()
        .unwrap();
    assert_eq!(pending.old_epoch_id, predecessor);
    assert_eq!(pending.successor_epoch_id, successor);
    assert_eq!(pending.old_epoch_ordinal, 0);
    assert_eq!(pending.old_wal_directory, root.join("epochs/0"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn v2_rollover_publishes_pending_continuation_before_successor_etl() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-v2-continuation-rollover-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let config = config(&root);
    let wal_root = Path::new(&config.domains[0].identity.canonical_root).join("wal");
    let old_recording_id = RecordingId::new();
    let writer =
        GroupCommitWalWriter::create(&wal_root, old_recording_id, DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config,
        &wal_root,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    let parent_id = RecordingId::new();
    service.set_parent_id(parent_id).unwrap();
    for tick in 0..20 {
        service.poll(tick * 250).unwrap();
    }
    assert!(service.recorder().ingest().commit_boundary().is_some());

    let successor = EpochId::new();
    let next = wal_root.join("epochs").join(format!("1-{successor}"));
    fs::create_dir_all(&next).unwrap();
    service
        .reserve_successor_quota(&next, config.epoch.max_bytes)
        .unwrap();
    service
        .begin_rollover_transition(parent_id, &next, successor, config.epoch.max_bytes)
        .unwrap();
    let mut catalog =
        crate::EpochCatalog::new(parent_id, EpochId::from_uuid(old_recording_id.0), ".").unwrap();
    let successor_path = format!("epochs/1-{successor}");
    catalog
        .append_successor(successor, &successor_path)
        .unwrap();
    catalog.write_atomic(&wal_root).unwrap();
    let next_writer = GroupCommitWalWriter::create(
        &next,
        RecordingId::from_uuid(successor.as_uuid()),
        DEFAULT_SEGMENT_BYTES,
        1,
        0,
    )
    .unwrap();
    let outcome = root.join("epoch-outcome.json");
    fs::write(&outcome, b"{}\n").unwrap();
    service
        .rollover_to(&next, next_writer, 10_000, &outcome)
        .unwrap();
    catalog.activate_successor(successor).unwrap();
    catalog.write_atomic(&wal_root).unwrap();
    service.complete_rollover_transition().unwrap();

    service.poll(10_250).unwrap();
    let dependency =
        ContinuationDependency::load(next.join(EPOCH_CONTINUATION_STATE_FILE)).unwrap();
    assert_eq!(dependency.state, ContinuationState::Ready);
    assert!(next.join(EPOCH_CONTINUATION_IN_FILE).exists());
    assert!(wal_root.join(EPOCH_CONTINUATION_OUT_FILE).exists());
    assert!(service.pending_continuation.is_none());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rollover_requires_prepared_transition_before_successor_mutation() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-rollover-journal-boundary-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let config = config(&root);
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config,
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    let next = root.join("next");
    let next_writer =
        GroupCommitWalWriter::create(&next, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let result = service.rollover_to(&next, next_writer, 1, root.join("outcome.json"));
    assert!(
        matches!(result, Err(ContinuousRecorderError::Metadata(message)) if message.contains("transition missing"))
    );
    assert!(
        crate::load_transition(&config.state_root)
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quota_denial_leaves_no_rollover_journal_or_successor() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-rollover-quota-denial-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let config = config(&root);
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let service = ContinuousRecorderService::start(
        &config,
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    let successor = root.join("successor");
    let result = service.reserve_successor_quota(&successor, u64::MAX);
    assert!(result.is_err());
    assert!(!successor.exists());
    assert!(
        crate::load_transition(&config.state_root)
            .unwrap()
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn rollover_failure_retains_prepared_evidence_for_restart_recovery() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-rollover-failure-boundary-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let config = config(&root);
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config,
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    for tick in 0..20 {
        service.poll(tick * 250).unwrap();
    }
    let old_metadata = fs::read(wal.join("recording.json")).unwrap();
    let parent_id = service.active_metadata.parent_id.unwrap();
    let next = std::path::PathBuf::from(&config.domains[0].identity.canonical_root).join("next");
    let next_epoch_id = EpochId::new();
    service
        .reserve_successor_quota(&next, config.epoch.max_bytes)
        .unwrap();
    service
        .begin_rollover_transition(parent_id, &next, next_epoch_id, config.epoch.max_bytes)
        .unwrap();
    let next_writer = GroupCommitWalWriter::create(
        &next,
        RecordingId::from_uuid(next_epoch_id.as_uuid()),
        DEFAULT_SEGMENT_BYTES,
        1,
        0,
    )
    .unwrap();
    let outcome_directory = root.join("outcome-directory");
    fs::create_dir_all(&outcome_directory).unwrap();
    let result = service.rollover_to(&next, next_writer, 1, &outcome_directory);
    assert!(result.is_err());
    assert_eq!(
        crate::load_transition(&config.state_root)
            .unwrap()
            .unwrap()
            .phase,
        crate::RolloverTransitionPhase::SuccessorCreated
    );
    assert!(next.exists());
    assert_eq!(fs::read(wal.join("recording.json")).unwrap(), old_metadata);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_startup_rejects_non_authoritative_epoch_before_capture() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-authoritative-epoch-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let config = config(&root);
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let started = Arc::new(AtomicBool::new(false));
    let recorder = RecorderOrchestrator::new(
        StartProbe(Arc::clone(&started)),
        crate::RecordingIngest::new(writer),
    );
    let recorder_id = uuid::Uuid::new_v4();
    let attempt_id = uuid::Uuid::new_v4();
    let startup = RecorderStartup::prepare_foundation_with_metadata(
        &config,
        |quotas, lifecycle| {
            persist_test_startup_metadata(&config, quotas, lifecycle, recorder_id, attempt_id)
        },
        |_| Ok(()),
        |_| Ok(()),
    )
    .unwrap();
    let result = ContinuousRecorderService::start_with_prepared_startup(
        &config,
        &wal,
        1,
        recorder,
        startup,
        || Ok(()),
        || Ok(()),
    );
    assert!(matches!(
        result,
        Err(ContinuousRecorderError::Startup(
            RecorderStartupError::RecoveryDetail(_)
        ))
    ));
    assert!(!started.load(Ordering::SeqCst));
    let metadata = crate::load_recorder_metadata(&config.state_root).unwrap();
    assert_eq!(metadata.lifecycle, crate::RecorderLifecycleState::Failed);
    assert_eq!(metadata.capture_readiness, RecorderReadiness::NotReady);
    assert_eq!(metadata.processing_readiness, RecorderReadiness::NotReady);
    assert_eq!(metadata.failure, Some(crate::MetadataCode::RecoveryFailure));
    assert_eq!(metadata.recorder_id, recorder_id);
    assert_eq!(metadata.attempt_id, attempt_id);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_publication_faults_reuse_immutable_output_after_restart_boundary() {
    let root = std::env::temp_dir().join(format!(
        "chronicle-publication-fault-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config(&root),
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    service.set_checkpoint_fault(chronicle_etl::CheckpointFault::BeforeCheckpoint);
    let output_dir = root.join("store/artifacts/deltas");
    let output_files = || {
        fs::read_dir(&output_dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .flat_map(|directory| fs::read_dir(directory.path()).into_iter().flatten())
            .filter_map(Result::ok)
            .filter_map(|entry| {
                fs::read(entry.path())
                    .ok()
                    .map(|bytes| (entry.path(), bytes))
            })
            .collect::<Vec<_>>()
    };
    let mut orphaned = Vec::new();
    for tick in 0..40 {
        service.poll(tick * 250).unwrap();
        orphaned = output_files();
        if !orphaned.is_empty() && !wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists() {
            break;
        }
    }
    assert!(!orphaned.is_empty());
    assert!(!wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
    let recording_id = service.recorder().ingest().recording_id();
    drop(service);
    let writer = chronicle_wal::prepare_group_commit_reopen(
        &wal,
        recording_id,
        DEFAULT_SEGMENT_BYTES,
        chronicle_wal::DEFAULT_MAX_WAL_BYTES,
        0,
    )
    .unwrap()
    .apply()
    .unwrap()
    .0;
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config(&root),
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    for tick in 40..80 {
        service.poll(tick * 250).unwrap();
    }
    assert!(wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
    let recovered = output_files();
    for (path, bytes) in orphaned {
        assert_eq!(
            recovered
                .iter()
                .find(|(candidate, _)| *candidate == path)
                .map(|(_, value)| value),
            Some(&bytes)
        );
    }
    drop(service);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn finalize_deduplicates_repeated_operations_after_restart_boundary() {
    // A restart replays the same capture source into the same epoch, so the
    // assembler holds identical requests twice. Deterministic operation ids
    // collide; finalization must deduplicate like incremental batches do.
    let root =
        std::env::temp_dir().join(format!("chronicle-finalize-dedup-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config(&root),
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    for tick in 0..12 {
        service.poll(tick * 250).unwrap();
    }
    assert!(wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE).exists());
    let recording_id = service.recorder().ingest().recording_id();
    drop(service);
    let writer = chronicle_wal::prepare_group_commit_reopen(
        &wal,
        recording_id,
        DEFAULT_SEGMENT_BYTES,
        chronicle_wal::DEFAULT_MAX_WAL_BYTES,
        0,
    )
    .unwrap()
    .apply()
    .unwrap()
    .0;
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config(&root),
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    for tick in 12..24 {
        service.poll(tick * 250).unwrap();
    }
    // Rollover finalizes the epoch whose reconstruction contains the
    // replayed identical requests; deduplication must keep it publishable.
    service
        .rollover(6000, root.join("epoch-outcomes.json"))
        .unwrap();
    service.finalize_incremental_session().unwrap();
    let session = chronicle_storage::FilesystemSessionStore::new(config(&root).store_root)
        .hydrate(SessionId(recording_id.0))
        .unwrap();
    let ids = session
        .connections
        .iter()
        .flat_map(|connection| connection.operations.iter().map(|op| op.id))
        .collect::<Vec<_>>();
    assert_eq!(
        ids.len(),
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
    session.validate().unwrap();
    drop(service);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_shot_checkpoint_persists_manifest_checksum() {
    // Regression: the persisted recording-local checkpoint must carry the
    // real manifest checksum (publication + verification happen inside the
    // ETL transaction before the checkpoint bytes are produced), not the
    // pre-publication placeholder.
    let root = std::env::temp_dir().join(format!(
        "chronicle-one-shot-checkpoint-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    let wal = root.join("wal");
    fs::create_dir_all(&wal).unwrap();
    crate::production_wal_from_fixture(
        &wal,
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ),
    )
    .unwrap();
    let output = root.join("output");
    let registry = registry().unwrap();
    let published = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
    let checkpoint: crate::RecordingEtlCheckpoint =
        serde_json::from_slice(&fs::read(wal.join("etl-checkpoint.json")).unwrap()).unwrap();
    assert!(
        !checkpoint.manifest_checksum.is_empty(),
        "persisted checkpoint must carry the real manifest checksum"
    );
    let manifest: serde_json::Value = serde_json::from_slice(
        &fs::read(
            output
                .join("sessions")
                .join(published.session_id.to_string())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        checkpoint.manifest_checksum,
        manifest["session_checksum"].as_str().unwrap()
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn three_epoch_fixture_has_one_etl_publication_and_replayable_output() {
    let root = std::env::temp_dir().join(format!("chronicle-three-epoch-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let wal = root.join("wal");
    let output = root.join("output");
    let recording_id = RecordingId::new();
    let writer =
        GroupCommitWalWriter::create(&wal, recording_id, DEFAULT_SEGMENT_BYTES, 1, 0).unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config(&root),
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();
    assert_eq!(
        crate::load_recorder_metadata(config(&root).state_root)
            .unwrap()
            .lifecycle,
        crate::RecorderLifecycleState::Running
    );
    let mut accepted = 0;
    while accepted < 4 {
        if matches!(
            service.poll(accepted * 10).unwrap(),
            RecorderPoll::Accepted(_)
        ) {
            accepted += 1;
            if accepted == 1 || accepted == 2 {
                service
                    .rollover(accepted * 10 + 1, root.join("epoch-outcomes.json"))
                    .unwrap();
            }
        }
    }
    let active = crate::load_recorder_metadata(config(&root).state_root).unwrap();
    assert_eq!(active.current_epoch.unwrap().ordinal, 2);
    assert_eq!(active.previous_epoch.unwrap().ordinal, 1);
    assert_eq!(active.counters.captured_records, 2);
    assert_eq!(active.counters.committed_records, 0);
    let result = service
        .shutdown(
            crate::ShutdownReason::SourceCompleted,
            100,
            Duration::from_secs(1),
        )
        .unwrap();
    assert_eq!(result.status, crate::RecordingStatus::Completed);
    let snapshot = service.committed_snapshot().unwrap();
    assert!(snapshot.snapshot.committed_record_count > 0);
    assert_eq!(
        snapshot.envelopes.last().unwrap().sequence,
        snapshot.snapshot.marker_sequence
    );
    let stopped = crate::load_recorder_metadata(config(&root).state_root).unwrap();
    assert_eq!(stopped.lifecycle, crate::RecorderLifecycleState::Stopped);
    assert_eq!(
        stopped.counters.committed_records,
        result.counters.committed.records
    );
    let incremental_session = SessionId(service.recorder().ingest().recording_id().0);
    service.finalize_incremental_session().unwrap();
    assert!(
        chronicle_storage::FilesystemSessionStore::new(config(&root).store_root)
            .hydrate(incremental_session)
            .is_ok()
    );
    drop(service);
    let registry = registry().unwrap();
    let published = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
    let inspection = crate::inspect_session(&output, published.session_id).unwrap();
    assert!(inspection.integrity_valid);
    assert_eq!(
        inspection.replayability,
        chronicle_replay::Replayability::FullyReplayable
    );
    let repeated = process_and_publish_recording_wal(&wal, &output, &registry).unwrap();
    assert_eq!(published.session_id, repeated.session_id);
    assert!(repeated.already_published);
    assert!(root.join("epoch-outcomes.json").exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn incremental_batches_advance_one_commit_range_at_a_time() {
    let root = std::env::temp_dir().join(format!("chronicle-bounded-etl-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let mut config = config(&root);
    config.etl.batch_records = 1;
    config.etl.max_lag_records = 4;
    let wal = root.join("wal");
    let writer =
        GroupCommitWalWriter::create(&wal, RecordingId::new(), DEFAULT_SEGMENT_BYTES, 1, 0)
            .unwrap();
    let source = FixtureCaptureSource::from_json(include_bytes!(
        "../../../../fixtures/http/basic-session.json"
    ))
    .unwrap();
    let recorder = RecorderOrchestrator::new(source, crate::RecordingIngest::new(writer));
    let mut service = ContinuousRecorderService::start(
        &config,
        &wal,
        recorder,
        |_| Ok(()),
        |_| Ok(()),
        || Ok(()),
        || Ok(()),
    )
    .unwrap();

    let mut output_counts = Vec::new();
    for tick in [0, 250, 500, 750, 1_000] {
        service.poll(tick).unwrap();
        let checkpoint_path = wal.join(INCREMENTAL_ETL_CHECKPOINT_FILE);
        if checkpoint_path.exists() {
            let checkpoint = IncrementalEtlCheckpoint::load(&checkpoint_path).unwrap();
            output_counts.push(checkpoint.outputs.len());
        }
    }
    assert!(output_counts.windows(2).any(|pair| pair[1] > pair[0]));
    assert!(output_counts.iter().max().copied().unwrap_or(0) >= 2);
    let metadata = crate::load_recorder_metadata(config.state_root).unwrap();
    assert_eq!(metadata.capture_readiness, RecorderReadiness::Ready);
    service
        .shutdown(
            crate::ShutdownReason::SourceCompleted,
            2_000,
            Duration::from_secs(1),
        )
        .unwrap();
    fs::remove_dir_all(root).unwrap();
}
