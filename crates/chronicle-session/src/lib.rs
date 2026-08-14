//! Bounded reconstruction of ordered bidirectional socket byte streams.

use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureEventKind, CaptureFlags,
    FragmentSequenceEvidence, LossWindowObserved, MonotonicTimestamp, PayloadDirection,
    PayloadFragment, RecordingScopeIdentity, SocketConnectIntent, SocketEvidence, SocketIdentity,
    SocketRole, SocketStateChangeObserved, TerminalPersistenceLoss, TruncationMetadata,
    TruncationState,
};
#[cfg(test)]
use chronicle_capture::{PersistenceLossAmbiguity, PersistenceLossInterval, PersistenceLossReason};
use chronicle_common::{ConnectionKey, Direction, Timestamp, TransportProtocol};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Clone, Copy, Debug)]
pub struct SessionLimits {
    pub max_connections: usize,
    pub max_bytes_per_connection: usize,
    pub max_chunks_per_connection: usize,
}

impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            max_connections: 1_024,
            max_bytes_per_connection: 8 * 1024 * 1024,
            max_chunks_per_connection: 65_536,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CaptureChunk {
    pub monotonic_sequence: u64,
    pub wall_time: Option<Timestamp>,
    pub direction: Direction,
    pub payload: Vec<u8>,
    pub truncated: bool,
    pub flags: CaptureFlags,
}

#[derive(Clone, Debug)]
pub struct ConnectionStream {
    pub key: ConnectionKey,
    pub chunks: Vec<CaptureChunk>,
    pub total_bytes: usize,
    pub truncated: bool,
}

impl ConnectionStream {
    pub fn first_sequence(&self) -> Option<u64> {
        self.chunks.first().map(|event| event.monotonic_sequence)
    }
}

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("connection limit {limit} exceeded")]
    ConnectionLimit { limit: usize },
    #[error("connection byte limit {limit} exceeded for {attempted} bytes")]
    ByteLimit { limit: usize, attempted: usize },
    #[error("capture event schema {schema_version} cannot enter session assembly")]
    UnsupportedCaptureEvent { schema_version: u16 },
    #[error("connection chunk limit {limit} exceeded")]
    ChunkLimit { limit: usize },
    #[error("payload references socket without complete endpoint evidence")]
    MissingSocketEvidence,
    #[error("socket identity has conflicting endpoint evidence")]
    ConflictingSocketEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ReconstructionConnectionIdentity {
    Fixture(ConnectionKey),
    Socket(SocketIdentity),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionTimestamp {
    pub monotonic: MonotonicTimestamp,
    pub fixture_derived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconstructionDirection {
    ClientToServer,
    ServerToClient,
    Ingress,
    Egress,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionPayload {
    pub direction: ReconstructionDirection,
    pub tcp_position: Option<FragmentSequenceEvidence>,
    pub bytes: Vec<u8>,
    pub truncation: TruncationMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawLifecycleEvidence {
    MissingOpen,
    ConnectObserved,
    Established,
    Closed,
    Reset,
    StateChanged { raw_state: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum ReconstructionEvidence {
    Lifecycle(RawLifecycleEvidence),
    Payload(ReconstructionPayload),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionWalProvenance {
    pub segment_ordinal: u64,
    pub segment_first_sequence: u64,
    pub frame_byte_offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionEvent {
    pub connection: ReconstructionConnectionIdentity,
    /// Persistence sequence is ordering evidence only; absent for live/unpersisted input.
    pub wal_sequence: Option<u64>,
    /// Exact placement of persisted input; absent for fixtures and live input.
    pub wal_provenance: Option<ReconstructionWalProvenance>,
    pub timestamp: ReconstructionTimestamp,
    pub recording_scope: Option<RecordingScopeIdentity>,
    pub socket_evidence: Option<SocketEvidence>,
    pub evidence: ReconstructionEvidence,
}

#[allow(clippy::large_enum_variant)] // Event retains complete endpoint provenance; input is not queue-stored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconstructionInput {
    Event(ReconstructionEvent),
    LossWindow(LossWindowObserved),
    /// WAL-limit discard evidence. Never coerced into kernel loss sampling.
    TerminalWalLoss(TerminalPersistenceLoss),
}

/// Groups reconstruction evidence by fixture key or kernel socket generation identity.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionConnection {
    pub identity: ReconstructionConnectionIdentity,
    pub events: Vec<ReconstructionEvent>,
    pub loss_windows: Vec<ConnectionLossWindow>,
    pub termination: DerivedTermination,
    pub finalization: Option<ReconstructionFinalization>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconstructionFinalization {
    ByteLimit,
    ChunkLimit,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DerivedTermination {
    CleanClose,
    /// Reserved until directional close evidence is available from kernel capture feasibility evidence.
    HalfClose,
    Reset,
    UnknownTermination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LossWindowClassification {
    Outside,
    Overlaps,
    Unknown,
    Reset,
    ClockMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionLossWindow {
    pub window: LossWindowObserved,
    pub classification: LossWindowClassification,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TcpOrderingAmbiguity {
    HalfSequenceRange,
    MissingProvenance,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpPayloadOrdering {
    /// One retained representative per exact TCP range.
    pub fragments: Vec<ReconstructionEvent>,
    /// Removed exact retransmissions, retained with full source evidence.
    pub retransmissions: Vec<TcpRetransmission>,
    /// Non-exact overlaps; both source events remain intact.
    pub overlaps: Vec<TcpOverlap>,
    pub ambiguity: Option<TcpOrderingAmbiguity>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpRetransmission {
    pub retained: ReconstructionEvent,
    pub retransmission: ReconstructionEvent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TcpOverlapKind {
    Partial,
    Conflicting,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpOverlap {
    pub first: ReconstructionEvent,
    pub second: ReconstructionEvent,
    pub kind: TcpOverlapKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolNeutralConnection {
    pub identity: ReconstructionConnectionIdentity,
    pub client_to_server: TcpPayloadOrdering,
    pub server_to_client: TcpPayloadOrdering,
    /// `None` means directions have incomparable clocks or missing completion evidence.
    pub cross_direction_completion: Option<Vec<DirectionCompletion>>,
    pub termination: DerivedTermination,
    pub loss_windows: Vec<ConnectionLossWindow>,
    pub finalization: Option<ReconstructionFinalization>,
    pub wal_sequence_range: Option<(u64, u64)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectionCompletion {
    pub direction: ReconstructionDirection,
    pub timestamp: ReconstructionTimestamp,
    pub wal_sequence: Option<u64>,
}

impl ReconstructionConnection {
    pub fn protocol_neutral(&self) -> ProtocolNeutralConnection {
        let role = self
            .events
            .iter()
            .find_map(|event| event.socket_evidence.as_ref().map(|evidence| evidence.role));
        let (client_direction, server_direction) = match (&self.identity, role) {
            (ReconstructionConnectionIdentity::Socket(_), Some(SocketRole::Passive)) => (
                ReconstructionDirection::Ingress,
                ReconstructionDirection::Egress,
            ),
            (ReconstructionConnectionIdentity::Socket(_), _) => (
                ReconstructionDirection::Egress,
                ReconstructionDirection::Ingress,
            ),
            (ReconstructionConnectionIdentity::Fixture(_), _) => (
                ReconstructionDirection::ClientToServer,
                ReconstructionDirection::ServerToClient,
            ),
        };
        let client_to_server = self.ordered_payloads(client_direction);
        let server_to_client = self.ordered_payloads(server_direction);
        let completions = [
            (client_direction, client_to_server.fragments.last()),
            (server_direction, server_to_client.fragments.last()),
        ];
        let mut completion: Vec<_> = completions
            .into_iter()
            .filter_map(|(direction, event)| {
                event.map(|event| DirectionCompletion {
                    direction,
                    timestamp: event.timestamp.clone(),
                    wal_sequence: event.wal_sequence,
                })
            })
            .collect();
        let cross_direction_completion = if completion.len() == 2
            && completion[0].timestamp.monotonic.clock == completion[1].timestamp.monotonic.clock
        {
            completion.sort_by_key(|event| {
                (
                    event.timestamp.monotonic.nanoseconds,
                    event.wal_sequence.unwrap_or(u64::MAX),
                )
            });
            Some(completion)
        } else {
            None
        };
        let mut sequences = self.events.iter().filter_map(|event| event.wal_sequence);
        let wal_sequence_range = sequences.next().map(|first| {
            sequences.fold((first, first), |(min, max), sequence| {
                (min.min(sequence), max.max(sequence))
            })
        });
        ProtocolNeutralConnection {
            identity: self.identity.clone(),
            client_to_server,
            server_to_client,
            cross_direction_completion,
            termination: self.termination,
            loss_windows: self.loss_windows.clone(),
            finalization: self.finalization,
            wal_sequence_range,
        }
    }

    /// Return one direction's payloads in TCP serial-number order, independent of opposite direction.
    pub fn ordered_payloads(&self, direction: ReconstructionDirection) -> TcpPayloadOrdering {
        let mut payloads: Vec<_> = self.events.iter().filter(|event| {
            matches!(&event.evidence, ReconstructionEvidence::Payload(payload) if payload.direction == direction)
        }).cloned().collect();
        let sequence = |event: &ReconstructionEvent| match &event.evidence {
            ReconstructionEvidence::Payload(payload) => payload
                .tcp_position
                .as_ref()
                .map(|position| position.tcp_sequence),
            ReconstructionEvidence::Lifecycle(_) => None,
        };
        let (start, mut ambiguity) = canonical_tcp_start(payloads.iter().filter_map(sequence));
        let mut positions = BTreeMap::new();
        for event in &payloads {
            let ReconstructionEvidence::Payload(payload) = &event.evidence else {
                continue;
            };
            let Some(position) = &payload.tcp_position else {
                continue;
            };
            if let Some(previous) = positions.insert(
                (position.tcp_sequence, position.continuation_position),
                event.wal_sequence,
            ) && (previous.is_none() || event.wal_sequence.is_none())
            {
                ambiguity = Some(TcpOrderingAmbiguity::MissingProvenance);
            }
        }
        payloads.sort_by_key(|event| match &event.evidence {
            ReconstructionEvidence::Payload(payload) => {
                let position = payload.tcp_position.as_ref();
                let tcp = position.map_or(u32::MAX, |position| position.tcp_sequence);
                let relative = start.map_or(u32::MAX, |start| tcp.wrapping_sub(start));
                (
                    relative,
                    position.map_or(u32::MAX, |position| position.continuation_position),
                    event.wal_sequence.unwrap_or(u64::MAX),
                    event.timestamp.monotonic.clock.boot_id.clone(),
                    event.timestamp.monotonic.nanoseconds,
                )
            }
            ReconstructionEvidence::Lifecycle(_) => {
                unreachable!("payload filter excludes lifecycle evidence")
            }
        });
        let mut fragments = Vec::new();
        let mut retransmissions = Vec::new();
        for payload in payloads {
            let exact = fragments.iter().find(|retained: &&ReconstructionEvent| {
                tcp_range(retained) == tcp_range(&payload)
                    && payload_bytes(retained) == payload_bytes(&payload)
            });
            if let Some(retained) = exact {
                retransmissions.push(TcpRetransmission {
                    retained: retained.clone(),
                    retransmission: payload,
                });
            } else {
                fragments.push(payload);
            }
        }
        let mut overlaps = Vec::new();
        for (index, first) in fragments.iter().enumerate() {
            for second in fragments.iter().skip(index + 1) {
                if tcp_ranges_overlap(first, second) {
                    overlaps.push(TcpOverlap {
                        kind: if tcp_range(first).map(|range| range.0)
                            == tcp_range(second).map(|range| range.0)
                        {
                            TcpOverlapKind::Conflicting
                        } else {
                            TcpOverlapKind::Partial
                        },
                        first: first.clone(),
                        second: second.clone(),
                    });
                }
            }
        }
        TcpPayloadOrdering {
            fragments,
            retransmissions,
            overlaps,
            ambiguity,
        }
    }
}

fn tcp_range(event: &ReconstructionEvent) -> Option<(u32, u32, u32)> {
    let ReconstructionEvidence::Payload(payload) = &event.evidence else {
        return None;
    };
    let position = payload.tcp_position.as_ref()?;
    Some((
        position.tcp_sequence,
        position.continuation_position,
        u32::try_from(payload.bytes.len()).unwrap_or(u32::MAX),
    ))
}

fn tcp_ranges_overlap(first: &ReconstructionEvent, second: &ReconstructionEvent) -> bool {
    let Some((first_start, _, first_len)) = tcp_range(first) else {
        return false;
    };
    let Some((second_start, _, second_len)) = tcp_range(second) else {
        return false;
    };
    first_len > 0
        && second_len > 0
        && (second_start.wrapping_sub(first_start) < first_len
            || first_start.wrapping_sub(second_start) < second_len)
}

fn payload_bytes(event: &ReconstructionEvent) -> &[u8] {
    let ReconstructionEvidence::Payload(payload) = &event.evidence else {
        return &[];
    };
    &payload.bytes
}

fn derive_termination(
    events: &[ReconstructionEvent],
    loss_windows: &[ConnectionLossWindow],
) -> DerivedTermination {
    if loss_windows
        .iter()
        .any(|window| window.classification != LossWindowClassification::Outside)
    {
        return DerivedTermination::UnknownTermination;
    }
    if events.iter().any(|event| {
        matches!(
            event.evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Reset)
        )
    }) {
        return DerivedTermination::Reset;
    }
    if events.iter().any(|event| {
        matches!(
            event.evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Closed)
        )
    }) {
        return DerivedTermination::CleanClose;
    }
    DerivedTermination::UnknownTermination
}

fn classify_loss_window(
    events: &[ReconstructionEvent],
    window: &LossWindowObserved,
) -> LossWindowClassification {
    if window.start.clock != window.end.clock {
        return LossWindowClassification::ClockMismatch;
    }
    if window.drop_delta.is_none() {
        return if window
            .ambiguity
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("generation") || reason.contains("regressed"))
        {
            LossWindowClassification::Reset
        } else {
            LossWindowClassification::Unknown
        };
    }
    let same_clock: Vec<_> = events
        .iter()
        .filter(|event| event.timestamp.monotonic.clock == window.start.clock)
        .collect();
    if same_clock.is_empty() {
        return LossWindowClassification::ClockMismatch;
    }
    let first = same_clock
        .iter()
        .map(|event| event.timestamp.monotonic.nanoseconds)
        .min()
        .unwrap();
    let last = same_clock
        .iter()
        .map(|event| event.timestamp.monotonic.nanoseconds)
        .max()
        .unwrap();
    if window.end.nanoseconds < first || window.start.nanoseconds > last {
        LossWindowClassification::Outside
    } else {
        LossWindowClassification::Overlaps
    }
}

fn canonical_tcp_start(
    sequences: impl Iterator<Item = u32>,
) -> (Option<u32>, Option<TcpOrderingAmbiguity>) {
    let mut sequences: Vec<_> = sequences.collect();
    sequences.sort_unstable();
    sequences.dedup();
    let Some(first) = sequences.first().copied() else {
        return (None, None);
    };
    let mut start = first;
    let mut largest_gap = 0_u32;
    for (current, next) in sequences.iter().copied().zip(
        sequences
            .iter()
            .copied()
            .skip(1)
            .chain(std::iter::once(first)),
    ) {
        let gap = next.wrapping_sub(current);
        if gap == (1 << 31) {
            return (None, Some(TcpOrderingAmbiguity::HalfSequenceRange));
        }
        if gap > largest_gap {
            largest_gap = gap;
            start = next;
        }
    }
    (Some(start), None)
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconstructionAssembly {
    /// Active connections followed by connections finalized by a reconstruction bound.
    pub connections: Vec<ReconstructionConnection>,
    pub loss_windows: Vec<LossWindowObserved>,
    pub terminal_wal_losses: Vec<TerminalPersistenceLoss>,
}

#[derive(Clone, Copy, Debug)]
pub struct ReconstructionLimits {
    pub max_active_connections: usize,
    pub max_bytes_per_connection: usize,
    pub max_chunks_per_connection: usize,
    pub idle_nanoseconds: u64,
}

impl Default for ReconstructionLimits {
    fn default() -> Self {
        Self {
            max_active_connections: 1_024,
            max_bytes_per_connection: 8 * 1024 * 1024,
            max_chunks_per_connection: 65_536,
            idle_nanoseconds: 5 * 60 * 1_000_000_000,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReconstructionPushError {
    #[error("active reconstruction connection limit {limit} exceeded")]
    ActiveConnectionLimit { limit: usize },
}

#[derive(Clone)]
pub struct ReconstructionAssembler {
    limits: ReconstructionLimits,
    connections: BTreeMap<ReconstructionConnectionIdentity, Vec<ReconstructionEvent>>,
    finalized: Vec<(
        ReconstructionConnectionIdentity,
        Vec<ReconstructionEvent>,
        ReconstructionFinalization,
    )>,
    loss_windows: Vec<LossWindowObserved>,
    terminal_wal_losses: Vec<TerminalPersistenceLoss>,
}

pub const RECONSTRUCTION_SNAPSHOT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionAssemblerSnapshot {
    pub version: u32,
    pub max_active_connections: usize,
    pub max_bytes_per_connection: usize,
    pub max_chunks_per_connection: usize,
    pub idle_nanoseconds: u64,
    pub active: Vec<ReconstructionPendingConnection>,
    pub finalized: Vec<ReconstructionFinalizedConnection>,
    pub loss_windows: Vec<LossWindowObserved>,
    pub terminal_wal_losses: Vec<TerminalPersistenceLoss>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionPendingConnection {
    pub identity: ReconstructionConnectionIdentity,
    pub events: Vec<ReconstructionEvent>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconstructionFinalizedConnection {
    pub identity: ReconstructionConnectionIdentity,
    pub events: Vec<ReconstructionEvent>,
    pub finalization: ReconstructionFinalization,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReconstructionSnapshotError {
    #[error("reconstruction snapshot could not be serialized")]
    Serialization,
    #[error("reconstruction snapshot could not be parsed")]
    Parse,
    #[error("reconstruction snapshot version {0} is unsupported")]
    UnsupportedVersion(u32),
    #[error("reconstruction snapshot limits do not match decoder configuration")]
    LimitMismatch,
    #[error("reconstruction snapshot exceeds decoder bounds")]
    Bounds,
    #[error("reconstruction snapshot contains duplicate connection state")]
    DuplicateConnection,
    #[error("reconstruction snapshot contains inconsistent connection state")]
    InconsistentConnection,
}

impl Default for ReconstructionAssembler {
    fn default() -> Self {
        Self::new(ReconstructionLimits::default())
    }
}

impl ReconstructionAssembler {
    pub fn new(limits: ReconstructionLimits) -> Self {
        Self {
            limits,
            connections: BTreeMap::new(),
            finalized: Vec::new(),
            loss_windows: Vec::new(),
            terminal_wal_losses: Vec::new(),
        }
    }

    pub fn push(&mut self, input: ReconstructionInput) -> Result<(), ReconstructionPushError> {
        match input {
            ReconstructionInput::Event(event) => self.push_event(event),
            ReconstructionInput::LossWindow(window) => {
                self.loss_windows.push(window);
                Ok(())
            }
            ReconstructionInput::TerminalWalLoss(loss) => {
                self.terminal_wal_losses.push(loss);
                Ok(())
            }
        }
    }

    fn push_event(&mut self, event: ReconstructionEvent) -> Result<(), ReconstructionPushError> {
        self.finalize_idle(&event.timestamp);
        let identity = event.connection.clone();
        if !self.connections.contains_key(&identity)
            && self.connections.len() >= self.limits.max_active_connections
        {
            return Err(ReconstructionPushError::ActiveConnectionLimit {
                limit: self.limits.max_active_connections,
            });
        }
        let events = self.connections.entry(identity.clone()).or_default();
        let payload_bytes = match &event.evidence {
            ReconstructionEvidence::Payload(payload) => payload.bytes.len(),
            ReconstructionEvidence::Lifecycle(_) => 0,
        };
        let chunk_count = events
            .iter()
            .filter(|event| matches!(event.evidence, ReconstructionEvidence::Payload(_)))
            .count();
        let byte_count: usize = events
            .iter()
            .filter_map(|event| match &event.evidence {
                ReconstructionEvidence::Payload(payload) => Some(payload.bytes.len()),
                ReconstructionEvidence::Lifecycle(_) => None,
            })
            .sum();
        let finalization = if matches!(event.evidence, ReconstructionEvidence::Payload(_))
            && chunk_count >= self.limits.max_chunks_per_connection
        {
            Some(ReconstructionFinalization::ChunkLimit)
        } else if byte_count.saturating_add(payload_bytes) > self.limits.max_bytes_per_connection {
            Some(ReconstructionFinalization::ByteLimit)
        } else {
            None
        };
        if let Some(reason) = finalization {
            let events = self.connections.remove(&identity).unwrap_or_default();
            self.finalized.push((identity, events, reason));
            return Ok(());
        }
        if events.is_empty() && matches!(&event.evidence, ReconstructionEvidence::Payload(_)) {
            events.push(ReconstructionEvent {
                connection: event.connection.clone(),
                wal_sequence: event.wal_sequence,
                wal_provenance: event.wal_provenance.clone(),
                timestamp: event.timestamp.clone(),
                recording_scope: event.recording_scope.clone(),
                socket_evidence: None,
                evidence: ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::MissingOpen),
            });
        }
        events.push(event);
        Ok(())
    }

    fn finalize_idle(&mut self, timestamp: &ReconstructionTimestamp) {
        let idle = self.limits.idle_nanoseconds;
        let expired: Vec<_> = self
            .connections
            .iter()
            .filter_map(|(identity, events)| {
                events
                    .last()
                    .filter(|event| {
                        event.timestamp.monotonic.clock == timestamp.monotonic.clock
                            && timestamp
                                .monotonic
                                .nanoseconds
                                .saturating_sub(event.timestamp.monotonic.nanoseconds)
                                >= idle
                    })
                    .map(|_| identity.clone())
            })
            .collect();
        for identity in expired {
            if let Some(events) = self.connections.remove(&identity) {
                self.finalized
                    .push((identity, events, ReconstructionFinalization::Idle));
            }
        }
    }

    pub fn pending_connection_count(&self) -> usize {
        self.connections.len() + self.finalized.len()
    }

    /// Serialize every mutable reconstruction input needed to resume at same WAL cursor.
    pub fn snapshot(&self) -> Result<Vec<u8>, ReconstructionSnapshotError> {
        let snapshot = ReconstructionAssemblerSnapshot {
            version: RECONSTRUCTION_SNAPSHOT_VERSION,
            max_active_connections: self.limits.max_active_connections,
            max_bytes_per_connection: self.limits.max_bytes_per_connection,
            max_chunks_per_connection: self.limits.max_chunks_per_connection,
            idle_nanoseconds: self.limits.idle_nanoseconds,
            active: self
                .connections
                .iter()
                .map(|(identity, events)| ReconstructionPendingConnection {
                    identity: identity.clone(),
                    events: events.clone(),
                })
                .collect(),
            finalized: self
                .finalized
                .iter()
                .map(
                    |(identity, events, finalization)| ReconstructionFinalizedConnection {
                        identity: identity.clone(),
                        events: events.clone(),
                        finalization: *finalization,
                    },
                )
                .collect(),
            loss_windows: self.loss_windows.clone(),
            terminal_wal_losses: self.terminal_wal_losses.clone(),
        };
        serde_json::to_vec(&snapshot).map_err(|_| ReconstructionSnapshotError::Serialization)
    }

    /// Restore only snapshots produced with same bounded decoder configuration.
    pub fn restore(
        limits: ReconstructionLimits,
        bytes: &[u8],
    ) -> Result<Self, ReconstructionSnapshotError> {
        let snapshot: ReconstructionAssemblerSnapshot =
            serde_json::from_slice(bytes).map_err(|_| ReconstructionSnapshotError::Parse)?;
        if snapshot.version != RECONSTRUCTION_SNAPSHOT_VERSION {
            return Err(ReconstructionSnapshotError::UnsupportedVersion(
                snapshot.version,
            ));
        }
        if snapshot.max_active_connections != limits.max_active_connections
            || snapshot.max_bytes_per_connection != limits.max_bytes_per_connection
            || snapshot.max_chunks_per_connection != limits.max_chunks_per_connection
            || snapshot.idle_nanoseconds != limits.idle_nanoseconds
        {
            return Err(ReconstructionSnapshotError::LimitMismatch);
        }
        if snapshot.active.len() > limits.max_active_connections {
            return Err(ReconstructionSnapshotError::Bounds);
        }
        let mut connections = BTreeMap::new();
        for pending in &snapshot.active {
            validate_connection_state(&pending.identity, &pending.events, &limits)?;
            if connections
                .insert(pending.identity.clone(), pending.events.clone())
                .is_some()
            {
                return Err(ReconstructionSnapshotError::DuplicateConnection);
            }
        }
        if snapshot.finalized.len() > limits.max_active_connections {
            return Err(ReconstructionSnapshotError::Bounds);
        }
        let mut seen = BTreeSet::new();
        seen.extend(connections.keys().cloned());
        let mut finalized = Vec::with_capacity(snapshot.finalized.len());
        for connection in &snapshot.finalized {
            validate_connection_state(&connection.identity, &connection.events, &limits)?;
            if !seen.insert(connection.identity.clone()) {
                return Err(ReconstructionSnapshotError::DuplicateConnection);
            }
            finalized.push((
                connection.identity.clone(),
                connection.events.clone(),
                connection.finalization,
            ));
        }
        Ok(Self {
            limits,
            connections,
            finalized,
            loss_windows: snapshot.loss_windows,
            terminal_wal_losses: snapshot.terminal_wal_losses,
        })
    }

    pub fn finish(self) -> ReconstructionAssembly {
        let Self {
            connections,
            finalized,
            loss_windows,
            terminal_wal_losses,
            ..
        } = self;
        let mut all: Vec<_> = connections
            .into_iter()
            .map(|(identity, events)| (identity, events, None))
            .collect();
        all.extend(
            finalized
                .into_iter()
                .map(|(identity, events, reason)| (identity, events, Some(reason))),
        );
        ReconstructionAssembly {
            connections: all
                .into_iter()
                .map(|(identity, events, finalization)| {
                    materialize_connection(identity, events, &loss_windows, finalization)
                })
                .collect(),
            loss_windows,
            terminal_wal_losses,
        }
    }
}

fn validate_connection_state(
    identity: &ReconstructionConnectionIdentity,
    events: &[ReconstructionEvent],
    limits: &ReconstructionLimits,
) -> Result<(), ReconstructionSnapshotError> {
    if events.iter().any(|event| &event.connection != identity) {
        return Err(ReconstructionSnapshotError::InconsistentConnection);
    }
    let payloads = events
        .iter()
        .filter_map(|event| match &event.evidence {
            ReconstructionEvidence::Payload(payload) => Some(payload),
            ReconstructionEvidence::Lifecycle(_) => None,
        })
        .collect::<Vec<_>>();
    if payloads.len() > limits.max_chunks_per_connection
        || payloads
            .iter()
            .map(|payload| payload.bytes.len())
            .sum::<usize>()
            > limits.max_bytes_per_connection
    {
        return Err(ReconstructionSnapshotError::Bounds);
    }
    Ok(())
}

fn materialize_connection(
    identity: ReconstructionConnectionIdentity,
    events: Vec<ReconstructionEvent>,
    loss_windows: &[LossWindowObserved],
    finalization: Option<ReconstructionFinalization>,
) -> ReconstructionConnection {
    let connection_windows = loss_windows
        .iter()
        .cloned()
        .map(|window| ConnectionLossWindow {
            classification: classify_loss_window(&events, &window),
            window,
        })
        .collect::<Vec<_>>();
    let termination = derive_termination(&events, &connection_windows);
    ReconstructionConnection {
        identity,
        events,
        loss_windows: connection_windows,
        termination,
        finalization,
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReconstructionInputError {
    #[error("capture event schema {0} cannot enter reconstruction input")]
    UnsupportedCaptureSchema(u16),
}

impl ReconstructionInput {
    pub fn from_capture_event(event: CaptureEvent) -> Result<Self, ReconstructionInputError> {
        if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
            return Err(ReconstructionInputError::UnsupportedCaptureSchema(
                event.schema_version,
            ));
        }
        Ok(capture_input(event.kind))
    }
}

fn capture_input(kind: CaptureEventKind) -> ReconstructionInput {
    match kind {
        CaptureEventKind::SocketConnectObserved(intent) => connect_intent_input(intent),
        CaptureEventKind::SocketConnected(evidence) => {
            lifecycle_input(evidence, RawLifecycleEvidence::Established)
        }
        CaptureEventKind::SocketClosedObserved(evidence) => {
            lifecycle_input(evidence, RawLifecycleEvidence::Closed)
        }
        CaptureEventKind::SocketResetObserved(evidence) => {
            lifecycle_input(evidence, RawLifecycleEvidence::Reset)
        }
        CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
            socket,
            raw_state,
        }) => lifecycle_input(socket, RawLifecycleEvidence::StateChanged { raw_state }),
        CaptureEventKind::PayloadFragment(fragment) => payload_input(fragment),
        CaptureEventKind::LossWindowObserved(window) => ReconstructionInput::LossWindow(window),
    }
}

fn connect_intent_input(intent: SocketConnectIntent) -> ReconstructionInput {
    ReconstructionInput::Event(ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Socket(intent.socket),
        wal_sequence: None,
        wal_provenance: None,
        timestamp: reconstruction_timestamp(intent.timestamp),
        recording_scope: Some(intent.recording_scope),
        socket_evidence: None,
        evidence: ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::ConnectObserved),
    })
}

fn lifecycle_input(
    evidence: SocketEvidence,
    lifecycle: RawLifecycleEvidence,
) -> ReconstructionInput {
    ReconstructionInput::Event(ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Socket(evidence.socket.clone()),
        wal_sequence: None,
        wal_provenance: None,
        timestamp: reconstruction_timestamp(evidence.timestamp.clone()),
        recording_scope: Some(evidence.recording_scope.clone()),
        socket_evidence: Some(evidence),
        evidence: ReconstructionEvidence::Lifecycle(lifecycle),
    })
}

fn payload_input(fragment: PayloadFragment) -> ReconstructionInput {
    ReconstructionInput::Event(ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Socket(fragment.socket),
        wal_sequence: None,
        wal_provenance: None,
        timestamp: reconstruction_timestamp(fragment.timestamp),
        recording_scope: Some(fragment.recording_scope),
        socket_evidence: None,
        evidence: ReconstructionEvidence::Payload(ReconstructionPayload {
            direction: match fragment.direction {
                PayloadDirection::Ingress => ReconstructionDirection::Ingress,
                PayloadDirection::Egress => ReconstructionDirection::Egress,
            },
            tcp_position: Some(fragment.sequence),
            bytes: fragment.payload,
            truncation: fragment.truncation,
        }),
    })
}

fn reconstruction_timestamp(monotonic: MonotonicTimestamp) -> ReconstructionTimestamp {
    let fixture_derived = monotonic.clock.boot_id == "fixture-v1";
    ReconstructionTimestamp {
        monotonic,
        fixture_derived,
    }
}

pub struct SessionAssembler {
    limits: SessionLimits,
    sockets: BTreeMap<SocketIdentity, SocketEvidence>,
    streams: BTreeMap<ConnectionKey, ConnectionStream>,
}

impl SessionAssembler {
    pub fn new(limits: SessionLimits) -> Self {
        Self {
            limits,
            sockets: BTreeMap::new(),
            streams: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, event: CaptureEvent) -> Result<(), SessionError> {
        if event.schema_version != CAPTURE_EVENT_SCHEMA_VERSION {
            return Err(SessionError::UnsupportedCaptureEvent {
                schema_version: event.schema_version,
            });
        }
        match event.kind {
            CaptureEventKind::SocketConnected(evidence)
            | CaptureEventKind::SocketClosedObserved(evidence)
            | CaptureEventKind::SocketResetObserved(evidence) => self.cache_evidence(evidence),
            CaptureEventKind::SocketStateChangedObserved(change) => {
                self.cache_evidence(change.socket)
            }
            CaptureEventKind::PayloadFragment(fragment) => self.push_payload(fragment),
            CaptureEventKind::SocketConnectObserved(_)
            | CaptureEventKind::LossWindowObserved(_) => Ok(()),
        }
    }

    fn cache_evidence(&mut self, evidence: SocketEvidence) -> Result<(), SessionError> {
        if let Some(existing) = self.sockets.get(&evidence.socket)
            && (existing.network_family != evidence.network_family
                || existing.local_endpoint != evidence.local_endpoint
                || existing.remote_endpoint != evidence.remote_endpoint
                || existing.role != evidence.role)
        {
            return Err(SessionError::ConflictingSocketEvidence);
        }
        self.sockets.insert(evidence.socket.clone(), evidence);
        Ok(())
    }

    fn push_payload(&mut self, fragment: PayloadFragment) -> Result<(), SessionError> {
        let evidence = self
            .sockets
            .get(&fragment.socket)
            .ok_or(SessionError::MissingSocketEvidence)?;
        let key = connection_key(evidence);
        let existing_bytes = self
            .streams
            .get(&key)
            .map_or(0, |stream| stream.total_bytes);
        let attempted = existing_bytes.saturating_add(fragment.payload.len());
        if attempted > self.limits.max_bytes_per_connection {
            return Err(SessionError::ByteLimit {
                limit: self.limits.max_bytes_per_connection,
                attempted,
            });
        }
        let existing_chunks = self
            .streams
            .get(&key)
            .map_or(0, |stream| stream.chunks.len());
        if existing_chunks >= self.limits.max_chunks_per_connection {
            return Err(SessionError::ChunkLimit {
                limit: self.limits.max_chunks_per_connection,
            });
        }
        if !self.streams.contains_key(&key) && self.streams.len() >= self.limits.max_connections {
            return Err(SessionError::ConnectionLimit {
                limit: self.limits.max_connections,
            });
        }
        let direction = match (evidence.role, fragment.direction) {
            (SocketRole::Active, PayloadDirection::Egress)
            | (SocketRole::Passive, PayloadDirection::Ingress) => Direction::ClientToServer,
            (SocketRole::Active, PayloadDirection::Ingress)
            | (SocketRole::Passive, PayloadDirection::Egress) => Direction::ServerToClient,
        };
        let truncated = fragment.truncation.state != TruncationState::Complete;
        let stream = self
            .streams
            .entry(key.clone())
            .or_insert_with(|| ConnectionStream {
                key,
                chunks: Vec::new(),
                total_bytes: 0,
                truncated: false,
            });
        stream.total_bytes = attempted;
        stream.truncated |= truncated;
        stream.chunks.push(CaptureChunk {
            monotonic_sequence: fragment.timestamp.nanoseconds,
            wall_time: None,
            direction,
            payload: fragment.payload,
            truncated,
            flags: fragment.flags,
        });
        Ok(())
    }

    pub fn finish(self) -> Vec<ConnectionStream> {
        let mut streams: Vec<_> = self.streams.into_values().collect();
        for stream in &mut streams {
            stream.chunks.sort_by_key(|event| event.monotonic_sequence);
        }
        streams.sort_by_key(ConnectionStream::first_sequence);
        streams
    }
}

fn connection_key(evidence: &SocketEvidence) -> ConnectionKey {
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
    ConnectionKey {
        network_namespace: evidence.socket.network_namespace,
        client,
        server,
        transport: TransportProtocol::Tcp,
    }
}

#[cfg(test)]
#[allow(unused_must_use)] // Success-path tests omit typed push results.
mod tests {
    use super::*;
    use chronicle_capture::{
        CAPTURE_EVENT_SCHEMA_VERSION, CaptureFlags, ClockIdentity, NetworkFamily,
    };
    use chronicle_common::{Direction, Endpoint};

    fn event(sequence: u64) -> CaptureEvent {
        event_for(sequence, "client", 1)
    }

    fn fixture_socket(client: &str) -> SocketIdentity {
        SocketIdentity {
            socket_cookie: client.bytes().map(u64::from).sum::<u64>().max(1),
            first_seen: MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: "fixture-v1".into(),
                },
                nanoseconds: 1,
            },
            network_namespace: None,
        }
    }

    fn event_for(sequence: u64, client: &str, payload_bytes: usize) -> CaptureEvent {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "fixture-v1".into(),
            },
            nanoseconds: sequence,
        };
        CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                timestamp: timestamp.clone(),
                socket: fixture_socket(client),
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 0,
                    canonical_path: client.into(),
                    namespace: None,
                },
                network_family: NetworkFamily::Ipv4,
                direction: PayloadDirection::Egress,
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: u32::try_from(sequence).unwrap(),
                    continuation_position: 0,
                },
                payload: vec![u8::try_from(sequence).unwrap(); payload_bytes],
                truncation: TruncationMetadata {
                    captured_length: u32::try_from(payload_bytes).unwrap(),
                    observed_length: Some(u32::try_from(payload_bytes).unwrap()),
                    state: TruncationState::Complete,
                    reason: None,
                },
                flags: CaptureFlags::default(),
            }),
        }
    }

    fn fixture_evidence(client: &str) -> CaptureEvent {
        let socket = fixture_socket(client);
        CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketConnected(SocketEvidence {
                timestamp: socket.first_seen.clone(),
                socket,
                recording_scope: RecordingScopeIdentity {
                    cgroup_id: 0,
                    canonical_path: client.into(),
                    namespace: None,
                },
                network_family: NetworkFamily::Ipv4,
                local_endpoint: Endpoint::new(client, 10),
                remote_endpoint: Endpoint::new("server", 20),
                role: SocketRole::Active,
                process: None,
                observed_cgroup_id: 0,
            }),
        }
    }

    fn push_fixture(
        assembler: &mut SessionAssembler,
        sequence: u64,
        client: &str,
        payload_bytes: usize,
    ) -> Result<(), SessionError> {
        assembler.push(fixture_evidence(client))?;
        assembler.push(event_for(sequence, client, payload_bytes))
    }

    fn socket_evidence(role: SocketRole) -> SocketEvidence {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 12,
        };
        SocketEvidence {
            timestamp: timestamp.clone(),
            socket: SocketIdentity {
                socket_cookie: 8,
                first_seen: timestamp,
                network_namespace: None,
            },
            recording_scope: RecordingScopeIdentity {
                cgroup_id: 12,
                canonical_path: "/scope".into(),
                namespace: None,
            },
            network_family: NetworkFamily::Ipv4,
            local_endpoint: Endpoint::new("192.0.2.10", 41_000),
            remote_endpoint: Endpoint::new("192.0.2.20", 8_080),
            role,
            process: None,
            observed_cgroup_id: 12,
        }
    }

    #[test]
    fn fixture_capture_uses_same_socket_reconstruction_input() {
        let input =
            ReconstructionInput::from_capture_event(event_for(7, "fixture-client", 2)).unwrap();
        let ReconstructionInput::Event(ReconstructionEvent {
            connection: ReconstructionConnectionIdentity::Socket(socket),
            timestamp,
            recording_scope,
            evidence:
                ReconstructionEvidence::Payload(ReconstructionPayload {
                    direction,
                    tcp_position,
                    bytes,
                    truncation,
                }),
            ..
        }) = input
        else {
            panic!("expected fixture reconstruction payload");
        };

        assert_ne!(socket.socket_cookie, 0);
        assert_eq!(timestamp.monotonic.clock.boot_id, "fixture-v1");
        assert_eq!(timestamp.monotonic.nanoseconds, 7);
        assert!(timestamp.fixture_derived);
        assert_eq!(recording_scope.unwrap().canonical_path, "fixture-client");
        assert_eq!(direction, ReconstructionDirection::Egress);
        assert_eq!(tcp_position.unwrap().tcp_sequence, 7);
        assert_eq!(bytes, [7, 7]);
        assert_eq!(truncation.captured_length, 2);
        assert_eq!(truncation.state, TruncationState::Complete);
    }

    #[test]
    fn capture_payload_adapts_without_fixture_connection_or_wal_provenance() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 11,
        };
        let input = ReconstructionInput::from_capture_event(CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
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
                network_family: chronicle_capture::NetworkFamily::Ipv6,
                direction: PayloadDirection::Egress,
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: 42,
                    continuation_position: 1,
                },
                payload: vec![0, 255],
                truncation: TruncationMetadata {
                    captured_length: 2,
                    observed_length: Some(3),
                    state: chronicle_capture::TruncationState::Truncated,
                    reason: Some("capture bound".into()),
                },
                flags: CaptureFlags::default(),
            }),
        })
        .unwrap();
        let ReconstructionInput::Event(event) = input else {
            panic!("expected production reconstruction payload");
        };
        let ReconstructionConnectionIdentity::Socket(socket) = event.connection else {
            panic!("production event must retain socket identity");
        };
        let ReconstructionEvidence::Payload(payload) = event.evidence else {
            panic!("expected payload evidence");
        };

        assert_eq!(socket.socket_cookie, 7);
        assert_eq!(event.timestamp.monotonic.nanoseconds, 11);
        assert!(!event.timestamp.fixture_derived);
        assert_eq!(event.recording_scope.unwrap().cgroup_id, 11);
        assert_eq!(payload.direction, ReconstructionDirection::Egress);
        assert_eq!(payload.tcp_position.unwrap().tcp_sequence, 42);
        assert_eq!(payload.bytes, [0, 255]);
        assert_eq!(
            payload.truncation.state,
            chronicle_capture::TruncationState::Truncated
        );
    }

    #[test]
    fn lifecycle_preserves_complete_socket_evidence() {
        let evidence = socket_evidence(SocketRole::Passive);
        let input = ReconstructionInput::from_capture_event(CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                socket: evidence.clone(),
                raw_state: 99,
            }),
        })
        .unwrap();
        let ReconstructionInput::Event(event) = input else {
            panic!("expected lifecycle evidence");
        };
        assert_eq!(event.socket_evidence, Some(evidence));
        assert_eq!(
            event.evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::StateChanged { raw_state: 99 })
        );
    }

    #[test]
    fn same_egress_direction_maps_by_active_or_passive_role() {
        for (role, expected_client_fragments, expected_server_fragments) in
            [(SocketRole::Active, 1, 0), (SocketRole::Passive, 0, 1)]
        {
            let evidence = socket_evidence(role);
            let payload = CaptureEvent {
                schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                    timestamp: evidence.timestamp.clone(),
                    socket: evidence.socket.clone(),
                    recording_scope: evidence.recording_scope.clone(),
                    network_family: evidence.network_family,
                    direction: PayloadDirection::Egress,
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
                }),
            };
            let mut assembler = ReconstructionAssembler::default();
            assembler
                .push(
                    ReconstructionInput::from_capture_event(CaptureEvent {
                        schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
                        kind: CaptureEventKind::SocketConnected(evidence),
                    })
                    .unwrap(),
                )
                .unwrap();
            assembler
                .push(ReconstructionInput::from_capture_event(payload).unwrap())
                .unwrap();
            let connection = assembler.finish().connections.pop().unwrap();
            let neutral = connection.protocol_neutral();
            assert_eq!(
                neutral.client_to_server.fragments.len(),
                expected_client_fragments
            );
            assert_eq!(
                neutral.server_to_client.fragments.len(),
                expected_server_fragments
            );
        }
    }

    #[test]
    fn capture_loss_window_stays_separate_reconstruction_input() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 13,
        };
        let window = LossWindowObserved {
            start: timestamp.clone(),
            end: MonotonicTimestamp {
                clock: timestamp.clock,
                nanoseconds: 14,
            },
            drop_delta: Some(2),
            loss_generation: Some(1),
            ambiguity: chronicle_capture::LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some("between samples".into()),
            },
        };
        let input = ReconstructionInput::from_capture_event(CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            kind: CaptureEventKind::LossWindowObserved(window.clone()),
        })
        .unwrap();
        assert_eq!(input, ReconstructionInput::LossWindow(window));
    }

    #[test]
    fn reconstruction_input_rejects_unknown_capture_schema() {
        let mut event = event_for(1, "client", 1);
        event.schema_version = 2;
        assert_eq!(
            ReconstructionInput::from_capture_event(event),
            Err(ReconstructionInputError::UnsupportedCaptureSchema(2))
        );
    }

    fn production_event(boot_id: &str, cookie: u64, first_seen: u64) -> ReconstructionInput {
        let clock = ClockIdentity {
            boot_id: boot_id.into(),
        };
        ReconstructionInput::Event(ReconstructionEvent {
            connection: ReconstructionConnectionIdentity::Socket(SocketIdentity {
                socket_cookie: cookie,
                first_seen: MonotonicTimestamp {
                    clock: clock.clone(),
                    nanoseconds: first_seen,
                },
                network_namespace: Some(1),
            }),
            wal_sequence: None,
            wal_provenance: None,
            timestamp: ReconstructionTimestamp {
                monotonic: MonotonicTimestamp {
                    clock,
                    nanoseconds: first_seen,
                },
                fixture_derived: false,
            },
            recording_scope: None,
            socket_evidence: None,
            evidence: ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Established),
        })
    }

    #[test]
    fn reconstruction_groups_production_by_socket_generation_and_keeps_fixture_key() {
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(production_event("boot-a", 7, 10));
        assembler.push(production_event("boot-a", 7, 10)); // Same kernel socket generation.
        assembler.push(production_event("boot-a", 7, 20)); // Socket cookie reuse: distinct generation.
        assembler.push(production_event("boot-b", 7, 10)); // Process restart on another boot.
        assembler.push(ReconstructionInput::from_capture_event(event(1)).unwrap());
        let assembly = assembler.finish();
        assert_eq!(assembly.connections.len(), 4);
        assert!(assembly.connections.iter().all(|connection| matches!(
            connection.identity,
            ReconstructionConnectionIdentity::Socket(_)
        )));
        assert!(assembly.connections.iter().any(|connection| matches!(
            &connection.identity,
            ReconstructionConnectionIdentity::Socket(socket)
                if socket.socket_cookie == 7
                    && socket.first_seen.clock.boot_id == "boot-a"
                    && socket.first_seen.nanoseconds == 10
                    && connection.events.len() == 2
        )));
        assert!(assembly.connections.iter().any(|connection| matches!(
            &connection.identity,
            ReconstructionConnectionIdentity::Socket(socket)
                if socket.socket_cookie == 7
                    && socket.first_seen.clock.boot_id == "boot-a"
                    && socket.first_seen.nanoseconds == 20
        )));
        assert!(assembly.connections.iter().any(|connection| matches!(
            &connection.identity,
            ReconstructionConnectionIdentity::Socket(socket)
                if socket.socket_cookie == 7
                    && socket.first_seen.clock.boot_id == "boot-b"
                    && socket.first_seen.nanoseconds == 10
        )));
    }

    fn tcp_payload(
        sequence: u32,
        wal_sequence: u64,
        direction: ReconstructionDirection,
    ) -> ReconstructionInput {
        let mut input = production_event("boot-a", 9, 1);
        let ReconstructionInput::Event(event) = &mut input else {
            unreachable!()
        };
        event.wal_sequence = Some(wal_sequence);
        event.evidence = ReconstructionEvidence::Payload(ReconstructionPayload {
            direction,
            tcp_position: Some(FragmentSequenceEvidence {
                tcp_sequence: sequence,
                continuation_position: 0,
            }),
            bytes: vec![u8::try_from(sequence & 0xff).unwrap()],
            truncation: TruncationMetadata {
                captured_length: 1,
                observed_length: None,
                state: chronicle_capture::TruncationState::Complete,
                reason: None,
            },
        });
        input
    }

    fn ordered_tcp(events: Vec<ReconstructionInput>) -> TcpPayloadOrdering {
        let mut assembler = ReconstructionAssembler::default();
        for event in events {
            assembler.push(event);
        }
        assembler
            .finish()
            .connections
            .pop()
            .unwrap()
            .ordered_payloads(ReconstructionDirection::Ingress)
    }

    fn loss_window(
        boot_id: &str,
        start: u64,
        end: u64,
        delta: Option<u64>,
        reason: &str,
    ) -> LossWindowObserved {
        LossWindowObserved {
            start: MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: boot_id.into(),
                },
                nanoseconds: start,
            },
            end: MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: boot_id.into(),
                },
                nanoseconds: end,
            },
            drop_delta: delta,
            loss_generation: delta.map(|_| 1),
            ambiguity: chronicle_capture::LossAmbiguity {
                exact_drop_timing_unknown: true,
                affected_sockets_unknown: true,
                reason: Some(reason.into()),
            },
        }
    }

    #[test]
    fn loss_windows_classify_without_cross_clock_timestamp_comparison() {
        let ReconstructionInput::Event(event) = production_event("boot-a", 1, 10) else {
            panic!("production_event must produce an event");
        };
        assert_eq!(
            classify_loss_window(
                std::slice::from_ref(&event),
                &loss_window("boot-a", 5, 15, Some(1), "sample")
            ),
            LossWindowClassification::Overlaps
        );
        assert_eq!(
            classify_loss_window(
                std::slice::from_ref(&event),
                &loss_window("boot-a", 20, 30, Some(1), "sample")
            ),
            LossWindowClassification::Outside
        );
        assert_eq!(
            classify_loss_window(
                std::slice::from_ref(&event),
                &loss_window("boot-a", 5, 15, None, "incomplete")
            ),
            LossWindowClassification::Unknown
        );
        assert_eq!(
            classify_loss_window(
                std::slice::from_ref(&event),
                &loss_window("boot-a", 5, 15, None, "counter generation changed")
            ),
            LossWindowClassification::Reset
        );
        assert_eq!(
            classify_loss_window(
                std::slice::from_ref(&event),
                &loss_window("boot-b", 5, 15, Some(1), "sample"),
            ),
            LossWindowClassification::ClockMismatch
        );
    }

    #[test]
    fn lifecycle_evidence_stays_raw_while_termination_is_derived() {
        let mut close = production_event("boot-a", 1, 10);
        let ReconstructionInput::Event(event) = &mut close else {
            unreachable!()
        };
        event.evidence = ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Closed);
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(close.clone());
        let connection = assembler.finish().connections.pop().unwrap();
        assert_eq!(connection.termination, DerivedTermination::CleanClose);
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(close);
        assembler.push(ReconstructionInput::LossWindow(loss_window(
            "boot-a",
            5,
            15,
            Some(1),
            "sample",
        )));
        assert_eq!(
            assembler.finish().connections[0].termination,
            DerivedTermination::UnknownTermination
        );
        assert!(matches!(
            connection.events[0].evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Closed)
        ));

        let mut reset = production_event("boot-a", 2, 10);
        let ReconstructionInput::Event(event) = &mut reset else {
            unreachable!()
        };
        event.evidence = ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::Reset);
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(reset);
        assert_eq!(
            assembler.finish().connections[0].termination,
            DerivedTermination::Reset
        );
    }

    #[test]
    fn terminal_wal_loss_is_not_kernel_loss_window() {
        let terminal = TerminalPersistenceLoss {
            interval: PersistenceLossInterval {
                start: MonotonicTimestamp {
                    clock: ClockIdentity {
                        boot_id: "boot-a".into(),
                    },
                    nanoseconds: 10,
                },
                end: MonotonicTimestamp {
                    clock: ClockIdentity {
                        boot_id: "boot-a".into(),
                    },
                    nanoseconds: 20,
                },
            },
            discarded_records: 2,
            discarded_payload_bytes: 3,
            reason: PersistenceLossReason::WalHardLimit,
            ambiguity: PersistenceLossAmbiguity::UnknownDownstreamEffects,
        };
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(ReconstructionInput::TerminalWalLoss(terminal.clone()));
        assembler.push(ReconstructionInput::LossWindow(loss_window(
            "boot-a",
            10,
            20,
            Some(1),
            "sample",
        )));
        let assembly = assembler.finish();
        assert_eq!(assembly.terminal_wal_losses, [terminal]);
        assert_eq!(assembly.loss_windows.len(), 1);
    }

    #[test]
    fn tcp_order_handles_increasing_and_wraparound_sequences() {
        let normal = ordered_tcp(vec![
            tcp_payload(5, 9, ReconstructionDirection::Egress),
            tcp_payload(20, 2, ReconstructionDirection::Ingress),
            tcp_payload(10, 1, ReconstructionDirection::Ingress),
            tcp_payload(30, 3, ReconstructionDirection::Ingress),
        ]);
        assert_eq!(normal.ambiguity, None);
        assert_eq!(
            normal
                .fragments
                .iter()
                .map(tcp_sequence)
                .collect::<Vec<_>>(),
            [10, 20, 30]
        );
        let wrapped = ordered_tcp(vec![
            tcp_payload(1, 4, ReconstructionDirection::Ingress),
            tcp_payload(u32::MAX - 1, 1, ReconstructionDirection::Ingress),
            tcp_payload(0, 3, ReconstructionDirection::Ingress),
            tcp_payload(u32::MAX, 2, ReconstructionDirection::Ingress),
        ]);
        assert_eq!(wrapped.ambiguity, None);
        assert_eq!(
            wrapped
                .fragments
                .iter()
                .map(tcp_sequence)
                .collect::<Vec<_>>(),
            [u32::MAX - 1, u32::MAX, 0, 1]
        );
    }

    fn tcp_sequence(event: &ReconstructionEvent) -> u32 {
        let ReconstructionEvidence::Payload(payload) = &event.evidence else {
            unreachable!()
        };
        payload.tcp_position.as_ref().unwrap().tcp_sequence
    }

    #[test]
    fn tcp_half_range_is_explicitly_ambiguous_and_permutations_are_deterministic() {
        let ambiguous = ordered_tcp(vec![
            tcp_payload(0, 2, ReconstructionDirection::Ingress),
            tcp_payload(1 << 31, 1, ReconstructionDirection::Ingress),
        ]);
        assert_eq!(
            ambiguous.ambiguity,
            Some(TcpOrderingAmbiguity::HalfSequenceRange)
        );
        assert_eq!(ambiguous.fragments.len(), 2);
        let mut first_duplicate = tcp_payload(9, 1, ReconstructionDirection::Ingress);
        let mut second_duplicate = tcp_payload(9, 2, ReconstructionDirection::Ingress);
        for input in [&mut first_duplicate, &mut second_duplicate] {
            let ReconstructionInput::Event(event) = input else {
                unreachable!()
            };
            event.wal_sequence = None;
        }
        assert_eq!(
            ordered_tcp(vec![first_duplicate, second_duplicate]).ambiguity,
            Some(TcpOrderingAmbiguity::MissingProvenance)
        );
        let first = ordered_tcp(vec![
            tcp_payload(2, 3, ReconstructionDirection::Ingress),
            tcp_payload(0, 1, ReconstructionDirection::Ingress),
            tcp_payload(1, 2, ReconstructionDirection::Ingress),
        ]);
        let second = ordered_tcp(vec![
            tcp_payload(1, 2, ReconstructionDirection::Ingress),
            tcp_payload(2, 3, ReconstructionDirection::Ingress),
            tcp_payload(0, 1, ReconstructionDirection::Ingress),
        ]);
        assert_eq!(first, second);
        assert_eq!(
            first.fragments.iter().map(tcp_sequence).collect::<Vec<_>>(),
            [0, 1, 2]
        );
    }

    #[test]
    fn tcp_provenance_breaks_equal_positions_and_preserves_evidence() {
        let mut later = tcp_payload(10, 2, ReconstructionDirection::Ingress);
        let ReconstructionInput::Event(later_event) = &mut later else {
            unreachable!()
        };
        later_event.timestamp.monotonic.clock.boot_id = "boot-evidence".into();
        let ReconstructionEvidence::Payload(payload) = &mut later_event.evidence else {
            unreachable!()
        };
        payload.bytes = vec![9, 8];
        payload.truncation = TruncationMetadata {
            captured_length: 2,
            observed_length: Some(3),
            state: chronicle_capture::TruncationState::Truncated,
            reason: Some("bound".into()),
        };
        let earlier = tcp_payload(10, 1, ReconstructionDirection::Ingress);
        let expected = later.clone();
        let ordered = ordered_tcp(vec![later, earlier]);
        assert_eq!(ordered.ambiguity, None);
        assert_eq!(
            ordered
                .fragments
                .iter()
                .map(|event| event.wal_sequence)
                .collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        assert_eq!(
            ordered.fragments[1],
            match expected {
                ReconstructionInput::Event(event) => event,
                _ => unreachable!(),
            }
        );
    }

    #[test]
    fn tcp_retransmissions_deduplicate_and_overlaps_retain_both_evidences() {
        let first = tcp_payload(10, 1, ReconstructionDirection::Ingress);
        let duplicate = tcp_payload(10, 2, ReconstructionDirection::Ingress);
        let exact = ordered_tcp(vec![first.clone(), duplicate.clone()]);
        assert_eq!(exact.fragments.len(), 1);
        assert_eq!(exact.retransmissions.len(), 1);
        assert_eq!(
            exact.retransmissions[0].retransmission,
            match duplicate {
                ReconstructionInput::Event(event) => event,
                _ => unreachable!(),
            }
        );

        let mut conflicting = tcp_payload(10, 3, ReconstructionDirection::Ingress);
        let ReconstructionInput::Event(event) = &mut conflicting else {
            unreachable!()
        };
        let ReconstructionEvidence::Payload(payload) = &mut event.evidence else {
            unreachable!()
        };
        payload.bytes = vec![99];
        let conflict = ordered_tcp(vec![first, conflicting]);
        assert_eq!(conflict.overlaps.len(), 1);
        assert_eq!(conflict.overlaps[0].kind, TcpOverlapKind::Conflicting);

        let mut wide = tcp_payload(20, 4, ReconstructionDirection::Ingress);
        let ReconstructionInput::Event(event) = &mut wide else {
            unreachable!()
        };
        let ReconstructionEvidence::Payload(payload) = &mut event.evidence else {
            unreachable!()
        };
        payload.bytes = vec![1, 2];
        let partial = ordered_tcp(vec![
            wide,
            tcp_payload(21, 5, ReconstructionDirection::Ingress),
        ]);
        assert_eq!(partial.overlaps.len(), 1);
        assert_eq!(partial.overlaps[0].kind, TcpOverlapKind::Partial);
    }

    #[test]
    fn protocol_neutral_output_preserves_directional_bytes_and_safe_chronology() {
        let mut ingress = tcp_payload(10, 3, ReconstructionDirection::Ingress);
        let mut egress = tcp_payload(20, 7, ReconstructionDirection::Egress);
        let (ReconstructionInput::Event(ingress_event), ReconstructionInput::Event(egress_event)) =
            (&mut ingress, &mut egress)
        else {
            unreachable!()
        };
        ingress_event.timestamp.monotonic.nanoseconds = 10;
        egress_event.timestamp.monotonic.nanoseconds = 20;
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(ingress).unwrap();
        assembler.push(egress).unwrap();
        let output = assembler
            .finish()
            .connections
            .pop()
            .unwrap()
            .protocol_neutral();
        assert_eq!(output.client_to_server.fragments.len(), 1);
        assert_eq!(output.server_to_client.fragments.len(), 1);
        assert_eq!(output.wal_sequence_range, Some((3, 7)));
        assert_eq!(
            output
                .cross_direction_completion
                .unwrap()
                .iter()
                .map(|event| event.direction)
                .collect::<Vec<_>>(),
            [
                ReconstructionDirection::Ingress,
                ReconstructionDirection::Egress
            ]
        );

        let mut ingress = tcp_payload(10, 3, ReconstructionDirection::Ingress);
        let mut egress = tcp_payload(20, 7, ReconstructionDirection::Egress);
        let (ReconstructionInput::Event(ingress_event), ReconstructionInput::Event(egress_event)) =
            (&mut ingress, &mut egress)
        else {
            unreachable!()
        };
        ingress_event.timestamp.monotonic.clock.boot_id = "boot-a".into();
        egress_event.timestamp.monotonic.clock.boot_id = "boot-b".into();
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(ingress).unwrap();
        assembler.push(egress).unwrap();
        assert!(
            assembler
                .finish()
                .connections
                .pop()
                .unwrap()
                .protocol_neutral()
                .cross_direction_completion
                .is_none()
        );
    }

    #[test]
    fn reconstruction_limits_finalize_only_target_and_reject_active_overflow() {
        let mut assembler = ReconstructionAssembler::new(ReconstructionLimits {
            max_active_connections: 1,
            max_bytes_per_connection: 8,
            max_chunks_per_connection: 8,
            idle_nanoseconds: 100,
        });
        assembler.push(production_event("boot-a", 1, 0)).unwrap();
        assert_eq!(
            assembler.push(production_event("boot-a", 2, 0)),
            Err(ReconstructionPushError::ActiveConnectionLimit { limit: 1 })
        );

        let mut assembler = ReconstructionAssembler::new(ReconstructionLimits {
            max_active_connections: 2,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 8,
            idle_nanoseconds: 100,
        });
        assembler
            .push(tcp_payload(1, 1, ReconstructionDirection::Ingress))
            .unwrap();
        assembler
            .push(tcp_payload(2, 2, ReconstructionDirection::Ingress))
            .unwrap();
        let assembly = assembler.finish();
        assert!(assembly.connections.iter().any(
            |connection| connection.finalization == Some(ReconstructionFinalization::ByteLimit)
        ));

        let mut assembler = ReconstructionAssembler::new(ReconstructionLimits {
            max_active_connections: 2,
            max_bytes_per_connection: 8,
            max_chunks_per_connection: 1,
            idle_nanoseconds: 100,
        });
        assembler
            .push(tcp_payload(1, 1, ReconstructionDirection::Ingress))
            .unwrap();
        assembler
            .push(tcp_payload(2, 2, ReconstructionDirection::Ingress))
            .unwrap();
        assert!(
            assembler
                .finish()
                .connections
                .iter()
                .any(|connection| connection.finalization
                    == Some(ReconstructionFinalization::ChunkLimit))
        );
    }

    #[test]
    fn reconstruction_idle_uses_only_comparable_clock_identity() {
        let limits = ReconstructionLimits {
            idle_nanoseconds: 100,
            ..ReconstructionLimits::default()
        };
        let mut assembler = ReconstructionAssembler::new(limits);
        assembler.push(production_event("boot-a", 1, 0)).unwrap();
        assembler.push(production_event("boot-a", 2, 100)).unwrap();
        assert!(assembler.finish().connections.iter().any(|connection| connection.finalization == Some(ReconstructionFinalization::Idle)));

        let mut assembler = ReconstructionAssembler::new(limits);
        assembler.push(production_event("boot-a", 1, 0)).unwrap();
        assembler.push(production_event("boot-b", 2, 100)).unwrap();
        assert!(
            assembler
                .finish()
                .connections
                .iter()
                .all(|connection| connection.finalization.is_none())
        );
    }

    #[test]
    fn payload_without_open_gets_synthetic_missing_open_evidence() {
        let mut input = production_event("boot-a", 8, 10);
        let ReconstructionInput::Event(event) = &mut input else {
            unreachable!();
        };
        event.evidence = ReconstructionEvidence::Payload(ReconstructionPayload {
            direction: ReconstructionDirection::Ingress,
            tcp_position: None,
            bytes: vec![1],
            truncation: TruncationMetadata {
                captured_length: 1,
                observed_length: None,
                state: chronicle_capture::TruncationState::Complete,
                reason: None,
            },
        });
        let mut assembler = ReconstructionAssembler::default();
        assembler.push(input);
        let connection = assembler.finish().connections.pop().unwrap();
        assert!(matches!(
            connection.events[0].evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::MissingOpen)
        ));
        assert_eq!(
            connection.termination,
            DerivedTermination::UnknownTermination
        );
    }

    #[test]
    fn reconstruction_snapshot_restores_pending_fragments_exactly() {
        let first = tcp_payload(1, 10, ReconstructionDirection::Ingress);
        let second = tcp_payload(2, 11, ReconstructionDirection::Ingress);
        let mut uninterrupted = ReconstructionAssembler::default();
        uninterrupted.push(first.clone()).unwrap();
        uninterrupted.push(second.clone()).unwrap();
        let expected = uninterrupted.finish();

        let mut before_restart = ReconstructionAssembler::default();
        before_restart.push(first).unwrap();
        let snapshot = before_restart.snapshot().unwrap();
        assert!(!snapshot.is_empty());
        let mut resumed =
            ReconstructionAssembler::restore(ReconstructionLimits::default(), &snapshot).unwrap();
        resumed.push(second).unwrap();
        assert_eq!(resumed.finish(), expected);
    }

    #[test]
    fn reconstruction_snapshot_rejects_unknown_version_and_limit_mismatch() {
        let assembler = ReconstructionAssembler::default();
        let mut value: serde_json::Value =
            serde_json::from_slice(&assembler.snapshot().unwrap()).unwrap();
        value["version"] = serde_json::json!(99);
        let bytes = serde_json::to_vec(&value).unwrap();
        assert!(matches!(
            ReconstructionAssembler::restore(ReconstructionLimits::default(), &bytes),
            Err(ReconstructionSnapshotError::UnsupportedVersion(99))
        ));

        let bytes = assembler.snapshot().unwrap();
        assert!(matches!(
            ReconstructionAssembler::restore(
                ReconstructionLimits {
                    max_bytes_per_connection: 1,
                    ..ReconstructionLimits::default()
                },
                &bytes,
            ),
            Err(ReconstructionSnapshotError::LimitMismatch)
        ));
    }

    #[test]
    fn orders_chunks_by_monotonic_sequence() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        push_fixture(&mut assembler, 2, "client", 1).unwrap();
        push_fixture(&mut assembler, 1, "client", 1).unwrap();
        let stream = assembler.finish().pop().unwrap();
        let sequences: Vec<_> = stream
            .chunks
            .iter()
            .map(|event| event.monotonic_sequence)
            .collect();
        assert_eq!(sequences, [1, 2]);
    }

    #[test]
    fn rejected_oversized_new_event_does_not_consume_connection_capacity() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 1,
        };
        let mut assembler = SessionAssembler::new(limits);
        assert!(matches!(
            push_fixture(&mut assembler, 1, "oversized", 2),
            Err(SessionError::ByteLimit { .. })
        ));
        push_fixture(&mut assembler, 2, "accepted", 1).unwrap();
        let streams = assembler.finish();
        assert_eq!(streams.len(), 1);
        assert_eq!(streams[0].key.client.host, "accepted");
    }

    #[test]
    fn enforces_connection_and_existing_stream_byte_limits() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 2,
        };
        let mut assembler = SessionAssembler::new(limits);
        push_fixture(&mut assembler, 1, "first", 1).unwrap();
        assert!(matches!(
            push_fixture(&mut assembler, 2, "first", 1),
            Err(SessionError::ByteLimit { .. })
        ));
        assert!(matches!(
            push_fixture(&mut assembler, 3, "second", 1),
            Err(SessionError::ConnectionLimit { limit: 1 })
        ));
    }

    #[test]
    fn limits_zero_byte_chunks() {
        let limits = SessionLimits {
            max_connections: 1,
            max_bytes_per_connection: 1,
            max_chunks_per_connection: 1,
        };
        let mut assembler = SessionAssembler::new(limits);
        push_fixture(&mut assembler, 1, "first", 0).unwrap();
        assert!(matches!(
            push_fixture(&mut assembler, 2, "first", 0),
            Err(SessionError::ChunkLimit { limit: 1 })
        ));
    }

    #[test]
    fn preserves_direction_order_and_truncation() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        let mut response = event_for(2, "client", 1);
        if let CaptureEventKind::PayloadFragment(fragment) = &mut response.kind {
            fragment.direction = PayloadDirection::Ingress;
            fragment.truncation.state = TruncationState::Truncated;
        }
        assembler.push(fixture_evidence("client")).unwrap();
        assembler.push(response).unwrap();
        push_fixture(&mut assembler, 1, "client", 1).unwrap();

        let stream = assembler.finish().pop().unwrap();
        assert_eq!(
            stream
                .chunks
                .iter()
                .map(|chunk| chunk.monotonic_sequence)
                .collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(stream.chunks[1].direction, Direction::ServerToClient);
        assert!(stream.truncated);
    }
}
