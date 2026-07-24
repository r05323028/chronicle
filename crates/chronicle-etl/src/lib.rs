//! Restartable WAL-to-canonical transformation boundary.

use chronicle_canonical::{
    Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalOperation,
    CanonicalSession, OperationEffect, OperationKind, PayloadRef, ProtocolData, RelativeTimeNanos,
    ReplayMetadata, SourceMetadata, TimelineEntry,
};
use chronicle_capture::decode_event;
use chronicle_common::{ConnectionId, OperationId, ProtocolId, SessionId};
use chronicle_protocol::{DecodedFrame, ProtocolRegistry, ProtocolStream, StreamChunk};
use chronicle_session::{ConnectionStream, SessionAssembler, SessionError, SessionLimits};
use chronicle_wal::{ReadOutcome, WalError, WalReader, WalRecord};
use std::io::Read;
use thiserror::Error;
use time::OffsetDateTime;

pub const DEFAULT_MAX_ISSUES: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EtlIssueKind {
    MalformedCaptureEvent,
    CaptureSequenceMismatch,
    PartialWalTail,
    UnknownProtocol,
    ProtocolDecode,
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
}

#[derive(Debug, Error)]
pub enum EtlError {
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Session(#[from] SessionError),
    #[error("ETL issue limit {limit} exceeded")]
    IssueLimit { limit: usize },
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

    pub fn process<R: Read>(
        &self,
        reader: &mut WalReader<R>,
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        let mut assembler = SessionAssembler::new(self.limits);
        let mut issues = Vec::new();
        loop {
            match reader.next_record()? {
                ReadOutcome::Record(record) => {
                    ingest_record(&record, &mut assembler, &mut issues, self.max_issues)?;
                }
                ReadOutcome::End => break,
                ReadOutcome::PartialTail { offset } => {
                    record_issue(
                        &mut issues,
                        self.max_issues,
                        EtlIssue {
                            sequence: None,
                            kind: EtlIssueKind::PartialWalTail,
                            message: format!("partial WAL record at byte offset {offset}"),
                        },
                    )?;
                    break;
                }
            }
        }

        let streams = assembler.finish();
        let started_at = streams
            .iter()
            .flat_map(|stream| stream.chunks.iter().filter_map(|event| event.wall_time))
            .min()
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);
        let ended_at = streams
            .iter()
            .flat_map(|stream| stream.chunks.iter().filter_map(|event| event.wall_time))
            .max();
        let mut connections = Vec::with_capacity(streams.len());
        let mut timeline = Vec::new();

        for stream in streams {
            let connection_id = ConnectionId::new();
            let protocol_stream = protocol_stream(&stream);
            let (protocol, operations) =
                if let Some((registration, _)) = registry.detect(&protocol_stream, None) {
                    match decode_and_canonicalize(registration, &protocol_stream) {
                        Ok(operations) => (registration.id.clone(), operations),
                        Err(error) => {
                            record_issue(
                                &mut issues,
                                self.max_issues,
                                EtlIssue {
                                    sequence: stream.first_sequence(),
                                    kind: EtlIssueKind::ProtocolDecode,
                                    message: error,
                                },
                            )?;
                            (registration.id.clone(), vec![opaque_operation(&stream)])
                        }
                    }
                } else {
                    record_issue(
                        &mut issues,
                        self.max_issues,
                        EtlIssue {
                            sequence: stream.first_sequence(),
                            kind: EtlIssueKind::UnknownProtocol,
                            message: "traffic preserved as opaque bytes".into(),
                        },
                    )?;
                    (ProtocolId::unknown(), vec![opaque_operation(&stream)])
                };
            for operation in &operations {
                timeline.push(TimelineEntry {
                    sequence: operation.sequence,
                    connection_id,
                    operation_id: operation.id,
                    offset: operation.started_at_offset,
                });
            }
            connections.push(CanonicalConnection {
                id: connection_id,
                protocol,
                client: stream.key.client.clone(),
                server: stream.key.server.clone(),
                attributes: Attributes::new(),
                operations,
                incomplete: stream.chunks.is_empty(),
                truncated: stream.truncated,
            });
        }
        timeline.sort_by_key(|entry| entry.sequence);
        Ok(EtlOutput {
            session: CanonicalSession {
                schema_version: CANONICAL_SCHEMA_VERSION,
                id: session_id,
                started_at,
                ended_at,
                source: SourceMetadata::default(),
                connections,
                timeline,
                replay: ReplayMetadata::default(),
            },
            issues,
        })
    }
}

fn ingest_record(
    record: &WalRecord,
    assembler: &mut SessionAssembler,
    issues: &mut Vec<EtlIssue>,
    max_issues: usize,
) -> Result<(), EtlError> {
    match decode_event(&record.payload) {
        Ok(event) if event.monotonic_sequence == record.sequence => {
            assembler.push(event)?;
            Ok(())
        }
        Ok(event) => record_issue(
            issues,
            max_issues,
            EtlIssue {
                sequence: Some(record.sequence),
                kind: EtlIssueKind::CaptureSequenceMismatch,
                message: format!(
                    "capture sequence {} does not match WAL sequence {}",
                    event.monotonic_sequence, record.sequence
                ),
            },
        ),
        Err(error) => record_issue(
            issues,
            max_issues,
            EtlIssue {
                sequence: Some(record.sequence),
                kind: EtlIssueKind::MalformedCaptureEvent,
                message: error.to_string(),
            },
        ),
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

fn protocol_stream(stream: &ConnectionStream) -> ProtocolStream<'_> {
    ProtocolStream {
        chunks: stream
            .chunks
            .iter()
            .map(|event| StreamChunk {
                direction: event.direction,
                sequence: event.monotonic_sequence,
                payload: &event.payload,
            })
            .collect(),
        truncated: stream.truncated,
    }
}

fn decode_and_canonicalize(
    registration: &chronicle_protocol::ProtocolRegistration,
    stream: &ProtocolStream<'_>,
) -> Result<Vec<CanonicalOperation>, String> {
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
    for chunk in &stream.chunks {
        frames.extend(
            decoder
                .push(DecodedFrame {
                    direction: chunk.direction,
                    sequence: chunk.sequence,
                    payload: chunk.payload.to_vec(),
                    attributes: Attributes::new(),
                })
                .map_err(|error| error.to_string())?,
        );
    }
    frames.extend(decoder.finish().map_err(|error| error.to_string())?);
    canonicalizer
        .canonicalize(stream, frames)
        .map_err(|error| error.to_string())
}

fn opaque_operation(stream: &ConnectionStream) -> CanonicalOperation {
    let bytes: Vec<u8> = stream
        .chunks
        .iter()
        .flat_map(|event| event.payload.iter().copied())
        .collect();
    CanonicalOperation {
        id: OperationId::new(),
        sequence: stream.first_sequence().unwrap_or_default(),
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
            schema_version: 1,
            media_type: Some("application/octet-stream".into()),
            bytes,
        },
        incomplete: true,
        truncated: stream.truncated,
        redactions: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_capture::{
        CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, encode_event,
    };
    use chronicle_common::{ConnectionKey, Direction, Endpoint, TransportProtocol};
    use chronicle_wal::{RecordKind, WalRecord, encode_record};
    use std::io::Cursor;

    fn event(sequence: u64) -> CaptureEvent {
        CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: sequence,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("client", 1),
                Endpoint::new("server", 2),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"opaque bytes".to_vec(),
            process: None,
            container: None,
            file_descriptor: None,
            truncated: true,
            flags: CaptureFlags::default(),
        }
    }

    #[test]
    fn unknown_traffic_is_preserved_as_opaque() {
        let event = event(1);
        let record = WalRecord {
            kind: RecordKind::CaptureEvent,
            flags: 0,
            sequence: 1,
            payload: encode_event(&event).unwrap(),
        };
        let mut reader = WalReader::new(Cursor::new(encode_record(&record).unwrap()), 1);
        let output = EtlPipeline::new(SessionLimits::default())
            .process(&mut reader, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert_eq!(output.issues[0].kind, EtlIssueKind::UnknownProtocol);
        let connection = &output.session.connections[0];
        assert_eq!(connection.protocol, ProtocolId::unknown());
        assert!(connection.truncated);
        assert_eq!(
            connection.operations[0].protocol_data.bytes,
            b"opaque bytes"
        );
    }

    #[test]
    fn rejects_capture_sequence_that_differs_from_wal_sequence() {
        let record = WalRecord {
            kind: RecordKind::CaptureEvent,
            flags: 0,
            sequence: 1,
            payload: encode_event(&event(2)).unwrap(),
        };
        let mut reader = WalReader::new(Cursor::new(encode_record(&record).unwrap()), 1);
        let output = EtlPipeline::new(SessionLimits::default())
            .process(&mut reader, &ProtocolRegistry::new(), SessionId::new())
            .unwrap();

        assert!(output.session.connections.is_empty());
        assert_eq!(output.issues.len(), 1);
        assert_eq!(output.issues[0].kind, EtlIssueKind::CaptureSequenceMismatch);
    }

    #[test]
    fn stops_when_issue_limit_is_reached() {
        let malformed = |sequence| WalRecord {
            kind: RecordKind::CaptureEvent,
            flags: 0,
            sequence,
            payload: b"not-json".to_vec(),
        };
        let mut bytes = encode_record(&malformed(1)).unwrap();
        bytes.extend(encode_record(&malformed(2)).unwrap());
        let mut reader = WalReader::new(Cursor::new(bytes), 1);
        let error = EtlPipeline::with_max_issues(SessionLimits::default(), 1)
            .process(&mut reader, &ProtocolRegistry::new(), SessionId::new())
            .unwrap_err();

        assert!(matches!(error, EtlError::IssueLimit { limit: 1 }));
    }
}
