//! Bounded reconstruction of ordered bidirectional socket byte streams.

use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CAPTURE_EVENT_V2_SCHEMA_VERSION, CaptureEvent, CaptureEventKind,
    CaptureEventV1, ClockIdentity, FragmentSequenceEvidence, LossWindowObserved,
    MonotonicTimestamp, PayloadDirection, PayloadFragment, RecordingScopeIdentity, SocketEvidence,
    SocketIdentity, SocketStateChangeObserved, TruncationMetadata, V2FixtureObservation,
};
use chronicle_common::{ConnectionKey, Direction};
use chronicle_wal::TerminalWalLoss;
use std::collections::BTreeMap;
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
pub struct ConnectionStream {
    pub key: ConnectionKey,
    pub chunks: Vec<CaptureEventV1>,
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
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReconstructionConnectionIdentity {
    Fixture(ConnectionKey),
    Socket(SocketIdentity),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconstructionTimestamp {
    pub monotonic: MonotonicTimestamp,
    pub fixture_derived: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconstructionDirection {
    ClientToServer,
    ServerToClient,
    Ingress,
    Egress,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconstructionPayload {
    pub direction: ReconstructionDirection,
    pub tcp_position: Option<FragmentSequenceEvidence>,
    pub bytes: Vec<u8>,
    pub truncation: TruncationMetadata,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawLifecycleEvidence {
    MissingOpen,
    ConnectObserved,
    Established,
    Closed,
    Reset,
    StateChanged { raw_state: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconstructionEvidence {
    Lifecycle(RawLifecycleEvidence),
    Payload(ReconstructionPayload),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconstructionEvent {
    pub connection: ReconstructionConnectionIdentity,
    /// Persistence sequence is ordering evidence only; absent for live/unpersisted input.
    pub wal_sequence: Option<u64>,
    pub timestamp: ReconstructionTimestamp,
    pub recording_scope: Option<RecordingScopeIdentity>,
    pub evidence: ReconstructionEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconstructionInput {
    Event(ReconstructionEvent),
    LossWindow(LossWindowObserved),
    /// WAL-limit discard evidence. Never coerced into kernel loss sampling.
    TerminalWalLoss(TerminalWalLoss),
}

/// Groups reconstruction evidence by fixture key or kernel socket generation identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconstructionConnection {
    pub identity: ReconstructionConnectionIdentity,
    pub events: Vec<ReconstructionEvent>,
    pub loss_windows: Vec<ConnectionLossWindow>,
    pub termination: DerivedTermination,
    pub finalization: Option<ReconstructionFinalization>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconstructionFinalization {
    ByteLimit,
    ChunkLimit,
    Idle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedTermination {
    CleanClose,
    /// Reserved until directional close evidence is available from Gate A.
    HalfClose,
    Reset,
    UnknownTermination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossWindowClassification {
    Outside,
    Overlaps,
    Unknown,
    Reset,
    ClockMismatch,
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
    pub ingress: TcpPayloadOrdering,
    pub egress: TcpPayloadOrdering,
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
        let ingress = self.ordered_payloads(ReconstructionDirection::Ingress);
        let egress = self.ordered_payloads(ReconstructionDirection::Egress);
        let completions = [
            (ReconstructionDirection::Ingress, ingress.fragments.last()),
            (ReconstructionDirection::Egress, egress.fragments.last()),
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
            ingress,
            egress,
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
            ) {
                if previous.is_none() || event.wal_sequence.is_none() {
                    ambiguity = Some(TcpOrderingAmbiguity::MissingProvenance);
                }
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
    pub terminal_wal_losses: Vec<TerminalWalLoss>,
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

pub struct ReconstructionAssembler {
    limits: ReconstructionLimits,
    connections: BTreeMap<ReconstructionConnectionIdentity, Vec<ReconstructionEvent>>,
    finalized: Vec<(
        ReconstructionConnectionIdentity,
        Vec<ReconstructionEvent>,
        ReconstructionFinalization,
    )>,
    loss_windows: Vec<LossWindowObserved>,
    terminal_wal_losses: Vec<TerminalWalLoss>,
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
            _ => 0,
        };
        let chunk_count = events
            .iter()
            .filter(|event| matches!(event.evidence, ReconstructionEvidence::Payload(_)))
            .count();
        let byte_count: usize = events
            .iter()
            .filter_map(|event| match &event.evidence {
                ReconstructionEvidence::Payload(payload) => Some(payload.bytes.len()),
                _ => None,
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
                timestamp: event.timestamp.clone(),
                recording_scope: event.recording_scope.clone(),
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
        match event {
            CaptureEvent::V1(event) if event.schema_version == CAPTURE_EVENT_SCHEMA_VERSION => {
                Ok(Self::Event(fixture_input(event)))
            }
            CaptureEvent::V2(event) if event.schema_version == CAPTURE_EVENT_V2_SCHEMA_VERSION => {
                Ok(v2_input(event.kind))
            }
            event => Err(ReconstructionInputError::UnsupportedCaptureSchema(
                event.schema_version(),
            )),
        }
    }

    pub fn from_v2_fixture_observation(
        observation: V2FixtureObservation,
    ) -> Result<Self, ReconstructionInputError> {
        match observation {
            V2FixtureObservation::Event(event)
                if event.schema_version == CAPTURE_EVENT_V2_SCHEMA_VERSION =>
            {
                Ok(v2_input(event.kind))
            }
            V2FixtureObservation::Event(event) => Err(
                ReconstructionInputError::UnsupportedCaptureSchema(event.schema_version),
            ),
            V2FixtureObservation::LossWindow(window) => Ok(Self::LossWindow(window)),
        }
    }
}

fn fixture_input(event: CaptureEventV1) -> ReconstructionEvent {
    let captured_length = u32::try_from(event.payload.len()).unwrap_or(u32::MAX);
    ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Fixture(event.connection),
        wal_sequence: None,
        timestamp: ReconstructionTimestamp {
            monotonic: MonotonicTimestamp {
                clock: ClockIdentity {
                    boot_id: "fixture-v1".into(),
                },
                nanoseconds: event.monotonic_sequence,
            },
            fixture_derived: true,
        },
        recording_scope: None,
        evidence: ReconstructionEvidence::Payload(ReconstructionPayload {
            direction: match event.direction {
                Direction::ClientToServer => ReconstructionDirection::ClientToServer,
                Direction::ServerToClient => ReconstructionDirection::ServerToClient,
            },
            tcp_position: None,
            bytes: event.payload,
            truncation: TruncationMetadata {
                captured_length,
                observed_length: None,
                state: if event.truncated {
                    chronicle_capture::TruncationState::Truncated
                } else {
                    chronicle_capture::TruncationState::Complete
                },
                reason: None,
            },
        }),
    }
}

fn v2_input(kind: CaptureEventKind) -> ReconstructionInput {
    match kind {
        CaptureEventKind::SocketConnectObserved(evidence) => {
            lifecycle_input(evidence, RawLifecycleEvidence::ConnectObserved)
        }
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

fn lifecycle_input(
    evidence: SocketEvidence,
    lifecycle: RawLifecycleEvidence,
) -> ReconstructionInput {
    ReconstructionInput::Event(ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Socket(evidence.socket),
        wal_sequence: None,
        timestamp: production_timestamp(evidence.timestamp),
        recording_scope: Some(evidence.recording_scope),
        evidence: ReconstructionEvidence::Lifecycle(lifecycle),
    })
}

fn payload_input(fragment: PayloadFragment) -> ReconstructionInput {
    ReconstructionInput::Event(ReconstructionEvent {
        connection: ReconstructionConnectionIdentity::Socket(fragment.socket),
        wal_sequence: None,
        timestamp: production_timestamp(fragment.timestamp),
        recording_scope: Some(fragment.recording_scope),
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

fn production_timestamp(monotonic: MonotonicTimestamp) -> ReconstructionTimestamp {
    ReconstructionTimestamp {
        monotonic,
        fixture_derived: false,
    }
}

pub struct SessionAssembler {
    limits: SessionLimits,
    streams: BTreeMap<ConnectionKey, ConnectionStream>,
}

impl SessionAssembler {
    pub fn new(limits: SessionLimits) -> Self {
        Self {
            limits,
            streams: BTreeMap::new(),
        }
    }

    pub fn push(&mut self, event: CaptureEvent) -> Result<(), SessionError> {
        let event = match event {
            CaptureEvent::V1(event) => event,
            event @ CaptureEvent::V2(_) => {
                return Err(SessionError::UnsupportedCaptureEvent {
                    schema_version: event.schema_version(),
                });
            }
        };
        let existing_bytes = self
            .streams
            .get(&event.connection)
            .map_or(0, |stream| stream.total_bytes);
        let attempted = existing_bytes.saturating_add(event.payload.len());
        if attempted > self.limits.max_bytes_per_connection {
            return Err(SessionError::ByteLimit {
                limit: self.limits.max_bytes_per_connection,
                attempted,
            });
        }
        let existing_chunks = self
            .streams
            .get(&event.connection)
            .map_or(0, |stream| stream.chunks.len());
        if existing_chunks >= self.limits.max_chunks_per_connection {
            return Err(SessionError::ChunkLimit {
                limit: self.limits.max_chunks_per_connection,
            });
        }
        if !self.streams.contains_key(&event.connection)
            && self.streams.len() >= self.limits.max_connections
        {
            return Err(SessionError::ConnectionLimit {
                limit: self.limits.max_connections,
            });
        }
        let stream = self
            .streams
            .entry(event.connection.clone())
            .or_insert_with(|| ConnectionStream {
                key: event.connection.clone(),
                chunks: Vec::new(),
                total_bytes: 0,
                truncated: false,
            });
        stream.total_bytes = attempted;
        stream.truncated |= event.truncated;
        stream.chunks.push(event);
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

#[cfg(test)]
#[allow(unused_must_use)] // Legacy success-path tests predate typed reconstruction push failures.
mod tests {
    use super::*;
    use chronicle_capture::{CAPTURE_EVENT_SCHEMA_VERSION, CaptureEventV1, CaptureFlags};
    use chronicle_common::{Direction, Endpoint, TransportProtocol};

    fn event(sequence: u64) -> CaptureEvent {
        event_for(sequence, "client", 1)
    }

    fn event_for(sequence: u64, client: &str, payload_bytes: usize) -> CaptureEvent {
        CaptureEvent::V1(CaptureEventV1 {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: sequence,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new(client, 10),
                Endpoint::new("server", 20),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: vec![u8::try_from(sequence).unwrap(); payload_bytes],
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        })
    }

    #[test]
    fn fixture_v1_adapts_to_fixture_derived_reconstruction_input() {
        let fixture = event_for(7, "fixture-client", 2);
        let CaptureEvent::V1(v1) = fixture.clone() else {
            panic!("fixture event must be v1");
        };
        let input = ReconstructionInput::from_capture_event(fixture).unwrap();
        let ReconstructionInput::Event(ReconstructionEvent {
            connection: ReconstructionConnectionIdentity::Fixture(connection),
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

        assert_eq!(connection, v1.connection);
        assert_eq!(timestamp.monotonic.clock.boot_id, "fixture-v1");
        assert_eq!(timestamp.monotonic.nanoseconds, 7);
        assert!(timestamp.fixture_derived);
        assert_eq!(recording_scope, None);
        assert_eq!(direction, ReconstructionDirection::ClientToServer);
        assert_eq!(tcp_position, None);
        assert_eq!(bytes, v1.payload);
        assert_eq!(truncation.captured_length, 2);
        assert_eq!(
            truncation.state,
            chronicle_capture::TruncationState::Complete
        );
    }

    #[test]
    fn v2_payload_adapts_without_legacy_connection_or_wal_provenance() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 11,
        };
        let input = ReconstructionInput::from_capture_event(CaptureEvent::V2(
            chronicle_capture::CaptureEventV2 {
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
                }),
            },
        ))
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
    fn v2_lifecycle_adapts_as_raw_evidence() {
        let timestamp = MonotonicTimestamp {
            clock: ClockIdentity {
                boot_id: "boot-a".into(),
            },
            nanoseconds: 12,
        };
        let input = ReconstructionInput::from_capture_event(CaptureEvent::V2(
            chronicle_capture::CaptureEventV2 {
                schema_version: CAPTURE_EVENT_V2_SCHEMA_VERSION,
                kind: CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                    socket: SocketEvidence {
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
                        network_family: chronicle_capture::NetworkFamily::Ipv4,
                        process: None,
                        observed_cgroup_id: 12,
                    },
                    raw_state: 99,
                }),
            },
        ))
        .unwrap();
        let ReconstructionInput::Event(event) = input else {
            panic!("expected lifecycle evidence");
        };
        assert_eq!(
            event.evidence,
            ReconstructionEvidence::Lifecycle(RawLifecycleEvidence::StateChanged { raw_state: 99 })
        );
    }

    #[test]
    fn v2_loss_window_stays_separate_reconstruction_input() {
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
        let input = ReconstructionInput::from_v2_fixture_observation(
            V2FixtureObservation::LossWindow(window.clone()),
        )
        .unwrap();
        assert_eq!(input, ReconstructionInput::LossWindow(window));
    }

    #[test]
    fn reconstruction_input_rejects_unknown_capture_schema() {
        let mut event = event_for(1, "client", 1);
        if let CaptureEvent::V1(event) = &mut event {
            event.schema_version = CAPTURE_EVENT_V2_SCHEMA_VERSION + 1;
        }
        assert_eq!(
            ReconstructionInput::from_capture_event(event),
            Err(ReconstructionInputError::UnsupportedCaptureSchema(3))
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
            timestamp: ReconstructionTimestamp {
                monotonic: MonotonicTimestamp {
                    clock,
                    nanoseconds: first_seen,
                },
                fixture_derived: false,
            },
            recording_scope: None,
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
        assert!(matches!(
            assembly.connections[0].identity,
            ReconstructionConnectionIdentity::Fixture(_)
        ));
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
        let event = match production_event("boot-a", 1, 10) {
            ReconstructionInput::Event(event) => event,
            _ => unreachable!(),
        };
        assert_eq!(
            classify_loss_window(
                &[event.clone()],
                &loss_window("boot-a", 5, 15, Some(1), "sample")
            ),
            LossWindowClassification::Overlaps
        );
        assert_eq!(
            classify_loss_window(
                &[event.clone()],
                &loss_window("boot-a", 20, 30, Some(1), "sample")
            ),
            LossWindowClassification::Outside
        );
        assert_eq!(
            classify_loss_window(
                &[event.clone()],
                &loss_window("boot-a", 5, 15, None, "incomplete")
            ),
            LossWindowClassification::Unknown
        );
        assert_eq!(
            classify_loss_window(
                &[event.clone()],
                &loss_window("boot-a", 5, 15, None, "counter generation changed")
            ),
            LossWindowClassification::Reset
        );
        assert_eq!(
            classify_loss_window(&[event], &loss_window("boot-b", 5, 15, Some(1), "sample")),
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
        let terminal = TerminalWalLoss {
            interval: chronicle_wal::TerminalWalLossInterval {
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
            reason: chronicle_wal::TerminalWalLossReason::WalHardLimit,
            ambiguity: chronicle_wal::TerminalWalLossAmbiguity::UnknownDownstreamEffects,
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
        assert_eq!(output.ingress.fragments.len(), 1);
        assert_eq!(output.egress.fragments.len(), 1);
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
    fn orders_chunks_by_monotonic_sequence() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        assembler.push(event(2)).unwrap();
        assembler.push(event(1)).unwrap();
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
            assembler.push(event_for(1, "oversized", 2)),
            Err(SessionError::ByteLimit { .. })
        ));
        assembler.push(event_for(2, "accepted", 1)).unwrap();
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
        assembler.push(event_for(1, "first", 1)).unwrap();
        assert!(matches!(
            assembler.push(event_for(2, "first", 1)),
            Err(SessionError::ByteLimit { .. })
        ));
        assert!(matches!(
            assembler.push(event_for(3, "second", 1)),
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
        assembler.push(event_for(1, "first", 0)).unwrap();
        assert!(matches!(
            assembler.push(event_for(2, "first", 0)),
            Err(SessionError::ChunkLimit { limit: 1 })
        ));
    }

    #[test]
    fn preserves_direction_order_and_truncation() {
        let mut assembler = SessionAssembler::new(SessionLimits::default());
        let mut response = event_for(2, "client", 1);
        if let CaptureEvent::V1(event) = &mut response {
            event.direction = Direction::ServerToClient;
            event.truncated = true;
        }
        assembler.push(response).unwrap();
        assembler.push(event_for(1, "client", 1)).unwrap();

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
