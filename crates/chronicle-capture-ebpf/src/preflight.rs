//! No-attach runtime prerequisites for production eBPF capture.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreflightCheck {
    Available,
    Unavailable(&'static str),
    NotChecked(&'static str),
}

impl PreflightCheck {
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EbpfPreflight {
    pub platform: PreflightCheck,
    pub architecture: PreflightCheck,
    pub cgroup_v2: PreflightCheck,
    pub btf: PreflightCheck,
    pub embedded_object: PreflightCheck,
    pub required_programs: PreflightCheck,
    pub attach: PreflightCheck,
    pub cap_bpf: PreflightCheck,
    pub cap_net_admin: PreflightCheck,
}

impl EbpfPreflight {
    pub const fn is_ready(self) -> bool {
        self.platform.is_available()
            && self.architecture.is_available()
            && self.cgroup_v2.is_available()
            && self.btf.is_available()
            && self.embedded_object.is_available()
            && self.required_programs.is_available()
            // Attach requires a caller-supplied selector; static probe leaves it NotChecked.
            && !matches!(self.attach, PreflightCheck::Unavailable(_))
            && self.cap_bpf.is_available()
            && self.cap_net_admin.is_available()
    }
}

#[cfg(any(test, all(target_os = "linux", target_endian = "little")))]
const CAP_NET_ADMIN: u32 = 12;
#[cfg(any(test, all(target_os = "linux", target_endian = "little")))]
const CAP_BPF: u32 = 39;
#[cfg(all(target_os = "linux", target_endian = "little"))]
const REQUIRED_PROGRAMS: &[&str] = &[
    "connect4",
    "connect6",
    "socket_lifecycle",
    "ingress",
    "egress",
];

/// Checks static host and embedded-object prerequisites without loading or attaching programs.
pub fn probe_embedded() -> EbpfPreflight {
    #[cfg(all(target_os = "linux", target_endian = "little"))]
    {
        let (embedded_object, required_programs) = embedded_object_checks();
        let capabilities = effective_capabilities();
        EbpfPreflight {
            platform: PreflightCheck::Available,
            architecture: supported_architecture(),
            cgroup_v2: required_file("/sys/fs/cgroup/cgroup.controllers", "cgroup v2 unavailable"),
            btf: required_file("/sys/kernel/btf/vmlinux", "kernel BTF unavailable"),
            embedded_object,
            required_programs,
            attach: PreflightCheck::NotChecked(
                "attach feasibility requires an explicit recording selector",
            ),
            cap_bpf: capability_check(capabilities, CAP_BPF, "CAP_BPF unavailable"),
            cap_net_admin: capability_check(
                capabilities,
                CAP_NET_ADMIN,
                "CAP_NET_ADMIN unavailable",
            ),
        }
    }
    #[cfg(all(target_os = "linux", target_endian = "big"))]
    {
        unavailable_preflight("big-endian embedded eBPF object unsupported")
    }
    #[cfg(not(target_os = "linux"))]
    {
        unavailable_preflight("live capture is unavailable on this platform")
    }
}

#[cfg(not(all(target_os = "linux", target_endian = "little")))]
const fn unavailable_preflight(reason: &'static str) -> EbpfPreflight {
    EbpfPreflight {
        platform: PreflightCheck::Unavailable(reason),
        architecture: PreflightCheck::Unavailable(reason),
        cgroup_v2: PreflightCheck::Unavailable(reason),
        btf: PreflightCheck::Unavailable(reason),
        embedded_object: PreflightCheck::Unavailable(reason),
        required_programs: PreflightCheck::Unavailable(reason),
        attach: PreflightCheck::Unavailable(reason),
        cap_bpf: PreflightCheck::Unavailable(reason),
        cap_net_admin: PreflightCheck::Unavailable(reason),
    }
}

#[cfg(all(target_os = "linux", target_endian = "little"))]
const EMBEDDED_OBJECT: &[u8] =
    include_bytes!(concat!(env!("OUT_DIR"), "/chronicle-ebpf-capture-bpfel.o"));

#[cfg(all(target_os = "linux", target_endian = "little"))]
fn embedded_object_checks() -> (PreflightCheck, PreflightCheck) {
    let Ok(object) = aya_obj::Object::parse(EMBEDDED_OBJECT) else {
        return (
            PreflightCheck::Unavailable("embedded eBPF object unreadable"),
            PreflightCheck::Unavailable("embedded eBPF object unreadable"),
        );
    };
    let programs_present = REQUIRED_PROGRAMS
        .iter()
        .all(|program| object.programs.contains_key(*program));
    (
        PreflightCheck::Available,
        if programs_present {
            PreflightCheck::Available
        } else {
            PreflightCheck::Unavailable("embedded eBPF object misses required program")
        },
    )
}

#[cfg(all(target_os = "linux", target_endian = "little"))]
fn required_file(path: &str, reason: &'static str) -> PreflightCheck {
    if std::path::Path::new(path).is_file() {
        PreflightCheck::Available
    } else {
        PreflightCheck::Unavailable(reason)
    }
}

#[cfg(all(target_os = "linux", target_endian = "little"))]
fn supported_architecture() -> PreflightCheck {
    matches!(std::env::consts::ARCH, "aarch64" | "x86_64")
        .then_some(PreflightCheck::Available)
        .unwrap_or(PreflightCheck::Unavailable("host architecture unsupported"))
}

#[cfg(all(target_os = "linux", target_endian = "little"))]
fn effective_capabilities() -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .and_then(|value| u64::from_str_radix(value, 16).ok())
}

#[cfg(any(test, all(target_os = "linux", target_endian = "little")))]
fn capability_check(
    capabilities: Option<u64>,
    capability: u32,
    missing: &'static str,
) -> PreflightCheck {
    match capabilities {
        Some(value) if value & (1_u64 << capability) != 0 => PreflightCheck::Available,
        Some(_) => PreflightCheck::Unavailable(missing),
        None => PreflightCheck::NotChecked("effective capabilities unavailable"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_checks_are_exact() {
        assert_eq!(
            capability_check(Some(1_u64 << CAP_BPF), CAP_BPF, "missing"),
            PreflightCheck::Available
        );
        assert_eq!(
            capability_check(Some(1_u64 << CAP_NET_ADMIN), CAP_BPF, "missing"),
            PreflightCheck::Unavailable("missing")
        );
        assert_eq!(
            capability_check(None, CAP_BPF, "missing"),
            PreflightCheck::NotChecked("effective capabilities unavailable")
        );
    }

    #[cfg(all(target_os = "linux", target_endian = "little"))]
    #[test]
    fn embedded_object_contains_required_programs() {
        assert_eq!(
            embedded_object_checks(),
            (PreflightCheck::Available, PreflightCheck::Available)
        );
    }

    #[test]
    fn ready_requires_every_check() {
        let ready = EbpfPreflight {
            platform: PreflightCheck::Available,
            architecture: PreflightCheck::Available,
            cgroup_v2: PreflightCheck::Available,
            btf: PreflightCheck::Available,
            embedded_object: PreflightCheck::Available,
            required_programs: PreflightCheck::Available,
            attach: PreflightCheck::Available,
            cap_bpf: PreflightCheck::Available,
            cap_net_admin: PreflightCheck::Available,
        };
        assert!(ready.is_ready());
        assert!(
            EbpfPreflight {
                attach: PreflightCheck::NotChecked("selector required"),
                ..ready
            }
            .is_ready()
        );
        assert!(
            !EbpfPreflight {
                attach: PreflightCheck::Unavailable("attach unavailable"),
                ..ready
            }
            .is_ready()
        );
        assert!(
            !EbpfPreflight {
                cap_bpf: PreflightCheck::Unavailable("missing"),
                ..ready
            }
            .is_ready()
        );
    }
}
