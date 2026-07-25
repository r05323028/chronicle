//! Protocol-neutral capture boundary. Events model ordered socket byte chunks, not packets.

use chronicle_common::{ConnectionKey, Direction, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;

pub const CAPTURE_EVENT_SCHEMA_VERSION: u16 = 1;
pub const FIXTURE_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessMetadata {
    pub pid: u32,
    pub tid: Option<u32>,
    pub executable: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContainerMetadata {
    pub id: String,
    pub name: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFlags(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub schema_version: u16,
    pub monotonic_sequence: u64,
    pub wall_time: Option<Timestamp>,
    pub connection: ConnectionKey,
    pub direction: Direction,
    pub payload: Vec<u8>,
    pub process: Option<ProcessMetadata>,
    pub container: Option<ContainerMetadata>,
    pub file_descriptor: Option<i32>,
    pub truncated: bool,
    pub flags: CaptureFlags,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture source failed: {0}")]
    Source(String),
    #[error("capture event encoding failed: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("unsupported capture event schema {0}")]
    UnsupportedSchema(u16),
    #[error(transparent)]
    Fixture(#[from] FixtureError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FixtureError {
    #[error("unsupported fixture schema {0}")]
    UnsupportedSchema(u16),
    #[error("fixture connection ID is empty")]
    EmptyConnectionId,
    #[error("fixture connection ID {0:?} is duplicated")]
    DuplicateConnectionId(String),
    #[error("fixture connection {connection_id:?} has unsupported transport {transport:?}")]
    UnsupportedTransport {
        connection_id: String,
        transport: String,
    },
    #[error("fixture connection {connection_id:?} has zero {endpoint} port")]
    ZeroPort {
        connection_id: String,
        endpoint: &'static str,
    },
    #[error("fixture connection {0:?} duplicates an existing connection key")]
    DuplicateConnectionKey(String),
    #[error("fixture event {sequence} references unknown connection {connection_id:?}")]
    UnknownConnection {
        sequence: u64,
        connection_id: String,
    },
    #[error("fixture event sequence {actual} does not match expected sequence {expected}")]
    Sequence { expected: u64, actual: u64 },
    #[error("fixture event {sequence} timestamp is invalid")]
    Timestamp { sequence: u64 },
    #[error("fixture event {sequence} timestamp decreases")]
    DecreasingTimestamp { sequence: u64 },
    #[error("fixture event {sequence} payload hex is invalid")]
    InvalidHex { sequence: u64 },
}

#[derive(Clone, Debug, Deserialize)]
pub struct Fixture {
    pub schema_version: u16,
    pub connections: Vec<FixtureConnection>,
    pub events: Vec<FixtureEvent>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FixtureConnection {
    pub id: String,
    pub network_namespace: Option<u64>,
    pub client: FixtureEndpoint,
    pub server: FixtureEndpoint,
    pub transport: String,
    pub process: Option<ProcessMetadata>,
    pub file_descriptor: Option<i32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FixtureEndpoint {
    pub host: String,
    pub port: u16,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureDirection {
    ClientToServer,
    ServerToClient,
}

impl From<FixtureDirection> for Direction {
    fn from(value: FixtureDirection) -> Self {
        match value {
            FixtureDirection::ClientToServer => Self::ClientToServer,
            FixtureDirection::ServerToClient => Self::ServerToClient,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct FixtureEvent {
    pub sequence: u64,
    pub timestamp: String,
    pub connection_id: String,
    pub direction: FixtureDirection,
    pub payload_hex: String,
    pub truncated: bool,
    pub flags: u32,
}

#[derive(Debug)]
pub struct FixtureCaptureSource {
    events: VecDeque<CaptureEvent>,
}

impl FixtureCaptureSource {
    pub fn from_json(bytes: &[u8]) -> Result<Self, CaptureError> {
        let fixture: Fixture = serde_json::from_slice(bytes)?;
        Self::from_fixture(fixture).map_err(CaptureError::from)
    }

    #[allow(clippy::too_many_lines)] // Fixture validation stays in input order.
    pub fn from_fixture(fixture: Fixture) -> Result<Self, FixtureError> {
        if fixture.schema_version != FIXTURE_SCHEMA_VERSION {
            return Err(FixtureError::UnsupportedSchema(fixture.schema_version));
        }

        let mut connections = BTreeMap::new();
        for connection in fixture.connections {
            if connection.id.is_empty() {
                return Err(FixtureError::EmptyConnectionId);
            }
            if connection.transport != "tcp" {
                return Err(FixtureError::UnsupportedTransport {
                    connection_id: connection.id,
                    transport: connection.transport,
                });
            }
            if connection.client.port == 0 {
                return Err(FixtureError::ZeroPort {
                    connection_id: connection.id,
                    endpoint: "client",
                });
            }
            if connection.server.port == 0 {
                return Err(FixtureError::ZeroPort {
                    connection_id: connection.id,
                    endpoint: "server",
                });
            }
            let key = ConnectionKey {
                network_namespace: connection.network_namespace,
                client: chronicle_common::Endpoint::new(
                    connection.client.host,
                    connection.client.port,
                ),
                server: chronicle_common::Endpoint::new(
                    connection.server.host,
                    connection.server.port,
                ),
                transport: chronicle_common::TransportProtocol::Tcp,
            };
            if connections
                .values()
                .any(|existing: &FixtureConnectionState| existing.key == key)
            {
                return Err(FixtureError::DuplicateConnectionKey(connection.id));
            }
            if connections
                .insert(
                    connection.id.clone(),
                    FixtureConnectionState {
                        key,
                        process: connection.process,
                        file_descriptor: connection.file_descriptor,
                    },
                )
                .is_some()
            {
                return Err(FixtureError::DuplicateConnectionId(connection.id));
            }
        }

        let mut expected_sequence = 1_u64;
        let mut previous_timestamp = None;
        let mut events = VecDeque::with_capacity(fixture.events.len());
        for event in fixture.events {
            if event.sequence != expected_sequence {
                return Err(FixtureError::Sequence {
                    expected: expected_sequence,
                    actual: event.sequence,
                });
            }
            expected_sequence = expected_sequence.saturating_add(1);
            let timestamp = Timestamp::parse(&event.timestamp, &Rfc3339).map_err(|_| {
                FixtureError::Timestamp {
                    sequence: event.sequence,
                }
            })?;
            if previous_timestamp.is_some_and(|previous| timestamp < previous) {
                return Err(FixtureError::DecreasingTimestamp {
                    sequence: event.sequence,
                });
            }
            previous_timestamp = Some(timestamp);
            let state = connections.get(&event.connection_id).ok_or_else(|| {
                FixtureError::UnknownConnection {
                    sequence: event.sequence,
                    connection_id: event.connection_id.clone(),
                }
            })?;
            events.push_back(CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                monotonic_sequence: event.sequence,
                wall_time: Some(timestamp),
                connection: state.key.clone(),
                direction: event.direction.into(),
                payload: decode_hex(&event.payload_hex).ok_or(FixtureError::InvalidHex {
                    sequence: event.sequence,
                })?,
                process: state.process.clone(),
                container: None,
                file_descriptor: state.file_descriptor,
                truncated: event.truncated,
                flags: CaptureFlags(event.flags),
            });
        }
        Ok(Self { events })
    }
}

struct FixtureConnectionState {
    key: ConnectionKey,
    process: Option<ProcessMetadata>,
    file_descriptor: Option<i32>,
}

impl CaptureSource for FixtureCaptureSource {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        Ok(self.events.pop_front())
    }
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = char::from(pair[0]).to_digit(16)?;
            let low = char::from(pair[1]).to_digit(16)?;
            u8::try_from((high << 4) | low).ok()
        })
        .collect()
}

pub trait CaptureSource: Send {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError>;
}

#[derive(Default)]
pub struct InMemoryCaptureSource {
    events: VecDeque<CaptureEvent>,
}

impl InMemoryCaptureSource {
    pub fn new(events: impl IntoIterator<Item = CaptureEvent>) -> Self {
        Self {
            events: events.into_iter().collect(),
        }
    }
}

impl CaptureSource for InMemoryCaptureSource {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        Ok(self.events.pop_front())
    }
}

pub fn encode_event(event: &CaptureEvent) -> Result<Vec<u8>, CaptureError> {
    if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
        return Err(CaptureError::UnsupportedSchema(event.schema_version));
    }
    Ok(serde_json::to_vec(event)?)
}

pub fn decode_event(bytes: &[u8]) -> Result<CaptureEvent, CaptureError> {
    let event: CaptureEvent = serde_json::from_slice(bytes)?;
    if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
        return Err(CaptureError::UnsupportedSchema(event.schema_version));
    }
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "schema_version": 1,
        "connections": [{
            "id": "basic",
            "network_namespace": null,
            "client": {"host": "fixture-client", "port": 41000},
            "server": {"host": "recorded.invalid", "port": 8080},
            "transport": "tcp",
            "process": {"pid": 7, "tid": null, "executable": "fixture"},
            "file_descriptor": 3
        }],
        "events": [{
            "sequence": 1,
            "timestamp": "2026-01-01T00:00:00Z",
            "connection_id": "basic",
            "direction": "client_to_server",
            "payload_hex": "00ff",
            "truncated": false,
            "flags": 0
        }]
    }"#;

    #[test]
    fn fixture_source_preserves_binary_socket_chunk() {
        let mut source = FixtureCaptureSource::from_json(FIXTURE.as_bytes()).unwrap();
        let event = source.next_event().unwrap().unwrap();
        assert_eq!(event.payload, [0, 255]);
        assert_eq!(event.direction, Direction::ClientToServer);
        assert_eq!(event.connection.server.host, "recorded.invalid");
        assert!(source.next_event().unwrap().is_none());
    }

    #[test]
    fn fixture_rejects_schema_sequence_timestamp_and_hex_errors() {
        for (input, expected) in [
            (
                FIXTURE.replacen("\"schema_version\": 1", "\"schema_version\": 2", 1),
                "unsupported fixture schema 2",
            ),
            (
                FIXTURE.replacen("\"sequence\": 1", "\"sequence\": 2", 1),
                "does not match expected sequence 1",
            ),
            (
                FIXTURE.replacen("2026-01-01T00:00:00Z", "bad", 1),
                "timestamp is invalid",
            ),
            (
                FIXTURE.replacen("\"payload_hex\": \"00ff\"", "\"payload_hex\": \"0\"", 1),
                "payload hex is invalid",
            ),
        ] {
            let error = FixtureCaptureSource::from_json(input.as_bytes()).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn fixture_rejects_unknown_connection_and_unsafe_connection_shape() {
        for (input, expected) in [
            (
                FIXTURE.replacen(
                    "\"connection_id\": \"basic\"",
                    "\"connection_id\": \"missing\"",
                    1,
                ),
                "unknown connection",
            ),
            (
                FIXTURE.replacen("\"transport\": \"tcp\"", "\"transport\": \"udp\"", 1),
                "unsupported transport",
            ),
            (
                FIXTURE.replacen("\"port\": 41000", "\"port\": 0", 1),
                "zero client port",
            ),
        ] {
            let error = FixtureCaptureSource::from_json(input.as_bytes()).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn checked_in_http_fixtures_are_valid_and_credential_free() {
        let fixture_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/http");
        for entry in std::fs::read_dir(fixture_dir).unwrap() {
            let path = entry.unwrap().path();
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(
                !text.to_ascii_lowercase().contains("authorization"),
                "{path:?}"
            );
            FixtureCaptureSource::from_json(&bytes).unwrap();
        }
    }
}
