use chronicle_application::{CooperativeNativeSource, NativeHttpBoundaryAdapter};
use chronicle_canonical::{
    CanonicalConnection, CanonicalSession, Completeness, SourceConnectionGeneration,
    SourceMetadata, SourceProvenance, TimelineEntry, WalByteRange,
};
use chronicle_common::{
    ConnectionId, Direction, Endpoint, EpochId, OperationId, ProtocolId, RecordingId, SessionId,
};
use chronicle_etl::{
    NativeBoundaryIndex, NativeOperationBoundaryReceipt, NativeSourceLimits,
    compose_correlation_with_native, stamp_epoch_operation_provenance,
};
use chronicle_protocol::{DecodedFrame, ProtocolCanonicalizer, ProtocolStream};
use chronicle_protocol_builtins::http::Canonicalizer;
use serde_json::json;
use std::collections::BTreeMap;
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

fn transport_receipt_with_generation(
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    sequence: u64,
    source_generation: SourceConnectionGeneration,
) -> NativeOperationBoundaryReceipt {
    NativeHttpBoundaryAdapter::new(recording_id, source_epoch_id, source_generation)
        .request_boundary(sequence, range(sequence, Direction::ClientToServer))
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
