use crate::{adapter::CaptureAdapter, error::EbpfCaptureError};
use chronicle_capture::{CaptureError, CaptureSource};
#[cfg(all(target_os = "linux", feature = "linux-ebpf", target_endian = "little"))]
use sha2::{Digest, Sha256};

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use crate::abi::{RawKernelObservation, RawLossCounters, decode_raw_kernel_observation};
#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
use chronicle_capture::{CaptureEvent, CaptureSourceState, CaptureSourceSummary};

#[cfg(all(target_os = "linux", feature = "linux-ebpf", target_endian = "little"))]
const EMBEDDED_OBJECT: &[u8] = include_bytes!("../objects/chronicle-ebpf-capture-bpfel.o");

/// SHA-256 identity for metadata; unavailable on unsupported object targets.
#[cfg(all(target_os = "linux", feature = "linux-ebpf", target_endian = "little"))]
pub fn embedded_object_sha256() -> Option<String> {
    Some(
        Sha256::digest(EMBEDDED_OBJECT)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

#[cfg(not(all(target_os = "linux", feature = "linux-ebpf", target_endian = "little")))]
pub fn embedded_object_sha256() -> Option<String> {
    None
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
const LOSS_SAMPLE_INTERVAL_NS: u64 = 100_000_000;

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
#[derive(Clone, Debug)]
struct LossSampleSchedule {
    next_due_ns: u64,
}

#[cfg_attr(
    not(all(target_os = "linux", feature = "linux-ebpf")),
    allow(dead_code)
)]
impl LossSampleSchedule {
    const fn new(attached_at_ns: u64) -> Self {
        Self {
            next_due_ns: attached_at_ns.saturating_add(LOSS_SAMPLE_INTERVAL_NS),
        }
    }

    fn due(&mut self, now_ns: u64) -> bool {
        if now_ns < self.next_due_ns {
            return false;
        }
        self.next_due_ns = now_ns.saturating_add(LOSS_SAMPLE_INTERVAL_NS);
        true
    }
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
pub struct EbpfCaptureSource {
    // Dropping this object detaches producer links while independently owned maps stay drainable.
    ebpf: Option<aya::Ebpf>,
    ring: Option<aya::maps::RingBuf<aya::maps::MapData>>,
    counters: Option<aya::maps::PerCpuArray<aya::maps::MapData, u64>>,
    adapter: CaptureAdapter,
    loss_schedule: LossSampleSchedule,
    pending: std::collections::VecDeque<CaptureEvent>,
    counter_generation: u64,
    state: CaptureSourceState,
    drain_complete: bool,
    final_loss: Option<CaptureEvent>,
    summary: CaptureSourceSummary,
}

#[cfg(any(not(target_os = "linux"), not(feature = "linux-ebpf")))]
pub struct EbpfCaptureSource;

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
impl EbpfCaptureSource {
    #[cfg(target_endian = "little")]
    pub fn load_embedded(
        cgroup: &std::fs::File,
        adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        Self::load(EMBEDDED_OBJECT, cgroup, adapter)
    }

    pub fn load_embedded_with_post_attach(
        cgroup: &std::fs::File,
        adapter: CaptureAdapter,
        post_attach: impl FnOnce() -> Result<(), EbpfCaptureError>,
    ) -> Result<Self, EbpfCaptureError> {
        Self::load_with_post_attach(EMBEDDED_OBJECT, cgroup, adapter, post_attach)
    }

    #[cfg(target_endian = "big")]
    pub fn load_embedded(
        _cgroup: &std::fs::File,
        _adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        Err(EbpfCaptureError::UnsupportedCapability(
            "big-endian embedded eBPF object",
        ))
    }

    pub fn load(
        object: &[u8],
        cgroup: &std::fs::File,
        adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        Self::load_with_post_attach(object, cgroup, adapter, || Ok(()))
    }

    pub fn load_with_post_attach(
        object: &[u8],
        cgroup: &std::fs::File,
        mut adapter: CaptureAdapter,
        post_attach: impl FnOnce() -> Result<(), EbpfCaptureError>,
    ) -> Result<Self, EbpfCaptureError> {
        if !matches!(std::env::consts::ARCH, "aarch64" | "x86_64") {
            return Err(EbpfCaptureError::UnsupportedCapability("host architecture"));
        }
        let mut ebpf = aya::Ebpf::load(object).map_err(|_| EbpfCaptureError::Verifier {
            program: "object",
            reason: "load failed",
        })?;
        if let Err(error) = attach_all(&mut ebpf, cgroup) {
            drop(ebpf);
            return Err(error);
        }
        if let Err(error) = post_attach() {
            drop(ebpf);
            return Err(error);
        }
        let attached_at_ns = monotonic_nanoseconds()?;
        adapter.reset_attachment_time(attached_at_ns);
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
            adapter,
            loss_schedule: LossSampleSchedule::new(attached_at_ns),
            pending: std::collections::VecDeque::new(),
            counter_generation: 0,
            state: CaptureSourceState::Running,
            drain_complete: false,
            final_loss: None,
            summary: CaptureSourceSummary::default(),
        })
    }
}

#[cfg(any(not(target_os = "linux"), not(feature = "linux-ebpf")))]
impl EbpfCaptureSource {
    pub fn load(
        _object: &[u8],
        _cgroup: &std::fs::File,
        _adapter: CaptureAdapter,
    ) -> Result<Self, EbpfCaptureError> {
        if cfg!(target_os = "linux") {
            return Err(EbpfCaptureError::FeatureDisabled);
        }
        Err(EbpfCaptureError::UnsupportedPlatform)
    }
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
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

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
impl EbpfCaptureSource {
    fn source_error(error: &EbpfCaptureError) -> CaptureError {
        CaptureError::Source(error.to_string())
    }

    fn sample_loss_at(
        &mut self,
        timestamp_ns: u64,
        final_sample: bool,
    ) -> Result<Option<CaptureEvent>, EbpfCaptureError> {
        let values = match self
            .counters
            .as_mut()
            .ok_or(EbpfCaptureError::RingLoss("counter map released"))?
            .get(&1, 0)
        {
            Ok(values) => values,
            Err(_) if !final_sample => {
                return Ok(Some(self.adapter.incomplete_loss_sample(
                    timestamp_ns,
                    "incomplete per-CPU counter read",
                )));
            }
            Err(_) => {
                return Err(EbpfCaptureError::RingLoss(
                    "incomplete per-CPU counter read",
                ));
            }
        };
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
        let raw = decode_raw_kernel_observation(item.as_ref())?;
        self.adapter.convert(raw)
    }

    fn emitted(&mut self, event: Option<CaptureEvent>) -> Option<CaptureEvent> {
        if event.is_some() {
            self.summary.emitted_events += 1;
        }
        event
    }
}

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
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
        if let Some(event) = self.pending.pop_front() {
            return Ok(self.emitted(Some(event)));
        }

        let timestamp_ns = monotonic_nanoseconds().map_err(|error| Self::source_error(&error))?;
        let ring_event = self
            .ring_event()
            .map_err(|error| Self::source_error(&error))?;
        let sampled_loss = if self.loss_schedule.due(timestamp_ns) {
            self.sample_loss_at(timestamp_ns, false)
                .map_err(|error| Self::source_error(&error))?
        } else {
            None
        };
        if let Some(ring_event) = ring_event {
            if let Some(sampled_loss) = sampled_loss {
                self.pending.push_back(sampled_loss);
            }
            return Ok(self.emitted(Some(ring_event)));
        }
        Ok(self.emitted(sampled_loss))
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
        let timestamp_ns = monotonic_nanoseconds()
            .map_err(|error| CaptureError::FinalLossSample(error.to_string()))?;
        self.final_loss = match self.sample_loss_at(timestamp_ns, true) {
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

        if let Some(event) = self.pending.pop_front() {
            return Ok(self.emitted(Some(event)));
        }

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

#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]
#[allow(unsafe_code)] // libc exposes Linux CLOCK_MONOTONIC only as an unsafe FFI call.
fn monotonic_nanoseconds() -> Result<u64, EbpfCaptureError> {
    let mut value = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut value) } != 0 {
        return Err(EbpfCaptureError::RingLoss("monotonic clock unavailable"));
    }
    u64::try_from(value.tv_sec)
        .ok()
        .and_then(|seconds| seconds.checked_mul(1_000_000_000))
        .and_then(|seconds| seconds.checked_add(u64::try_from(value.tv_nsec).ok()?))
        .ok_or(EbpfCaptureError::RingLoss("monotonic clock out of range"))
}

#[cfg(any(not(target_os = "linux"), not(feature = "linux-ebpf")))]
impl CaptureSource for EbpfCaptureSource {
    fn next_event(&mut self) -> Result<Option<chronicle_capture::CaptureEvent>, CaptureError> {
        Err(CaptureError::Source(
            EbpfCaptureError::UnsupportedPlatform.to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{LOSS_SAMPLE_INTERVAL_NS, LossSampleSchedule};

    #[test]
    fn fake_clock_schedules_every_100ms_and_uses_delayed_actual_time() {
        let mut schedule = LossSampleSchedule::new(10);
        assert!(!schedule.due(10 + LOSS_SAMPLE_INTERVAL_NS - 1));
        assert!(schedule.due(10 + LOSS_SAMPLE_INTERVAL_NS));
        assert!(!schedule.due(10 + LOSS_SAMPLE_INTERVAL_NS * 2 - 1));

        // Delayed poll advances from observed time, never inventing a 100ms boundary.
        assert!(schedule.due(350_000_000));
        assert!(!schedule.due(449_999_999));
        assert!(schedule.due(450_000_000));
    }
}
