//! Linux eBPF capture adapter boundary.
//!
//! Aya handles and generated eBPF types stay in this crate. The private ABI is decoded
//! into `RawKernelObservation`, then later normalized by `CaptureAdapter`.

use chronicle_capture::{
    CaptureError, CaptureEvent, CaptureEventKind, CaptureEventV2, CaptureSource,
    CaptureSourceState, CaptureSourceSummary, ClockIdentity, FragmentSequenceEvidence,
    LossAmbiguity, LossWindowObserved, MonotonicTimestamp, NetworkFamily, PayloadDirection,
    PayloadFragment, ProcessMetadata, RecordingScopeIdentity, SocketEvidence, SocketIdentity,
    SocketStateChangeObserved, TruncationMetadata, TruncationState,
};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const ABI_MAGIC: [u8; 4] = *b"CHRN";
const ABI_VERSION: u16 = 1;
const ABI_HEADER_BYTES: usize = 12;
const LITTLE_ENDIAN: u8 = 1;

const KIND_CONNECT4: u8 = 1;
const KIND_CONNECT6: u8 = 2;
const KIND_SOCKOPS: u8 = 3;
const KIND_INGRESS: u8 = 4;
const KIND_EGRESS: u8 = 5;
const KIND_LOSS_COUNTERS: u8 = 6;

const SIGNAL_ACTIVE_ESTABLISHED: u8 = 1;
const SIGNAL_CLOSE: u8 = 2;
const SIGNAL_RESET: u8 = 3;
const SIGNAL_AMBIGUOUS: u8 = 4;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EbpfCaptureError {
    #[error("eBPF capture requires Linux")]
    UnsupportedPlatform,
    #[error("unsupported eBPF capability: {0}")]
    UnsupportedCapability(&'static str),
    #[error("eBPF attach failed for {hook}: {reason}")]
    Attach {
        hook: &'static str,
        reason: &'static str,
    },
    #[error("eBPF verifier rejected {program}: {reason}")]
    Verifier {
        program: &'static str,
        reason: &'static str,
    },
    #[error("eBPF ABI decode failed at {context}: {reason}")]
    Decode {
        context: &'static str,
        reason: &'static str,
    },
    #[error("eBPF payload is invalid at {context}: {reason}")]
    InvalidPayload {
        context: &'static str,
        reason: &'static str,
    },
    #[error("eBPF observation lacks required identity: {0}")]
    MissingIdentity(&'static str),
    #[error("eBPF ring-loss evidence is incomplete: {0}")]
    RingLoss(&'static str),
    #[error("eBPF cleanup failed: {0}")]
    Cleanup(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KernelArchitecture {
    Aarch64,
    X86_64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawNetworkFamily {
    Ipv4,
    Ipv6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawDirection {
    Ingress,
    Egress,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RawLifecycleSignal {
    ActiveEstablished,
    Close,
    Reset,
    Ambiguous(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawSocketObservation {
    pub timestamp_ns: u64,
    pub socket_cookie: u64,
    pub first_seen_ns: u64,
    pub network_namespace: Option<u64>,
    pub cgroup_id: u64,
    pub pid: u32,
    pub tid: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawConnectObservation {
    pub socket: RawSocketObservation,
    pub family: RawNetworkFamily,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawLifecycleObservation {
    pub socket: RawSocketObservation,
    pub signal: RawLifecycleSignal,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawPayloadObservation {
    pub socket: RawSocketObservation,
    pub family: RawNetworkFamily,
    pub direction: RawDirection,
    pub tcp_sequence: u32,
    pub continuation_position: u32,
    pub observed_length: Option<u32>,
    pub truncated: bool,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RawLossCounters {
    pub timestamp_ns: u64,
    pub generation: Option<u64>,
    pub per_cpu: Vec<u64>,
}

/// Architecture-aware decoded kernel evidence. Not exported outside this adapter crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RawKernelObservation {
    Connect(RawConnectObservation),
    Lifecycle(RawLifecycleObservation),
    Payload(RawPayloadObservation),
    LossCounters(RawLossCounters),
}

fn decode_raw_kernel_observation(
    bytes: &[u8],
    architecture: KernelArchitecture,
) -> Result<RawKernelObservation, EbpfCaptureError> {
    if bytes.len() < ABI_HEADER_BYTES {
        return Err(decode_error("header", "item is shorter than ABI header"));
    }
    if bytes[..4] != ABI_MAGIC {
        return Err(decode_error("header", "invalid ABI magic"));
    }
    if read_u16(bytes, 4, "header")? != ABI_VERSION {
        return Err(decode_error("header", "unsupported ABI version"));
    }
    if bytes[7] != LITTLE_ENDIAN {
        return Err(decode_error("header", "unsupported ABI byte order"));
    }
    let declared_size = usize::try_from(read_u32(bytes, 8, "header")?)
        .map_err(|_| decode_error("header", "declared size does not fit usize"))?;
    if declared_size != bytes.len() {
        return Err(decode_error(
            "header",
            "declared size differs from item size",
        ));
    }

    let kind = bytes[6];
    let body = &bytes[ABI_HEADER_BYTES..];
    match kind {
        KIND_CONNECT4 | KIND_CONNECT6 => {
            if body.len() != 48 {
                return Err(decode_error("connect", "invalid connect item size"));
            }
            Ok(RawKernelObservation::Connect(RawConnectObservation {
                socket: decode_socket(body, "connect")?,
                family: if kind == KIND_CONNECT4 {
                    RawNetworkFamily::Ipv4
                } else {
                    RawNetworkFamily::Ipv6
                },
            }))
        }
        KIND_SOCKOPS => decode_lifecycle(body),
        KIND_INGRESS | KIND_EGRESS => decode_payload(body, kind),
        KIND_LOSS_COUNTERS => decode_loss_counters(body),
        _ => Err(decode_error("header", "unknown ABI discriminant")),
    }
    .map(|observation| match (architecture, observation) {
        // Parsing uses explicit little-endian reads, so both Gate A architectures share one ABI.
        (_, observation) => observation,
    })
}

fn decode_socket(
    bytes: &[u8],
    source: &'static str,
) -> Result<RawSocketObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    if bytes.len() < SOCKET_BYTES {
        return Err(decode_error(source, "item is shorter than socket evidence"));
    }
    let socket_cookie = read_u64(bytes, 8, source)?;
    let first_seen_ns = read_u64(bytes, 16, source)?;
    if socket_cookie == 0 {
        return Err(EbpfCaptureError::MissingIdentity("socket cookie"));
    }
    if first_seen_ns == 0 {
        return Err(EbpfCaptureError::MissingIdentity(
            "first-seen monotonic timestamp",
        ));
    }
    let network_namespace = read_u64(bytes, 24, source)?;
    Ok(RawSocketObservation {
        timestamp_ns: read_u64(bytes, 0, source)?,
        socket_cookie,
        first_seen_ns,
        network_namespace: (network_namespace != 0).then_some(network_namespace),
        cgroup_id: read_u64(bytes, 32, source)?,
        pid: read_u32(bytes, 40, source)?,
        tid: read_u32(bytes, 44, source)?,
    })
}

fn decode_lifecycle(bytes: &[u8]) -> Result<RawKernelObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    if bytes.len() != SOCKET_BYTES + 5 {
        return Err(decode_error("sockops", "invalid lifecycle item size"));
    }
    let signal = match bytes[SOCKET_BYTES] {
        SIGNAL_ACTIVE_ESTABLISHED => RawLifecycleSignal::ActiveEstablished,
        SIGNAL_CLOSE => RawLifecycleSignal::Close,
        SIGNAL_RESET => RawLifecycleSignal::Reset,
        SIGNAL_AMBIGUOUS => {
            RawLifecycleSignal::Ambiguous(read_u32(bytes, SOCKET_BYTES + 1, "sockops")?)
        }
        _ => return Err(decode_error("sockops", "unknown lifecycle signal")),
    };
    Ok(RawKernelObservation::Lifecycle(RawLifecycleObservation {
        socket: decode_socket(bytes, "sockops")?,
        signal,
    }))
}

fn decode_payload(bytes: &[u8], kind: u8) -> Result<RawKernelObservation, EbpfCaptureError> {
    const SOCKET_BYTES: usize = 48;
    const PAYLOAD_HEADER_BYTES: usize = 18;
    if bytes.len() < SOCKET_BYTES + PAYLOAD_HEADER_BYTES {
        return Err(decode_error(
            "payload",
            "item is shorter than payload header",
        ));
    }
    let family = match bytes[SOCKET_BYTES] {
        4 => RawNetworkFamily::Ipv4,
        6 => RawNetworkFamily::Ipv6,
        _ => return Err(decode_error("payload", "unknown network family")),
    };
    let captured_length = usize::try_from(read_u32(bytes, SOCKET_BYTES + 13, "payload")?)
        .map_err(|_| decode_error("payload", "captured length does not fit usize"))?;
    if bytes.len() != SOCKET_BYTES + PAYLOAD_HEADER_BYTES + captured_length {
        return Err(EbpfCaptureError::InvalidPayload {
            context: "payload",
            reason: "captured length exceeds ABI item bounds",
        });
    }
    let observed_length = read_u32(bytes, SOCKET_BYTES + 9, "payload")?;
    Ok(RawKernelObservation::Payload(RawPayloadObservation {
        socket: decode_socket(bytes, "payload")?,
        family,
        direction: if kind == KIND_INGRESS {
            RawDirection::Ingress
        } else {
            RawDirection::Egress
        },
        tcp_sequence: read_u32(bytes, SOCKET_BYTES + 1, "payload")?,
        continuation_position: read_u32(bytes, SOCKET_BYTES + 5, "payload")?,
        observed_length: (observed_length != 0).then_some(observed_length),
        truncated: bytes[SOCKET_BYTES + 17] != 0,
        payload: bytes[SOCKET_BYTES + PAYLOAD_HEADER_BYTES..].to_vec(),
    }))
}

fn decode_loss_counters(bytes: &[u8]) -> Result<RawKernelObservation, EbpfCaptureError> {
    if bytes.len() < 12 || !(bytes.len() - 12).is_multiple_of(8) {
        return Err(decode_error("loss_counters", "invalid counter item size"));
    }
    let timestamp_ns = read_u64(bytes, 0, "loss_counters")?;
    let generation = read_u32(bytes, 8, "loss_counters")?;
    let per_cpu = bytes[12..]
        .chunks_exact(8)
        .enumerate()
        .map(|(index, _)| read_u64(bytes, 12 + index * 8, "loss_counters"))
        .collect::<Result<Vec<_>, _>>()?;
    if per_cpu.is_empty() {
        return Err(EbpfCaptureError::RingLoss(
            "loss counters have no CPU values",
        ));
    }
    Ok(RawKernelObservation::LossCounters(RawLossCounters {
        timestamp_ns,
        generation: (generation != 0).then_some(u64::from(generation)),
        per_cpu,
    }))
}

fn read_u16(bytes: &[u8], offset: usize, source: &'static str) -> Result<u16, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u16::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u16"))?,
    ))
}

fn read_u32(bytes: &[u8], offset: usize, source: &'static str) -> Result<u32, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u32::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u32"))?,
    ))
}

fn read_u64(bytes: &[u8], offset: usize, source: &'static str) -> Result<u64, EbpfCaptureError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| decode_error(source, "integer extends beyond item bounds"))?;
    Ok(u64::from_le_bytes(
        value
            .try_into()
            .map_err(|_| decode_error(source, "invalid u64"))?,
    ))
}

const fn decode_error(source: &'static str, reason: &'static str) -> EbpfCaptureError {
    EbpfCaptureError::Decode {
        context: source,
        reason,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordingScopeConfig {
    pub identity: RecordingScopeIdentity,
    pub descendant_cgroup_ids: BTreeSet<u64>,
}

impl RecordingScopeConfig {
    pub fn contains(&self, cgroup_id: u64) -> bool {
        cgroup_id == self.identity.cgroup_id || self.descendant_cgroup_ids.contains(&cgroup_id)
    }
}

#[derive(Clone, Debug)]
struct LossSample {
    timestamp_ns: u64,
    total: u64,
    generation: Option<u64>,
}

/// Sole conversion path from private kernel evidence to public capture-domain evidence.
pub struct CaptureAdapter {
    clock: ClockIdentity,
    scope: RecordingScopeConfig,
    attached_at_ns: u64,
    sockets: BTreeMap<SocketIdentity, SocketEvidence>,
    previous_loss_sample: Option<LossSample>,
}

impl CaptureAdapter {
    pub fn new(
        clock: ClockIdentity,
        scope: RecordingScopeConfig,
        attached_at_ns: u64,
    ) -> Result<Self, EbpfCaptureError> {
        if clock.boot_id.is_empty() {
            return Err(EbpfCaptureError::MissingIdentity("boot/clock identity"));
        }
        if scope.identity.cgroup_id == 0 {
            return Err(EbpfCaptureError::MissingIdentity(
                "recording-scope cgroup ID",
            ));
        }
        if scope.identity.canonical_path.is_empty() {
            return Err(EbpfCaptureError::MissingIdentity(
                "recording-scope canonical path",
            ));
        }
        Ok(Self {
            clock,
            scope,
            attached_at_ns,
            sockets: BTreeMap::new(),
            previous_loss_sample: None,
        })
    }

    pub(crate) fn convert(
        &mut self,
        observation: RawKernelObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        match observation {
            RawKernelObservation::Connect(observation) => self.convert_connect(observation),
            RawKernelObservation::Lifecycle(observation) => self.convert_lifecycle(observation),
            RawKernelObservation::Payload(observation) => self.convert_payload(observation),
            RawKernelObservation::LossCounters(observation) => {
                self.convert_loss_counters(observation)
            }
        }
    }

    fn convert_connect(
        &mut self,
        observation: RawConnectObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let evidence = self.socket_evidence(&observation.socket, observation.family)?;
        self.sockets
            .insert(evidence.socket.clone(), evidence.clone());
        Ok(Some(event(CaptureEventKind::SocketConnectObserved(
            evidence,
        ))))
    }

    fn convert_lifecycle(
        &self,
        observation: RawLifecycleObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let evidence = self.correlated_evidence(&observation.socket)?;
        let kind = match observation.signal {
            RawLifecycleSignal::ActiveEstablished => CaptureEventKind::SocketConnected(evidence),
            RawLifecycleSignal::Close => CaptureEventKind::SocketClosedObserved(evidence),
            RawLifecycleSignal::Reset => CaptureEventKind::SocketResetObserved(evidence),
            RawLifecycleSignal::Ambiguous(raw_state) => {
                CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                    socket: evidence,
                    raw_state,
                })
            }
        };
        Ok(Some(event(kind)))
    }

    fn convert_payload(
        &self,
        observation: RawPayloadObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let evidence = self.correlated_evidence(&observation.socket)?;
        let captured_length = u32::try_from(observation.payload.len()).map_err(|_| {
            EbpfCaptureError::InvalidPayload {
                context: "payload",
                reason: "captured payload length exceeds u32",
            }
        })?;
        let truncation = if observation.truncated {
            TruncationMetadata {
                captured_length,
                observed_length: observation.observed_length,
                state: TruncationState::Truncated,
                reason: Some("kernel capture bound".into()),
            }
        } else {
            TruncationMetadata {
                captured_length,
                observed_length: observation.observed_length,
                state: if observation
                    .observed_length
                    .is_some_and(|length| length != captured_length)
                {
                    TruncationState::Unknown
                } else {
                    TruncationState::Complete
                },
                reason: None,
            }
        };
        Ok(Some(event(CaptureEventKind::PayloadFragment(
            PayloadFragment {
                timestamp: self.timestamp(observation.socket.timestamp_ns),
                socket: evidence.socket,
                recording_scope: evidence.recording_scope,
                network_family: map_family(observation.family),
                direction: match observation.direction {
                    RawDirection::Ingress => PayloadDirection::Ingress,
                    RawDirection::Egress => PayloadDirection::Egress,
                },
                sequence: FragmentSequenceEvidence {
                    tcp_sequence: observation.tcp_sequence,
                    continuation_position: observation.continuation_position,
                },
                payload: observation.payload,
                truncation,
            },
        ))))
    }

    fn convert_loss_counters(
        &mut self,
        observation: RawLossCounters,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let total = observation
            .per_cpu
            .iter()
            .try_fold(0_u64, |total, value| total.checked_add(*value))
            .ok_or(EbpfCaptureError::RingLoss("per-CPU counter total overflow"))?;
        let current = LossSample {
            timestamp_ns: observation.timestamp_ns,
            total,
            generation: observation.generation,
        };
        let previous = self.previous_loss_sample.replace(current.clone());
        let (start_ns, drop_delta, ambiguity) = match previous {
            None if total == 0 => return Ok(None),
            None => (
                self.attached_at_ns,
                Some(total),
                LossAmbiguity {
                    exact_drop_timing_unknown: true,
                    affected_sockets_unknown: true,
                    reason: Some("before first complete sample".into()),
                },
            ),
            Some(previous)
                if previous.generation != current.generation || current.total < previous.total =>
            {
                (
                    previous.timestamp_ns,
                    None,
                    LossAmbiguity {
                        exact_drop_timing_unknown: true,
                        affected_sockets_unknown: true,
                        reason: Some("counter generation changed or regressed".into()),
                    },
                )
            }
            Some(previous) => {
                let delta = current.total - previous.total;
                if delta == 0 {
                    return Ok(None);
                }
                (
                    previous.timestamp_ns,
                    Some(delta),
                    LossAmbiguity {
                        exact_drop_timing_unknown: false,
                        affected_sockets_unknown: true,
                        reason: None,
                    },
                )
            }
        };
        Ok(Some(event(CaptureEventKind::LossWindowObserved(
            LossWindowObserved {
                start: self.timestamp(start_ns),
                end: self.timestamp(current.timestamp_ns),
                drop_delta,
                loss_generation: current.generation,
                ambiguity,
            },
        ))))
    }

    fn socket_evidence(
        &self,
        socket: &RawSocketObservation,
        family: RawNetworkFamily,
    ) -> Result<SocketEvidence, EbpfCaptureError> {
        if !self.scope.contains(socket.cgroup_id) {
            return Err(EbpfCaptureError::MissingIdentity(
                "connect-observed recording-scope cgroup",
            ));
        }
        Ok(SocketEvidence {
            timestamp: self.timestamp(socket.timestamp_ns),
            socket: self.socket_identity(socket)?,
            recording_scope: self.scope.identity.clone(),
            network_family: map_family(family),
            process: Some(ProcessMetadata {
                pid: socket.pid,
                tid: (socket.tid != socket.pid).then_some(socket.tid),
                executable: None,
            }),
            observed_cgroup_id: socket.cgroup_id,
        })
    }

    fn correlated_evidence(
        &self,
        socket: &RawSocketObservation,
    ) -> Result<SocketEvidence, EbpfCaptureError> {
        let identity = self.socket_identity(socket)?;
        let mut evidence =
            self.sockets
                .get(&identity)
                .cloned()
                .ok_or(EbpfCaptureError::MissingIdentity(
                    "connect-derived socket attribution",
                ))?;
        evidence.timestamp = self.timestamp(socket.timestamp_ns);
        Ok(evidence)
    }

    fn socket_identity(
        &self,
        socket: &RawSocketObservation,
    ) -> Result<SocketIdentity, EbpfCaptureError> {
        if socket.socket_cookie == 0 {
            return Err(EbpfCaptureError::MissingIdentity("socket cookie"));
        }
        if socket.first_seen_ns == 0 {
            return Err(EbpfCaptureError::MissingIdentity(
                "first-seen monotonic timestamp",
            ));
        }
        Ok(SocketIdentity {
            socket_cookie: socket.socket_cookie,
            first_seen: self.timestamp(socket.first_seen_ns),
            network_namespace: socket.network_namespace,
        })
    }

    fn timestamp(&self, nanoseconds: u64) -> MonotonicTimestamp {
        MonotonicTimestamp {
            clock: self.clock.clone(),
            nanoseconds,
        }
    }
}

fn event(kind: CaptureEventKind) -> CaptureEvent {
    CaptureEvent::V2(CaptureEventV2 {
        schema_version: chronicle_capture::CAPTURE_EVENT_V2_SCHEMA_VERSION,
        kind,
    })
}

const fn map_family(family: RawNetworkFamily) -> NetworkFamily {
    match family {
        RawNetworkFamily::Ipv4 => NetworkFamily::Ipv4,
        RawNetworkFamily::Ipv6 => NetworkFamily::Ipv6,
    }
}

#[cfg(target_os = "linux")]
pub struct EbpfCaptureSource {
    // Dropping this object detaches producer links while independently owned maps stay drainable.
    ebpf: Option<aya::Ebpf>,
    ring: Option<aya::maps::RingBuf<aya::maps::MapData>>,
    counters: Option<aya::maps::PerCpuArray<aya::maps::MapData, u64>>,
    architecture: KernelArchitecture,
    adapter: CaptureAdapter,
    counter_generation: u64,
    state: CaptureSourceState,
    drain_complete: bool,
    final_loss: Option<CaptureEvent>,
    summary: CaptureSourceSummary,
}

#[cfg(not(target_os = "linux"))]
pub struct EbpfCaptureSource;

#[cfg(target_os = "linux")]
impl EbpfCaptureSource {
    pub fn load(
        object: &[u8],
        cgroup: &std::fs::File,
        mut adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        let mut ebpf = aya::Ebpf::load(object).map_err(|_| EbpfCaptureError::Verifier {
            program: "object",
            reason: "load failed",
        })?;
        let architecture = match std::env::consts::ARCH {
            "aarch64" => KernelArchitecture::Aarch64,
            "x86_64" => KernelArchitecture::X86_64,
            _ => return Err(EbpfCaptureError::UnsupportedCapability("host architecture")),
        };
        if let Err(error) = attach_all(&mut ebpf, cgroup) {
            drop(ebpf);
            return Err(error);
        }
        adapter.attached_at_ns = monotonic_nanoseconds()?;
        let ring = aya::maps::RingBuf::try_from(ebpf.take_map("EVENTS").ok_or(
            EbpfCaptureError::Attach {
                hook: "EVENTS",
                reason: "map missing",
            },
        )?)
        .map_err(|_| EbpfCaptureError::Attach {
            hook: "EVENTS",
            reason: "ring buffer unavailable",
        })?;
        let counters = aya::maps::PerCpuArray::try_from(
            ebpf.take_map("COUNTERS")
                .ok_or(EbpfCaptureError::RingLoss("counter map missing"))?,
        )
        .map_err(|_| EbpfCaptureError::RingLoss("counter map unavailable"))?;
        Ok(Self {
            ebpf: Some(ebpf),
            ring: Some(ring),
            counters: Some(counters),
            architecture,
            adapter,
            counter_generation: 0,
            state: CaptureSourceState::Running,
            drain_complete: false,
            final_loss: None,
            summary: CaptureSourceSummary::default(),
        })
    }
}

#[cfg(not(target_os = "linux"))]
impl EbpfCaptureSource {
    pub fn load(
        _object: &[u8],
        _cgroup: &std::fs::File,
        _adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        Err(EbpfCaptureError::UnsupportedPlatform)
    }
}

#[cfg(target_os = "linux")]
fn attach_all(ebpf: &mut aya::Ebpf, cgroup: &std::fs::File) -> Result<(), EbpfCaptureError> {
    use aya::programs::{
        CgroupAttachMode, CgroupSkb, CgroupSkbAttachType, CgroupSockAddr, SockOps,
    };

    for name in ["connect4", "connect6"] {
        let program: &mut CgroupSockAddr = ebpf
            .program_mut(name)
            .ok_or(EbpfCaptureError::Attach {
                hook: name,
                reason: "program missing",
            })?
            .try_into()
            .map_err(|_| EbpfCaptureError::Verifier {
                program: name,
                reason: "program type mismatch",
            })?;
        program.load().map_err(|_| EbpfCaptureError::Verifier {
            program: name,
            reason: "load rejected",
        })?;
        program
            .attach(cgroup, CgroupAttachMode::Single)
            .map_err(|_| EbpfCaptureError::Attach {
                hook: name,
                reason: "attach failed",
            })?;
    }
    let lifecycle: &mut SockOps = ebpf
        .program_mut("socket_lifecycle")
        .ok_or(EbpfCaptureError::Attach {
            hook: "socket_lifecycle",
            reason: "program missing",
        })?
        .try_into()
        .map_err(|_| EbpfCaptureError::Verifier {
            program: "socket_lifecycle",
            reason: "program type mismatch",
        })?;
    lifecycle.load().map_err(|_| EbpfCaptureError::Verifier {
        program: "socket_lifecycle",
        reason: "load rejected",
    })?;
    lifecycle
        .attach(cgroup, CgroupAttachMode::Single)
        .map_err(|_| EbpfCaptureError::Attach {
            hook: "socket_lifecycle",
            reason: "attach failed",
        })?;
    for (name, attach_type) in [
        ("ingress", CgroupSkbAttachType::Ingress),
        ("egress", CgroupSkbAttachType::Egress),
    ] {
        let program: &mut CgroupSkb = ebpf
            .program_mut(name)
            .ok_or(EbpfCaptureError::Attach {
                hook: name,
                reason: "program missing",
            })?
            .try_into()
            .map_err(|_| EbpfCaptureError::Verifier {
                program: name,
                reason: "program type mismatch",
            })?;
        program.load().map_err(|_| EbpfCaptureError::Verifier {
            program: name,
            reason: "load rejected",
        })?;
        program
            .attach(cgroup, attach_type, CgroupAttachMode::Single)
            .map_err(|_| EbpfCaptureError::Attach {
                hook: name,
                reason: "attach failed",
            })?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
impl EbpfCaptureSource {
    fn source_error(error: EbpfCaptureError) -> CaptureError {
        CaptureError::Source(error.to_string())
    }

    fn sample_loss(&mut self) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let values = self
            .counters
            .as_mut()
            .ok_or(EbpfCaptureError::RingLoss("counter map released"))?
            .get(&1, 0)
            .map_err(|_| EbpfCaptureError::RingLoss("incomplete per-CPU counter read"))?;
        let timestamp_ns = monotonic_nanoseconds()?;
        self.adapter
            .convert(RawKernelObservation::LossCounters(RawLossCounters {
                timestamp_ns,
                per_cpu: values.iter().copied().collect(),
                generation: Some(self.counter_generation),
            }))
    }

    fn ring_event(&mut self) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let Some(item) = self
            .ring
            .as_mut()
            .ok_or(EbpfCaptureError::RingLoss("ring buffer released"))?
            .next()
        else {
            return Ok(None);
        };
        let raw = decode_raw_kernel_observation(item.as_ref(), self.architecture)?;
        self.adapter.convert(raw)
    }

    fn emitted(&mut self, event: Option<CaptureEvent>) -> Option<CaptureEvent> {
        if event.is_some() {
            self.summary.emitted_events += 1;
        }
        event
    }
}

#[cfg(target_os = "linux")]
impl CaptureSource for EbpfCaptureSource {
    fn start(&mut self) -> Result<(), CaptureError> {
        match self.state {
            CaptureSourceState::Running => Ok(()),
            state => Err(CaptureError::Lifecycle {
                action: "start",
                state,
            }),
        }
    }

    fn poll(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        if self.state != CaptureSourceState::Running {
            return Err(CaptureError::Lifecycle {
                action: "poll",
                state: self.state,
            });
        }
        let event = match self.ring_event().map_err(Self::source_error)? {
            Some(event) => Some(event),
            None => self.sample_loss().map_err(Self::source_error)?,
        };
        Ok(self.emitted(event))
    }

    fn request_shutdown(&mut self) -> Result<(), CaptureError> {
        if matches!(
            self.state,
            CaptureSourceState::ShutdownRequested | CaptureSourceState::Draining
        ) {
            return Ok(());
        }
        if self.state != CaptureSourceState::Running {
            return Err(CaptureError::Lifecycle {
                action: "request_shutdown",
                state: self.state,
            });
        }

        // Drop links first: retained map handles keep accepted ring items and counters readable.
        drop(self.ebpf.take());
        self.state.request_shutdown()?;
        self.final_loss = match self.sample_loss() {
            Ok(event) => event,
            Err(error) => {
                self.summary.final_loss_sample_failed = true;
                return Err(CaptureError::FinalLossSample(error.to_string()));
            }
        };
        Ok(())
    }

    fn drain(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.state.begin_drain()?;

        let ring_event = match self.ring_event() {
            Ok(event) => event,
            Err(error) => {
                self.summary.drain_failed = true;
                return Err(CaptureError::Drain(error.to_string()));
            }
        };
        if let Some(event) = ring_event {
            return Ok(self.emitted(Some(event)));
        }
        if let Some(event) = self.final_loss.take() {
            self.summary.final_loss_events += 1;
            return Ok(self.emitted(Some(event)));
        }
        self.drain_complete = true;
        Ok(None)
    }

    fn finalize(&mut self) -> Result<CaptureSourceSummary, CaptureError> {
        if self.state != CaptureSourceState::Draining
            || !(self.drain_complete || self.summary.drain_failed)
        {
            return Err(CaptureError::Lifecycle {
                action: "finalize",
                state: self.state,
            });
        }
        drop(self.ring.take());
        drop(self.counters.take());
        drop(self.ebpf.take());
        self.state.finalize(true)?;
        if self.summary.drain_failed {
            return Err(CaptureError::Drain("previous drain failure".into()));
        }
        if self.summary.final_loss_sample_failed {
            return Err(CaptureError::FinalLossSample(
                "previous final loss sample failure".into(),
            ));
        }
        Ok(self.summary)
    }

    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.poll()
    }
}

#[cfg(target_os = "linux")]
#[allow(unsafe_code)] // libc exposes Linux CLOCK_MONOTONIC only as an unsafe FFI call.
fn monotonic_nanoseconds() -> Result<u64, EbpfCaptureError> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut value) } != 0 {
        return Err(EbpfCaptureError::RingLoss("monotonic clock unavailable"));
    }
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|seconds| seconds.checked_add(u64::try_from(value.tv_nsec).ok()?))
        .ok_or(EbpfCaptureError::RingLoss("monotonic clock out of range"))
}

#[cfg(not(target_os = "linux"))]
impl CaptureSource for EbpfCaptureSource {
    fn next_event(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        Err(CaptureError::Source(
            EbpfCaptureError::UnsupportedPlatform.to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn socket_body() -> Vec<u8> {
        let mut body = vec![0; 48];
        body[0..8].copy_from_slice(&10_u64.to_le_bytes());
        body[8..16].copy_from_slice(&20_u64.to_le_bytes());
        body[16..24].copy_from_slice(&5_u64.to_le_bytes());
        body[32..40].copy_from_slice(&30_u64.to_le_bytes());
        body[40..44].copy_from_slice(&40_u32.to_le_bytes());
        body[44..48].copy_from_slice(&41_u32.to_le_bytes());
        body
    }

    fn abi_item(kind: u8, body: Vec<u8>) -> Vec<u8> {
        let mut item = Vec::with_capacity(ABI_HEADER_BYTES + body.len());
        item.extend_from_slice(&ABI_MAGIC);
        item.extend_from_slice(&ABI_VERSION.to_le_bytes());
        item.push(kind);
        item.push(LITTLE_ENDIAN);
        item.extend_from_slice(
            &u32::try_from(ABI_HEADER_BYTES + body.len())
                .unwrap()
                .to_le_bytes(),
        );
        item.extend(body);
        item
    }

    fn adapter() -> CaptureAdapter {
        CaptureAdapter::new(
            ClockIdentity {
                boot_id: "boot-a".into(),
            },
            RecordingScopeConfig {
                identity: RecordingScopeIdentity {
                    cgroup_id: 30,
                    canonical_path: "/scope".into(),
                    namespace: Some(9),
                },
                descendant_cgroup_ids: BTreeSet::from([31]),
            },
            1,
        )
        .unwrap()
    }

    fn raw_socket(timestamp_ns: u64) -> RawSocketObservation {
        raw_socket_with_identity(timestamp_ns, 20, 5, Some(9), 31, 40)
    }

    fn raw_socket_with_identity(
        timestamp_ns: u64,
        socket_cookie: u64,
        first_seen_ns: u64,
        network_namespace: Option<u64>,
        cgroup_id: u64,
        pid: u32,
    ) -> RawSocketObservation {
        RawSocketObservation {
            timestamp_ns,
            socket_cookie,
            first_seen_ns,
            network_namespace,
            cgroup_id,
            pid,
            tid: pid + 1,
        }
    }

    #[test]
    fn capture_adapter_preserves_observation_boundaries() {
        let mut adapter = adapter();
        let connect = adapter
            .convert(RawKernelObservation::Connect(RawConnectObservation {
                socket: raw_socket(10),
                family: RawNetworkFamily::Ipv6,
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            connect,
            CaptureEvent::V2(CaptureEventV2 {
                kind: CaptureEventKind::SocketConnectObserved(SocketEvidence {
                    socket: SocketIdentity {
                        socket_cookie: 20,
                        first_seen: MonotonicTimestamp { nanoseconds: 5, .. },
                        ..
                    },
                    recording_scope: RecordingScopeIdentity { cgroup_id: 30, .. },
                    network_family: NetworkFamily::Ipv6,
                    observed_cgroup_id: 31,
                    ..
                }),
                ..
            })
        ));

        let lifecycle = adapter
            .convert(RawKernelObservation::Lifecycle(RawLifecycleObservation {
                socket: raw_socket(11),
                signal: RawLifecycleSignal::Ambiguous(99),
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            lifecycle,
            CaptureEvent::V2(CaptureEventV2 {
                kind: CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                    raw_state: 99,
                    ..
                }),
                ..
            })
        ));

        let fragment = adapter
            .convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: raw_socket(12),
                family: RawNetworkFamily::Ipv6,
                direction: RawDirection::Egress,
                tcp_sequence: 100,
                continuation_position: 2,
                observed_length: Some(8),
                truncated: true,
                payload: b"bytes".to_vec(),
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            fragment,
            CaptureEvent::V2(CaptureEventV2 {
                kind: CaptureEventKind::PayloadFragment(PayloadFragment {
                    socket: SocketIdentity {
                        socket_cookie: 20,
                        ..
                    },
                    recording_scope: RecordingScopeIdentity { cgroup_id: 30, .. },
                    sequence: FragmentSequenceEvidence {
                        tcp_sequence: 100,
                        continuation_position: 2,
                    },
                    truncation: TruncationMetadata {
                        state: TruncationState::Truncated,
                        ..
                    },
                    ..
                }),
                ..
            })
        ));

        let loss = adapter
            .convert(RawKernelObservation::LossCounters(RawLossCounters {
                timestamp_ns: 20,
                generation: Some(1),
                per_cpu: vec![2, 3],
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            loss,
            CaptureEvent::V2(CaptureEventV2 {
                kind: CaptureEventKind::LossWindowObserved(LossWindowObserved {
                    drop_delta: Some(5),
                    ambiguity: LossAmbiguity {
                        exact_drop_timing_unknown: true,
                        affected_sockets_unknown: true,
                        ..
                    },
                    ..
                }),
                ..
            })
        ));
    }

    fn loss_observation(timestamp_ns: u64, total: u64) -> RawKernelObservation {
        loss_observation_with_generation(timestamp_ns, total, Some(1))
    }

    fn loss_observation_with_generation(
        timestamp_ns: u64,
        total: u64,
        generation: Option<u64>,
    ) -> RawKernelObservation {
        RawKernelObservation::LossCounters(RawLossCounters {
            timestamp_ns,
            generation,
            per_cpu: vec![total],
        })
    }

    fn loss_window(event: CaptureEvent) -> LossWindowObserved {
        let CaptureEvent::V2(CaptureEventV2 {
            kind: CaptureEventKind::LossWindowObserved(window),
            ..
        }) = event
        else {
            panic!("expected loss window");
        };
        window
    }

    #[test]
    fn socket_cookie_reuse_never_merges_pid_or_namespace_reuse() {
        let mut adapter = adapter();
        let first = raw_socket_with_identity(10, 20, 5, Some(9), 31, 40);
        let reused = raw_socket_with_identity(11, 20, 6, None, 31, 40);
        for socket in [first.clone(), reused.clone()] {
            adapter
                .convert(RawKernelObservation::Connect(RawConnectObservation {
                    socket,
                    family: RawNetworkFamily::Ipv4,
                }))
                .unwrap();
        }

        let event = adapter
            .convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: reused.clone(),
                family: RawNetworkFamily::Ipv4,
                direction: RawDirection::Egress,
                tcp_sequence: 1,
                continuation_position: 0,
                observed_length: Some(1),
                truncated: false,
                payload: vec![1],
            }))
            .unwrap()
            .unwrap();
        let CaptureEvent::V2(CaptureEventV2 {
            kind: CaptureEventKind::PayloadFragment(fragment),
            ..
        }) = event
        else {
            panic!("expected payload fragment");
        };
        assert_eq!(fragment.socket.first_seen.nanoseconds, 6);
        assert_eq!(fragment.socket.network_namespace, None);
        assert_eq!(fragment.recording_scope.cgroup_id, 30);

        let orphan = raw_socket_with_identity(12, 20, 7, None, 31, 40);
        assert!(matches!(
            adapter.convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: orphan,
                family: RawNetworkFamily::Ipv4,
                direction: RawDirection::Egress,
                tcp_sequence: 2,
                continuation_position: 0,
                observed_length: Some(1),
                truncated: false,
                payload: vec![2],
            })),
            Err(EbpfCaptureError::MissingIdentity(
                "connect-derived socket attribution"
            ))
        ));
    }

    #[test]
    fn loss_windows_use_actual_sample_times_and_advance_zero_delta_boundary() {
        let mut adapter = adapter();
        assert!(adapter.convert(loss_observation(10, 0)).unwrap().is_none());

        let positive = loss_window(adapter.convert(loss_observation(50, 3)).unwrap().unwrap());
        assert_eq!(positive.start.nanoseconds, 10);
        assert_eq!(positive.end.nanoseconds, 50);
        assert_eq!(positive.drop_delta, Some(3));

        assert!(adapter.convert(loss_observation(60, 3)).unwrap().is_none());
        let after_zero = loss_window(adapter.convert(loss_observation(80, 4)).unwrap().unwrap());
        assert_eq!(after_zero.start.nanoseconds, 60);
        assert_eq!(after_zero.end.nanoseconds, 80);
        assert_eq!(after_zero.drop_delta, Some(1));
    }

    #[test]
    fn first_and_regressed_loss_samples_remain_uncertain() {
        let mut adapter = adapter();
        let first = loss_window(adapter.convert(loss_observation(40, 2)).unwrap().unwrap());
        assert_eq!(first.start.nanoseconds, 1);
        assert_eq!(first.end.nanoseconds, 40);
        assert_eq!(first.drop_delta, Some(2));
        assert!(first.ambiguity.exact_drop_timing_unknown);

        let regressed = loss_window(adapter.convert(loss_observation(60, 1)).unwrap().unwrap());
        assert_eq!(regressed.start.nanoseconds, 40);
        assert_eq!(regressed.end.nanoseconds, 60);
        assert_eq!(regressed.drop_delta, None);
        assert!(regressed.ambiguity.exact_drop_timing_unknown);
    }

    #[test]
    fn loss_generation_change_remains_unattributed() {
        let mut adapter = adapter();
        adapter.convert(loss_observation(10, 5)).unwrap();
        let replaced = loss_window(
            adapter
                .convert(loss_observation_with_generation(30, 1, Some(2)))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(replaced.start.nanoseconds, 10);
        assert_eq!(replaced.end.nanoseconds, 30);
        assert_eq!(replaced.drop_delta, None);
        assert!(replaced.ambiguity.exact_drop_timing_unknown);
        assert!(replaced.ambiguity.affected_sockets_unknown);
    }

    #[test]
    fn valid_connect_abi_becomes_private_raw_evidence() {
        let observation = decode_raw_kernel_observation(
            &abi_item(KIND_CONNECT4, socket_body()),
            KernelArchitecture::Aarch64,
        )
        .unwrap();
        assert!(matches!(
            observation,
            RawKernelObservation::Connect(RawConnectObservation {
                family: RawNetworkFamily::Ipv4,
                socket: RawSocketObservation {
                    socket_cookie: 20,
                    first_seen_ns: 5,
                    ..
                },
            })
        ));
    }

    #[test]
    fn abi_magic_discriminant_and_connect_size_are_rejected() {
        let item = abi_item(KIND_CONNECT4, socket_body());
        for (offset, replacement, expected) in [
            (0, b'X', "invalid ABI magic"),
            (6, 99, "unknown ABI discriminant"),
        ] {
            let mut invalid = item.clone();
            invalid[offset] = replacement;
            let error =
                decode_raw_kernel_observation(&invalid, KernelArchitecture::Aarch64).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }

        for invalid_body in [
            {
                let mut body = socket_body();
                body.pop();
                body
            },
            {
                let mut body = socket_body();
                body.push(0);
                body
            },
        ] {
            let error = decode_raw_kernel_observation(
                &abi_item(KIND_CONNECT6, invalid_body),
                KernelArchitecture::Aarch64,
            )
            .unwrap_err();
            assert!(error.to_string().contains("invalid connect item size"));
        }
    }

    #[test]
    fn malformed_payload_metadata_is_not_silently_dropped() {
        let mut body = socket_body();
        body.push(4);
        body.extend_from_slice(&100_u32.to_le_bytes());
        body.extend_from_slice(&2_u32.to_le_bytes());
        body.extend_from_slice(&8_u32.to_le_bytes());
        body.extend_from_slice(&8_u32.to_le_bytes());
        body.push(0);
        let error = decode_raw_kernel_observation(
            &abi_item(KIND_EGRESS, body),
            KernelArchitecture::Aarch64,
        )
        .unwrap_err();
        assert!(matches!(
            error,
            EbpfCaptureError::InvalidPayload {
                context: "payload",
                ..
            }
        ));
    }

    #[test]
    fn abi_version_size_and_byte_order_are_rejected() {
        let item = abi_item(KIND_CONNECT6, socket_body());
        for (offset, replacement, expected) in [
            (4, 2, "unsupported ABI version"),
            (7, 2, "unsupported ABI byte order"),
            (8, 1, "declared size differs from item size"),
        ] {
            let mut invalid = item.clone();
            invalid[offset] = replacement;
            let error =
                decode_raw_kernel_observation(&invalid, KernelArchitecture::X86_64).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
    }
}
