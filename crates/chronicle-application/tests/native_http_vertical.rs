use chronicle_application::CooperativeNativeSource;
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

#[allow(clippy::too_many_lines)] // Keep protocol-boundary fixture construction visible.
fn session_from_http_canonicalizer() -> (CanonicalSession, RecordingId, EpochId) {
    let canonicalizer = Canonicalizer::new();
    let frames = [
        (
            Direction::ClientToServer,
            1,
            message("Request", 1, Some("GET"), Some("/parent"), None),
        ),
        (
            Direction::ServerToClient,
            2,
            message("Response", 2, None, None, Some(200)),
        ),
        (
            Direction::ClientToServer,
            3,
            message("Request", 3, Some("GET"), Some("/child"), None),
        ),
        (
            Direction::ServerToClient,
            4,
            message("Response", 4, None, None, Some(200)),
        ),
    ]
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
    assert_eq!(operations.len(), 2);
    for (index, operation) in operations.iter_mut().enumerate() {
        operation.operation.provenance.connection_generation =
            Some(SourceConnectionGeneration::Fixture);
        operation.operation.provenance.wal_ranges = if index == 0 {
            vec![
                range(1, Direction::ClientToServer),
                range(2, Direction::ServerToClient),
            ]
        } else {
            vec![
                range(3, Direction::ClientToServer),
                range(4, Direction::ServerToClient),
            ]
        };
    }
    let recording_id = RecordingId::from_uuid(Uuid::from_u128(100));
    let epoch_id = EpochId::from_uuid(Uuid::from_u128(101));
    let session_id = SessionId::from_uuid(Uuid::from_u128(102));
    let connection_id = ConnectionId::from_uuid(Uuid::from_u128(103));
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
    stamp_epoch_operation_provenance(&mut session, recording_id, epoch_id, Some(0), Some((1, 4)));
    session.validate().unwrap();
    (session, recording_id, epoch_id)
}

fn receipt_for(
    session: &CanonicalSession,
    reference: chronicle_canonical::CanonicalOperationRef,
    index: &NativeBoundaryIndex,
) -> NativeOperationBoundaryReceipt {
    let operation = session.connections[0]
        .operations
        .iter()
        .find(|operation| operation.id == reference.operation_id)
        .unwrap();
    NativeOperationBoundaryReceipt {
        recording_id: reference.recording_id,
        source_epoch_id: reference.owner_epoch_id,
        source_generation: operation.provenance.connection_generation.clone().unwrap(),
        protocol_id: session.connections[0].protocol.clone(),
        direction: Direction::ClientToServer,
        protocol_operation_boundary: index.boundary_for(&reference).unwrap().clone(),
        source_range: operation.provenance.wal_ranges[0].clone(),
    }
}

#[test]
fn http_canonicalizer_to_cooperative_handoff_to_resolver_vertical_slice() {
    let (session, recording_id, epoch_id) = session_from_http_canonicalizer();
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
    let mut source =
        CooperativeNativeSource::new(NativeSourceLimits::new(8, 8, 8).unwrap(), 8).unwrap();
    let parent_context = source.start_root().unwrap();
    let child_context = source.continue_from(parent_context).unwrap();
    source
        .record_child_boundary(child_context, receipt_for(&session, child, &index))
        .unwrap();
    source
        .record_parent_boundary(parent_context, receipt_for(&session, parent, &index))
        .unwrap();
    let observations = source.drain().observations;
    assert_eq!(observations.len(), 1);

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
