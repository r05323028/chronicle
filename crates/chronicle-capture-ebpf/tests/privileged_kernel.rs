//! Privileged kernel-behavior acceptance against the PRODUCTION eBPF object.
//!
//! The retired Gate A feasibility harness (`ebpf-feasibility/`) proved kernel
//! semantics with a separate experiment probe. The production eBPF adapter is
//! now the only validation authority: this suite loads the production probe
//! (`ebpf/` build, fallback object `objects/chronicle-ebpf-capture-bpfel.o`)
//! and asserts the kernel behaviors the adapter depends on:
//!
//! - hook availability: connect4/connect6, sockops lifecycle, cgroup-skb
//!   ingress/egress attach and clean detach;
//! - connect evidence: tuple, active role, host-visible PID/TGID identity,
//!   socket-owner cgroup id;
//! - sockops lifecycle evidence (state changes, close);
//! - cgroup-skb direction and TCP sequence ordering across continuations;
//! - truncation (64-byte capture cap with observed length);
//! - ring buffer behavior and loss accounting (forced ring saturation yields
//!   typed loss windows with drop deltas; drain + final sample).
//!
//! Feasibility-probe-only claims (for example 16 KiB continuation batches of
//! the experiment probe) are NOT asserted here: production-observed behavior
//! is the only truth.
//!
//! Runs only on the verified matrix: Ubuntu 24.04, Linux 6.8, aarch64,
//! cgroup v2, readable BTF, privileged root (same matrix as the retired
//! harness; other kernels/architectures require their own privileged run).

#![cfg(target_os = "linux")]

use chronicle_capture::{
    CaptureEvent, CaptureEventKind, CaptureSource, ClockIdentity, NetworkFamily, PayloadDirection,
    RecordingScopeIdentity, SocketRole, TruncationState,
};
use chronicle_capture_ebpf::{CaptureAdapter, EbpfCaptureSource, RecordingScopeConfig};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    net::TcpListener,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
use uuid::Uuid;

struct CgroupGuard {
    path: PathBuf,
}

impl CgroupGuard {
    fn create() -> std::io::Result<Self> {
        let path = Path::new("/sys/fs/cgroup").join(format!("chronicle-kernel-{}", Uuid::new_v4()));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }

    fn id(&self) -> std::io::Result<u64> {
        Ok(self.path.metadata()?.ino())
    }
}

impl Drop for CgroupGuard {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.path);
    }
}

fn production_object() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture")
}

fn bpftool_program_names() -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let output = Command::new("bpftool")
        .args(["prog", "show", "-j"])
        .output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!("bpftool prog show failed")).into());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    Ok(value
        .as_array()
        .map(|programs| {
            programs
                .iter()
                .filter_map(|program| program["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default())
}

fn bpftool_ids(kind: &str) -> Result<BTreeSet<u32>, Box<dyn std::error::Error>> {
    let output = Command::new("bpftool").args([kind, "show"]).output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!("bpftool {kind} show failed")).into());
    }
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter_map(|line| line.split_once(':')?.0.trim().parse().ok())
        .collect())
}

/// Kernel-behavior matrix against the production probe: hook availability,
/// connect/sockops/cgroup-skb evidence, identity, truncation, and loss
/// accounting in one cgroup-scoped capture.
#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
#[allow(clippy::too_many_lines)]
fn production_kernel_behavior_matrix() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(std::env::consts::ARCH, "aarch64");
    // Verified matrix is Ubuntu 24.04 / Linux 6.8 / aarch64; patch releases
    // within the 6.8 series (for example -136 and -137) are the same series.
    assert!(
        fs::read_to_string("/proc/sys/kernel/osrelease")?.starts_with("6.8.0-"),
        "unverified kernel series"
    );
    assert!(Path::new("/sys/kernel/btf/vmlinux").is_file());
    let object = fs::read(production_object())?;
    let programs_before = bpftool_ids("prog")?;
    let links_before = bpftool_ids("link")?;
    let maps_before = bpftool_ids("map")?;

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::io::copy(&mut stream, &mut std::io::sink()).unwrap();
    });

    let cgroup = CgroupGuard::create()?;
    let cgroup_path = cgroup.path.clone();
    let cgroup_id = cgroup.id()?;
    let scope = RecordingScopeConfig {
        identity: RecordingScopeIdentity {
            cgroup_id,
            canonical_path: cgroup.path.to_string_lossy().into_owned(),
            namespace: None,
        },
        descendant_cgroup_ids: BTreeSet::new(),
    };
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    let adapter = CaptureAdapter::new(ClockIdentity { boot_id }, scope, 0)?;
    let cgroup_file = File::open(&cgroup.path)?;
    let mut source = EbpfCaptureSource::load(&object, &cgroup_file, adapter)?;

    // All production hook programs must be loaded: connect4/connect6
    // (cgroup_sock_addr), socket_lifecycle (sock_ops), ingress/egress
    // (cgroup_skb).
    let loaded = bpftool_program_names()?;
    // Kernel BPF program names are truncated to 15 bytes (socket_lifecycle
    // loads as socket_lifecycl); match the kernel-visible prefix.
    for required in [
        "connect4",
        "connect6",
        "socket_lifecycl",
        "ingress",
        "egress",
    ] {
        assert!(
            loaded.iter().any(|name| name.starts_with(required)),
            "production hook program {required} not loaded: {loaded:?}"
        );
    }

    // Phase A: representative traffic. Identity, lifecycle, and payload
    // evidence are asserted BEFORE ring saturation so early events cannot be
    // dropped by the loss phase (the feasibility harness proved saturation
    // increments per-CPU loss counters; early evidence must not race it).
    let mut client = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; exec python3 -c 'import socket; s=socket.create_connection((\"127.0.0.1\", {})); s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1); [s.sendall(b\"x\" * 8192) for _ in range(4)]; s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    let client_pid = client.id();
    fs::write(cgroup.path.join("cgroup.procs"), client_pid.to_string())?;
    client.stdin.take().unwrap().write_all(b"start\n")?;
    assert!(client.wait()?.success());
    server.join().unwrap();

    let mut events = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut quiet = Duration::ZERO;
    while Instant::now() < deadline && quiet < Duration::from_millis(400) {
        if let Some(event) = source.next_event()? {
            events.push(event);
            quiet = Duration::ZERO;
        } else {
            thread::sleep(Duration::from_millis(10));
            quiet += Duration::from_millis(10);
        }
    }
    let frags: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => Some(fragment),
            _ => None,
        })
        .collect();
    eprintln!(
        "phase A collected {} events, {} fragments, lens {:?}, positions {:?}",
        events.len(),
        frags.len(),
        frags
            .iter()
            .map(|f| f.truncation.captured_length)
            .collect::<Vec<_>>(),
        frags
            .iter()
            .map(|f| f.sequence.continuation_position)
            .collect::<Vec<_>>(),
    );

    // Connect intent: tuple + socket-owner cgroup, emitted before connect.
    let intents: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            CaptureEventKind::SocketConnectObserved(intent) => Some(intent),
            _ => None,
        })
        .collect();
    assert!(!intents.is_empty(), "no connect intent observed");
    assert!(intents.iter().any(|intent| {
        intent.network_family == NetworkFamily::Ipv4
            && intent.recording_scope.cgroup_id == cgroup_id
            && intent.remote_endpoint.host == "127.0.0.1"
            && intent.remote_endpoint.port == address.port()
    }));

    // Connected evidence: active role, complete tuple, host-visible process
    // identity (connect hooks expose PID/TGID), and observed cgroup id.
    let connected: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            CaptureEventKind::SocketConnected(evidence) => Some(evidence),
            _ => None,
        })
        .collect();
    assert!(!connected.is_empty(), "no connected evidence observed");
    assert!(connected.iter().any(|evidence| {
        evidence.network_family == NetworkFamily::Ipv4
            && evidence.recording_scope.cgroup_id == cgroup_id
            && evidence.role == SocketRole::Active
            && evidence.local_endpoint.host == "127.0.0.1"
            && evidence.local_endpoint.port != 0
            && evidence.remote_endpoint.host == "127.0.0.1"
            && evidence.remote_endpoint.port == address.port()
            && evidence.process.as_ref().is_some_and(|process| {
                // Host-visible PID/TGID identity from connect hooks; the
                // production probe does not populate an executable name.
                process.pid == client_pid
            })
            && evidence.observed_cgroup_id == cgroup_id
    }));

    // Sockops lifecycle: raw state changes and/or close evidence for the
    // same socket generation are observed (state alone never distinguishes
    // reset from orderly close; raw states are preserved).
    assert!(events.iter().any(|event| matches!(
        event.kind,
        CaptureEventKind::SocketStateChangedObserved(_)
            | CaptureEventKind::SocketClosedObserved(_)
            | CaptureEventKind::SocketResetObserved(_)
    )));

    // cgroup-skb: egress fragments carry TCP sequence evidence; continuation
    // positions for one socket are ordered by TCP sequence (exact offsets,
    // never fabricated).
    let mut fragments: Vec<_> = events
        .iter()
        .filter_map(|event| match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => Some(fragment),
            _ => None,
        })
        .collect();
    assert!(fragments.len() >= 2);
    assert!(fragments.iter().any(|fragment| {
        fragment.timestamp.nanoseconds > 0
            && fragment.direction == PayloadDirection::Egress
            && fragment.truncation.captured_length <= 64
            && fragment.truncation.observed_length >= Some(fragment.truncation.captured_length)
            && fragment.truncation.state == TruncationState::Truncated
            && fragment.sequence.tcp_sequence > 0
    }));
    fragments.sort_by_key(|fragment| {
        (
            fragment.socket.socket_cookie,
            fragment.sequence.tcp_sequence,
        )
    });
    let mut by_cookie: std::collections::BTreeMap<u64, Vec<_>> = std::collections::BTreeMap::new();
    for fragment in fragments {
        by_cookie
            .entry(fragment.socket.socket_cookie)
            .or_default()
            .push(fragment);
    }
    assert!(
        by_cookie
            .values()
            .filter(|group| group.len() >= 2)
            .any(|group| group
                .windows(2)
                .all(|pair| pair[0].sequence.tcp_sequence < pair[1].sequence.tcp_sequence)),
        "TCP sequence must be strictly ordered within one socket"
    );

    // Phase B: forced ring saturation. The production ring is 8 MiB; a burst
    // of small writes saturates it and must produce typed loss evidence with
    // a positive drop delta (100 ms-or-later per-CPU sampling, delayed
    // intervals, mandatory final sample).
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::io::copy(&mut stream, &mut std::io::sink()).unwrap();
    });
    let mut saturator = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; exec python3 -c 'import socket; s=socket.create_connection((\"127.0.0.1\", {})); [s.sendall(b\"y\") for _ in range(200000)]; s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    fs::write(cgroup.path.join("cgroup.procs"), saturator.id().to_string())?;
    saturator.stdin.take().unwrap().write_all(b"start\n")?;
    assert!(saturator.wait()?.success());
    server.join().unwrap();

    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if let Some(event) = source.next_event()? {
            events.push(event);
        }
    }
    source.request_shutdown()?;
    while let Some(event) = source.drain()? {
        events.push(event);
    }
    let summary = source.finalize()?;
    assert!(!summary.drain_failed);
    assert!(events.iter().any(|event| matches!(
        event,
        CaptureEvent { kind, .. } if matches!(
            kind,
            CaptureEventKind::LossWindowObserved(window)
                if window.start.nanoseconds > 0
                    && window.end.nanoseconds >= window.start.nanoseconds
                    && window.drop_delta.is_some_and(|delta| delta > 0)
                    && window.ambiguity.exact_drop_timing_unknown
                    && window.ambiguity.affected_sockets_unknown
        )
    )));

    drop(source);
    drop(cgroup_file);
    drop(cgroup);
    assert!(!cgroup_path.exists());
    assert_eq!(bpftool_ids("prog")?, programs_before);
    assert_eq!(bpftool_ids("link")?, links_before);
    assert_eq!(bpftool_ids("map")?, maps_before);
    Ok(())
}
