use chronicle_application::{
    CooperativeNativeSource, NativeHttpBoundaryAdapter, RecordingMetadata, RecordingStatus,
    ShutdownReason, inspect_session, process_and_publish_recording_wal,
    reconcile_recording_metadata, write_capture_to_wal, write_recording_metadata,
};
use chronicle_canonical::{
    CanonicalConnection, CanonicalSession, Completeness, SourceConnectionGeneration,
    SourceMetadata, SourceProvenance, TimelineEntry, WalByteRange,
};
use chronicle_capture::FixtureCaptureSource;
use chronicle_common::{
    ConnectionId, Direction, Endpoint, EpochId, OperationId, ProtocolId, RecordingId, SessionId,
};
use chronicle_etl::{
    NativeBoundaryIndex, NativeObservationDiagnostic, NativeObservationDiagnosticKind,
    NativeOperationBoundaryReceipt, NativeSourceLimits, compose_correlation_with_native,
    stamp_epoch_operation_provenance,
};
use chronicle_protocol::{DecodedFrame, ProtocolCanonicalizer, ProtocolDecoder, ProtocolStream};
use chronicle_protocol_builtins::http::Canonicalizer;
use chronicle_storage::FilesystemSessionStore;
use chronicle_wal::{DEFAULT_MAX_RECORD_BYTES, scan_wal, segment_file_name};
use serde_json::json;
use std::collections::BTreeMap;
use std::fs;
use uuid::Uuid;

fn message(
    kind: &str,
    sequence: u64,
    method: Option<&str>,
    target: Option<&str>,
    status: Option<u16>,
) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "kind": kind,
        "sequence": sequence,
        "method": method,
        "target": target,
        "status": status,
        "reason": status.map(|_| vec![79, 75]),
        "headers": [],
        "body": [],
        "pipeline_depth": 1,
        "orphan_response": false,
        "warnings": [],
        "connection_generation": null,
        "provenance": [],
        "missing_payload_provenance": false,
    }))
    .unwrap()
}

fn range(sequence: u64, direction: Direction) -> WalByteRange {
    WalByteRange {
        segment_ordinal: 0,
        segment_first_sequence: 1,
        frame_byte_offset: sequence,
        wal_sequence: sequence,
        direction,
        payload_byte_offset: 0,
        payload_byte_length: 1,
        gap_before: false,
    }
}

fn session_from_http_canonicalizer() -> (CanonicalSession, RecordingId, EpochId) {
    session_from_http_requests(
        &[(1, "/parent"), (3, "/child")],
        SessionId::from_uuid(Uuid::from_u128(102)),
    )
}

#[allow(clippy::too_many_lines)] // Keep protocol-boundary fixture construction visible.
fn session_from_http_requests(
    requests: &[(u64, &str)],
    session_id: SessionId,
) -> (CanonicalSession, RecordingId, EpochId) {
    assert!(!requests.is_empty());
    let canonicalizer = Canonicalizer::new();
    let mut frames = Vec::with_capacity(requests.len() * 2);
    for &(sequence, target) in requests {
        frames.push((
            Direction::ClientToServer,
            sequence,
            message("Request", sequence, Some("GET"), Some(target), None),
        ));
        frames.push((
            Direction::ServerToClient,
            sequence + 1,
            message("Response", sequence + 1, None, None, Some(200)),
        ));
    }
    let frames = frames
        .into_iter()
        .map(|(direction, sequence, payload)| DecodedFrame {
            direction,
            sequence,
            payload,
            attributes: BTreeMap::new(),
            connection_generation: Some(SourceConnectionGeneration::Fixture),
            provenance: vec![range(sequence, direction)],
            missing_payload_provenance: false,
        })
        .collect();
    let stream = ProtocolStream {
        started_at: None,
        chunks: Vec::new(),
        truncated: false,
    };
    let mut operations = canonicalizer.canonicalize(&stream, frames).unwrap();
    assert_eq!(operations.len(), requests.len());
    for (index, operation) in operations.iter_mut().enumerate() {
        let request_sequence = requests[index].0;
        operation.operation.provenance.connection_generation =
            Some(SourceConnectionGeneration::Fixture);
        operation.operation.provenance.wal_ranges = vec![
            range(request_sequence, Direction::ClientToServer),
            range(request_sequence + 1, Direction::ServerToClient),
        ];
    }
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let connection_id = ConnectionId::from_uuid(Uuid::from_u128(103));
    let first_sequence = requests[0].0;
    let last_sequence = requests[requests.len() - 1].0 + 1;
    let old_ids: Vec<OperationId> = operations.iter().map(|operation| operation.id).collect();
    let mut session = CanonicalSession {
        schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
        id: session_id,
        started_at: time::OffsetDateTime::UNIX_EPOCH,
        ended_at: Some(time::OffsetDateTime::UNIX_EPOCH),
        source: SourceMetadata::default(),
        source_provenance: SourceProvenance::default(),
        connections: vec![CanonicalConnection {
            id: connection_id,
            protocol: ProtocolId::new("http/1.1"),
            client: Endpoint::new("client", 1),
            server: Endpoint::new("server", 2),
            attributes: BTreeMap::new(),
            operations: operations
                .into_iter()
                .map(|operation| operation.operation)
                .collect(),
        }],
        connection_completeness: BTreeMap::from([(connection_id, Completeness::Complete)]),
        operation_completeness: old_ids
            .iter()
            .copied()
            .map(|id| (id, Completeness::Complete))
            .collect(),
        timeline: old_ids
            .iter()
            .copied()
            .enumerate()
            .map(|(index, operation_id)| TimelineEntry {
                connection_id,
                operation_id,
                offset: chronicle_canonical::RelativeTimeNanos(index as u64),
            })
            .collect(),
        replay: Default::default(),
        replay_attributes: BTreeMap::new(),
    };
    stamp_epoch_operation_provenance(
        &mut session,
        recording_id,
        epoch_id,
        Some(0),
        Some((first_sequence, last_sequence)),
    );
    session.validate().unwrap();
    (session, recording_id, epoch_id)
}

fn transport_receipt(
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    sequence: u64,
) -> NativeOperationBoundaryReceipt {
    transport_receipt_with_generation(
        recording_id,
        source_epoch_id,
        sequence,
        SourceConnectionGeneration::Fixture,
    )
}

fn request_observation(
    sequence: u64,
    source_range: WalByteRange,
) -> chronicle_protocol_builtins::http::HttpRequestBoundaryObservation {
    let mut decoder = chronicle_protocol_builtins::http::Decoder::new();
    let frames = decoder
        .push(DecodedFrame {
            direction: Direction::ClientToServer,
            sequence,
            payload: b"GET /one HTTP/1.1\r\nHost: recorded.invalid\r\n\r\n".to_vec(),
            attributes: BTreeMap::new(),
            connection_generation: Some(SourceConnectionGeneration::Fixture),
            provenance: vec![source_range],
            missing_payload_provenance: false,
        })
        .unwrap();
    chronicle_protocol_builtins::http::HttpRequestBoundaryObservation::from_decoded_request(
        &frames[0],
    )
    .unwrap()
}

fn transport_receipt_with_generation(
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    sequence: u64,
    source_generation: SourceConnectionGeneration,
) -> NativeOperationBoundaryReceipt {
    let observation = request_observation(sequence, range(sequence, Direction::ClientToServer));
    NativeHttpBoundaryAdapter::new(recording_id, source_epoch_id, source_generation)
        .request_boundary(&observation)
        .unwrap()
}

fn transport_receipt_for_operation(
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    operation: &chronicle_canonical::CanonicalOperation,
) -> NativeOperationBoundaryReceipt {
    let source_generation = operation
        .provenance
        .connection_generation
        .clone()
        .expect("recorded operation has source generation");
    let source_range = operation
        .provenance
        .wal_ranges
        .iter()
        .find(|range| range.direction == Direction::ClientToServer)
        .cloned()
        .expect("recorded operation has request provenance");
    let observation = request_observation(operation.sequence, source_range);
    NativeHttpBoundaryAdapter::new(recording_id, source_epoch_id, source_generation)
        .request_boundary(&observation)
        .unwrap()
}

fn references_for(
    session: &CanonicalSession,
    recording_id: RecordingId,
    epoch_id: EpochId,
) -> Vec<chronicle_canonical::CanonicalOperationRef> {
    session.connections[0]
        .operations
        .iter()
        .map(|operation| {
            chronicle_canonical::CanonicalOperationRef::new(
                recording_id,
                epoch_id,
                session.id,
                operation.id,
            )
        })
        .collect()
}

fn roles_for(
    references: &[chronicle_canonical::CanonicalOperationRef],
    ingress_indices: &[usize],
) -> BTreeMap<
    chronicle_canonical::CanonicalOperationRef,
    chronicle_canonical::InteractionRoleResolution,
> {
    references
        .iter()
        .enumerate()
        .map(|(index, reference)| {
            let role = if ingress_indices.contains(&index) {
                chronicle_canonical::InteractionRole::Ingress
            } else {
                chronicle_canonical::InteractionRole::Egress
            };
            let evidence = match role {
                chronicle_canonical::InteractionRole::Ingress => {
                    chronicle_canonical::CorrelationEvidence::passive_http()
                }
                chronicle_canonical::InteractionRole::Egress => {
                    chronicle_canonical::CorrelationEvidence::active_http()
                }
            };
            (
                *reference,
                chronicle_canonical::InteractionRoleResolution::known(role, vec![evidence]),
            )
        })
        .collect()
}

fn contextual_infrastructure_evidence(
    generation: u64,
) -> Vec<chronicle_canonical::CorrelationEvidence> {
    vec![
        chronicle_canonical::CorrelationEvidence::new(
            chronicle_canonical::CorrelationEvidenceKind::ExecutionTaskLineage {
                task: "shared-task".into(),
                worker: Some("worker".into()),
            },
        ),
        chronicle_canonical::CorrelationEvidence::new(
            chronicle_canonical::CorrelationEvidenceKind::ProcessThreadGeneration {
                process_id: Some(7),
                thread_id: Some(8),
                process_generation: Some(generation),
                thread_generation: Some(generation),
            },
        ),
        chronicle_canonical::CorrelationEvidence::new(
            chronicle_canonical::CorrelationEvidenceKind::ConnectionSocketGeneration {
                connection_id: Some(ConnectionId::from_uuid(Uuid::from_u128(77))),
                socket_id: Some("socket".into()),
                generation: Some(generation),
            },
        ),
        chronicle_canonical::CorrelationEvidence::new(
            chronicle_canonical::CorrelationEvidenceKind::ProtocolStream {
                stream_id: "stream".into(),
                generation: Some(generation),
            },
        ),
        chronicle_canonical::CorrelationEvidence::custom("fd", "value", "42"),
    ]
}

#[test]
fn cooperative_precanonical_handoff_reaches_http_resolver() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = source.start_root().unwrap();
    let child_context = source.continue_from(parent_context).unwrap();

    // Runtime and transport facts arrive before canonicalization. No session,
    // operation id, canonical reference, or boundary index exists here. Child
    // completion arrives first, modeling an asynchronous/post-response handoff.
    source
        .record_child_boundary(child_context, transport_receipt(recording_id, epoch_id, 3))
        .unwrap();
    assert!(source.drain().observations.is_empty());
    source
        .record_parent_boundary(parent_context, transport_receipt(recording_id, epoch_id, 1))
        .unwrap();
    let observations = source.drain().observations;
    assert_eq!(observations.len(), 1);

    let (session, recording_id, epoch_id) = session_from_http_canonicalizer();
    assert_eq!(session.source_provenance.recording_id, Some(recording_id));
    assert_eq!(session.source_provenance.epoch_id, Some(epoch_id));
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let parent = chronicle_canonical::CanonicalOperationRef::new(
        recording_id,
        epoch_id,
        session.id,
        session.connections[0].operations[0].id,
    );
    let child = chronicle_canonical::CanonicalOperationRef::new(
        recording_id,
        epoch_id,
        session.id,
        session.connections[0].operations[1].id,
    );
    let context = chronicle_canonical::CorrelationContext {
        role_resolutions: BTreeMap::from([
            (
                parent,
                chronicle_canonical::InteractionRoleResolution::known(
                    chronicle_canonical::InteractionRole::Ingress,
                    vec![chronicle_canonical::CorrelationEvidence::passive_http()],
                ),
            ),
            (
                child,
                chronicle_canonical::InteractionRoleResolution::known(
                    chronicle_canonical::InteractionRole::Egress,
                    vec![chronicle_canonical::CorrelationEvidence::active_http()],
                ),
            ),
        ]),
        evidence: BTreeMap::new(),
    };
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &context,
        &index,
        &observations,
    )
    .unwrap();
    assert_eq!(composed.facts.len(), 1);
    assert!(composed.diagnostics.is_empty());
    assert!(matches!(
        composed.graph.resolution(&child),
        Some(chronicle_canonical::CorrelationResolution::Resolved {
            scenario,
            confidence: chronicle_canonical::CorrelationConfidence::Strong,
            ..
        }) if *scenario == chronicle_canonical::scenario_id_v1(&parent)
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn cooperative_overlapping_abc_handoffs_start_precanonical() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let requests = [
        (1, "/a"),
        (3, "/a1"),
        (5, "/a2"),
        (7, "/b"),
        (9, "/b1"),
        (11, "/c"),
        (13, "/c1"),
    ];
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(16, 16, 16).unwrap(), 16).unwrap();
    let a = source.start_root().unwrap();
    let b = source.start_root().unwrap();
    let c = source.start_root().unwrap();
    let a1 = source.continue_from(a).unwrap();
    let a2 = source.continue_from(a).unwrap();
    let b1 = source.continue_from(b).unwrap();
    let c1 = source.continue_from(c).unwrap();

    // All source observations are emitted before any canonical operation
    // references or ETL boundary index exists.
    for (context, sequence) in [(a1, 3), (a2, 5), (b1, 9), (c1, 13)] {
        source
            .record_child_boundary(context, transport_receipt(recording_id, epoch_id, sequence))
            .unwrap();
    }
    source
        .record_parent_boundary(a, transport_receipt(recording_id, epoch_id, 1))
        .unwrap();
    source
        .record_parent_boundary(b, transport_receipt(recording_id, epoch_id, 7))
        .unwrap();
    source
        .record_parent_boundary(c, transport_receipt(recording_id, epoch_id, 11))
        .unwrap();
    let observations = source.drain().observations;
    assert_eq!(observations.len(), 4);

    let session =
        session_from_http_requests(&requests, SessionId::from_uuid(Uuid::from_u128(102))).0;
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let references: Vec<_> = session.connections[0]
        .operations
        .iter()
        .map(|operation| {
            chronicle_canonical::CanonicalOperationRef::new(
                recording_id,
                epoch_id,
                session.id,
                operation.id,
            )
        })
        .collect();
    let mut roles = BTreeMap::new();
    for (index, reference) in references.iter().enumerate() {
        let role = if [0, 3, 5].contains(&index) {
            chronicle_canonical::InteractionRole::Ingress
        } else {
            chronicle_canonical::InteractionRole::Egress
        };
        roles.insert(
            *reference,
            chronicle_canonical::InteractionRoleResolution::known(
                role,
                vec![match role {
                    chronicle_canonical::InteractionRole::Ingress => {
                        chronicle_canonical::CorrelationEvidence::passive_http()
                    }
                    chronicle_canonical::InteractionRole::Egress => {
                        chronicle_canonical::CorrelationEvidence::active_http()
                    }
                }],
            ),
        );
    }
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &chronicle_canonical::CorrelationContext {
            role_resolutions: roles,
            evidence: BTreeMap::from([
                (
                    references[0],
                    vec![chronicle_canonical::CorrelationEvidence::new(
                        chronicle_canonical::CorrelationEvidenceKind::TraceRelationship {
                            provider: "chronicle-test".into(),
                            trace_id: "a".into(),
                            span_id: Some("root".into()),
                            parent_span_id: None,
                        },
                    )],
                ),
                (
                    references[1],
                    vec![chronicle_canonical::CorrelationEvidence::new(
                        chronicle_canonical::CorrelationEvidenceKind::TraceRelationship {
                            provider: "chronicle-test".into(),
                            trace_id: "a".into(),
                            span_id: Some("child".into()),
                            parent_span_id: Some("root".into()),
                        },
                    )],
                ),
            ]),
        },
        &index,
        &observations,
    )
    .unwrap();
    assert!(composed.diagnostics.is_empty());
    assert_eq!(composed.facts.len(), 4);
    for (root, children) in [
        (0, [1, 2].as_slice()),
        (3, [4].as_slice()),
        (5, [6].as_slice()),
    ] {
        let scenario = chronicle_canonical::scenario_id_v1(&references[root]);
        for child in children {
            assert!(matches!(
                composed.graph.resolution(&references[*child]),
                Some(chronicle_canonical::CorrelationResolution::Resolved {
                    scenario: found,
                    ..
                }) if *found == scenario
            ));
        }
    }
}

#[test]
fn cooperative_stale_generation_fails_closed_before_resolver() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let old_generation = SourceConnectionGeneration::Socket {
        socket_cookie: 7,
        first_seen_boot_id: "old-boot".into(),
        first_seen_nanoseconds: 10,
        network_namespace: Some(1),
    };
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = source.start_root().unwrap();
    let child_context = source.continue_from(parent_context).unwrap();
    source
        .record_child_boundary(
            child_context,
            transport_receipt_with_generation(recording_id, epoch_id, 3, old_generation),
        )
        .unwrap();
    source
        .record_parent_boundary(parent_context, transport_receipt(recording_id, epoch_id, 1))
        .unwrap();
    let observations = source.drain().observations;

    let (mut session, _, _) = session_from_http_canonicalizer();
    session.connections[0].operations[1]
        .provenance
        .connection_generation = Some(SourceConnectionGeneration::Socket {
        socket_cookie: 7,
        first_seen_boot_id: "new-boot".into(),
        first_seen_nanoseconds: 20,
        network_namespace: Some(1),
    });
    session.validate().unwrap();
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let references = references_for(&session, recording_id, epoch_id);
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &chronicle_canonical::CorrelationContext {
            role_resolutions: roles_for(&references, &[0]),
            evidence: BTreeMap::from([
                (references[0], contextual_infrastructure_evidence(1)),
                (references[1], contextual_infrastructure_evidence(2)),
            ]),
        },
        &index,
        &observations,
    )
    .unwrap();
    assert!(composed.facts.is_empty());
    assert_eq!(composed.diagnostics.len(), 1);
    assert!(matches!(
        composed.graph.resolution(&references[1]),
        Some(chronicle_canonical::CorrelationResolution::Uncorrelated { .. })
    ));
    assert_eq!(
        composed.diagnostics[0].kind,
        chronicle_etl::NativeBindingDiagnosticKind::AnchorBindingUnresolved
    );
}

#[test]
fn runtime_boundary_claim_disagreement_is_unresolved() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = source.start_root().unwrap();
    let child_context = source.continue_from(parent_context).unwrap();
    source
        .record_child_boundary(child_context, transport_receipt(recording_id, epoch_id, 3))
        .unwrap();
    let mut parent_receipt = transport_receipt(recording_id, epoch_id, 1);
    parent_receipt.protocol_operation_boundary =
        chronicle_protocol::ProtocolOperationBoundaryClaim::new(
            ProtocolId::new("http/1.1"),
            "http-request-sequence:3",
            Direction::ClientToServer,
        )
        .unwrap();
    source
        .record_parent_boundary(parent_context, parent_receipt)
        .unwrap();
    let observations = source.drain().observations;

    let (session, _, _) = session_from_http_canonicalizer();
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let references = references_for(&session, recording_id, epoch_id);
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &chronicle_canonical::CorrelationContext {
            role_resolutions: roles_for(&references, &[0]),
            evidence: BTreeMap::new(),
        },
        &index,
        &observations,
    )
    .unwrap();
    assert!(composed.facts.is_empty());
    assert_eq!(composed.diagnostics.len(), 1);
    assert_eq!(
        composed.diagnostics[0].kind,
        chronicle_etl::NativeBindingDiagnosticKind::AnchorBindingUnresolved
    );
    assert!(matches!(
        composed.graph.resolution(&references[1]),
        Some(chronicle_canonical::CorrelationResolution::Uncorrelated { .. })
    ));
}

#[test]
fn cooperative_precanonical_binding_ambiguity_is_not_causal_ambiguity() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = source.start_root().unwrap();
    let child_context = source.continue_from(parent_context).unwrap();
    source
        .record_child_boundary(child_context, transport_receipt(recording_id, epoch_id, 3))
        .unwrap();
    source
        .record_parent_boundary(parent_context, transport_receipt(recording_id, epoch_id, 1))
        .unwrap();
    let observations = source.drain().observations;

    let (session, _, _) = session_from_http_canonicalizer();
    let mut duplicate = session.clone();
    duplicate.id = SessionId::from_uuid(Uuid::from_u128(104));
    duplicate.validate().unwrap();
    let sessions = vec![session, duplicate];
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(&sessions, &registry);
    let mut roles = BTreeMap::new();
    for session in &sessions {
        let references = references_for(session, recording_id, epoch_id);
        roles.extend(roles_for(&references, &[0]));
    }
    let composed = compose_correlation_with_native(
        &sessions,
        &chronicle_canonical::CorrelationContext {
            role_resolutions: roles,
            evidence: BTreeMap::new(),
        },
        &index,
        &observations,
    )
    .unwrap();
    assert!(composed.facts.is_empty());
    assert_eq!(composed.diagnostics.len(), 2);
    assert!(composed.diagnostics.iter().all(|diagnostic| matches!(
        diagnostic.kind,
        chronicle_etl::NativeBindingDiagnosticKind::AnchorBindingAmbiguous
    )));
}

#[test]
fn cooperative_precanonical_competing_parents_are_causally_ambiguous() {
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let requests = [(1, "/a"), (3, "/b"), (5, "/child")];
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let first_parent = source.start_root().unwrap();
    let second_parent = source.start_root().unwrap();
    let first_child = source.continue_from(first_parent).unwrap();
    let second_child = source.continue_from(second_parent).unwrap();
    source
        .record_child_boundary(first_child, transport_receipt(recording_id, epoch_id, 5))
        .unwrap();
    source
        .record_child_boundary(second_child, transport_receipt(recording_id, epoch_id, 5))
        .unwrap();
    source
        .record_parent_boundary(first_parent, transport_receipt(recording_id, epoch_id, 1))
        .unwrap();
    source
        .record_parent_boundary(second_parent, transport_receipt(recording_id, epoch_id, 3))
        .unwrap();
    let observations = source.drain().observations;
    assert_eq!(observations.len(), 2);

    let session =
        session_from_http_requests(&requests, SessionId::from_uuid(Uuid::from_u128(102))).0;
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let references = references_for(&session, recording_id, epoch_id);
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &chronicle_canonical::CorrelationContext {
            role_resolutions: roles_for(&references, &[0, 1]),
            evidence: BTreeMap::from([
                (
                    references[1],
                    vec![chronicle_canonical::CorrelationEvidence::new(
                        chronicle_canonical::CorrelationEvidenceKind::TraceRelationship {
                            provider: "chronicle-test".into(),
                            trace_id: "trace-parent".into(),
                            span_id: Some("root".into()),
                            parent_span_id: None,
                        },
                    )],
                ),
                (
                    references[2],
                    vec![chronicle_canonical::CorrelationEvidence::new(
                        chronicle_canonical::CorrelationEvidenceKind::TraceRelationship {
                            provider: "chronicle-test".into(),
                            trace_id: "trace-parent".into(),
                            span_id: Some("child".into()),
                            parent_span_id: Some("root".into()),
                        },
                    )],
                ),
            ]),
        },
        &index,
        &observations,
    )
    .unwrap();
    assert_eq!(composed.facts.len(), 2);
    assert!(matches!(
        composed.graph.resolution(&references[2]),
        Some(chronicle_canonical::CorrelationResolution::Ambiguous { .. })
    ));
}

#[test]
fn passive_only_without_native_observation_stays_uncorrelated() {
    let (session, recording_id, epoch_id) = session_from_http_canonicalizer();
    let parent = chronicle_canonical::CanonicalOperationRef::new(
        recording_id,
        epoch_id,
        session.id,
        session.connections[0].operations[0].id,
    );
    let child = chronicle_canonical::CanonicalOperationRef::new(
        recording_id,
        epoch_id,
        session.id,
        session.connections[0].operations[1].id,
    );
    let registry = chronicle_protocol_builtins::registry().unwrap();
    let index = NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);
    let composed = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &chronicle_canonical::CorrelationContext {
            role_resolutions: BTreeMap::from([
                (
                    parent,
                    chronicle_canonical::InteractionRoleResolution::known(
                        chronicle_canonical::InteractionRole::Ingress,
                        vec![chronicle_canonical::CorrelationEvidence::passive_http()],
                    ),
                ),
                (
                    child,
                    chronicle_canonical::InteractionRoleResolution::known(
                        chronicle_canonical::InteractionRole::Egress,
                        vec![chronicle_canonical::CorrelationEvidence::active_http()],
                    ),
                ),
            ]),
            evidence: BTreeMap::new(),
        },
        &index,
        &[],
    )
    .unwrap();
    assert!(composed.facts.is_empty());
    assert!(composed.diagnostics.is_empty());
    assert!(matches!(
        composed.graph.resolution(&child),
        Some(chronicle_canonical::CorrelationResolution::Uncorrelated { .. })
    ));
}

#[test]
#[allow(clippy::too_many_lines)]
fn native_restart_loss_preserves_wal_checkpoint_completeness_and_replayability() {
    let root =
        std::env::temp_dir().join(format!("chronicle-native-restart-proof-{}", Uuid::new_v4()));
    let wal_directory = root.join("wal");
    let mut fixture: serde_json::Value = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/http/multiple-exchanges.json"
    )))
    .unwrap();
    // Keep two complete operations for the native-linked replayable path and
    // append one request without a response for the unreplayable path.
    let events = fixture["events"].as_array_mut().unwrap();
    let mut incomplete = events[0].clone();
    incomplete["sequence"] = json!(5);
    incomplete["timestamp"] = json!("2026-01-01T00:00:04Z");
    incomplete["payload_hex"] = json!(
        "474554202f746872656520485454502f312e310d0a486f73743a207265636f726465642e696e76616c69640d0a0d0a"
    );
    events.push(incomplete);
    let fixture = serde_json::to_vec(&fixture).unwrap();
    let mut capture = FixtureCaptureSource::from_json(&fixture).unwrap();
    let recorded = write_capture_to_wal(
        &mut capture,
        &wal_directory,
        RecordingId::new(),
        1024 * 1024,
    )
    .unwrap();
    let recording_id = recorded.recording_id;
    write_recording_metadata(
        &wal_directory,
        &RecordingMetadata {
            version: chronicle_application::RECORDING_METADATA_SCHEMA_VERSION,
            recording_id,
            selector: None,
            status: RecordingStatus::Completed,
            shutdown_reason: Some(ShutdownReason::SourceCompleted),
            last_valid_commit: None,
            counters: Default::default(),
            terminal_wal_loss: None,
            capture: None,
        },
    )
    .unwrap();
    reconcile_recording_metadata(&wal_directory, recording_id, None).unwrap();

    let registry = chronicle_protocol_builtins::registry().unwrap();
    let published = process_and_publish_recording_wal(&wal_directory, &root, &registry).unwrap();
    assert!(!published.already_published);
    let session = FilesystemSessionStore::new(&root)
        .hydrate(published.session_id)
        .unwrap();
    session.validate().unwrap();
    assert_eq!(session.connections[0].operations.len(), 3);
    let epoch_id = session
        .source_provenance
        .epoch_id
        .expect("published session has epoch provenance");
    let references = references_for(&session, recording_id, epoch_id);
    let parent_receipt = transport_receipt_for_operation(
        recording_id,
        epoch_id,
        &session.connections[0].operations[0],
    );
    let child_receipt = transport_receipt_for_operation(
        recording_id,
        epoch_id,
        &session.connections[0].operations[1],
    );
    let context = chronicle_canonical::CorrelationContext {
        role_resolutions: roles_for(&references, &[0]),
        evidence: BTreeMap::from([(references[1], contextual_infrastructure_evidence(1))]),
    };
    let boundary_index =
        NativeBoundaryIndex::from_sessions(std::slice::from_ref(&session), &registry);

    // Before loss, the same recording has one explicit native handoff. The
    // test-local observations are not persisted anywhere.
    let mut before_source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = before_source.start_root().unwrap();
    let child_context = before_source.continue_from(parent_context).unwrap();
    before_source
        .record_child_boundary(child_context, child_receipt.clone())
        .unwrap();
    before_source
        .record_parent_boundary(parent_context, parent_receipt.clone())
        .unwrap();
    let before_observations = before_source.drain().observations;
    assert_eq!(before_observations.len(), 1);
    let before_correlation = compose_correlation_with_native(
        std::slice::from_ref(&session),
        &context,
        &boundary_index,
        &before_observations,
    )
    .unwrap();
    assert!(matches!(
        before_correlation.graph.resolution(&references[1]),
        Some(chronicle_canonical::CorrelationResolution::Resolved {
            scenario,
            confidence: chronicle_canonical::CorrelationConfidence::Strong,
            ..
        }) if *scenario == chronicle_canonical::scenario_id_v1(&references[0])
    ));

    // A second process instance has one incomplete handoff and one complete
    // handoff still queued. Restart loses both non-durable states exactly once.
    let mut lost_source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let pending_parent = lost_source.start_root().unwrap();
    let pending_child = lost_source.continue_from(pending_parent).unwrap();
    lost_source
        .record_child_boundary(pending_child, child_receipt.clone())
        .unwrap();
    let queued_parent = lost_source.start_root().unwrap();
    let queued_child = lost_source.continue_from(queued_parent).unwrap();
    lost_source
        .record_child_boundary(queued_child, child_receipt)
        .unwrap();
    lost_source
        .record_parent_boundary(queued_parent, parent_receipt)
        .unwrap();
    let restart = lost_source.restart();
    assert!(restart.observations.is_empty());
    assert_eq!(
        restart.diagnostics,
        vec![
            NativeObservationDiagnostic {
                kind: NativeObservationDiagnosticKind::RestartLostPending,
                context_generation: Some(pending_child.generation()),
                message: "native pending handoff was lost during source restart".into(),
            },
            NativeObservationDiagnostic {
                kind: NativeObservationDiagnosticKind::SideChannelLoss,
                context_generation: None,
                message: "native side-channel lost 1 queued observations during restart".into(),
            },
        ]
    );
    assert!(lost_source.drain().observations.is_empty());
    assert!(lost_source.drain().diagnostics.is_empty());
    assert!(lost_source.restart().diagnostics.is_empty());

    let before_scan = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
    let before_segment =
        fs::read(wal_directory.join("segments").join(segment_file_name(1))).unwrap();
    let before_recording_metadata = fs::read(wal_directory.join("recording.json")).unwrap();
    let before_checkpoint = fs::read(wal_directory.join("etl-checkpoint.json")).unwrap();
    let before_session = session.clone();
    let before_inspection = inspect_session(&root, published.session_id).unwrap();
    let before_etl = chronicle_application::process_recording_wal(
        &wal_directory,
        &registry,
        published.session_id,
    )
    .unwrap();

    // Recovery/publication runs with no native input. It must read the same
    // committed WAL and converge on the already published canonical state.
    let repeated = process_and_publish_recording_wal(&wal_directory, &root, &registry).unwrap();
    assert!(repeated.already_published);
    let after_etl = chronicle_application::process_recording_wal(
        &wal_directory,
        &registry,
        published.session_id,
    )
    .unwrap();
    let after_scan = scan_wal(&wal_directory, recording_id, DEFAULT_MAX_RECORD_BYTES).unwrap();
    let after_session = FilesystemSessionStore::new(&root)
        .hydrate(published.session_id)
        .unwrap();
    let after_inspection = inspect_session(&root, published.session_id).unwrap();

    // WAL authority and physical bytes are unchanged; native state never
    // participates in commit-marker recovery or repair.
    assert_eq!(before_scan, after_scan);
    assert_eq!(
        before_segment,
        fs::read(wal_directory.join("segments").join(segment_file_name(1))).unwrap()
    );
    assert_eq!(
        before_recording_metadata,
        fs::read(wal_directory.join("recording.json")).unwrap()
    );
    assert_eq!(
        before_checkpoint,
        fs::read(wal_directory.join("etl-checkpoint.json")).unwrap()
    );
    assert_eq!(published.checkpoint, repeated.checkpoint);

    // This recording deliberately contains two replayable and one incomplete
    // operation; native loss must not change either classification.
    assert_eq!(
        before_inspection.replayability,
        chronicle_replay::Replayability::PartiallyReplayable
    );
    assert_eq!(before_inspection.executable_operations, 2);
    assert_eq!(before_inspection.non_executable_operations, 1);

    // Canonical state, completeness, loss accounting, and replay blockers are
    // byte/semantic identical because only the non-durable native input died.
    assert_eq!(before_session, after_session);
    assert_eq!(before_inspection, after_inspection);
    assert_eq!(before_etl.output.session, after_etl.output.session);
    assert_eq!(before_etl.output.issues, after_etl.output.issues);
    assert_eq!(
        before_etl.output.evidence.loss_windows,
        after_etl.output.evidence.loss_windows
    );
    assert_eq!(
        before_etl.output.evidence.terminal_wal_losses,
        after_etl.output.evidence.terminal_wal_losses
    );
    assert_eq!(
        before_etl.output.evidence.commit_marker_sequences,
        after_etl.output.evidence.commit_marker_sequences
    );
    assert_eq!(before_etl.commit_boundary, after_etl.commit_boundary);
    assert_eq!(
        before_etl.commit_marker_byte_offset,
        after_etl.commit_marker_byte_offset
    );
    assert_eq!(
        before_etl.wal_snapshot_sha256,
        after_etl.wal_snapshot_sha256
    );
    assert_eq!(before_etl.recovery_sha256, after_etl.recovery_sha256);
    assert_eq!(
        before_session.connection_completeness,
        after_session.connection_completeness
    );
    assert_eq!(
        before_session.operation_completeness,
        after_session.operation_completeness
    );
    assert_eq!(
        before_inspection.replayability,
        after_inspection.replayability
    );
    assert_eq!(before_inspection.blockers, after_inspection.blockers);

    let after_correlation = compose_correlation_with_native(
        std::slice::from_ref(&after_session),
        &context,
        &boundary_index,
        &[],
    )
    .unwrap();
    assert!(after_correlation.facts.is_empty());
    assert!(matches!(
        after_correlation.graph.resolution(&references[1]),
        Some(chronicle_canonical::CorrelationResolution::Uncorrelated { .. })
    ));

    fs::remove_dir_all(root).unwrap();
}
