//! Restartable WAL-to-canonical transformation boundary.

mod checkpoint;
mod delta;
mod incremental;
mod publication;
pub use checkpoint::{
    CheckpointError, CheckpointLifecycle, CheckpointOwner, CheckpointSummary,
    DECODER_IMPLEMENTATION_VERSION, DECODER_KIND, DECODER_SNAPSHOT_VERSION,
    DecoderReconstructionState, DeltaBatchReference, INCREMENTAL_CHECKPOINT_SCHEMA_VERSION,
    IncrementalEtlCheckpointV1, MarkerLineage, RecoveryAuthoritativeSnapshot, SegmentLineage,
    SourceStatus, read_checkpoint, write_checkpoint_atomic,
};
pub use delta::{CANONICAL_DELTA_SCHEMA_VERSION, CanonicalDeltaBatchV1, DeltaBatchError};
pub use incremental::{CommittedWalSnapshot, IncrementalProcessor, IncrementalResult};
pub use publication::{
    CheckpointFault, PublicationError, ReconcileOutcome, finalize_incremental_session,
    publish_delta_then_checkpoint, publish_delta_then_checkpoint_with_fault,
    reconcile_delta_checkpoint, recover_pending_checkpoint, verify_one_shot_equivalence,
};

/// ETL input boundary before protocol-specific decoding.
pub use chronicle_protocol::ProtocolNeutralConnection;

use chronicle_canonical::{
    Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalOperation,
    CanonicalSession, Completeness, OperationEffect, OperationKind, OperationProvenance,
    PROTOCOL_DATA_SCHEMA_VERSION, PayloadRef, ProtocolData, RelativeTimeNanos, ReplayMetadata,
    SourceMetadata, TimelineEntry,
};
use chronicle_capture::{
    CaptureEvent, CaptureEventKind, LossWindowObserved, SocketRole, decode_event,
};
use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId, SessionId};
use chronicle_protocol::{
    CanonicalizedOperation, DecodedFrame, ProtocolRegistry, ProtocolStream, StreamChunk,
    reconstructed_frames,
};
use chronicle_session::{
    LossWindowClassification, ReconstructionAssembler, ReconstructionConnection,
    ReconstructionConnectionIdentity, ReconstructionEvidence, ReconstructionInput,
    ReconstructionLimits, ReconstructionPushError, ReconstructionWalProvenance, SessionError,
    SessionLimits,
};
use chronicle_wal::{
    COMMIT_MARKER_VERSION, RecordKind, TerminalWalLoss, WalError, WalRecordEnvelope,
    decode_commit_marker, decode_terminal_wal_loss, encode_envelope,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use time::OffsetDateTime;
use uuid::Uuid;

pub const DEFAULT_MAX_ISSUES: usize = 1_024;
pub const ENVELOPE_BATCH_SIZE: usize = 4_096;
pub const ETL_PIPELINE_VERSION: &str = "p1";
pub const MAX_CANONICAL_OPERATIONS: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EtlIssueKind {
    MalformedCaptureEvent,
    PartialWalTail,
    UnknownProtocol,
    ProtocolDecode,
    MalformedLossWindow,
    MalformedTerminalWalLoss,
    MalformedCommitMarker,
    IncompleteSocketEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EtlIssue {
    pub sequence: Option<u64>,
    pub kind: EtlIssueKind,
    pub message: String,
}

#[derive(Clone, Debug)]
pub struct EtlOutput {
    pub session: CanonicalSession,
    pub issues: Vec<EtlIssue>,
    /// Typed loss evidence preserved from committed envelopes for reconstruction.
    pub evidence: EtlEvidence,
}

/// Assigns stable recording-derived identifiers after canonicalization.
pub fn assign_recording_ids(
    session: &mut CanonicalSession,
    recording_id: chronicle_common::RecordingId,
    envelopes: &[WalRecordEnvelope],
) -> Result<(), EtlError> {
    let snapshot = recording_snapshot(recording_id, envelopes)?;
    assign_recording_ids_with_snapshot(session, snapshot)
}

/// Assigns stable identifiers from an already-verified recording WAL snapshot.
pub fn assign_recording_ids_with_snapshot(
    session: &mut CanonicalSession,
    snapshot: [u8; 32],
) -> Result<(), EtlError> {
    session.id = SessionId(deterministic_uuid("session", &snapshot, 0, 0));
    let mut connection_ids = BTreeMap::new();
    let mut operation_ids = BTreeMap::new();
    for (connection_index, connection) in session.connections.iter_mut().enumerate() {
        let first_sequence = connection
            .operations
            .first()
            .map_or(0, |operation| operation.sequence);
        let connection_index = u64::try_from(connection_index).unwrap_or(u64::MAX);
        let old_id = connection.id;
        connection.id = ConnectionId(deterministic_uuid(
            "connection",
            &snapshot,
            connection_index,
            first_sequence,
        ));
        connection_ids.insert(old_id, connection.id);
        for (operation_index, operation) in connection.operations.iter_mut().enumerate() {
            let old_id = operation.id;
            operation.id = OperationId(deterministic_uuid(
                "operation",
                &snapshot,
                connection_index,
                u64::try_from(operation_index).unwrap_or(u64::MAX),
            ));
            operation_ids.insert(old_id, operation.id);
        }
    }
    session.connection_completeness = session
        .connection_completeness
        .iter()
        .filter_map(|(id, state)| connection_ids.get(id).map(|new_id| (*new_id, *state)))
        .collect();
    session.operation_completeness = session
        .operation_completeness
        .iter()
        .filter_map(|(id, state)| operation_ids.get(id).map(|new_id| (*new_id, *state)))
        .collect();
    for entry in &mut session.timeline {
        entry.connection_id = *connection_ids
            .get(&entry.connection_id)
            .ok_or(EtlError::TimelineReference)?;
        entry.operation_id = *operation_ids
            .get(&entry.operation_id)
            .ok_or(EtlError::TimelineReference)?;
    }
    Ok(())
}

fn recording_snapshot(
    recording_id: chronicle_common::RecordingId,
    envelopes: &[WalRecordEnvelope],
) -> Result<[u8; 32], EtlError> {
    let mut digest = Sha256::new();
    digest.update(b"chronicle/etl/snapshot\0");
    digest.update(ETL_PIPELINE_VERSION.as_bytes());
    digest.update(recording_id.0.as_bytes());
    for envelope in envelopes {
        digest.update(encode_envelope(envelope)?);
    }
    Ok(digest.finalize().into())
}

fn deterministic_uuid(domain: &str, snapshot: &[u8; 32], first: u64, second: u64) -> Uuid {
    let mut digest = Sha256::new();
    digest.update(b"chronicle/etl/id\0");
    digest.update(ETL_PIPELINE_VERSION.as_bytes());
    digest.update(domain.as_bytes());
    digest.update(snapshot);
    digest.update(first.to_le_bytes());
    digest.update(second.to_le_bytes());
    let mut bytes: [u8; 16] = digest.finalize()[..16]
        .try_into()
        .expect("SHA-256 prefix has fixed length");
    bytes[6] = (bytes[6] & 0x0f) | 0x80; // UUID variant/version bits, deterministic payload retained.
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(Clone, Debug, Default)]
pub struct EtlEvidence {
    pub loss_windows: Vec<SequencedEvidence<LossWindowObserved>>,
    pub terminal_wal_losses: Vec<SequencedEvidence<TerminalWalLoss>>,
    pub commit_marker_sequences: Vec<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SequencedEvidence<T> {
    /// `None` means finalized recording metadata supplied this evidence.
    pub sequence: Option<u64>,
    pub value: T,
}

#[derive(Debug, Error)]
pub enum EtlError {
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error(transparent)]
    Reconstruction(#[from] ReconstructionPushError),
    #[error("canonical operation limit {limit} exceeded")]
    OperationLimit { limit: usize },
    #[error("canonical timeline references an unknown identifier")]
    TimelineReference,
    #[error("ETL issue limit {limit} exceeded")]
    IssueLimit { limit: usize },
    #[error("reconstruction connection lacks complete socket endpoint evidence")]
    MissingSocketEvidence,
    #[error("reconstruction connection has conflicting socket endpoint evidence")]
    ConflictingSocketEvidence,
    #[error("incremental snapshot marker regressed")]
    SnapshotRegression,
    #[error("incremental snapshot has a gap after prior checkpoint")]
    SnapshotGap,
    #[error("incremental snapshot segment lineage does not extend prior checkpoint")]
    SnapshotLineageMismatch,
    #[error(transparent)]
    ReconstructionSnapshot(#[from] chronicle_session::ReconstructionSnapshotError),
    #[error("decoder checkpoint does not match current session or WAL cursor")]
    DecoderCheckpointMismatch,
}

pub struct EtlPipeline {
    limits: SessionLimits,
    max_issues: usize,
}

impl EtlPipeline {
    pub fn new(limits: SessionLimits) -> Self {
        Self::with_max_issues(limits, DEFAULT_MAX_ISSUES)
    }

    pub fn with_max_issues(limits: SessionLimits, max_issues: usize) -> Self {
        Self { limits, max_issues }
    }

    /// Processes only recovery-authoritative WAL v1 envelopes. Commit markers remain
    /// provenance, never protocol input.
    pub fn process_envelopes(
        &self,
        envelopes: &[WalRecordEnvelope],
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        self.process_envelopes_with_terminal_losses(envelopes, &[], registry, session_id)
    }

    /// Adds terminal WAL-loss evidence persisted only in finalized recording metadata.
    pub fn process_envelopes_with_terminal_losses(
        &self,
        envelopes: &[WalRecordEnvelope],
        metadata_terminal_losses: &[TerminalWalLoss],
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        let evidence = EtlEvidence {
            terminal_wal_losses: metadata_terminal_losses
                .iter()
                .cloned()
                .map(|value| SequencedEvidence {
                    sequence: None,
                    value,
                })
                .collect(),
            ..EtlEvidence::default()
        };
        self.process_reconstructed_envelopes(envelopes, evidence, registry, session_id)
    }

    fn process_reconstructed_envelopes(
        &self,
        envelopes: &[WalRecordEnvelope],
        mut evidence: EtlEvidence,
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        let mut assembler = ReconstructionAssembler::new(ReconstructionLimits {
            max_active_connections: self.limits.max_connections,
            max_bytes_per_connection: self.limits.max_bytes_per_connection,
            max_chunks_per_connection: self.limits.max_chunks_per_connection,
            ..ReconstructionLimits::default()
        });
        for loss in &evidence.terminal_wal_losses {
            assembler.push(ReconstructionInput::TerminalWalLoss(loss.value.clone()))?;
        }
        let mut issues = Vec::new();
        for batch in envelopes.chunks(ENVELOPE_BATCH_SIZE) {
            for envelope in batch {
                ingest_reconstructed_envelope(
                    envelope,
                    &mut assembler,
                    &mut evidence,
                    &mut issues,
                    self.max_issues,
                )?;
            }
        }
        self.finish_reconstructed(assembler.finish(), issues, evidence, registry, session_id)
    }

    #[allow(clippy::too_many_lines)] // Reconstruction preserves same bounded ETL error boundary as fixture assembly.
    fn finish_reconstructed(
        &self,
        assembly: chronicle_session::ReconstructionAssembly,
        mut issues: Vec<EtlIssue>,
        evidence: EtlEvidence,
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        let started_at = OffsetDateTime::UNIX_EPOCH;
        let mut connections = Vec::with_capacity(assembly.connections.len());
        let mut connection_completeness = BTreeMap::new();
        let mut operation_completeness = BTreeMap::new();
        let mut timeline = Vec::new();
        let mut operation_count = 0;

        for connection in assembly.connections {
            let connection_id = deterministic_connection_id(&connection);
            let (client, server, attributes, missing_socket_evidence) =
                match reconstructed_connection_identity(&connection) {
                    Ok(identity) => identity,
                    Err(EtlError::MissingSocketEvidence) => {
                        record_issue(
                            &mut issues,
                            self.max_issues,
                            EtlIssue {
                                sequence: connection
                                    .events
                                    .iter()
                                    .find_map(|event| event.wal_sequence),
                                kind: EtlIssueKind::IncompleteSocketEvidence,
                                message:
                                    "socket endpoint evidence did not cross recording boundary"
                                        .into(),
                            },
                        )?;
                        let mut attributes = Attributes::new();
                        attributes.insert("chronicle.endpoint_evidence".into(), "missing".into());
                        (
                            Endpoint::new("unknown", 0),
                            Endpoint::new("unknown", 0),
                            attributes,
                            true,
                        )
                    }
                    Err(error) => return Err(error),
                };
            let incomplete = missing_socket_evidence
                || reconstructed_connection_incomplete(&connection, &assembly.terminal_wal_losses);
            let neutral = connection.protocol_neutral();
            let mut frames = reconstructed_frames(&neutral);
            sort_reconstructed_frames(&connection, &mut frames);
            let protocol_stream = ProtocolStream {
                started_at: Some(started_at),
                chunks: frames
                    .iter()
                    .map(|frame| StreamChunk {
                        direction: frame.direction,
                        sequence: frame.sequence,
                        timestamp: None,
                        payload: &frame.payload,
                    })
                    .collect(),
                truncated: incomplete,
            };
            let first_sequence = protocol_stream.chunks.first().map(|chunk| chunk.sequence);
            let (protocol, mut decoded_operations) = if let Some((registration, _)) =
                registry.detect(&protocol_stream, None)
            {
                match decode_and_canonicalize(registration, &protocol_stream, &frames) {
                    Ok(operations) => (registration.id.clone(), operations),
                    Err(error) => {
                        record_issue(
                            &mut issues,
                            self.max_issues,
                            EtlIssue {
                                sequence: first_sequence,
                                kind: EtlIssueKind::ProtocolDecode,
                                message: error,
                            },
                        )?;
                        (
                            registration.id.clone(),
                            vec![CanonicalizedOperation {
                                operation: opaque_operation_from_protocol_stream(&protocol_stream),
                                completeness: Completeness::Malformed,
                            }],
                        )
                    }
                }
            } else {
                record_issue(
                    &mut issues,
                    self.max_issues,
                    EtlIssue {
                        sequence: first_sequence,
                        kind: EtlIssueKind::UnknownProtocol,
                        message: "traffic preserved as opaque bytes".into(),
                    },
                )?;
                (
                    ProtocolId::unknown(),
                    vec![CanonicalizedOperation {
                        operation: opaque_operation_from_protocol_stream(&protocol_stream),
                        completeness: Completeness::Unsupported,
                    }],
                )
            };
            for decoded in &mut decoded_operations {
                decoded.operation.id = deterministic_operation_id(&decoded.operation);
            }
            // Deterministic operation ids collide for repeated identical
            // traffic within one recording, so keep the last occurrence of
            // each operation in the final session (incremental batch output
            // applies the same deduplication).
            let mut emitted_operation_ids = BTreeSet::new();
            decoded_operations.reverse();
            decoded_operations.retain(|decoded| emitted_operation_ids.insert(decoded.operation.id));
            decoded_operations.reverse();
            operation_count = bounded_operation_count(operation_count, decoded_operations.len())?;
            for decoded in &decoded_operations {
                let operation = &decoded.operation;
                timeline.push((
                    operation.sequence,
                    TimelineEntry {
                        connection_id,
                        operation_id: operation.id,
                        offset: operation.started_at_offset,
                    },
                ));
                operation_completeness.insert(
                    operation.id,
                    if incomplete {
                        Completeness::Incomplete
                    } else {
                        decoded.completeness
                    },
                );
            }
            connection_completeness.insert(
                connection_id,
                if protocol_stream.chunks.is_empty() || incomplete {
                    Completeness::Incomplete
                } else {
                    Completeness::Complete
                },
            );
            connections.push(CanonicalConnection {
                id: connection_id,
                protocol,
                client,
                server,
                attributes,
                operations: decoded_operations
                    .into_iter()
                    .map(|decoded| decoded.operation)
                    .collect(),
            });
        }
        timeline.sort_by_key(|(sequence, _)| *sequence);
        Ok(EtlOutput {
            session: CanonicalSession {
                schema_version: CANONICAL_SCHEMA_VERSION,
                id: session_id,
                started_at,
                ended_at: None,
                source: SourceMetadata::default(),
                source_provenance: Default::default(),
                connections,
                connection_completeness,
                operation_completeness,
                timeline: timeline.into_iter().map(|(_, entry)| entry).collect(),
                replay: ReplayMetadata::default(),
                replay_attributes: Attributes::new(),
            },
            issues,
            evidence,
        })
    }
}

fn deterministic_operation_id(operation: &CanonicalOperation) -> OperationId {
    let mut normalized = operation.clone();
    normalized.id = OperationId(Uuid::nil());
    let bytes = serde_json::to_vec(&normalized).unwrap_or_default();
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    OperationId(Uuid::from_bytes(digest[..16].try_into().unwrap_or([0; 16])))
}

fn deterministic_connection_id(connection: &ReconstructionConnection) -> ConnectionId {
    let mut digest = Sha256::new();
    digest.update(b"chronicle/connection/v1\0");
    digest.update(format!("{:?}", connection.identity).as_bytes());
    for sequence in connection
        .events
        .iter()
        .filter_map(|event| event.wal_sequence)
    {
        digest.update(sequence.to_le_bytes());
    }
    let bytes: [u8; 32] = digest.finalize().into();
    ConnectionId(Uuid::from_bytes(bytes[..16].try_into().unwrap_or([0; 16])))
}

fn bounded_operation_count(existing: usize, additional: usize) -> Result<usize, EtlError> {
    let count = existing.saturating_add(additional);
    if count > MAX_CANONICAL_OPERATIONS {
        return Err(EtlError::OperationLimit {
            limit: MAX_CANONICAL_OPERATIONS,
        });
    }
    Ok(count)
}

fn sort_reconstructed_frames(connection: &ReconstructionConnection, frames: &mut [DecodedFrame]) {
    let timestamps: BTreeMap<_, _> = connection
        .events
        .iter()
        .filter_map(|event| {
            event
                .wal_sequence
                .map(|sequence| (sequence, &event.timestamp))
        })
        .collect();
    frames.sort_by_key(|frame| {
        timestamps.get(&frame.sequence).map_or_else(
            || (String::new(), u64::MAX, frame.sequence),
            |timestamp| {
                (
                    timestamp.monotonic.clock.boot_id.clone(),
                    timestamp.monotonic.nanoseconds,
                    frame.sequence,
                )
            },
        )
    });
}

fn reconstructed_connection_identity(
    connection: &ReconstructionConnection,
) -> Result<(Endpoint, Endpoint, Attributes, bool), EtlError> {
    match &connection.identity {
        ReconstructionConnectionIdentity::Fixture(key) => Ok((
            key.client.clone(),
            key.server.clone(),
            Attributes::new(),
            false,
        )),
        ReconstructionConnectionIdentity::Socket(socket) => {
            let evidence = connection
                .events
                .iter()
                .find_map(|event| event.socket_evidence.as_ref())
                .ok_or(EtlError::MissingSocketEvidence)?;
            if connection
                .events
                .iter()
                .filter_map(|event| event.socket_evidence.as_ref())
                .any(|candidate| {
                    candidate.network_family != evidence.network_family
                        || candidate.local_endpoint != evidence.local_endpoint
                        || candidate.remote_endpoint != evidence.remote_endpoint
                        || candidate.role != evidence.role
                })
            {
                return Err(EtlError::ConflictingSocketEvidence);
            }
            let mut attributes = Attributes::new();
            attributes.insert(
                "chronicle.socket_cookie".into(),
                socket.socket_cookie.to_string(),
            );
            attributes.insert(
                "chronicle.socket_first_seen_ns".into(),
                socket.first_seen.nanoseconds.to_string(),
            );
            attributes.insert(
                "chronicle.socket_boot_id".into(),
                socket.first_seen.clock.boot_id.clone(),
            );
            attributes.insert(
                "chronicle.socket_role".into(),
                match evidence.role {
                    SocketRole::Active => "active",
                    SocketRole::Passive => "passive",
                }
                .into(),
            );
            if let Some(namespace) = socket.network_namespace {
                attributes.insert("chronicle.network_namespace".into(), namespace.to_string());
            }
            let (client, server) = match evidence.role {
                SocketRole::Active => (
                    evidence.local_endpoint.clone(),
                    evidence.remote_endpoint.clone(),
                ),
                SocketRole::Passive => (
                    evidence.remote_endpoint.clone(),
                    evidence.local_endpoint.clone(),
                ),
            };
            Ok((client, server, attributes, false))
        }
    }
}

fn reconstructed_connection_incomplete(
    connection: &ReconstructionConnection,
    terminal_losses: &[TerminalWalLoss],
) -> bool {
    connection.finalization.is_some()
        || connection
            .loss_windows
            .iter()
            .any(|loss| loss.classification != LossWindowClassification::Outside)
        || connection.events.iter().any(|event| {
            matches!(
                &event.evidence,
                ReconstructionEvidence::Payload(payload)
                    if payload.truncation.state != chronicle_capture::TruncationState::Complete
            )
        })
        || terminal_losses
            .iter()
            .any(|loss| terminal_loss_affects_connection(connection, loss))
}

fn terminal_loss_affects_connection(
    connection: &ReconstructionConnection,
    loss: &TerminalWalLoss,
) -> bool {
    connection.events.iter().any(|event| {
        let timestamp = &event.timestamp.monotonic;
        timestamp.clock != loss.interval.start.clock
            || (timestamp.nanoseconds >= loss.interval.start.nanoseconds
                && timestamp.nanoseconds <= loss.interval.end.nanoseconds)
    })
}

fn opaque_operation_from_protocol_stream(stream: &ProtocolStream<'_>) -> CanonicalOperation {
    let bytes: Vec<u8> = stream
        .chunks
        .iter()
        .flat_map(|chunk| chunk.payload.iter().copied())
        .collect();
    CanonicalOperation {
        id: OperationId::new(),
        sequence: stream.chunks.first().map_or(0, |chunk| chunk.sequence),
        started_at_offset: RelativeTimeNanos(0),
        completed_at_offset: None,
        kind: OperationKind::Opaque,
        effect: OperationEffect::Unknown,
        request: PayloadRef::Inline {
            content_type: Some("application/octet-stream".into()),
            bytes: bytes.clone(),
        },
        recorded_response: None,
        attributes: Attributes::new(),
        protocol_data: ProtocolData {
            schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
            media_type: Some("application/octet-stream".into()),
            bytes,
        },
        provenance: OperationProvenance::default(),
        redactions: Vec::new(),
        warnings: Vec::new(),
    }
}

#[allow(clippy::too_many_lines)] // Record-kind dispatch must retain one authoritative committed-envelope path.
fn ingest_reconstructed_envelope(
    envelope: &WalRecordEnvelope,
    assembler: &mut ReconstructionAssembler,
    evidence: &mut EtlEvidence,
    issues: &mut Vec<EtlIssue>,
    max_issues: usize,
) -> Result<(), EtlError> {
    match envelope.kind {
        RecordKind::CaptureEvent => {
            match decode_event(&envelope.payload) {
                Ok(event) => match ReconstructionInput::from_capture_event(event) {
                    Ok(ReconstructionInput::Event(mut event)) => {
                        event.wal_sequence = Some(envelope.sequence);
                        event.wal_provenance = envelope.provenance.as_ref().map(|source| {
                            ReconstructionWalProvenance {
                                segment_ordinal: source.segment_ordinal,
                                segment_first_sequence: source.segment_first_sequence,
                                frame_byte_offset: source.byte_offset,
                            }
                        });
                        assembler.push(ReconstructionInput::Event(event))?;
                        Ok(())
                    }
                    Ok(input) => {
                        assembler.push(input)?;
                        Ok(())
                    }
                    Err(error) => record_issue(
                        issues,
                        max_issues,
                        EtlIssue {
                            sequence: Some(envelope.sequence),
                            kind: EtlIssueKind::MalformedCaptureEvent,
                            message: error.to_string(),
                        },
                    ),
                },
                Err(error) => record_issue(
                    issues,
                    max_issues,
                    EtlIssue {
                        sequence: Some(envelope.sequence),
                        kind: EtlIssueKind::MalformedCaptureEvent,
                        message: error.to_string(),
                    },
                ),
            }
        }
        RecordKind::LossWindow => match decode_event(&envelope.payload) {
            Ok(CaptureEvent {
                kind: CaptureEventKind::LossWindowObserved(window),
                ..
            }) => {
                assembler.push(ReconstructionInput::LossWindow(window.clone()))?;
                evidence.loss_windows.push(SequencedEvidence {
                    sequence: Some(envelope.sequence),
                    value: window,
                });
                Ok(())
            }
            Ok(_) => record_issue(
                issues,
                max_issues,
                EtlIssue {
                    sequence: Some(envelope.sequence),
                    kind: EtlIssueKind::MalformedLossWindow,
                    message: "LossWindow envelope did not contain a loss window".into(),
                },
            ),
            Err(error) => record_issue(
                issues,
                max_issues,
                EtlIssue {
                    sequence: Some(envelope.sequence),
                    kind: EtlIssueKind::MalformedLossWindow,
                    message: error.to_string(),
                },
            ),
        },
        RecordKind::TerminalWalLoss => {
            match decode_terminal_wal_loss(envelope.schema_version, &envelope.payload) {
                Ok(loss) => {
                    assembler.push(ReconstructionInput::TerminalWalLoss(loss.clone()))?;
                    evidence.terminal_wal_losses.push(SequencedEvidence {
                        sequence: Some(envelope.sequence),
                        value: loss,
                    });
                    Ok(())
                }
                Err(error) => record_issue(
                    issues,
                    max_issues,
                    EtlIssue {
                        sequence: Some(envelope.sequence),
                        kind: EtlIssueKind::MalformedTerminalWalLoss,
                        message: error.to_string(),
                    },
                ),
            }
        }
        RecordKind::CommitMarker => ingest_commit_marker(envelope, evidence, issues, max_issues),
    }
}

fn ingest_commit_marker(
    envelope: &WalRecordEnvelope,
    evidence: &mut EtlEvidence,
    issues: &mut Vec<EtlIssue>,
    max_issues: usize,
) -> Result<(), EtlError> {
    if envelope.schema_version == COMMIT_MARKER_VERSION
        && decode_commit_marker(&envelope.payload).is_ok()
    {
        evidence.commit_marker_sequences.push(envelope.sequence);
        Ok(())
    } else {
        record_issue(
            issues,
            max_issues,
            EtlIssue {
                sequence: Some(envelope.sequence),
                kind: EtlIssueKind::MalformedCommitMarker,
                message: "invalid commit-marker schema or payload".into(),
            },
        )
    }
}

fn record_issue(
    issues: &mut Vec<EtlIssue>,
    max_issues: usize,
    issue: EtlIssue,
) -> Result<(), EtlError> {
    if issues.len() >= max_issues {
        return Err(EtlError::IssueLimit { limit: max_issues });
    }
    issues.push(issue);
    Ok(())
}

fn decode_and_canonicalize(
    registration: &chronicle_protocol::ProtocolRegistration,
    stream: &ProtocolStream<'_>,
    input_frames: &[DecodedFrame],
) -> Result<Vec<CanonicalizedOperation>, String> {
    let factory = registration
        .decoder_factory
        .as_ref()
        .ok_or_else(|| "decoder capability unavailable".to_string())?;
    let canonicalizer = registration
        .canonicalizer
        .as_ref()
        .ok_or_else(|| "canonicalizer capability unavailable".to_string())?;
    let mut decoder = factory.create();
    let mut frames = Vec::new();
    for frame in input_frames {
        frames.extend(
            decoder
                .push(frame.clone())
                .map_err(|error| error.to_string())?,
        );
    }
    frames.extend(decoder.finish().map_err(|error| error.to_string())?);
    canonicalizer
        .canonicalize(stream, frames)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_capture::{
        CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureEventKind, CaptureFlags, ClockIdentity,
        FragmentSequenceEvidence, LossAmbiguity, LossWindowObserved, MonotonicTimestamp,
        NetworkFamily, PayloadDirection, PayloadFragment, RecordingScopeIdentity, SocketEvidence,
        SocketIdentity, SocketRole, TruncationMetadata, TruncationState, encode_event,
    };
    use chronicle_common::{Endpoint, RecordingId};
    use chronicle_wal::{RecordKind, WalRecordEnvelope};

    fn payload_envelope(
        recording_id: RecordingId,
        sequence: u64,
        nanoseconds: u64,
    ) -> WalRecordEnvelope {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot".into(),
            },
            nanoseconds,
        };
        WalRecordEnvelope::unplaced(
            recording_id,
            sequence,
            RecordKind::CaptureEvent,
            CAPTURE_EVENT_SCHEMA_VERSION,
            0,
            encode_event(&CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                    timestamp: timestamp.clone(),
                    socket: SocketIdentity {
                        socket_cookie: 7,
                        first_seen: timestamp,
                        network_namespace: None,
                    },
                    recording_scope: RecordingScopeIdentity {
                        cgroup_id: 1,
                        canonical_path: "/scope".into(),
                        namespace: None,
                    },
                    network_family: NetworkFamily::Ipv4,
                    direction: PayloadDirection::Ingress,
                    sequence: FragmentSequenceEvidence {
                        tcp_sequence: 0,
                        continuation_position: 0,
                    },
                    payload: b"opaque".to_vec(),
                    truncation: TruncationMetadata {
                        captured_length: 6,
                        observed_length: Some(6),
                        state: TruncationState::Complete,
                        reason: None,
                    },
                    flags: CaptureFlags::default(),
                }),
            })
            .unwrap(),
        )
    }

    fn socket_envelope(
        recording_id: RecordingId,
        sequence: u64,
        nanoseconds: u64,
    ) -> WalRecordEnvelope {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot".into(),
            },
            nanoseconds,
        };
        WalRecordEnvelope::unplaced(
            recording_id,
            sequence,
            RecordKind::CaptureEvent,
            CAPTURE_EVENT_SCHEMA_VERSION,
            0,
            encode_event(&CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::SocketConnected(SocketEvidence {
                    timestamp: timestamp.clone(),
                    socket: SocketIdentity {
                        socket_cookie: 7,
                        first_seen: timestamp,
                        network_namespace: None,
                    },
                    recording_scope: RecordingScopeIdentity {
                        cgroup_id: 1,
                        canonical_path: "/scope".into(),
                        namespace: None,
                    },
                    network_family: NetworkFamily::Ipv4,
                    local_endpoint: Endpoint::new("192.0.2.10", 41_000),
                    remote_endpoint: Endpoint::new("192.0.2.20", 8_080),
                    role: SocketRole::Active,
                    process: None,
                    observed_cgroup_id: 1,
                }),
            })
            .unwrap(),
        )
    }

    fn event(sequence: u64) -> CaptureEvent {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot".into(),
            },
            nanoseconds: sequence,
        };
        CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                timestamp: timestamp.clone(),
                socket: SocketIdentity {
                    socket_cookie: 7,
                    first_seen: timestamp,
                    network_namespace: None,
                },
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 1,
                    canonical_path: "/scope".into(),
                    namespace: None,
                },
                network_family: NetworkFamily::Ipv4,
                direction: PayloadDirection::Egress,
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: u32::try_from(sequence).unwrap(),
                    continuation_position: 0,
                },
                payload: b"opaque bytes".to_vec(),
                truncation: TruncationMetadata {
                    captured_length: 12,
                    observed_length: Some(12),
                    state: TruncationState::Truncated,
                    reason: Some("test".into()),
                },
                flags: CaptureFlags::default(),
            }),
        }
    }

    fn endpoint_envelopes(
        recording_id: RecordingId,
        family: NetworkFamily,
        role: SocketRole,
        direction: PayloadDirection,
        local: Endpoint,
        remote: Endpoint,
    ) -> [WalRecordEnvelope; 2] {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "endpoint-test".into(),
            },
            nanoseconds: 1,
        };
        let socket = SocketIdentity {
            socket_cookie: 42,
            first_seen: timestamp.clone(),
            network_namespace: None,
        };
        let scope = RecordingScopeIdentity {
            cgroup_id: 1,
            canonical_path: "/scope".into(),
            namespace: None,
        };
        let event = |kind| {
            encode_event(&CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind,
            })
            .unwrap()
        };
        [
            WalRecordEnvelope::unplaced(
                recording_id,
                1,
                RecordKind::CaptureEvent,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                event(CaptureEventKind::SocketConnected(SocketEvidence {
                    timestamp: timestamp.clone(),
                    socket: socket.clone(),
                    recording_scope: scope.clone(),
                    network_family: family,
                    local_endpoint: local,
                    remote_endpoint: remote,
                    role,
                    process: None,
                    observed_cgroup_id: 1,
                })),
            ),
            WalRecordEnvelope::unplaced(
                recording_id,
                2,
                RecordKind::CaptureEvent,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                event(CaptureEventKind::PayloadFragment(PayloadFragment {
                    timestamp,
                    socket,
                    recording_scope: scope,
                    network_family: family,
                    direction,
                    sequence: FragmentSequenceEvidence {
                        tcp_sequence: 1,
                        continuation_position: 0,
                    },
                    payload: vec![1],
                    truncation: TruncationMetadata {
                        captured_length: 1,
                        observed_length: Some(1),
                        state: TruncationState::Complete,
                        reason: None,
                    },
                    flags: CaptureFlags::default(),
                })),
            ),
        ]
    }

    fn commit_marker_payload() -> Vec<u8> {
        chronicle_wal::encode_commit_marker(&chronicle_wal::CommitMarker {
            durable_through_sequence: 1,
            durable_record_count: 1,
            durable_payload_bytes: 1,
            batch_first_sequence: 1,
            batch_last_sequence: 1,
            batch_sha256: [0; 32],
        })
        .to_vec()
    }

    #[test]
    fn role_and_family_derive_real_canonical_endpoints() {
        for (family, role, direction, local, remote, expected_client, expected_server) in [
            (
                NetworkFamily::Ipv4,
                SocketRole::Active,
                PayloadDirection::Egress,
                Endpoint::new("192.0.2.10", 41_000),
                Endpoint::new("192.0.2.20", 8_080),
                Endpoint::new("192.0.2.10", 41_000),
                Endpoint::new("192.0.2.20", 8_080),
            ),
            (
                NetworkFamily::Ipv6,
                SocketRole::Passive,
                PayloadDirection::Ingress,
                Endpoint::new("2001:db8::20", 8_080),
                Endpoint::new("2001:db8::10", 41_000),
                Endpoint::new("2001:db8::10", 41_000),
                Endpoint::new("2001:db8::20", 8_080),
            ),
        ] {
            let envelopes =
                endpoint_envelopes(RecordingId::new(), family, role, direction, local, remote);
            let output = EtlPipeline::new(SessionLimits::default())
                .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
                .unwrap();
            let connection = &output.session.connections[0];
            assert_eq!(connection.client, expected_client);
            assert_eq!(connection.server, expected_server);
            assert_ne!(connection.client.host, "unknown");
            assert_ne!(connection.server.host, "unknown");
            assert_ne!(connection.client.port, 0);
            assert_ne!(connection.server.port, 0);
        }
    }

    #[test]
    fn missing_socket_evidence_is_incomplete_but_conflict_fails_closed() {
        let recording_id = RecordingId::new();
        let active = endpoint_envelopes(
            recording_id,
            NetworkFamily::Ipv4,
            SocketRole::Active,
            PayloadDirection::Egress,
            Endpoint::new("192.0.2.10", 41_000),
            Endpoint::new("192.0.2.20", 8_080),
        );
        let pipeline = EtlPipeline::new(SessionLimits::default());
        let incomplete = pipeline
            .process_envelopes(
                &[active[1].clone()],
                &ProtocolRegistry::new(),
                SessionId::new(),
            )
            .unwrap();
        let connection = &incomplete.session.connections[0];
        assert_eq!(connection.client, Endpoint::new("unknown", 0));
        assert_eq!(connection.server, Endpoint::new("unknown", 0));
        assert_eq!(
            incomplete.session.connection_completeness.values().next(),
            Some(&Completeness::Incomplete)
        );
        assert!(
            incomplete
                .issues
                .iter()
                .any(|issue| issue.kind == EtlIssueKind::IncompleteSocketEvidence)
        );

        let mut conflicting = endpoint_envelopes(
            recording_id,
            NetworkFamily::Ipv4,
            SocketRole::Passive,
            PayloadDirection::Egress,
            Endpoint::new("192.0.2.10", 41_000),
            Endpoint::new("192.0.2.30", 9_090),
        );
        conflicting[0].sequence = 2;
        let mut payload = active[1].clone();
        payload.sequence = 3;
        assert!(matches!(
            pipeline.process_envelopes(
                &[active[0].clone(), conflicting[0].clone(), payload],
                &ProtocolRegistry::new(),
                SessionId::new(),
            ),
            Err(EtlError::ConflictingSocketEvidence)
        ));
    }

    #[test]
    fn repeated_reconstruction_is_one_shot_equivalent() {
        let recording_id = RecordingId::new();
        let envelopes = endpoint_envelopes(
            recording_id,
            NetworkFamily::Ipv4,
            SocketRole::Active,
            PayloadDirection::Egress,
            Endpoint::new("192.0.2.10", 41_000),
            Endpoint::new("192.0.2.20", 8_080),
        );
        let pipeline = EtlPipeline::new(SessionLimits::default());
        let first = pipeline
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();
        let second = pipeline
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), first.session.id)
            .unwrap();
        verify_one_shot_equivalence(&first.session, &second.session).unwrap();
    }

    #[test]
    fn recovery_envelopes_exclude_commit_marker_from_protocol_input() {
        let recording_id = RecordingId::new();
        let envelopes = [
            socket_envelope(recording_id, 1, 1),
            WalRecordEnvelope::unplaced(
                recording_id,
                2,
                RecordKind::CaptureEvent,
                1,
                0,
                encode_event(&event(1)).unwrap(),
            ),
            WalRecordEnvelope::unplaced(
                recording_id,
                3,
                RecordKind::CommitMarker,
                1,
                0,
                commit_marker_payload(),
            ),
        ];
        let output = EtlPipeline::new(SessionLimits::default())
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert_eq!(output.session.connections.len(), 1);
        assert!(
            !output
                .issues
                .iter()
                .any(|issue| issue.kind == EtlIssueKind::MalformedCaptureEvent)
        );
    }

    #[test]
    fn recovery_envelopes_preserve_loss_and_commit_evidence() {
        let recording_id = RecordingId::new();
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot".into(),
            },
            nanoseconds: 10,
        };
        let loss = LossWindowObserved {
            start: timestamp.clone(),
            end: timestamp.clone(),
            drop_delta: Some(1),
            loss_generation: Some(1),
            ambiguity: LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some("test".into()),
            },
        };
        let terminal = chronicle_wal::TerminalWalLoss {
            interval: chronicle_wal::TerminalWalLossInterval {
                start: timestamp.clone(),
                end: timestamp,
            },
            discarded_records: 1,
            discarded_payload_bytes: 2,
            reason: chronicle_wal::TerminalWalLossReason::WalHardLimit,
            ambiguity: chronicle_wal::TerminalWalLossAmbiguity::UnknownDownstreamEffects,
        };
        let envelopes = [
            WalRecordEnvelope::unplaced(
                recording_id,
                1,
                RecordKind::LossWindow,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                encode_event(&CaptureEvent {
                    schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                    kind: CaptureEventKind::LossWindowObserved(loss.clone()),
                })
                .unwrap(),
            ),
            WalRecordEnvelope::unplaced(
                recording_id,
                2,
                RecordKind::TerminalWalLoss,
                1,
                0,
                chronicle_wal::encode_terminal_wal_loss(&terminal).unwrap(),
            ),
            WalRecordEnvelope::unplaced(
                recording_id,
                3,
                RecordKind::CommitMarker,
                1,
                0,
                commit_marker_payload(),
            ),
        ];
        let output = EtlPipeline::new(SessionLimits::default())
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert_eq!(
            output.evidence.loss_windows,
            [SequencedEvidence {
                sequence: Some(1),
                value: loss,
            }]
        );
        assert_eq!(
            output.evidence.terminal_wal_losses,
            [SequencedEvidence {
                sequence: Some(2),
                value: terminal.clone(),
            }]
        );
        assert_eq!(output.evidence.commit_marker_sequences, [3]);
        assert!(output.issues.is_empty());

        let metadata_output = EtlPipeline::new(SessionLimits::default())
            .process_envelopes_with_terminal_losses(
                &[],
                &[terminal],
                &ProtocolRegistry::new(),
                SessionId::new(),
            )
            .unwrap();
        assert_eq!(
            metadata_output.evidence.terminal_wal_losses[0].sequence,
            None
        );
    }

    #[test]
    fn reconstructed_loss_windows_only_degrade_overlapping_connections() {
        let recording_id = RecordingId::new();
        let loss = |start, end| {
            encode_event(&CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::LossWindowObserved(LossWindowObserved {
                    start: MonotonicTimestamp {
                        clock: ClockIdentity {
                            boot_id: "boot".into(),
                        },
                        nanoseconds: start,
                    },
                    end: MonotonicTimestamp {
                        clock: ClockIdentity {
                            boot_id: "boot".into(),
                        },
                        nanoseconds: end,
                    },
                    drop_delta: Some(1),
                    loss_generation: Some(1),
                    ambiguity: LossAmbiguity {
                        exact_drop_timing_unknown: true,
                        affected_sockets_unknown: true,
                        reason: None,
                    },
                }),
            })
            .unwrap()
        };
        let outside = [
            socket_envelope(recording_id, 1, 10),
            payload_envelope(recording_id, 2, 10),
            WalRecordEnvelope::unplaced(
                recording_id,
                3,
                RecordKind::LossWindow,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                loss(20, 30),
            ),
        ];
        let overlapping = [
            socket_envelope(recording_id, 1, 25),
            payload_envelope(recording_id, 2, 25),
            WalRecordEnvelope::unplaced(
                recording_id,
                3,
                RecordKind::LossWindow,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                loss(20, 30),
            ),
        ];
        let pipeline = EtlPipeline::new(SessionLimits::default());
        let outside = pipeline
            .process_envelopes(&outside, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();
        let overlapping = pipeline
            .process_envelopes(&overlapping, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert_eq!(
            outside
                .session
                .connection_completeness
                .values()
                .copied()
                .collect::<Vec<_>>(),
            [Completeness::Complete]
        );
        assert_eq!(
            overlapping
                .session
                .connection_completeness
                .values()
                .copied()
                .collect::<Vec<_>>(),
            [Completeness::Incomplete]
        );
        assert!(
            overlapping
                .session
                .operation_completeness
                .values()
                .all(|state| *state == Completeness::Incomplete)
        );
    }

    #[test]
    fn malformed_commit_marker_is_not_promoted_to_provenance() {
        let envelope = WalRecordEnvelope::unplaced(
            RecordingId::new(),
            1,
            RecordKind::CommitMarker,
            COMMIT_MARKER_VERSION + 1,
            0,
            commit_marker_payload(),
        );
        let output = EtlPipeline::new(SessionLimits::default())
            .process_envelopes(&[envelope], &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert!(output.evidence.commit_marker_sequences.is_empty());
        assert!(
            output
                .issues
                .iter()
                .any(|issue| issue.kind == EtlIssueKind::MalformedCommitMarker)
        );
    }

    #[test]
    fn malformed_committed_loss_envelopes_respect_issue_bound() {
        let recording_id = RecordingId::new();
        let envelopes = [
            WalRecordEnvelope::unplaced(
                recording_id,
                1,
                RecordKind::LossWindow,
                CAPTURE_EVENT_SCHEMA_VERSION,
                0,
                b"malformed".to_vec(),
            ),
            WalRecordEnvelope::unplaced(
                recording_id,
                2,
                RecordKind::TerminalWalLoss,
                1,
                0,
                b"malformed".to_vec(),
            ),
        ];
        let error = EtlPipeline::with_max_issues(SessionLimits::default(), 1)
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
            .unwrap_err();

        assert!(matches!(error, EtlError::IssueLimit { limit: 1 }));
    }

    #[test]
    fn terminal_wal_loss_only_degrades_overlapping_connections() {
        let recording_id = RecordingId::new();
        let terminal_loss = || chronicle_wal::TerminalWalLoss {
            interval: chronicle_wal::TerminalWalLossInterval {
                start: MonotonicTimestamp {
                    clock: ClockIdentity {
                        boot_id: "boot".into(),
                    },
                    nanoseconds: 20,
                },
                end: MonotonicTimestamp {
                    clock: ClockIdentity {
                        boot_id: "boot".into(),
                    },
                    nanoseconds: 30,
                },
            },
            discarded_records: 1,
            discarded_payload_bytes: 1,
            reason: chronicle_wal::TerminalWalLossReason::WalHardLimit,
            ambiguity: chronicle_wal::TerminalWalLossAmbiguity::UnknownDownstreamEffects,
        };
        let terminal_envelope = |sequence| {
            let loss = terminal_loss();
            WalRecordEnvelope::unplaced(
                recording_id,
                sequence,
                RecordKind::TerminalWalLoss,
                1,
                0,
                chronicle_wal::encode_terminal_wal_loss(&loss).unwrap(),
            )
        };
        let pipeline = EtlPipeline::new(SessionLimits::default());
        let outside = pipeline
            .process_envelopes(
                &[
                    socket_envelope(recording_id, 1, 10),
                    payload_envelope(recording_id, 2, 10),
                    terminal_envelope(3),
                ],
                &ProtocolRegistry::new(),
                SessionId::new(),
            )
            .unwrap();
        let overlapping = pipeline
            .process_envelopes(
                &[
                    socket_envelope(recording_id, 1, 25),
                    payload_envelope(recording_id, 2, 25),
                    terminal_envelope(3),
                ],
                &ProtocolRegistry::new(),
                SessionId::new(),
            )
            .unwrap();

        assert!(
            outside
                .session
                .connection_completeness
                .values()
                .all(|state| *state == Completeness::Complete)
        );
        assert!(
            overlapping
                .session
                .connection_completeness
                .values()
                .all(|state| *state == Completeness::Incomplete)
        );
    }

    #[test]
    fn recovery_envelopes_cross_batch_boundary_deterministically() {
        let recording_id = RecordingId::new();
        let envelopes: Vec<_> = (1..=u64::try_from(ENVELOPE_BATCH_SIZE).unwrap() + 1)
            .map(|sequence| {
                WalRecordEnvelope::unplaced(
                    recording_id,
                    sequence,
                    RecordKind::CommitMarker,
                    1,
                    0,
                    commit_marker_payload(),
                )
            })
            .collect();
        let output = EtlPipeline::new(SessionLimits::default())
            .process_envelopes(&envelopes, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert_eq!(
            output.evidence.commit_marker_sequences.len(),
            ENVELOPE_BATCH_SIZE + 1
        );
        assert_eq!(output.evidence.commit_marker_sequences[0], 1);
        assert_eq!(
            output.evidence.commit_marker_sequences.last(),
            Some(&(u64::try_from(ENVELOPE_BATCH_SIZE).unwrap() + 1))
        );
    }
}
