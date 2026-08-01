#![cfg(target_os = "linux")]

use chronicle_capture::{
    CaptureEvent, CaptureEventKind, CaptureSource, ClockIdentity, PayloadDirection,
    RecordingScopeIdentity, SocketRole, TruncationState,
};
use chronicle_capture_ebpf::{CaptureAdapter, EbpfCaptureSource, RecordingScopeConfig};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Write,
    net::{TcpListener, TcpStream},
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
        let path =
            Path::new("/sys/fs/cgroup").join(format!("chronicle-adapter-{}", Uuid::new_v4()));
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

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
#[allow(clippy::too_many_lines)]
fn ipv4_connect_and_active_establish_emit_capture_events() -> Result<(), Box<dyn std::error::Error>>
{
    assert_eq!(std::env::consts::ARCH, "aarch64");
    assert_eq!(
        fs::read_to_string("/proc/sys/kernel/osrelease")?.trim(),
        "6.8.0-136-generic"
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

    let mut client = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; python3 -c 'import socket; s=socket.create_connection((\"127.0.0.1\", {})); s.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1); [s.sendall(b\"x\" * 32768) for _ in range(8)]; [s.sendall(b\"y\") for _ in range(200000)]; s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    fs::write(cgroup.path.join("cgroup.procs"), client.id().to_string())?;
    client.stdin.take().unwrap().write_all(b"start\n")?;
    assert!(client.wait()?.success());
    server.join().unwrap();

    let mut events = Vec::new();
    for _ in 0..100 {
        if let Some(event) = source.next_event()? {
            events.push(event);
        } else {
            thread::sleep(Duration::from_millis(10));
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

    assert!(events.iter().any(|event| matches!(
        event,
        CaptureEvent { kind, .. } if matches!(
            kind,
            CaptureEventKind::SocketConnectObserved(evidence)
                if evidence.network_family == chronicle_capture::NetworkFamily::Ipv4
                    && evidence.recording_scope.cgroup_id == cgroup_id
        )
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        CaptureEvent {
            kind: CaptureEventKind::SocketConnected(evidence),
            ..
        } if evidence.network_family == chronicle_capture::NetworkFamily::Ipv4
            && evidence.role == SocketRole::Active
            && evidence.local_endpoint.host == "127.0.0.1"
            && evidence.local_endpoint.port != 0
            && evidence.remote_endpoint.host == "127.0.0.1"
            && evidence.remote_endpoint.port == address.port()
    )));
    let fragments: Vec<_> = events
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
    }));
    assert!(
        fragments
            .iter()
            .any(|fragment| fragment.sequence.continuation_position > 0)
    );
    drop(source);
    drop(cgroup_file);
    drop(cgroup);
    assert!(!cgroup_path.exists());
    assert_eq!(bpftool_ids("prog")?, programs_before);
    assert_eq!(bpftool_ids("link")?, links_before);
    assert_eq!(bpftool_ids("map")?, maps_before);
    Ok(())
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn ipv4_passive_establish_emits_complete_endpoint_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let object = fs::read(production_object())?;
    let reservation = TcpListener::bind(("127.0.0.1", 0))?;
    let address = reservation.local_addr()?;
    drop(reservation);

    let cgroup = CgroupGuard::create()?;
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

    let mut server = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; python3 -c 'import socket; s=socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind((\"127.0.0.1\", {})); s.listen(1); c,_=s.accept(); c.recv(1024); c.close(); s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    fs::write(cgroup.path.join("cgroup.procs"), server.id().to_string())?;
    server.stdin.take().unwrap().write_all(b"start\n")?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = loop {
        match TcpStream::connect(address) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    client.write_all(b"passive")?;
    drop(client);
    assert!(server.wait()?.success());

    let mut events = Vec::new();
    for _ in 0..100 {
        if let Some(event) = source.next_event()? {
            events.push(event);
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }
    source.request_shutdown()?;
    while let Some(event) = source.drain()? {
        events.push(event);
    }
    source.finalize()?;

    assert!(events.iter().any(|event| matches!(
        event,
        CaptureEvent {
            kind: CaptureEventKind::SocketConnected(evidence),
            ..
        } if evidence.network_family == chronicle_capture::NetworkFamily::Ipv4
            && evidence.recording_scope.cgroup_id == cgroup_id
            && evidence.role == SocketRole::Passive
            && evidence.local_endpoint.host == "127.0.0.1"
            && evidence.local_endpoint.port == address.port()
            && evidence.remote_endpoint.host == "127.0.0.1"
            && evidence.remote_endpoint.port != 0
    )));
    Ok(())
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn ipv6_passive_establish_emits_complete_endpoint_evidence()
-> Result<(), Box<dyn std::error::Error>> {
    let object = fs::read(production_object())?;
    let reservation = TcpListener::bind(("::1", 0))?;
    let address = reservation.local_addr()?;
    drop(reservation);

    let cgroup = CgroupGuard::create()?;
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

    let mut server = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; python3 -c 'import socket; s=socket.socket(socket.AF_INET6); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1); s.bind((\"::1\", {})); s.listen(1); c,_=s.accept(); c.recv(1024); c.close(); s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    fs::write(cgroup.path.join("cgroup.procs"), server.id().to_string())?;
    server.stdin.take().unwrap().write_all(b"start\n")?;

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut client = loop {
        match TcpStream::connect(address) {
            Ok(stream) => break stream,
            Err(error) if Instant::now() < deadline => {
                let _ = error;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    };
    client.write_all(b"passive")?;
    drop(client);
    assert!(server.wait()?.success());

    let mut events = Vec::new();
    for _ in 0..100 {
        if let Some(event) = source.next_event()? {
            events.push(event);
        } else {
            thread::sleep(Duration::from_millis(10));
        }
    }
    source.request_shutdown()?;
    while let Some(event) = source.drain()? {
        events.push(event);
    }
    source.finalize()?;

    assert!(events.iter().any(|event| matches!(
        event,
        CaptureEvent {
            kind: CaptureEventKind::SocketConnected(evidence),
            ..
        } if evidence.network_family == chronicle_capture::NetworkFamily::Ipv6
            && evidence.recording_scope.cgroup_id == cgroup_id
            && evidence.role == SocketRole::Passive
            && evidence.local_endpoint.host == "::1"
            && evidence.local_endpoint.port == address.port()
            && evidence.remote_endpoint.host == "::1"
            && evidence.remote_endpoint.port != 0
    )));
    Ok(())
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn ipv6_connect_emits_ipv6_evidence() -> Result<(), Box<dyn std::error::Error>> {
    let object = fs::read(production_object())?;
    let listener = TcpListener::bind(("::1", 0))?;
    let address = listener.local_addr()?;
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        std::io::copy(&mut stream, &mut std::io::sink()).unwrap();
    });
    let cgroup = CgroupGuard::create()?;
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
    let mut client = Command::new("sh")
        .args([
            "-c",
            &format!(
                "read start; python3 -c 'import socket; s=socket.create_connection((\"::1\", {})); s.sendall(b\"capture\"); s.close()'",
                address.port()
            ),
        ])
        .stdin(Stdio::piped())
        .spawn()?;
    fs::write(cgroup.path.join("cgroup.procs"), client.id().to_string())?;
    client.stdin.take().unwrap().write_all(b"start\n")?;
    assert!(client.wait()?.success());
    server.join().unwrap();
    let mut observed = false;
    for _ in 0..100 {
        if let Some(event) = source.next_event()? {
            observed |= matches!(
                event.kind,
                CaptureEventKind::SocketConnected(ref evidence)
                    if evidence.network_family == chronicle_capture::NetworkFamily::Ipv6
                        && evidence.recording_scope.cgroup_id == cgroup_id
                        && evidence.role == SocketRole::Active
                        && evidence.local_endpoint.host == "::1"
                        && evidence.local_endpoint.port != 0
                        && evidence.remote_endpoint.host == "::1"
                        && evidence.remote_endpoint.port == address.port()
            );
        }
        if observed {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    source.request_shutdown()?;
    while source.drain()?.is_some() {}
    let summary = source.finalize()?;
    assert!(!summary.drain_failed);
    if observed {
        Ok(())
    } else {
        Err("no IPv6 connect evidence".into())
    }
}
