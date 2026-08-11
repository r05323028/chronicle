//! Protocol-neutral capture boundary. Events model ordered socket byte chunks, not packets.

use chronicle_common::{Endpoint, Timestamp};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    net::IpAddr,
};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureFlags(pub u32);

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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketRole {
    Active,
    Passive,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketConnectIntent {
    pub timestamp: MonotonicTimestamp,
    pub socket: SocketIdentity,
    pub recording_scope: RecordingScopeIdentity,
    pub network_family: NetworkFamily,
    pub remote_endpoint: Endpoint,
    pub role: SocketRole,
    pub process: Option<ProcessMetadata>,
    pub observed_cgroup_id: u64,
}

impl SocketConnectIntent {
    pub fn validate(&self) -> Result<(), SocketEvidenceError> {
        if self.role != SocketRole::Active {
            return Err(SocketEvidenceError::InvalidConnectRole(self.role));
        }
        let remote = validate_endpoint("remote", &self.remote_endpoint)?;
        if !matches!(
            (self.network_family, remote),
            (NetworkFamily::Ipv4, IpAddr::V4(_)) | (NetworkFamily::Ipv6, IpAddr::V6(_))
        ) {
            return Err(SocketEvidenceError::FamilyMismatch(self.network_family));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SocketEvidence {
    pub timestamp: MonotonicTimestamp,
    pub socket: SocketIdentity,
    pub recording_scope: RecordingScopeIdentity,
    pub network_family: NetworkFamily,
    pub local_endpoint: Endpoint,
    pub remote_endpoint: Endpoint,
    pub role: SocketRole,
    pub process: Option<ProcessMetadata>,
    pub observed_cgroup_id: u64,
}

impl SocketEvidence {
    pub fn validate(&self) -> Result<(), SocketEvidenceError> {
        let local = validate_endpoint("local", &self.local_endpoint)?;
        let remote = validate_endpoint("remote", &self.remote_endpoint)?;
        let matches_family = |address: IpAddr| {
            matches!(
                (self.network_family, address),
                (NetworkFamily::Ipv4, IpAddr::V4(_)) | (NetworkFamily::Ipv6, IpAddr::V6(_))
            )
        };
        if !matches_family(local) || !matches_family(remote) {
            return Err(SocketEvidenceError::FamilyMismatch(self.network_family));
        }
        Ok(())
    }
}

fn validate_endpoint(
    name: &'static str,
    endpoint: &Endpoint,
) -> Result<IpAddr, SocketEvidenceError> {
    if endpoint.port == 0 {
        return Err(SocketEvidenceError::ZeroPort(name));
    }
    let address: IpAddr =
        endpoint
            .host
            .parse()
            .map_err(|_| SocketEvidenceError::InvalidAddress {
                endpoint: name,
                value: endpoint.host.clone(),
            })?;
    if endpoint.host != address.to_string() {
        return Err(SocketEvidenceError::NonCanonicalAddress {
            endpoint: name,
            value: endpoint.host.clone(),
            canonical: address.to_string(),
        });
    }
    Ok(address)
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SocketEvidenceError {
    #[error("{0} socket endpoint has zero port")]
    ZeroPort(&'static str),
    #[error("{endpoint} socket endpoint address {value:?} is not an IP literal")]
    InvalidAddress {
        endpoint: &'static str,
        value: String,
    },
    #[error("{endpoint} socket endpoint address {value:?} is not canonical; use {canonical:?}")]
    NonCanonicalAddress {
        endpoint: &'static str,
        value: String,
        canonical: String,
    },
    #[error("socket endpoints do not match declared network family {0:?}")]
    FamilyMismatch(NetworkFamily),
    #[error("connect intent must have active role, got {0:?}")]
    InvalidConnectRole(SocketRole),
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
    pub flags: CaptureFlags,
}

/// Transport-neutral terminal persistence-loss evidence: records discarded
/// at the durability boundary before reconstruction could observe them.
/// WAL-owned wire records are converted into this type by ETL extraction so
/// reconstruction stays independent of any persistence implementation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalPersistenceLoss {
    pub interval: PersistenceLossInterval,
    pub discarded_records: u64,
    pub discarded_payload_bytes: u64,
    pub reason: PersistenceLossReason,
    pub ambiguity: PersistenceLossAmbiguity,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceLossInterval {
    pub start: MonotonicTimestamp,
    pub end: MonotonicTimestamp,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum PersistenceLossReason {
    /// The durable WAL hard limit was reached and remaining records were discarded.
    WalHardLimit = 1,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum PersistenceLossAmbiguity {
    /// Effects downstream of the discarded records cannot be determined.
    UnknownDownstreamEffects = 1,
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
    SocketConnectObserved(SocketConnectIntent),
    SocketConnected(SocketEvidence),
    SocketClosedObserved(SocketEvidence),
    SocketResetObserved(SocketEvidence),
    SocketStateChangedObserved(SocketStateChangeObserved),
    PayloadFragment(PayloadFragment),
    LossWindowObserved(LossWindowObserved),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureEvent {
    pub schema_version: u16,
    #[serde(flatten)]
    pub kind: CaptureEventKind,
}

impl CaptureEvent {
    pub fn validate(&self) -> Result<(), CaptureError> {
        if self.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
            return Err(CaptureError::UnsupportedSchema(self.schema_version));
        }
        match &self.kind {
            CaptureEventKind::SocketConnectObserved(intent) => intent.validate()?,
            CaptureEventKind::SocketConnected(evidence)
            | CaptureEventKind::SocketClosedObserved(evidence)
            | CaptureEventKind::SocketResetObserved(evidence) => evidence.validate()?,
            CaptureEventKind::SocketStateChangedObserved(change) => change.socket.validate()?,
            CaptureEventKind::PayloadFragment(_) | CaptureEventKind::LossWindowObserved(_) => {}
        }
        Ok(())
    }

    pub fn monotonic_sequence(&self) -> Option<u64> {
        match &self.kind {
            CaptureEventKind::SocketConnectObserved(intent) => Some(intent.timestamp.nanoseconds),
            CaptureEventKind::SocketConnected(evidence)
            | CaptureEventKind::SocketClosedObserved(evidence)
            | CaptureEventKind::SocketResetObserved(evidence) => {
                Some(evidence.timestamp.nanoseconds)
            }
            CaptureEventKind::SocketStateChangedObserved(change) => {
                Some(change.socket.timestamp.nanoseconds)
            }
            CaptureEventKind::PayloadFragment(fragment) => Some(fragment.timestamp.nanoseconds),
            CaptureEventKind::LossWindowObserved(_) => None,
        }
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
    InvalidSocketEvidence(#[from] SocketEvidenceError),
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
    #[error("fixture connection {connection_id:?} has invalid {endpoint} endpoint: {source}")]
    InvalidEndpoint {
        connection_id: String,
        endpoint: &'static str,
        source: SocketEvidenceError,
    },
    #[error("fixture connection {0:?} duplicates an existing socket identity")]
    DuplicateSocketIdentity(String),
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
    #[error("fixture event sequence {sequence} exceeds transport sequence range")]
    SequenceOverflow { sequence: u64 },
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
    pub socket_cookie: u64,
    pub first_seen_ns: u64,
    pub network_namespace: Option<u64>,
    pub local_endpoint: FixtureEndpoint,
    pub remote_endpoint: FixtureEndpoint,
    pub role: SocketRole,
    pub transport: String,
    pub process: Option<ProcessMetadata>,
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

impl FixtureDirection {
    fn payload_direction(self, role: SocketRole) -> PayloadDirection {
        match (self, role) {
            (Self::ClientToServer, SocketRole::Active)
            | (Self::ServerToClient, SocketRole::Passive) => PayloadDirection::Egress,
            (Self::ServerToClient, SocketRole::Active)
            | (Self::ClientToServer, SocketRole::Passive) => PayloadDirection::Ingress,
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

        let event_capacity = fixture
            .connections
            .len()
            .saturating_add(fixture.events.len());
        let mut events = VecDeque::with_capacity(event_capacity);
        let mut connections = BTreeMap::new();
        let mut socket_ids = BTreeMap::new();
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
            let first_seen = MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: "fixture-v1".into(),
                },
                nanoseconds: connection.first_seen_ns,
            };
            let socket = SocketIdentity {
                socket_cookie: connection.socket_cookie,
                first_seen: first_seen.clone(),
                network_namespace: connection.network_namespace,
            };
            if socket_ids
                .insert(socket.clone(), connection.id.clone())
                .is_some()
            {
                return Err(FixtureError::DuplicateSocketIdentity(connection.id));
            }
            let evidence = SocketEvidence {
                timestamp: first_seen,
                socket,
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 0,
                    canonical_path: "fixture-v1".into(),
                    namespace: connection.network_namespace,
                },
                network_family: fixture_network_family(
                    &connection.id,
                    &connection.local_endpoint,
                    &connection.remote_endpoint,
                )?,
                local_endpoint: Endpoint::new(
                    connection.local_endpoint.host,
                    connection.local_endpoint.port,
                ),
                remote_endpoint: Endpoint::new(
                    connection.remote_endpoint.host,
                    connection.remote_endpoint.port,
                ),
                role: connection.role,
                process: connection.process,
                observed_cgroup_id: 0,
            };
            evidence
                .validate()
                .map_err(|source| FixtureError::InvalidEndpoint {
                    connection_id: connection.id.clone(),
                    endpoint: "socket",
                    source,
                })?;
            if connections
                .insert(
                    connection.id.clone(),
                    FixtureConnectionState {
                        evidence: evidence.clone(),
                    },
                )
                .is_some()
            {
                return Err(FixtureError::DuplicateConnectionId(connection.id));
            }
            events.push_back(CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::SocketConnected(evidence),
            });
        }

        let mut expected_sequence = 1_u64;
        let mut previous_timestamp = None;
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
            let payload = decode_hex(&event.payload_hex).ok_or(FixtureError::InvalidHex {
                sequence: event.sequence,
            })?;
            let captured_length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
            let tcp_sequence =
                u32::try_from(event.sequence).map_err(|_| FixtureError::SequenceOverflow {
                    sequence: event.sequence,
                })?;
            events.push_back(CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                    timestamp: MonotonicTimestamp {
                        clock: state.evidence.timestamp.clock.clone(),
                        nanoseconds: state
                            .evidence
                            .timestamp
                            .nanoseconds
                            .saturating_add(event.sequence),
                    },
                    socket: state.evidence.socket.clone(),
                    recording_scope: state.evidence.recording_scope.clone(),
                    network_family: state.evidence.network_family,
                    direction: event.direction.payload_direction(state.evidence.role),
                    sequence: FragmentSequenceEvidence {
                        tcp_sequence,
                        continuation_position: 0,
                    },
                    payload,
                    truncation: TruncationMetadata {
                        captured_length,
                        observed_length: (!event.truncated).then_some(captured_length),
                        state: if event.truncated {
                            TruncationState::Truncated
                        } else {
                            TruncationState::Complete
                        },
                        reason: event
                            .truncated
                            .then(|| "fixture source marked payload truncated".into()),
                    },
                    flags: CaptureFlags(event.flags),
                }),
            });
        }
        Ok(Self { events })
    }
}

struct FixtureConnectionState {
    evidence: SocketEvidence,
}

fn fixture_network_family(
    connection_id: &str,
    local: &FixtureEndpoint,
    remote: &FixtureEndpoint,
) -> Result<NetworkFamily, FixtureError> {
    let parse = |endpoint: &'static str, value: &FixtureEndpoint| {
        let evidence = Endpoint::new(value.host.clone(), value.port);
        validate_endpoint(endpoint, &evidence).map_err(|source| FixtureError::InvalidEndpoint {
            connection_id: connection_id.to_owned(),
            endpoint,
            source,
        })
    };
    match (parse("local", local)?, parse("remote", remote)?) {
        (IpAddr::V4(_), IpAddr::V4(_)) => Ok(NetworkFamily::Ipv4),
        (IpAddr::V6(_), IpAddr::V6(_)) => Ok(NetworkFamily::Ipv6),
        _ => Err(FixtureError::InvalidEndpoint {
            connection_id: connection_id.to_owned(),
            endpoint: "socket",
            source: SocketEvidenceError::FamilyMismatch(NetworkFamily::Ipv4),
        }),
    }
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
    event.validate()?;
    Ok(serde_json::to_vec(event)?)
}

pub fn decode_event(bytes: &[u8]) -> Result<CaptureEvent, CaptureError> {
    let event: CaptureEvent = serde_json::from_slice(bytes)?;
    event.validate()?;
    Ok(event)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "schema_version": 1,
        "connections": [{
            "id": "basic",
            "socket_cookie": 7,
            "first_seen_ns": 100,
            "network_namespace": null,
            "local_endpoint": {"host": "192.0.2.10", "port": 41000},
            "remote_endpoint": {"host": "192.0.2.20", "port": 8080},
            "role": "active",
            "transport": "tcp",
            "process": {"pid": 7, "tid": null, "executable": "fixture"}
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
    fn fixture_source_emits_socket_evidence_before_binary_payload() {
        let mut source = FixtureCaptureSource::from_json(FIXTURE.as_bytes()).unwrap();
        let evidence = source.next_event().unwrap().unwrap();
        let CaptureEventKind::SocketConnected(evidence) = evidence.kind else {
            panic!("fixture must emit socket evidence first");
        };
        assert_eq!(evidence.local_endpoint, Endpoint::new("192.0.2.10", 41000));
        assert_eq!(evidence.remote_endpoint, Endpoint::new("192.0.2.20", 8080));
        assert_eq!(evidence.role, SocketRole::Active);

        let payload = source.next_event().unwrap().unwrap();
        let CaptureEventKind::PayloadFragment(payload) = payload.kind else {
            panic!("fixture must emit payload after socket evidence");
        };
        assert_eq!(payload.payload, [0, 255]);
        assert_eq!(payload.direction, PayloadDirection::Egress);
        assert_eq!(payload.socket, evidence.socket);
        assert!(source.next_event().unwrap().is_none());
    }

    fn socket_evidence() -> SocketEvidence {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 1,
        };
        SocketEvidence {
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
            local_endpoint: Endpoint::new("192.0.2.10", 41000),
            remote_endpoint: Endpoint::new("192.0.2.20", 8080),
            role: SocketRole::Active,
            process: None,
            observed_cgroup_id: 12,
        }
    }

    #[test]
    fn sole_v1_socket_evidence_round_trips() {
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketConnected(socket_evidence()),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["schema_version"], CAPTURE_EVENT_SCHEMA_VERSION);
        assert_eq!(json["kind"], "socket_connected");
        assert_eq!(decode_event(&encode_event(&event).unwrap()).unwrap(), event);
    }

    #[test]
    fn payload_round_trips_without_endpoint_duplication() {
        let evidence = socket_evidence();
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                timestamp: evidence.timestamp.clone(),
                socket: evidence.socket,
                recording_scope: evidence.recording_scope,
                network_family: NetworkFamily::Ipv4,
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
                flags: CaptureFlags(3),
            }),
        };
        let bytes = encode_event(&event).unwrap();
        let json = String::from_utf8(bytes.clone()).unwrap();
        assert!(!json.contains("local_endpoint"));
        assert!(!json.contains("remote_endpoint"));
        assert!(!json.contains("role"));
        assert_eq!(decode_event(&bytes).unwrap(), event);

        let mut missing_flags = serde_json::to_value(&event).unwrap();
        missing_flags["evidence"]
            .as_object_mut()
            .unwrap()
            .remove("flags");
        assert!(matches!(
            decode_event(&serde_json::to_vec(&missing_flags).unwrap()),
            Err(CaptureError::Codec(_))
        ));
    }

    #[test]
    fn capture_rejects_non_v1_and_invalid_socket_evidence() {
        let unsupported = CaptureEvent {
            schema_version: 2,
            kind: CaptureEventKind::SocketConnected(socket_evidence()),
        };
        assert!(matches!(
            encode_event(&unsupported),
            Err(CaptureError::UnsupportedSchema(2))
        ));

        let mut invalid = socket_evidence();
        invalid.remote_endpoint.port = 0;
        let invalid = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketConnected(invalid),
        };
        assert!(matches!(
            encode_event(&invalid),
            Err(CaptureError::InvalidSocketEvidence(
                SocketEvidenceError::ZeroPort("remote")
            ))
        ));

        let mut noncanonical = socket_evidence();
        noncanonical.network_family = NetworkFamily::Ipv6;
        noncanonical.local_endpoint = Endpoint::new("2001:0db8::1", 41_000);
        noncanonical.remote_endpoint = Endpoint::new("2001:db8::2", 8_080);
        assert!(matches!(
            encode_event(&CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::SocketConnected(noncanonical),
            }),
            Err(CaptureError::InvalidSocketEvidence(
                SocketEvidenceError::NonCanonicalAddress {
                    endpoint: "local",
                    ..
                }
            ))
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
                "local socket endpoint has zero port",
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
