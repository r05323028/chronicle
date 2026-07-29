use chronicle_application::{process_fixture_wal, write_capture_to_wal};
use chronicle_canonical::PayloadRef;
use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureEventKind, CaptureFlags, ClockIdentity,
    FragmentSequenceEvidence, InMemoryCaptureSource, MonotonicTimestamp, NetworkFamily,
    PayloadDirection, PayloadFragment, RecordingScopeIdentity, SocketEvidence, SocketIdentity,
    SocketRole, TruncationMetadata, TruncationState,
};
use chronicle_common::{ConnectionKey, Endpoint, ProtocolId, SessionId, TransportProtocol};
use chronicle_protocol::{ReplayContext, VerificationStatus};
use chronicle_replay::{
    ReplayExecutor, ReplayPlanner, ReplayPolicy, TargetMap, TargetRule, TimingMode,
};
use chronicle_storage::{
    ArtifactObject, ArtifactStore, InMemoryArtifactStore, InMemoryMetadataRepository,
    MetadataRepository,
};
use std::collections::BTreeMap;
use std::fs;
use uuid::Uuid;

fn capture_events(connection: &ConnectionKey) -> Vec<CaptureEvent> {
    let first_seen = MonotonicTimestamp {
        clock: ClockIdentity {
            boot_id: "test-boot".into(),
        },
        nanoseconds: 1,
    };
    let socket = SocketIdentity {
        socket_cookie: 1,
        first_seen: first_seen.clone(),
        network_namespace: connection.network_namespace,
    };
    let scope = RecordingScopeIdentity {
        cgroup_id: 1,
        canonical_path: "/test".into(),
        namespace: None,
    };
    let payload = |sequence, direction, bytes: &[u8]| CaptureEvent {
        schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
        kind: CaptureEventKind::PayloadFragment(PayloadFragment {
            timestamp: MonotonicTimestamp {
                nanoseconds: sequence,
                ..first_seen.clone()
            },
            socket: socket.clone(),
            recording_scope: scope.clone(),
            network_family: NetworkFamily::Ipv4,
            direction,
            sequence: FragmentSequenceEvidence {
                tcp_sequence: u32::try_from(sequence).unwrap(),
                continuation_position: 0,
            },
            payload: bytes.to_vec(),
            truncation: TruncationMetadata {
                captured_length: u32::try_from(bytes.len()).unwrap(),
                observed_length: Some(u32::try_from(bytes.len()).unwrap()),
                state: TruncationState::Complete,
                reason: None,
            },
            flags: CaptureFlags::default(),
        }),
    };
    vec![
        CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketConnected(SocketEvidence {
                timestamp: first_seen.clone(),
                socket: socket.clone(),
                recording_scope: scope.clone(),
                network_family: NetworkFamily::Ipv4,
                local_endpoint: connection.client.clone(),
                remote_endpoint: connection.server.clone(),
                role: SocketRole::Active,
                process: None,
                observed_cgroup_id: 1,
            }),
        },
        payload(2, PayloadDirection::Egress, b"FAKE request"),
        payload(3, PayloadDirection::Ingress, b"FAKE response"),
    ]
}

#[tokio::test]
async fn fake_capture_wal_etl_storage_replay_verify() {
    let directory = std::env::temp_dir().join(format!("chronicle-test-{}", Uuid::new_v4()));
    let connection = ConnectionKey::new(
        Endpoint::new("192.0.2.10", 41_000),
        Endpoint::new("192.0.2.20", 7_001),
        TransportProtocol::Tcp,
    );
    let mut source = InMemoryCaptureSource::new(capture_events(&connection));
    let recorded = write_capture_to_wal(&mut source, &directory, 1024 * 1024).unwrap();
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let (output, _) = process_fixture_wal(
        &directory,
        recorded.recording_id,
        &registry,
        SessionId::new(),
    )
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
            host: Some("192.0.2.20".into()),
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
    assert_eq!(results.results.len(), 1);
    assert_eq!(
        results.results[0].verification.as_ref().unwrap().status,
        VerificationStatus::Passed
    );
    assert!(matches!(
        canonical_connection.operations[0].recorded_response,
        Some(PayloadRef::Inline { .. })
    ));

    fs::remove_dir_all(directory).unwrap();
}
