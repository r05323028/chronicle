use chronicle_canonical::PayloadRef;
use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, CaptureSource, InMemoryCaptureSource,
    encode_event,
};
use chronicle_common::{
    ConnectionKey, Direction, Endpoint, ProtocolId, SessionId, TransportProtocol,
};
use chronicle_etl::EtlPipeline;
use chronicle_protocol::{ReplayContext, VerificationStatus};
use chronicle_replay::{
    ReplayExecutor, ReplayPlanner, ReplayPolicy, TargetMap, TargetRule, TimingMode,
};
use chronicle_session::SessionLimits;
use chronicle_storage::{
    ArtifactObject, ArtifactStore, InMemoryArtifactStore, InMemoryMetadataRepository,
    MetadataRepository,
};
use chronicle_wal::{
    RecordKind, SegmentedWalWriter, WalReader, WalRecord, WalWriter, segment_file_name,
};
use std::collections::BTreeMap;
use std::fs::{self, File};
use time::OffsetDateTime;
use uuid::Uuid;

fn capture_event(
    connection: &ConnectionKey,
    sequence: u64,
    direction: Direction,
    payload: &[u8],
) -> CaptureEvent {
    CaptureEvent {
        schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
        monotonic_sequence: sequence,
        wall_time: Some(
            OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(i64::try_from(sequence).unwrap()),
        ),
        connection: connection.clone(),
        direction,
        payload: payload.to_vec(),
        process: None,
        container: None,
        file_descriptor: Some(7),
        truncated: false,
        flags: CaptureFlags::default(),
    }
}

#[tokio::test]
async fn fake_capture_wal_etl_storage_replay_verify() {
    let directory = std::env::temp_dir().join(format!("chronicle-test-{}", Uuid::new_v4()));
    let connection = ConnectionKey::new(
        Endpoint::new("captured-client", 41_000),
        Endpoint::new("production.invalid", 7_001),
        TransportProtocol::Tcp,
    );
    let mut source = InMemoryCaptureSource::new([
        capture_event(&connection, 1, Direction::ClientToServer, b"FAKE request"),
        capture_event(&connection, 2, Direction::ServerToClient, b"FAKE response"),
    ]);
    let mut writer = SegmentedWalWriter::create(&directory, 1024 * 1024).unwrap();
    while let Some(event) = source.next_event().unwrap() {
        writer
            .append(&WalRecord {
                kind: RecordKind::CaptureEvent,
                flags: u16::try_from(event.flags.0).unwrap(),
                sequence: event.monotonic_sequence,
                payload: encode_event(&event).unwrap(),
            })
            .unwrap();
    }
    writer.flush().unwrap();

    let file = File::open(directory.join(segment_file_name(1))).unwrap();
    let mut reader = WalReader::new(file, 1);
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let output = EtlPipeline::new(SessionLimits::default())
        .process(&mut reader, &registry, SessionId::new())
        .unwrap();
    assert!(output.issues.is_empty());
    assert_eq!(output.session.connections.len(), 1);
    assert_eq!(output.session.connections[0].protocol.as_str(), "fake");
    assert_eq!(output.session.connections[0].operations.len(), 1);

    let metadata = InMemoryMetadataRepository::default();
    metadata.save_session(&output.session).await.unwrap();
    let stored = metadata.load_session(output.session.id).await.unwrap();
    assert_eq!(stored, output.session);

    let artifacts = InMemoryArtifactStore::default();
    artifacts
        .put(ArtifactObject {
            key: "fake/raw-response".into(),
            checksum: "fixture-checksum".into(),
            bytes: b"FAKE response".to_vec(),
            content_type: Some("application/x-chronicle-fake".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        artifacts.get("fake/raw-response").await.unwrap().bytes,
        b"FAKE response"
    );

    let canonical_connection = &stored.connections[0];
    let targets = TargetMap {
        rules: vec![TargetRule {
            protocol: Some(ProtocolId::new("fake")),
            host: Some("production.invalid".into()),
            port: Some(7_001),
            connection_id: Some(canonical_connection.id),
            target: Endpoint::new("replay.invalid", 17_001),
        }],
    };
    let policy = ReplayPolicy {
        dry_run: false,
        allow_reads: true,
        ..ReplayPolicy::default()
    };
    let plan = ReplayPlanner::plan(&stored, &targets, &policy, TimingMode::Asap).unwrap();
    let contexts = BTreeMap::from([(ProtocolId::new("fake"), ReplayContext::default())]);
    let results = ReplayExecutor::new(&registry)
        .execute(&plan, &contexts)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].verification.status, VerificationStatus::Passed);
    assert!(matches!(
        canonical_connection.operations[0].recorded_response,
        Some(PayloadRef::Inline { .. })
    ));

    fs::remove_dir_all(directory).unwrap();
}
