use crate::{
    abi::{
        RawConnectObservation, RawDirection, RawEndpoint, RawEstablishedSocketEvidence,
        RawKernelObservation, RawLifecycleObservation, RawLifecycleSignal, RawLossCounters,
        RawNetworkFamily, RawPayloadObservation, RawSocketObservation, RawSocketRole,
    },
    error::EbpfCaptureError,
};
use chronicle_capture::{
    CaptureEvent, CaptureEventKind, CaptureFlags, ClockIdentity, FragmentSequenceEvidence,
    LossCounterSample, LossWindowSampler, MonotonicTimestamp, NetworkFamily, PayloadDirection,
    PayloadFragment, ProcessMetadata, RecordingScopeIdentity, SocketConnectIntent, SocketEvidence,
    SocketIdentity, SocketRole, SocketStateChangeObserved, TruncationMetadata, TruncationState,
};
use chronicle_common::Endpoint;
use std::{
    collections::{BTreeMap, BTreeSet},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
};

const PAYLOAD_LENGTH_MISMATCH_REASON: &str =
    "observed and captured payload lengths differ without kernel truncation flag";

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

/// Sole conversion path from private kernel evidence to public capture-domain evidence.
pub struct CaptureAdapter {
    clock: ClockIdentity,
    scope: RecordingScopeConfig,
    loss_sampler: LossWindowSampler,
    pending_connects: BTreeMap<SocketIdentity, SocketConnectIntent>,
    sockets: BTreeMap<SocketIdentity, SocketEvidence>,
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
            loss_sampler: LossWindowSampler::new(MonotonicTimestamp {
                clock: clock.clone(),
                nanoseconds: attached_at_ns,
            }),
            clock,
            scope,
            pending_connects: BTreeMap::new(),
            sockets: BTreeMap::new(),
        })
    }

    #[cfg(target_os = "linux")]
    pub(crate) fn reset_attachment_time(&mut self, attached_at_ns: u64) {
        self.loss_sampler.reset_attachment(MonotonicTimestamp {
            clock: self.clock.clone(),
            nanoseconds: attached_at_ns,
        });
    }

    pub(crate) fn incomplete_loss_sample(
        &self,
        timestamp_ns: u64,
        reason: &'static str,
    ) -> CaptureEvent {
        event(CaptureEventKind::LossWindowObserved(
            self.loss_sampler
                .incomplete_sample(self.timestamp(timestamp_ns), reason),
        ))
    }

    pub(crate) fn convert(
        &mut self,
        observation: RawKernelObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        match observation {
            RawKernelObservation::Connect(observation) => self.convert_connect(&observation),
            RawKernelObservation::Lifecycle(observation) => self.convert_lifecycle(&observation),
            RawKernelObservation::Payload(observation) => self.convert_payload(observation),
            RawKernelObservation::LossCounters(observation) => {
                self.convert_loss_counters(&observation)
            }
        }
    }

    fn convert_connect(
        &mut self,
        observation: &RawConnectObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let observed_cgroup_id = self.require_scope(
            &observation.socket,
            "connect-observed recording-scope cgroup",
            false,
        )?;
        let intent = SocketConnectIntent {
            timestamp: self.timestamp(observation.socket.timestamp_ns),
            socket: self.socket_identity(&observation.socket)?,
            recording_scope: self.scope.identity.clone(),
            network_family: map_family(observation.family),
            remote_endpoint: map_endpoint(observation.family, &observation.remote_endpoint),
            role: SocketRole::Active,
            process: process_metadata(&observation.socket),
            observed_cgroup_id,
        };
        intent
            .validate()
            .map_err(|error| EbpfCaptureError::InvalidSocketEvidence(error.to_string()))?;
        if let Some(existing) = self.pending_connects.get(&intent.socket)
            && (existing.network_family != intent.network_family
                || existing.remote_endpoint != intent.remote_endpoint)
        {
            return Err(EbpfCaptureError::ConflictingSocketEvidence);
        }
        self.pending_connects
            .insert(intent.socket.clone(), intent.clone());
        Ok(Some(event(CaptureEventKind::SocketConnectObserved(intent))))
    }

    fn convert_lifecycle(
        &mut self,
        observation: &RawLifecycleObservation,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let kind =
            match observation.signal {
                RawLifecycleSignal::ActiveEstablished | RawLifecycleSignal::PassiveEstablished => {
                    let established = observation.established.as_ref().ok_or(
                        EbpfCaptureError::MissingIdentity("established endpoint evidence"),
                    )?;
                    let evidence = self.established_evidence(&observation.socket, established)?;
                    if let Some(existing) = self.sockets.get(&evidence.socket)
                        && (existing.network_family != evidence.network_family
                            || existing.local_endpoint != evidence.local_endpoint
                            || existing.remote_endpoint != evidence.remote_endpoint
                            || existing.role != evidence.role)
                    {
                        return Err(EbpfCaptureError::ConflictingSocketEvidence);
                    }
                    self.pending_connects.remove(&evidence.socket);
                    self.sockets
                        .insert(evidence.socket.clone(), evidence.clone());
                    CaptureEventKind::SocketConnected(evidence)
                }
                RawLifecycleSignal::Close => CaptureEventKind::SocketClosedObserved(
                    self.correlated_evidence(&observation.socket)?,
                ),
                RawLifecycleSignal::Reset => CaptureEventKind::SocketResetObserved(
                    self.correlated_evidence(&observation.socket)?,
                ),
                RawLifecycleSignal::Ambiguous(raw_state) => {
                    CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                        socket: self.correlated_evidence(&observation.socket)?,
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
        } else if observation
            .observed_length
            .is_some_and(|length| length != captured_length)
        {
            TruncationMetadata {
                captured_length,
                observed_length: observation.observed_length,
                state: TruncationState::Unknown,
                reason: Some(PAYLOAD_LENGTH_MISMATCH_REASON.into()),
            }
        } else {
            TruncationMetadata {
                captured_length,
                observed_length: observation.observed_length,
                state: TruncationState::Complete,
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
                flags: CaptureFlags::default(),
            },
        ))))
    }

    fn convert_loss_counters(
        &mut self,
        observation: &RawLossCounters,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let total = observation
            .per_cpu
            .iter()
            .try_fold(0_u64, |total, value| total.checked_add(*value))
            .ok_or(EbpfCaptureError::RingLoss("per-CPU counter total overflow"))?;
        Ok(self
            .loss_sampler
            .observe(LossCounterSample {
                timestamp: self.timestamp(observation.timestamp_ns),
                total,
                generation: observation.generation,
            })
            .map(|window| event(CaptureEventKind::LossWindowObserved(window))))
    }

    fn established_evidence(
        &self,
        socket: &RawSocketObservation,
        established: &RawEstablishedSocketEvidence,
    ) -> Result<SocketEvidence, EbpfCaptureError> {
        let attachment_scope_id =
            self.require_scope(socket, "established recording-scope cgroup", true)?;
        let identity = self.socket_identity(socket)?;
        let pending = self.pending_connects.get(&identity);
        let observed_cgroup_id =
            pending.map_or(attachment_scope_id, |intent| intent.observed_cgroup_id);
        let evidence = SocketEvidence {
            timestamp: self.timestamp(socket.timestamp_ns),
            socket: identity,
            recording_scope: self.scope.identity.clone(),
            network_family: map_family(established.family),
            local_endpoint: map_endpoint(established.family, &established.local_endpoint),
            remote_endpoint: map_endpoint(established.family, &established.remote_endpoint),
            role: map_role(established.role),
            process: pending
                .and_then(|intent| intent.process.clone())
                .or_else(|| process_metadata(socket)),
            observed_cgroup_id,
        };
        if let Some(intent) = pending
            && (evidence.role != SocketRole::Active
                || evidence.network_family != intent.network_family
                || evidence.remote_endpoint != intent.remote_endpoint)
        {
            return Err(EbpfCaptureError::ConflictingSocketEvidence);
        }
        evidence
            .validate()
            .map_err(|error| EbpfCaptureError::InvalidSocketEvidence(error.to_string()))?;
        Ok(evidence)
    }

    fn require_scope(
        &self,
        socket: &RawSocketObservation,
        label: &'static str,
        allow_attachment_scope: bool,
    ) -> Result<u64, EbpfCaptureError> {
        if socket.cgroup_id == 0 && allow_attachment_scope {
            // SockOps may run in peer context; attachment proves root scope.
            Ok(self.scope.identity.cgroup_id)
        } else if self.scope.contains(socket.cgroup_id) {
            Ok(socket.cgroup_id)
        } else {
            Err(EbpfCaptureError::MissingIdentity(label))
        }
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
    CaptureEvent {
        schema_version: chronicle_capture::CAPTURE_EVENT_SCHEMA_VERSION,
        kind,
    }
}

const fn map_family(family: RawNetworkFamily) -> NetworkFamily {
    match family {
        RawNetworkFamily::Ipv4 => NetworkFamily::Ipv4,
        RawNetworkFamily::Ipv6 => NetworkFamily::Ipv6,
    }
}

const fn map_role(role: RawSocketRole) -> SocketRole {
    match role {
        RawSocketRole::Active => SocketRole::Active,
        RawSocketRole::Passive => SocketRole::Passive,
    }
}

fn map_endpoint(family: RawNetworkFamily, endpoint: &RawEndpoint) -> Endpoint {
    let address = match family {
        RawNetworkFamily::Ipv4 => IpAddr::V4(Ipv4Addr::new(
            endpoint.address[0],
            endpoint.address[1],
            endpoint.address[2],
            endpoint.address[3],
        )),
        RawNetworkFamily::Ipv6 => IpAddr::V6(Ipv6Addr::from(endpoint.address)),
    };
    Endpoint::new(address.to_string(), endpoint.port)
}

fn process_metadata(socket: &RawSocketObservation) -> Option<ProcessMetadata> {
    (socket.pid != 0).then(|| ProcessMetadata {
        pid: socket.pid,
        tid: (socket.tid != 0 && socket.tid != socket.pid).then_some(socket.tid),
        executable: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::abi::{
        RawConnectObservation, RawEndpoint, RawEstablishedSocketEvidence, RawKernelObservation,
        RawLifecycleObservation, RawLifecycleSignal, RawLossCounters, RawPayloadObservation,
        RawSocketObservation, RawSocketRole,
    };

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

    fn raw_endpoint(family: RawNetworkFamily, local: bool) -> RawEndpoint {
        let mut address = [0; 16];
        match family {
            RawNetworkFamily::Ipv4 => address[..4].copy_from_slice(if local {
                &[192, 0, 2, 10]
            } else {
                &[192, 0, 2, 20]
            }),
            RawNetworkFamily::Ipv6 => {
                address = if local { [1; 16] } else { [2; 16] };
            }
        }
        RawEndpoint {
            address,
            port: if local { 41_000 } else { 8_080 },
        }
    }

    fn establish(
        adapter: &mut CaptureAdapter,
        socket: RawSocketObservation,
        family: RawNetworkFamily,
        role: RawSocketRole,
    ) -> CaptureEvent {
        adapter
            .convert(RawKernelObservation::Lifecycle(RawLifecycleObservation {
                socket,
                signal: match role {
                    RawSocketRole::Active => RawLifecycleSignal::ActiveEstablished,
                    RawSocketRole::Passive => RawLifecycleSignal::PassiveEstablished,
                },
                established: Some(RawEstablishedSocketEvidence {
                    family,
                    local_endpoint: raw_endpoint(family, true),
                    remote_endpoint: raw_endpoint(family, false),
                    role,
                }),
            }))
            .unwrap()
            .unwrap()
    }

    fn connect(adapter: &mut CaptureAdapter) {
        adapter
            .convert(RawKernelObservation::Connect(RawConnectObservation {
                socket: raw_socket(10),
                family: RawNetworkFamily::Ipv6,
                remote_endpoint: raw_endpoint(RawNetworkFamily::Ipv6, false),
            }))
            .unwrap();
        establish(
            adapter,
            raw_socket(11),
            RawNetworkFamily::Ipv6,
            RawSocketRole::Active,
        );
    }

    fn payload_fragment(
        adapter: &mut CaptureAdapter,
        observed_length: Option<u32>,
        truncated: bool,
        payload: &[u8],
    ) -> PayloadFragment {
        connect(adapter);
        let event = adapter
            .convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: raw_socket(12),
                family: RawNetworkFamily::Ipv6,
                direction: RawDirection::Egress,
                tcp_sequence: 100,
                continuation_position: 2,
                observed_length,
                truncated,
                payload: payload.to_vec(),
            }))
            .unwrap()
            .unwrap();
        let CaptureEvent {
            kind: CaptureEventKind::PayloadFragment(fragment),
            ..
        } = event
        else {
            panic!("expected payload fragment");
        };
        fragment
    }

    fn loss_observation(timestamp_ns: u64, per_cpu: Vec<u64>) -> RawKernelObservation {
        loss_observation_with_generation(timestamp_ns, per_cpu, Some(1))
    }

    fn loss_observation_with_generation(
        timestamp_ns: u64,
        per_cpu: Vec<u64>,
        generation: Option<u64>,
    ) -> RawKernelObservation {
        RawKernelObservation::LossCounters(RawLossCounters {
            timestamp_ns,
            generation,
            per_cpu,
        })
    }

    fn loss_window(event: CaptureEvent) -> chronicle_capture::LossWindowObserved {
        let CaptureEvent {
            kind: CaptureEventKind::LossWindowObserved(window),
            ..
        } = event
        else {
            panic!("expected loss window");
        };
        window
    }

    #[test]
    fn capture_adapter_preserves_observation_boundaries() {
        let mut adapter = adapter();
        let connect = adapter
            .convert(RawKernelObservation::Connect(RawConnectObservation {
                socket: raw_socket(10),
                family: RawNetworkFamily::Ipv6,
                remote_endpoint: raw_endpoint(RawNetworkFamily::Ipv6, false),
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            connect,
            CaptureEvent {
                kind: CaptureEventKind::SocketConnectObserved(SocketConnectIntent {
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
            }
        ));

        let established = establish(
            &mut adapter,
            raw_socket(11),
            RawNetworkFamily::Ipv6,
            RawSocketRole::Active,
        );
        assert!(matches!(
            established,
            CaptureEvent {
                kind: CaptureEventKind::SocketConnected(SocketEvidence {
                    observed_cgroup_id: 31,
                    ..
                }),
                ..
            }
        ));
        let lifecycle = adapter
            .convert(RawKernelObservation::Lifecycle(RawLifecycleObservation {
                socket: raw_socket(12),
                signal: RawLifecycleSignal::Ambiguous(99),
                established: None,
            }))
            .unwrap()
            .unwrap();
        assert!(matches!(
            lifecycle,
            CaptureEvent {
                kind: CaptureEventKind::SocketStateChangedObserved(SocketStateChangeObserved {
                    raw_state: 99,
                    ..
                }),
                ..
            }
        ));
    }

    #[test]
    fn complete_payload_has_no_truncation_reason() {
        let fragment = payload_fragment(&mut adapter(), Some(5), false, b"bytes");
        assert_eq!(fragment.truncation.state, TruncationState::Complete);
        assert_eq!(fragment.truncation.reason, None);
    }

    #[test]
    fn explicit_kernel_truncation_keeps_existing_reason() {
        let fragment = payload_fragment(&mut adapter(), Some(8), true, b"bytes");
        assert_eq!(fragment.truncation.state, TruncationState::Truncated);
        assert_eq!(
            fragment.truncation.reason.as_deref(),
            Some("kernel capture bound")
        );
    }

    #[test]
    fn payload_length_mismatch_has_stable_unknown_reason() {
        let fragment = payload_fragment(&mut adapter(), Some(8), false, b"bytes");
        assert_eq!(fragment.truncation.state, TruncationState::Unknown);
        assert_eq!(
            fragment.truncation.reason.as_deref(),
            Some(PAYLOAD_LENGTH_MISMATCH_REASON)
        );
    }

    #[test]
    fn payload_fragment_preserves_transport_evidence() {
        let fragment = payload_fragment(&mut adapter(), Some(5), false, b"bytes");
        assert_eq!(fragment.network_family, NetworkFamily::Ipv6);
        assert_eq!(fragment.direction, PayloadDirection::Egress);
        assert_eq!(fragment.sequence.tcp_sequence, 100);
        assert_eq!(fragment.sequence.continuation_position, 2);
        assert_eq!(fragment.payload, b"bytes");
    }

    #[test]
    fn socket_cookie_reuse_never_merges_pid_or_namespace_reuse() {
        let mut adapter = adapter();
        let first = raw_socket_with_identity(10, 20, 5, Some(9), 31, 40);
        let reused = raw_socket_with_identity(11, 20, 6, None, 31, 40);
        for socket in [first, reused.clone()] {
            adapter
                .convert(RawKernelObservation::Connect(RawConnectObservation {
                    socket: socket.clone(),
                    family: RawNetworkFamily::Ipv4,
                    remote_endpoint: raw_endpoint(RawNetworkFamily::Ipv4, false),
                }))
                .unwrap();
            establish(
                &mut adapter,
                socket,
                RawNetworkFamily::Ipv4,
                RawSocketRole::Active,
            );
        }

        let event = adapter
            .convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: reused,
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
        let CaptureEvent {
            kind: CaptureEventKind::PayloadFragment(fragment),
            ..
        } = event
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
    fn passive_establishment_creates_complete_socket_cache() {
        let mut adapter = adapter();
        let mut socket = raw_socket(10);
        socket.cgroup_id = 0;
        let event = establish(
            &mut adapter,
            socket,
            RawNetworkFamily::Ipv4,
            RawSocketRole::Passive,
        );
        let CaptureEvent {
            kind: CaptureEventKind::SocketConnected(evidence),
            ..
        } = event
        else {
            panic!("expected connected evidence");
        };
        assert_eq!(evidence.role, SocketRole::Passive);
        assert_eq!(evidence.observed_cgroup_id, 30);
        assert_eq!(evidence.local_endpoint, Endpoint::new("192.0.2.10", 41_000));
        assert_eq!(evidence.remote_endpoint, Endpoint::new("192.0.2.20", 8_080));

        let payload = adapter
            .convert(RawKernelObservation::Payload(RawPayloadObservation {
                socket: raw_socket(11),
                family: RawNetworkFamily::Ipv4,
                direction: RawDirection::Ingress,
                tcp_sequence: 1,
                continuation_position: 0,
                observed_length: Some(1),
                truncated: false,
                payload: vec![1],
            }))
            .unwrap();
        assert!(payload.is_some());
    }

    #[test]
    fn conflicting_established_evidence_does_not_overwrite_cache() {
        let mut adapter = adapter();
        establish(
            &mut adapter,
            raw_socket(10),
            RawNetworkFamily::Ipv4,
            RawSocketRole::Passive,
        );
        let mut conflicting = RawEstablishedSocketEvidence {
            family: RawNetworkFamily::Ipv4,
            local_endpoint: raw_endpoint(RawNetworkFamily::Ipv4, true),
            remote_endpoint: raw_endpoint(RawNetworkFamily::Ipv4, false),
            role: RawSocketRole::Passive,
        };
        conflicting.remote_endpoint.port = 9_090;
        assert!(matches!(
            adapter.convert(RawKernelObservation::Lifecycle(RawLifecycleObservation {
                socket: raw_socket(11),
                signal: RawLifecycleSignal::PassiveEstablished,
                established: Some(conflicting),
            })),
            Err(EbpfCaptureError::ConflictingSocketEvidence)
        ));
        assert_eq!(
            adapter
                .sockets
                .values()
                .next()
                .unwrap()
                .remote_endpoint
                .port,
            8_080
        );
    }

    #[test]
    fn first_zero_loss_sample_emits_nothing() {
        let mut adapter = adapter();
        assert!(
            adapter
                .convert(loss_observation(10, vec![0, 0]))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn first_positive_loss_sample_is_ambiguous() {
        let mut adapter = adapter();
        let first = loss_window(
            adapter
                .convert(loss_observation(40, vec![2, 3]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(first.start.nanoseconds, 1);
        assert_eq!(first.end.nanoseconds, 40);
        assert_eq!(first.drop_delta, Some(5));
        assert!(first.ambiguity.exact_drop_timing_unknown);
        assert!(first.ambiguity.affected_sockets_unknown);
        assert_eq!(
            first.ambiguity.reason.as_deref(),
            Some("before first complete sample")
        );
    }

    #[test]
    fn positive_loss_delta_is_temporally_and_socket_ambiguous() {
        let mut adapter = adapter();
        assert!(
            adapter
                .convert(loss_observation(10, vec![1, 1]))
                .unwrap()
                .is_some()
        );
        let positive = loss_window(
            adapter
                .convert(loss_observation(50, vec![2, 3]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(positive.start.nanoseconds, 10);
        assert_eq!(positive.end.nanoseconds, 50);
        assert_eq!(positive.drop_delta, Some(3));
        assert!(positive.ambiguity.exact_drop_timing_unknown);
        assert!(positive.ambiguity.affected_sockets_unknown);
        assert_eq!(
            positive.ambiguity.reason.as_deref(),
            Some("loss occurred between complete counter samples")
        );
    }

    #[test]
    fn zero_loss_delta_emits_nothing_and_advances_boundary() {
        let mut adapter = adapter();
        adapter.convert(loss_observation(10, vec![3])).unwrap();
        assert!(
            adapter
                .convert(loss_observation(60, vec![3]))
                .unwrap()
                .is_none()
        );
        let after_zero = loss_window(
            adapter
                .convert(loss_observation(80, vec![4]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(after_zero.start.nanoseconds, 60);
        assert_eq!(after_zero.end.nanoseconds, 80);
        assert_eq!(after_zero.drop_delta, Some(1));
        assert!(after_zero.ambiguity.exact_drop_timing_unknown);
        assert!(after_zero.ambiguity.affected_sockets_unknown);
    }

    #[test]
    fn loss_generation_change_remains_ambiguous() {
        let mut adapter = adapter();
        adapter.convert(loss_observation(10, vec![5])).unwrap();
        let replaced = loss_window(
            adapter
                .convert(loss_observation_with_generation(30, vec![1], Some(2)))
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
    fn counter_regression_remains_ambiguous() {
        let mut adapter = adapter();
        adapter.convert(loss_observation(10, vec![5])).unwrap();
        let regressed = loss_window(
            adapter
                .convert(loss_observation(30, vec![4]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(regressed.start.nanoseconds, 10);
        assert_eq!(regressed.end.nanoseconds, 30);
        assert_eq!(regressed.drop_delta, None);
        assert!(regressed.ambiguity.exact_drop_timing_unknown);
        assert!(regressed.ambiguity.affected_sockets_unknown);
    }

    #[test]
    fn final_loss_sample_uses_last_complete_boundary_and_stays_ambiguous() {
        let mut adapter = adapter();
        assert!(
            adapter
                .convert(loss_observation(10, vec![0]))
                .unwrap()
                .is_none()
        );
        let final_sample = loss_window(
            adapter
                .convert(loss_observation(90, vec![2]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(final_sample.start.nanoseconds, 10);
        assert_eq!(final_sample.end.nanoseconds, 90);
        assert_eq!(final_sample.drop_delta, Some(2));
        assert!(final_sample.ambiguity.exact_drop_timing_unknown);
        assert!(final_sample.ambiguity.affected_sockets_unknown);
    }

    #[test]
    fn incomplete_per_cpu_sample_is_ambiguous_without_advancing_boundary() {
        let mut adapter = adapter();
        assert!(
            adapter
                .convert(loss_observation(10, vec![0]))
                .unwrap()
                .is_none()
        );
        let incomplete =
            loss_window(adapter.incomplete_loss_sample(80, "incomplete per-CPU counter read"));
        assert_eq!(incomplete.start.nanoseconds, 10);
        assert_eq!(incomplete.end.nanoseconds, 80);
        assert_eq!(incomplete.drop_delta, None);
        assert_eq!(
            incomplete.ambiguity.reason.as_deref(),
            Some("incomplete per-CPU counter read")
        );

        let recovered = loss_window(
            adapter
                .convert(loss_observation(90, vec![1]))
                .unwrap()
                .unwrap(),
        );
        assert_eq!(recovered.start.nanoseconds, 10);
        assert_eq!(recovered.end.nanoseconds, 90);
        assert_eq!(recovered.drop_delta, Some(1));
    }

    #[test]
    fn per_cpu_counter_aggregation_overflow_is_typed() {
        let error = adapter()
            .convert(loss_observation(10, vec![u64::MAX, 1]))
            .unwrap_err();
        assert_eq!(
            error,
            EbpfCaptureError::RingLoss("per-CPU counter total overflow")
        );
    }
}
