//! Protocol-neutral capture boundary. Events model ordered socket byte chunks, not packets.

use chronicle_common::{ConnectionKey, Direction, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use thiserror::Error;
use time::format_description::well_known::Rfc3339;

pub const CAPTURE_EVENT_SCHEMA_VERSION: u16 = 1;
pub const CAPTURE_EVENT_V2_SCHEMA_VERSION: u16 = 2;
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

/// Capture Event v1. Retained for fixtures and existing WAL readers.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureEventV1 {
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

/// Monotonic clock domain. Values from distinct boot identities are never correlated.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ClockIdentity {
    pub boot_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MonotonicTimestamp {
    pub clock: ClockIdentity,
    pub nanoseconds: u64,
}

/// Kernel socket identity. PID, file descriptor, and five-tuple are evidence, not identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SocketIdentity {
    pub socket_cookie: u64,
    pub first_seen: MonotonicTimestamp,
    pub network_namespace: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingScopeIdentity {
    pub cgroup_id: u64,
    pub canonical_path: String,
    pub namespace: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketEvidence {
    pub timestamp: MonotonicTimestamp,
    pub socket: SocketIdentity,
    pub recording_scope: RecordingScopeIdentity,
    pub network_family: NetworkFamily,
    pub process: Option<ProcessMetadata>,
    pub observed_cgroup_id: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketStateChangeObserved {
    pub socket: SocketEvidence,
    pub raw_state: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadDirection {
    Ingress,
    Egress,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FragmentSequenceEvidence {
    pub tcp_sequence: u32,
    pub continuation_position: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TruncationState {
    Complete,
    Truncated,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TruncationMetadata {
    pub captured_length: u32,
    pub observed_length: Option<u32>,
    pub state: TruncationState,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PayloadFragment {
    pub timestamp: MonotonicTimestamp,
    pub socket: SocketIdentity,
    pub recording_scope: RecordingScopeIdentity,
    pub network_family: NetworkFamily,
    pub direction: PayloadDirection,
    pub sequence: FragmentSequenceEvidence,
    pub payload: Vec<u8>,
    pub truncation: TruncationMetadata,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossAmbiguity {
    pub exact_drop_timing_unknown: bool,
    pub affected_sockets_unknown: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LossWindowObserved {
    pub start: MonotonicTimestamp,
    pub end: MonotonicTimestamp,
    pub drop_delta: Option<u64>,
    pub loss_generation: Option<u64>,
    pub ambiguity: LossAmbiguity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LossCounterSample {
    pub timestamp: MonotonicTimestamp,
    pub total: u64,
    pub generation: Option<u64>,
}

/// Converts cumulative loss counters into bounded, explicitly ambiguous windows.
#[derive(Clone, Debug)]
pub struct LossWindowSampler {
    attached_at: MonotonicTimestamp,
    previous: Option<LossCounterSample>,
}

impl LossWindowSampler {
    pub fn new(attached_at: MonotonicTimestamp) -> Self {
        Self {
            attached_at,
            previous: None,
        }
    }

    pub fn reset_attachment(&mut self, attached_at: MonotonicTimestamp) {
        self.attached_at = attached_at;
        self.previous = None;
    }

    pub fn incomplete_sample(
        &self,
        timestamp: MonotonicTimestamp,
        reason: impl Into<String>,
    ) -> LossWindowObserved {
        LossWindowObserved {
            start: self.previous.as_ref().map_or_else(
                || self.attached_at.clone(),
                |previous| previous.timestamp.clone(),
            ),
            end: timestamp,
            drop_delta: None,
            loss_generation: None,
            ambiguity: LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some(reason.into()),
            },
        }
    }

    pub fn observe(&mut self, current: LossCounterSample) -> Option<LossWindowObserved> {
        let previous = self.previous.replace(current.clone());
        let (start, drop_delta, reason) = match previous {
            None if current.total == 0 => return None,
            None => (
                self.attached_at.clone(),
                Some(current.total),
                "before first complete sample",
            ),
            Some(previous) if previous.timestamp.clock != current.timestamp.clock => {
                (previous.timestamp, None, "counter clock identity changed")
            }
            Some(previous)
                if previous.generation != current.generation || current.total < previous.total =>
            {
                (
                    previous.timestamp,
                    None,
                    "counter generation changed or regressed",
                )
            }
            Some(previous) => {
                let delta = current.total - previous.total;
                if delta == 0 {
                    return None;
                }
                (
                    previous.timestamp,
                    Some(delta),
                    "loss occurred between complete counter samples",
                )
            }
        };
        Some(LossWindowObserved {
            start,
            end: current.timestamp,
            drop_delta,
            loss_generation: current.generation,
            ambiguity: LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some(reason.into()),
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "evidence", rename_all = "snake_case")]
pub enum CaptureEventKind {
    SocketConnectObserved(SocketEvidence),
    SocketConnected(SocketEvidence),
    SocketClosedObserved(SocketEvidence),
    SocketResetObserved(SocketEvidence),
    SocketStateChangedObserved(SocketStateChangeObserved),
    PayloadFragment(PayloadFragment),
    LossWindowObserved(LossWindowObserved),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureEventV2 {
    pub schema_version: u16,
    #[serde(flatten)]
    pub kind: CaptureEventKind,
}

/// Versioned capture stream. V1 serializes to its original flat JSON shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CaptureEvent {
    V2(CaptureEventV2),
    V1(CaptureEventV1),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum V2FixtureObservation {
    Event(CaptureEventV2),
    LossWindow(LossWindowObserved),
}

#[derive(Debug)]
pub struct V2FixtureSource {
    observations: VecDeque<V2FixtureObservation>,
}

impl V2FixtureSource {
    pub fn new(
        observations: impl IntoIterator<Item = V2FixtureObservation>,
    ) -> Result<Self, CaptureError> {
        let observations: VecDeque<_> = observations.into_iter().collect();
        for observation in &observations {
            if let V2FixtureObservation::Event(event) = observation
                && event.schema_version != CAPTURE_EVENT_V2_SCHEMA_VERSION
            {
                return Err(CaptureError::UnsupportedSchema(event.schema_version));
            }
        }
        Ok(Self { observations })
    }

    pub fn next_observation(&mut self) -> Option<V2FixtureObservation> {
        self.observations.pop_front()
    }
}

impl CaptureEvent {
    pub fn schema_version(&self) -> u16 {
        match self {
            Self::V1(event) => event.schema_version,
            Self::V2(event) => event.schema_version,
        }
    }

    pub fn as_v1(&self) -> Option<&CaptureEventV1> {
        match self {
            Self::V1(event) => Some(event),
            Self::V2(_) => None,
        }
    }

    pub fn monotonic_sequence(&self) -> Option<u64> {
        self.as_v1().map(|event| event.monotonic_sequence)
    }
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("capture source failed: {0}")]
    Source(String),
    #[error("capture source lifecycle action {action} is invalid while {state}")]
    Lifecycle {
        action: &'static str,
        state: CaptureSourceState,
    },
    #[error("final capture loss sample failed: {0}")]
    FinalLossSample(String),
    #[error("capture drain failed: {0}")]
    Drain(String),
    #[error("capture cleanup failed: {0}")]
    Cleanup(String),
    #[error("capture event encoding failed: {0}")]
    Codec(#[from] serde_json::Error),
    #[error("unsupported capture event schema {0}")]
    UnsupportedSchema(u16),
    #[error(transparent)]
    Fixture(#[from] FixtureError),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptureSourceState {
    Created,
    Running,
    ShutdownRequested,
    Draining,
    Finalized,
}

impl std::fmt::Display for CaptureSourceState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl CaptureSourceState {
    pub fn request_shutdown(&mut self) -> Result<(), CaptureError> {
        match self {
            Self::Running => *self = Self::ShutdownRequested,
            Self::ShutdownRequested | Self::Draining => {}
            state => {
                return Err(CaptureError::Lifecycle {
                    action: "request_shutdown",
                    state: *state,
                });
            }
        }
        Ok(())
    }

    pub fn begin_drain(&mut self) -> Result<(), CaptureError> {
        match self {
            Self::ShutdownRequested => *self = Self::Draining,
            Self::Draining => {}
            state => {
                return Err(CaptureError::Lifecycle {
                    action: "drain",
                    state: *state,
                });
            }
        }
        Ok(())
    }

    pub fn finalize(&mut self, drain_complete: bool) -> Result<(), CaptureError> {
        if *self != Self::Draining || !drain_complete {
            return Err(CaptureError::Lifecycle {
                action: "finalize",
                state: *self,
            });
        }
        *self = Self::Finalized;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CaptureSourceSummary {
    pub emitted_events: u64,
    pub final_loss_events: u64,
    pub final_loss_sample_failed: bool,
    pub drain_failed: bool,
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
            events.push_back(CaptureEvent::V1(CaptureEventV1 {
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
            }));
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

/// Ordered kernel-capture lifecycle.
///
/// `request_shutdown` freezes intake and makes final diagnostics eligible; `drain`
/// emits events already accepted before that boundary; `finalize` releases source
/// resources only after draining. Recording status, shutdown reason, WAL commits,
/// and replay decisions remain outside this boundary.
pub trait CaptureSource: Send {
    /// Enter the running state. Sources returned already running may make this idempotent.
    fn start(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    /// Consume currently available normalized events. Never final-sample or release resources.
    fn poll(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.next_event()
    }

    /// Stop accepting new input and make final loss observation eligible.
    fn request_shutdown(&mut self) -> Result<(), CaptureError> {
        Ok(())
    }

    /// Consume events accepted before shutdown, then final diagnostics.
    fn drain(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.poll()
    }

    /// Release source resources after drain and return source-local diagnostics.
    fn finalize(&mut self) -> Result<CaptureSourceSummary, CaptureError> {
        Ok(CaptureSourceSummary::default())
    }

    /// Compatibility alias for existing finite fixture sources and callers.
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
    match event {
        CaptureEvent::V1(event) if event.schema_version == CAPTURE_EVENT_SCHEMA_VERSION => {
            Ok(serde_json::to_vec(event)?)
        }
        CaptureEvent::V2(event) if event.schema_version == CAPTURE_EVENT_V2_SCHEMA_VERSION => {
            Ok(serde_json::to_vec(event)?)
        }
        _ => Err(CaptureError::UnsupportedSchema(event.schema_version())),
    }
}

pub fn decode_event(bytes: &[u8]) -> Result<CaptureEvent, CaptureError> {
    let event: CaptureEvent = serde_json::from_slice(bytes)?;
    match event {
        CaptureEvent::V1(event) if event.schema_version == CAPTURE_EVENT_SCHEMA_VERSION => {
            Ok(CaptureEvent::V1(event))
        }
        CaptureEvent::V2(event) if event.schema_version == CAPTURE_EVENT_V2_SCHEMA_VERSION => {
            Ok(CaptureEvent::V2(event))
        }
        event => Err(CaptureError::UnsupportedSchema(event.schema_version())),
    }
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
        let event = event.as_v1().unwrap();
        assert_eq!(event.payload, [0, 255]);
        assert_eq!(event.direction, Direction::ClientToServer);
        assert_eq!(event.connection.server.host, "recorded.invalid");
        assert!(source.next_event().unwrap().is_none());
    }

    #[test]
    fn v1_bytes_stay_flat_while_v2_carries_explicit_evidence_version() {
        let mut source = FixtureCaptureSource::from_json(FIXTURE.as_bytes()).unwrap();
        let v1 = source.next_event().unwrap().unwrap();
        let v1_bytes = encode_event(&v1).unwrap();
        let v1_json: serde_json::Value = serde_json::from_slice(&v1_bytes).unwrap();
        assert!(v1_json.get("kind").is_none());
        assert!(matches!(
            decode_event(&v1_bytes).unwrap(),
            CaptureEvent::V1(_)
        ));

        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 1,
        };
        let v2 = CaptureEvent::V2(CaptureEventV2 {
            schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketConnectObserved(SocketEvidence {
                timestamp: timestamp.clone(),
                socket: SocketIdentity {
                    socket_cookie: 7,
                    first_seen: timestamp,
                    network_namespace: Some(9),
                },
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 11,
                    canonical_path: "/scope".into(),
                    namespace: Some(9),
                },
                network_family: NetworkFamily::Ipv4,
                process: None,
                observed_cgroup_id: 12,
            }),
        });
        let v2_json = serde_json::to_value(&v2).unwrap();
        assert_eq!(v2_json["schema_version"], CAPTURE_EVENT_V2_SCHEMA_VERSION);
        assert_eq!(v2_json["kind"], "socket_connect_observed");
        let round_trip = decode_event(&encode_event(&v2).unwrap()).unwrap();
        assert_eq!(round_trip, v2);
    }

    #[test]
    fn v2_payload_round_trips_binary_capture_evidence() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 10,
        };
        let event = CaptureEvent::V2(CaptureEventV2 {
            schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION,
            kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                timestamp: timestamp.clone(),
                socket: SocketIdentity {
                    socket_cookie: 7,
                    first_seen: timestamp,
                    network_namespace: Some(9),
                },
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 11,
                    canonical_path: "/scope".into(),
                    namespace: Some(9),
                },
                network_family: NetworkFamily::Ipv6,
                direction: PayloadDirection::Ingress,
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: 42,
                    continuation_position: 1,
                },
                payload: vec![0, 255, 1],
                truncation: TruncationMetadata {
                    captured_length: 3,
                    observed_length: Some(5),
                    state: TruncationState::Truncated,
                    reason: Some("kernel capture bound".into()),
                },
            }),
        });

        assert_eq!(decode_event(&encode_event(&event).unwrap()).unwrap(), event);
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn v2_fixture_source_keeps_lifecycle_payload_and_loss_observations_distinct() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "fixture-boot".into(),
            },
            nanoseconds: 100,
        };
        let socket = SocketIdentity {
            socket_cookie: 7,
            first_seen: timestamp.clone(),
            network_namespace: Some(9),
        };
        let scope = RecordingScopeIdentity {
            cgroup_id: 11,
            canonical_path: "/scope".into(),
            namespace: Some(9),
        };
        let lifecycle = CaptureEventV2 {
            schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                socket: SocketEvidence {
                    timestamp: timestamp.clone(),
                    socket: socket.clone(),
                    recording_scope: scope.clone(),
                    network_family: NetworkFamily::Ipv4,
                    process: None,
                    observed_cgroup_id: 11,
                },
                raw_state: 99,
            }),
        };
        let payload = CaptureEventV2 {
            schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION,
            kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                timestamp: MonotonicTimestamp {
                    clock: timestamp.clock.clone(),
                    nanoseconds: 101,
                },
                socket,
                recording_scope: scope,
                network_family: NetworkFamily::Ipv4,
                direction: PayloadDirection::Egress,
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: 42,
                    continuation_position: 1,
                },
                payload: vec![0, 255],
                truncation: TruncationMetadata {
                    captured_length: 2,
                    observed_length: Some(3),
                    state: TruncationState::Truncated,
                    reason: Some("fixture capture bound".into()),
                },
            }),
        };
        let loss = LossWindowObserved {
            start: timestamp.clone(),
            end: MonotonicTimestamp {
                clock: timestamp.clock,
                nanoseconds: 200,
            },
            drop_delta: Some(2),
            loss_generation: Some(1),
            ambiguity: LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some("fixture loss".into()),
            },
        };
        let mut source = V2FixtureSource::new([
            V2FixtureObservation::Event(lifecycle.clone()),
            V2FixtureObservation::Event(payload.clone()),
            V2FixtureObservation::LossWindow(loss.clone()),
        ])
        .unwrap();

        assert_eq!(
            source.next_observation(),
            Some(V2FixtureObservation::Event(lifecycle))
        );
        assert_eq!(
            source.next_observation(),
            Some(V2FixtureObservation::Event(payload))
        );
        assert_eq!(
            source.next_observation(),
            Some(V2FixtureObservation::LossWindow(loss))
        );
        assert_eq!(source.next_observation(), None);
        assert!(matches!(
            V2FixtureSource::new([V2FixtureObservation::Event(CaptureEventV2 {
                schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION + 1,
                kind: CaptureEventKind::SocketConnected(SocketEvidence {
                    timestamp: MonotonicTimestamp {
                        clock: ClockIdentity {
                            boot_id: "fixture-boot".into(),
                        },
                        nanoseconds: 1,
                    },
                    socket: SocketIdentity {
                        socket_cookie: 1,
                        first_seen: MonotonicTimestamp {
                            clock: ClockIdentity {
                                boot_id: "fixture-boot".into(),
                            },
                            nanoseconds: 1,
                        },
                        network_namespace: None,
                    },
                    recording_scope: RecordingScopeIdentity {
                        cgroup_id: 1,
                        canonical_path: "/scope".into(),
                        namespace: None,
                    },
                    network_family: NetworkFamily::Ipv4,
                    process: None,
                    observed_cgroup_id: 1,
                }),
            })]),
            Err(CaptureError::UnsupportedSchema(_))
        ));
    }

    #[test]
    fn loss_window_sampler_uses_actual_fake_monotonic_boundaries() {
        let clock = ClockIdentity {
            boot_id: "fixture-boot".into(),
        };
        let sample = |nanoseconds, total, generation| LossCounterSample {
            timestamp: MonotonicTimestamp {
                clock: clock.clone(),
                nanoseconds,
            },
            total,
            generation,
        };
        let mut sampler = LossWindowSampler::new(sample(0, 0, Some(1)).timestamp);
        assert_eq!(sampler.observe(sample(100, 0, Some(1))), None);
        assert_eq!(sampler.observe(sample(200, 0, Some(1))), None);

        let regular = sampler.observe(sample(300, 2, Some(1))).unwrap();
        assert_eq!(regular.start.nanoseconds, 200);
        assert_eq!(regular.end.nanoseconds, 300);
        assert_eq!(regular.drop_delta, Some(2));

        let delayed = sampler.observe(sample(550, 3, Some(1))).unwrap();
        assert_eq!(delayed.start.nanoseconds, 300);
        assert_eq!(delayed.end.nanoseconds, 550);
        assert_eq!(delayed.drop_delta, Some(1));

        let reset = sampler.observe(sample(650, 1, Some(2))).unwrap();
        assert_eq!(reset.drop_delta, None);
        assert_eq!(
            reset.ambiguity.reason.as_deref(),
            Some("counter generation changed or regressed")
        );

        let final_sample = sampler.observe(sample(750, 2, Some(2))).unwrap();
        assert_eq!(final_sample.start.nanoseconds, 650);
        assert_eq!(final_sample.end.nanoseconds, 750);
        assert_eq!(final_sample.drop_delta, Some(1));
    }

    #[test]
    fn incomplete_loss_sample_keeps_last_complete_boundary() {
        let clock = ClockIdentity {
            boot_id: "fixture-boot".into(),
        };
        let timestamp = |nanoseconds| MonotonicTimestamp {
            clock: clock.clone(),
            nanoseconds,
        };
        let mut sampler = LossWindowSampler::new(timestamp(1));
        assert_eq!(
            sampler.observe(LossCounterSample {
                timestamp: timestamp(10),
                total: 0,
                generation: Some(1),
            }),
            None
        );

        let incomplete =
            sampler.incomplete_sample(timestamp(80), "incomplete per-CPU counter read");
        assert_eq!(incomplete.start.nanoseconds, 10);
        assert_eq!(incomplete.end.nanoseconds, 80);
        assert_eq!(incomplete.drop_delta, None);
        assert_eq!(
            incomplete.ambiguity.reason.as_deref(),
            Some("incomplete per-CPU counter read")
        );

        let recovered = sampler
            .observe(LossCounterSample {
                timestamp: timestamp(90),
                total: 1,
                generation: Some(1),
            })
            .unwrap();
        assert_eq!(recovered.start.nanoseconds, 10);
        assert_eq!(recovered.end.nanoseconds, 90);
        assert_eq!(recovered.drop_delta, Some(1));
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

    struct LifecycleTestSource {
        state: CaptureSourceState,
        pending: std::collections::VecDeque<CaptureEvent>,
        final_loss: Option<CaptureEvent>,
        final_sample_fails: bool,
        drain_fails: bool,
        drain_complete: bool,
        summary: CaptureSourceSummary,
    }

    impl CaptureSource for LifecycleTestSource {
        fn poll(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
            if self.state != CaptureSourceState::Running {
                return Err(CaptureError::Lifecycle {
                    action: "poll",
                    state: self.state,
                });
            }
            Ok(self.pending.pop_front())
        }

        fn request_shutdown(&mut self) -> Result<(), CaptureError> {
            self.state.request_shutdown()?;
            if self.final_sample_fails {
                return Err(CaptureError::FinalLossSample("counter read".into()));
            }
            Ok(())
        }

        fn drain(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
            self.state.begin_drain()?;
            if self.drain_fails {
                return Err(CaptureError::Drain("ring decode".into()));
            }
            if let Some(event) = self.pending.pop_front() {
                return Ok(Some(event));
            }
            if let Some(event) = self.final_loss.take() {
                self.summary.final_loss_events += 1;
                return Ok(Some(event));
            }
            self.drain_complete = true;
            Ok(None)
        }

        fn finalize(&mut self) -> Result<CaptureSourceSummary, CaptureError> {
            self.state.finalize(self.drain_complete)?;
            Ok(self.summary)
        }

        fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
            self.poll()
        }
    }

    #[test]
    fn capture_source_lifecycle_requires_shutdown_then_drain_before_finalize() {
        let mut state = CaptureSourceState::Running;
        assert!(matches!(
            state.finalize(false),
            Err(CaptureError::Lifecycle {
                action: "finalize",
                state: CaptureSourceState::Running,
            })
        ));
        state.request_shutdown().unwrap();
        state.request_shutdown().unwrap();
        assert_eq!(state, CaptureSourceState::ShutdownRequested);
        state.begin_drain().unwrap();
        assert!(matches!(
            state.finalize(false),
            Err(CaptureError::Lifecycle {
                action: "finalize",
                state: CaptureSourceState::Draining,
            })
        ));
        state.finalize(true).unwrap();
        assert_eq!(state, CaptureSourceState::Finalized);
    }

    #[test]
    fn lifecycle_drains_pending_events_before_final_loss_and_rejects_late_polling() {
        let event = FixtureCaptureSource::from_json(FIXTURE.as_bytes())
            .unwrap()
            .next_event()
            .unwrap()
            .unwrap();
        let mut source = LifecycleTestSource {
            state: CaptureSourceState::Running,
            pending: [event.clone()].into(),
            final_loss: Some(event.clone()),
            final_sample_fails: false,
            drain_fails: false,
            drain_complete: false,
            summary: CaptureSourceSummary::default(),
        };
        assert!(matches!(
            source.finalize(),
            Err(CaptureError::Lifecycle { .. })
        ));
        source.request_shutdown().unwrap();
        source.request_shutdown().unwrap();
        assert!(matches!(source.poll(), Err(CaptureError::Lifecycle { .. })));
        assert_eq!(source.drain().unwrap(), Some(event.clone()));
        assert_eq!(source.drain().unwrap(), Some(event));
        assert!(source.drain().unwrap().is_none());
        assert_eq!(source.finalize().unwrap().final_loss_events, 1);
    }

    #[test]
    fn lifecycle_exposes_final_sample_and_drain_failures() {
        let mut final_sample = LifecycleTestSource {
            state: CaptureSourceState::Running,
            pending: Default::default(),
            final_loss: None,
            final_sample_fails: true,
            drain_fails: false,
            drain_complete: false,
            summary: CaptureSourceSummary::default(),
        };
        assert!(matches!(
            final_sample.request_shutdown(),
            Err(CaptureError::FinalLossSample(_))
        ));
        final_sample.drain().unwrap();
        final_sample.finalize().unwrap();

        let mut drain = LifecycleTestSource {
            state: CaptureSourceState::Running,
            pending: Default::default(),
            final_loss: None,
            final_sample_fails: false,
            drain_fails: true,
            drain_complete: false,
            summary: CaptureSourceSummary::default(),
        };
        drain.request_shutdown().unwrap();
        assert!(matches!(drain.drain(), Err(CaptureError::Drain(_))));
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
