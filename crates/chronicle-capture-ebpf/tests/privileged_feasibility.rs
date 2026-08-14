#![cfg(target_os = "linux")]
#![allow(unsafe_code)] // Feasibility-only ABI decoding and euid probe.

use aya::{
    Ebpf, Pod,
    maps::{PerCpuArray, RingBuf},
    programs::{CgroupAttachMode, CgroupSkb, CgroupSkbAttachType, CgroupSockAddr, SockOps},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs::{self, File},
    io::{self, BufRead, BufReader, Read, Write},
    mem::size_of,
    net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const PAYLOAD_BYTES: usize = 256;
const CONTINUATION_MAGIC: u32 = 0x4348_4331;
const CONTINUATION_HEADER_BYTES: usize = 48;
const CONTINUATION_BYTES: usize = 16 * 1024;
const EVENT_CONNECT4: u8 = 1;
const EVENT_CONNECT6: u8 = 2;
const EVENT_SOCKOPS: u8 = 3;
const EVENT_INGRESS: u8 = 4;
const EVENT_EGRESS: u8 = 5;
const FLAG_TRUNCATED: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct ProbeEvent {
    timestamp_ns: u64,
    cgroup_id: u64,
    socket_cookie: u64,
    pid_tgid: u64,
    local_addr: [u8; 16],
    remote_addr: [u8; 16],
    tcp_sequence: u32,
    observed_len: u32,
    captured_len: u32,
    gso_size: u32,
    gso_segs: u32,
    skb_len: u32,
    linear_len: u32,
    flags: u32,
    local_port: u16,
    remote_port: u16,
    kind: u8,
    family: u8,
    direction: u8,
    state: u8,
    payload: [u8; PAYLOAD_BYTES],
}

unsafe impl Pod for ProbeEvent {}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct ContinuationHeader {
    magic: u32,
    header_bytes: u16,
    family: u8,
    direction: u8,
    timestamp_ns: u64,
    socket_cookie: u64,
    cgroup_id: u64,
    tcp_sequence: u32,
    observed_len: u32,
    payload_offset: u32,
    payload_len: u32,
}

unsafe impl Pod for ContinuationHeader {}

struct ContinuationEvidence {
    header: ContinuationHeader,
    payload: Vec<u8>,
}

#[derive(Serialize)]
struct ContinuationReport {
    family: u8,
    direction: u8,
    socket_cookie: u64,
    tcp_sequence: u32,
    observed_len: u32,
    payload_offset: u32,
    payload_len: u32,
}

#[derive(Serialize)]
struct EventEvidence {
    kind: &'static str,
    family: u8,
    direction: u8,
    socket_cookie: u64,
    cgroup_id: u64,
    pid_tgid: u64,
    local_addr: [u8; 16],
    remote_addr: [u8; 16],
    local_port: u16,
    remote_port: u16,
    tcp_sequence: u32,
    lifecycle_signal: &'static str,
    previous_state: u8,
    state: u8,
    observed_len: u32,
    captured_len: u32,
    gso_size: u32,
    gso_segs: u32,
    skb_len: u32,
    linear_len: u32,
    truncated: bool,
    contains_request: bool,
    contains_response: bool,
}

#[derive(Serialize, Clone)]
struct LossSampleEvidence {
    interval_start_ns: u64,
    interval_end_ns: u64,
    emitted: u64,
    ring_reservation_failures: u64,
    cpu_count: usize,
    final_sample: bool,
}

#[derive(Serialize)]
#[allow(clippy::struct_excessive_bools)] // Machine evidence keeps independent probe outcomes explicit.
struct FeasibilityReport {
    report_version: u16,
    disposable_harness: bool,
    kernel: String,
    architecture: String,
    cgroup_v2: bool,
    selected_cgroup_id: u64,
    workload_cgroup_id: u64,
    selected_direct_tgids: Vec<u32>,
    workload_direct_tgids: Vec<u32>,
    descendant_cgroup_count: usize,
    observed_cgroup_ids: BTreeSet<u64>,
    btf: bool,
    effective_capabilities: String,
    aya_version: &'static str,
    aya_ebpf_version: &'static str,
    programs: Vec<&'static str>,
    helpers: Vec<&'static str>,
    privileges: Vec<&'static str>,
    ring_bytes: u32,
    clock_id: &'static str,
    event_bytes: usize,
    payload_bytes_per_event: usize,
    continuation_bytes: usize,
    max_continuations_per_skb: usize,
    ipv4: bool,
    ipv6: bool,
    stable_socket_cookie: bool,
    candidate_generations: BTreeMap<u64, u64>,
    lifecycle_observed: bool,
    request_payload_observed: bool,
    response_payload_observed: bool,
    tcp_sequence_observed: bool,
    events_emitted: u64,
    ring_reservation_failures: u64,
    max_observed_payload_bytes: u32,
    continuation_records: usize,
    continuations: Vec<ContinuationReport>,
    exact_32k_continuation_reconstruction: bool,
    gso_metadata_observed: bool,
    large_aggregate_observed: bool,
    nonlinear_skb_load_succeeded: bool,
    offload_features: BTreeMap<String, bool>,
    loss_samples: Vec<LossSampleEvidence>,
    delayed_loss_interval_observed: bool,
    pre_first_sample_loss_observed_in_live_run: bool,
    loss_model_scenarios: Vec<&'static str>,
    mandatory_final_sample: bool,
    events: Vec<EventEvidence>,
    supported: Vec<&'static str>,
    unsupported: Vec<&'static str>,
}

struct CgroupGuard {
    path: PathBuf,
    workload: PathBuf,
    parent_procs: PathBuf,
    cleaned: bool,
}

impl CgroupGuard {
    fn create() -> Result<Self, Box<dyn Error>> {
        let parent = current_cgroup_path()?;
        let path = parent.join(format!("chronicle-feasibility-{}", std::process::id()));
        fs::create_dir(&path)?;
        let workload = path.join("descendant-workload");
        fs::create_dir(&workload)?;
        Ok(Self {
            path,
            workload,
            parent_procs: parent.join("cgroup.procs"),
            cleaned: false,
        })
    }

    fn enter(&self) -> io::Result<()> {
        fs::write(
            self.workload.join("cgroup.procs"),
            std::process::id().to_string(),
        )
    }

    fn cleanup(&mut self) -> io::Result<()> {
        if self.cleaned {
            return Ok(());
        }
        fs::write(&self.parent_procs, std::process::id().to_string())?;
        let mut last_error = None;
        for _ in 0..20 {
            let _ = fs::remove_dir(&self.workload);
            match fs::remove_dir(&self.path) {
                Ok(()) => {
                    self.cleaned = true;
                    return Ok(());
                }
                Err(error) => last_error = Some(error),
            }
            thread::sleep(Duration::from_millis(10));
        }
        Err(last_error.unwrap_or_else(|| io::Error::other("cgroup cleanup failed")))
    }
}

impl Drop for CgroupGuard {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

struct NetworkNamespaceGuard {
    namespace: String,
    host_interface: String,
}

impl NetworkNamespaceGuard {
    fn create() -> Result<Self, Box<dyn Error>> {
        let suffix = std::process::id() % 10_000;
        let guard = Self {
            namespace: format!("chronicle-gso-{suffix}"),
            host_interface: format!("cgh{suffix}"),
        };
        let peer = format!("cgp{suffix}");
        run_command("ip", &["netns", "add", &guard.namespace])?;
        run_command(
            "ip",
            &[
                "link",
                "add",
                &guard.host_interface,
                "type",
                "veth",
                "peer",
                "name",
                &peer,
            ],
        )?;
        run_command(
            "ip",
            &["addr", "add", "10.231.0.1/24", "dev", &guard.host_interface],
        )?;
        run_command("ip", &["link", "set", &guard.host_interface, "up"])?;
        run_command("ip", &["link", "set", &peer, "netns", &guard.namespace])?;
        run_command(
            "ip",
            &[
                "netns",
                "exec",
                &guard.namespace,
                "ip",
                "addr",
                "add",
                "10.231.0.2/24",
                "dev",
                &peer,
            ],
        )?;
        for interface in ["lo", peer.as_str()] {
            run_command(
                "ip",
                &[
                    "netns",
                    "exec",
                    &guard.namespace,
                    "ip",
                    "link",
                    "set",
                    interface,
                    "up",
                ],
            )?;
        }
        run_command(
            "ethtool",
            &[
                "-K",
                &guard.host_interface,
                "tso",
                "on",
                "gso",
                "on",
                "gro",
                "on",
            ],
        )?;
        Ok(guard)
    }
}

impl Drop for NetworkNamespaceGuard {
    fn drop(&mut self) {
        let _ = Command::new("ip")
            .args(["link", "del", &self.host_interface])
            .status();
        let _ = Command::new("ip")
            .args(["netns", "del", &self.namespace])
            .status();
    }
}

fn run_command(command: &str, arguments: &[&str]) -> Result<(), Box<dyn Error>> {
    let status = Command::new(command).args(arguments).status()?;
    if !status.success() {
        return Err(io::Error::other(format!("{command} {arguments:?} failed")).into());
    }
    Ok(())
}

struct ChildGuard(Vec<Child>);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and a prebuilt feasibility eBPF object"]
#[allow(clippy::too_many_lines)]
fn privileged_feasibility() -> Result<(), Box<dyn Error>> {
    require_supported_host()?;
    let object = feasibility_object();
    if !object.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "missing {}; run `cd ebpf-feasibility && cargo build --release` first",
                object.display()
            ),
        )
        .into());
    }

    let (mut server, ipv4_port, ipv6_port) = start_servers()?;
    let mut cgroup = CgroupGuard::create()?;
    let direct_members = spawn_direct_members(&cgroup.path)?;
    let direct_member_ids: BTreeSet<_> = direct_members.0.iter().map(Child::id).collect();
    let cgroup_file = File::open(&cgroup.path)?;
    let mode = std::env::var("CHRONICLE_FEASIBILITY_MODE").unwrap_or_else(|_| "baseline".into());
    println!(
        "selected_program={mode} object_sha256={}",
        sha256_file(&object)?
    );
    let mut ebpf = Ebpf::load_file(&object)?;
    attach_programs(&mut ebpf, &cgroup_file, &mode)?;
    let mut ring = RingBuf::try_from(
        ebpf.take_map("EVENTS")
            .ok_or_else(|| io::Error::other("EVENTS map missing"))?,
    )?;
    let counters = PerCpuArray::<_, u64>::try_from(
        ebpf.take_map("COUNTERS")
            .ok_or_else(|| io::Error::other("COUNTERS map missing"))?,
    )?;
    let network = NetworkNamespaceGuard::create()?;
    let (mut veth_server, veth_address) = start_veth_server(&network)?;

    cgroup.enter()?;
    validate_scope(&cgroup, &direct_member_ids)?;
    if let Some(cap) = dynamic_cap(&mode) {
        let matrix = run_dynamic_matrix(&mut ring, cap)?;
        validate_dynamic_matrix(&matrix, cap)?;
        println!("program_symbol=dynamic_len_{cap} load_result=ok attach_result=ok");
        drop(server);
        drop(ebpf);
        drop(direct_members);
        cgroup.cleanup()?;
        return Ok(());
    }
    exchange(SocketAddr::from((Ipv4Addr::LOCALHOST, ipv4_port)))?;
    exchange_large(SocketAddr::from((Ipv4Addr::LOCALHOST, ipv4_port)))?;
    exchange(SocketAddr::from((Ipv6Addr::LOCALHOST, ipv6_port)))?;
    exchange_large(veth_address)?;
    thread::sleep(Duration::from_millis(250));
    let drained = drain_events(&mut ring)?;
    let events = drained.events;
    let loss_samples = force_ring_loss_and_sample(&counters, ipv4_port)?;
    let emitted = sum_per_cpu(&counters, 0)?;
    let lost = sum_per_cpu(&counters, 1)?;
    if mode == "baseline" && lost == 0 {
        return Err(
            io::Error::other("forced traffic did not produce ring reservation loss").into(),
        );
    }
    for event in &events {
        println!(
            "event kind={} family={} direction={} cookie={} ports={}->{} sequence={} observed={} captured={} flags={:#x} payload={:?}",
            event.kind,
            event.family,
            event.direction,
            event.socket_cookie,
            event.local_port,
            event.remote_port,
            event.tcp_sequence,
            event.observed_len,
            event.captured_len,
            event.flags,
            String::from_utf8_lossy(payload(event)),
        );
    }
    if mode == "baseline" {
        validate_events(&events, emitted, cgroup.workload.metadata()?.ino())?;
        validate_continuations(&drained.continuations)?;
    }

    let report = build_report(
        &events,
        &drained.continuations,
        &loss_samples,
        emitted,
        lost,
        &cgroup,
        &network.host_interface,
    )?;
    let report_path = report_path();
    if let Some(parent) = report_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&report_path, serde_json::to_vec_pretty(&report)?)?;
    validate_report_contract(&report)?;
    println!(
        "Kernel capture feasibility report: {}",
        report_path.display()
    );
    println!("events={emitted} ring_reservation_failures={lost}");

    let _ = server.0.wait();
    let _ = veth_server.0.wait();
    drop(ebpf);
    drop(direct_members);
    cgroup.cleanup()?;
    if cgroup.path.exists() {
        return Err(io::Error::other("cgroup remained after detach cleanup").into());
    }
    Ok(())
}

fn attach_programs(ebpf: &mut Ebpf, cgroup: &File, mode: &str) -> Result<(), Box<dyn Error>> {
    for name in ["connect4", "connect6"] {
        let program: &mut CgroupSockAddr = ebpf
            .program_mut(name)
            .ok_or_else(|| io::Error::other(format!("program {name} missing")))?
            .try_into()?;
        program.load()?;
        program.attach(cgroup, CgroupAttachMode::Single)?;
    }
    let program: &mut SockOps = ebpf
        .program_mut("socket_lifecycle")
        .ok_or_else(|| io::Error::other("program socket_lifecycle missing"))?
        .try_into()?;
    program.load()?;
    program.attach(cgroup, CgroupAttachMode::Single)?;
    let skb_programs = if let Some(cap) = dynamic_cap(mode) {
        vec![(
            (if cap == 64 {
                "dynamic_len_64"
            } else {
                "dynamic_len_16384"
            }),
            CgroupSkbAttachType::Egress,
        )]
    } else {
        vec![
            ("ingress", CgroupSkbAttachType::Ingress),
            ("egress", CgroupSkbAttachType::Egress),
        ]
    };
    for (name, attach_type) in &skb_programs {
        let program: &mut CgroupSkb = ebpf
            .program_mut(name)
            .ok_or_else(|| io::Error::other(format!("program {name} missing")))?
            .try_into()?;
        program.load()?;
        program.attach(cgroup, *attach_type, CgroupAttachMode::Single)?;
    }
    Ok(())
}

fn start_veth_server(
    network: &NetworkNamespaceGuard,
) -> Result<(ServerGuard, SocketAddr), Box<dyn Error>> {
    let script = r"
import socket
listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(('10.231.0.2', 18080)); listener.listen(1)
print('READY', flush=True)
conn, _ = listener.accept()
data = b''
while b'\r\n\r\n' not in data:
    data += conn.recv(4096)
head, body = data.split(b'\r\n\r\n', 1)
length = 0
for line in head.split(b'\r\n')[1:]:
    if line.lower().startswith(b'content-length:'):
        length = int(line.split(b':', 1)[1])
while len(body) < length:
    body += conn.recv(min(65536, length - len(body)))
conn.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: 32768\r\nConnection: close\r\n\r\n' + b'V' * 32768)
conn.close()
";
    let mut child = Command::new("ip")
        .args([
            "netns",
            "exec",
            &network.namespace,
            "python3",
            "-u",
            "-c",
            script,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("veth server stdout unavailable"))?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    if line.trim() != "READY" {
        return Err(io::Error::other("veth server failed readiness").into());
    }
    Ok((
        ServerGuard(child),
        SocketAddr::from(([10, 231, 0, 2], 18080)),
    ))
}

fn dynamic_cap(mode: &str) -> Option<usize> {
    match mode {
        "dynamic-64" => Some(64),
        "dynamic-16384" => Some(16 * 1024),
        _ => None,
    }
}

fn dynamic_pattern(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| {
            u8::try_from((index * 73 + length * 29) % 251 + 1)
                .expect("dynamic pattern byte is bounded")
        })
        .collect()
}

fn run_dynamic_matrix<T: std::borrow::Borrow<aya::maps::MapData>>(
    ring: &mut RingBuf<T>,
    cap: usize,
) -> Result<Vec<ContinuationEvidence>, Box<dyn Error>> {
    let lengths: &[usize] = if cap == 64 {
        &[0, 1, 33, 53, 63, 64, 65]
    } else {
        &[
            0, 1, 53, 64, 65, 255, 256, 1024, 4095, 4096, 8192, 16383, 16384, 16385,
        ]
    };
    let script = format!(
        r"import socket
listener = socket.socket(); listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(('127.0.0.1', 0)); listener.listen({})
print(listener.getsockname()[1], flush=True)
for expected in {:?}:
    conn, _ = listener.accept()
    data = b''
    while len(data) < expected:
        chunk = conn.recv(expected - len(data))
        if not chunk: break
        data += chunk
    if len(data) != expected: raise RuntimeError((expected, len(data)))
    conn.close()
",
        lengths.len(),
        lengths,
    );
    let mut server = Command::new("python3")
        .args(["-u", "-c", &script])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = server
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("matrix server stdout unavailable"))?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    let port = line.trim().parse::<u16>()?;
    for &length in lengths {
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))?;
        let payload = dynamic_pattern(length);
        if !payload.is_empty() {
            stream.write_all(&payload)?;
        }
        stream.shutdown(std::net::Shutdown::Write)?;
    }
    if !server.wait()?.success() {
        return Err(io::Error::other("matrix server failed").into());
    }
    thread::sleep(Duration::from_millis(100));
    Ok(drain_events(ring)?.continuations)
}

fn validate_dynamic_matrix(
    records: &[ContinuationEvidence],
    cap: usize,
) -> Result<(), Box<dyn Error>> {
    let lengths: &[usize] = if cap == 64 {
        &[1, 33, 53, 63, 64, 65]
    } else {
        &[
            1, 53, 64, 65, 255, 256, 1024, 4095, 4096, 8192, 16383, 16384, 16385,
        ]
    };
    for &length in lengths {
        let expected = dynamic_pattern(length);
        let declared = length.min(cap);
        let record = records
            .iter()
            .find(|record| record.payload == expected[..declared])
            .ok_or_else(|| io::Error::other(format!("matrix payload {length} missing")))?;
        let expected_len = u32::try_from(length).expect("matrix length is bounded");
        let declared_len = u32::try_from(declared).expect("matrix length is bounded");
        if record.header.observed_len != expected_len
            || record.header.payload_len != declared_len
            || record.header.payload_offset != 0
            || record.payload.len() != declared
        {
            return Err(io::Error::other(format!("matrix header mismatch for {length}")).into());
        }
        if record.payload != expected[..declared] {
            return Err(io::Error::other(format!("matrix bytes mismatch for {length}")).into());
        }
        println!(
            "dynamic_matrix payload={length} observed={} declared={} truncated={} exact=true",
            record.header.observed_len,
            record.header.payload_len,
            length > cap
        );
    }
    if records.iter().any(|record| record.header.payload_len == 0) {
        return Err(io::Error::other("zero payload emitted continuation record").into());
    }
    println!("dynamic_matrix payload=0 emitted=false");
    Ok(())
}

fn start_servers() -> Result<(ServerGuard, u16, u16), Box<dyn Error>> {
    let script = r"
import socket
import time
v4 = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
v4.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
v4.bind(('127.0.0.1', 0)); v4.listen(1)
v6 = socket.socket(socket.AF_INET6, socket.SOCK_STREAM)
v6.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
v6.setsockopt(socket.IPPROTO_IPV6, socket.IPV6_V6ONLY, 1)
v6.bind(('::1', 0)); v6.listen(1)
print(f'{v4.getsockname()[1]} {v6.getsockname()[1]}', flush=True)
for listener, response_size in ((v4, 8), (v4, 32768), (v6, 8), (v4, 8)):
    conn, _ = listener.accept()
    data = b''
    while b'\r\n\r\n' not in data:
        data += conn.recv(4096)
    head, body = data.split(b'\r\n\r\n', 1)
    length = 0
    for line in head.split(b'\r\n')[1:]:
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    while len(body) < length:
        body += conn.recv(min(65536, length - len(body)))
        if length > 1024 * 1024:
            time.sleep(0.002)
    response_body = b'chronicle' if response_size == 8 else b'R' * response_size
    conn.sendall(b'HTTP/1.1 200 OK\r\nContent-Length: ' + str(response_size).encode() + b'\r\nConnection: close\r\n\r\n' + response_body)
    conn.close()
";
    let mut child = Command::new("python3")
        .args(["-u", "-c", script])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("server stdout unavailable"))?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line)?;
    let mut ports = line.split_whitespace().map(str::parse::<u16>);
    let ipv4 = ports
        .next()
        .ok_or_else(|| io::Error::other("server omitted IPv4 port"))??;
    let ipv6 = ports
        .next()
        .ok_or_else(|| io::Error::other("server omitted IPv6 port"))??;
    Ok((ServerGuard(child), ipv4, ipv6))
}

fn exchange(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.write_all(
        b"GET /capture-feasibility HTTP/1.1\r\nHost: chronicle.test\r\nConnection: close\r\n\r\n",
    )?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 OK") || !response.ends_with(b"chronicle") {
        return Err(io::Error::other("unexpected feasibility HTTP response"));
    }
    Ok(())
}

fn exchange_large(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    stream.set_nodelay(true)?;
    let body = vec![b'Q'; 32 * 1024];
    write!(
        stream,
        "POST /capture-feasibility-gso HTTP/1.1\r\nHost: chronicle.test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.flush()?;
    thread::sleep(Duration::from_millis(50));
    stream.write_all(&body)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 OK") || response.len() < body.len() {
        return Err(io::Error::other(
            "unexpected large feasibility HTTP response",
        ));
    }
    Ok(())
}

struct DrainedEvidence {
    events: Vec<ProbeEvent>,
    continuations: Vec<ContinuationEvidence>,
}

fn force_loss_exchange(address: SocketAddr) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    let body = vec![b'L'; 12 * 1024 * 1024];
    write!(
        stream,
        "POST /force-loss HTTP/1.1\r\nHost: chronicle.test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(&body)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200 OK") {
        return Err(io::Error::other("unexpected force-loss response"));
    }
    Ok(())
}

fn force_ring_loss_and_sample<T: std::borrow::Borrow<aya::maps::MapData>>(
    counters: &PerCpuArray<T, u64>,
    ipv4_port: u16,
) -> Result<Vec<LossSampleEvidence>, Box<dyn Error>> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, ipv4_port));
    let interval_start = boot_time_ns()?;
    let exchange = thread::spawn(move || force_loss_exchange(address));
    let mut samples = Vec::new();
    let mut previous_end = interval_start;
    loop {
        thread::sleep(Duration::from_millis(100));
        let interval_end = boot_time_ns()?;
        samples.push(loss_sample(counters, previous_end, interval_end, false)?);
        previous_end = interval_end;
        if samples.len() == 1 {
            thread::sleep(Duration::from_millis(150));
        }
        if exchange.is_finished() {
            break;
        }
    }
    exchange
        .join()
        .map_err(|_| io::Error::other("force-loss exchange thread panicked"))??;
    let interval_end = boot_time_ns()?;
    samples.push(loss_sample(counters, previous_end, interval_end, true)?);
    if !samples.last().is_some_and(|sample| sample.final_sample)
        || !samples.iter().any(|sample| {
            sample
                .interval_end_ns
                .saturating_sub(sample.interval_start_ns)
                >= 200_000_000
        })
    {
        return Err(
            io::Error::other("loss schedule omitted delayed or final sample evidence").into(),
        );
    }
    Ok(samples)
}

fn loss_sample<T: std::borrow::Borrow<aya::maps::MapData>>(
    counters: &PerCpuArray<T, u64>,
    interval_start_ns: u64,
    interval_end_ns: u64,
    final_sample: bool,
) -> Result<LossSampleEvidence, Box<dyn Error>> {
    let emitted = counters.get(&0, 0)?;
    let lost = counters.get(&1, 0)?;
    let expected_cpus = std::thread::available_parallelism()?.get();
    if emitted.len() != lost.len() || emitted.len() != expected_cpus {
        return Err(io::Error::other("partial per-CPU counter read").into());
    }
    Ok(LossSampleEvidence {
        interval_start_ns,
        interval_end_ns,
        emitted: emitted.iter().copied().sum(),
        ring_reservation_failures: lost.iter().copied().sum(),
        cpu_count: emitted.len(),
        final_sample,
    })
}

fn boot_time_ns() -> io::Result<u64> {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &raw mut time) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let seconds = u64::try_from(time.tv_sec).map_err(|_| io::Error::other("negative boot time"))?;
    let nanoseconds =
        u64::try_from(time.tv_nsec).map_err(|_| io::Error::other("negative boot nanoseconds"))?;
    Ok(seconds
        .saturating_mul(1_000_000_000)
        .saturating_add(nanoseconds))
}

fn drain_events<T: std::borrow::Borrow<aya::maps::MapData>>(
    ring: &mut RingBuf<T>,
) -> Result<DrainedEvidence, Box<dyn Error>> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut events = Vec::new();
    let mut continuations = Vec::new();
    loop {
        while let Some(item) = ring.next() {
            if item.len() == size_of::<ProbeEvent>() {
                let event = unsafe { std::ptr::read_unaligned(item.as_ptr().cast::<ProbeEvent>()) };
                events.push(event);
            } else if item.len() == CONTINUATION_HEADER_BYTES + CONTINUATION_BYTES {
                let header =
                    unsafe { std::ptr::read_unaligned(item.as_ptr().cast::<ContinuationHeader>()) };
                let payload_len = usize::try_from(header.payload_len)?;
                if header.magic != CONTINUATION_MAGIC
                    || usize::from(header.header_bytes) != CONTINUATION_HEADER_BYTES
                    || payload_len > CONTINUATION_BYTES
                {
                    return Err(io::Error::other("invalid continuation header").into());
                }
                continuations.push(ContinuationEvidence {
                    header,
                    payload: item
                        [CONTINUATION_HEADER_BYTES..CONTINUATION_HEADER_BYTES + payload_len]
                        .to_vec(),
                });
            } else {
                return Err(
                    io::Error::other(format!("unexpected event size {}", item.len())).into(),
                );
            }
        }
        if !events.is_empty() || Instant::now() >= deadline {
            return Ok(DrainedEvidence {
                events,
                continuations,
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn sum_per_cpu(
    counters: &PerCpuArray<impl std::borrow::Borrow<aya::maps::MapData>, u64>,
    index: u32,
) -> Result<u64, Box<dyn Error>> {
    Ok(counters.get(&index, 0)?.iter().copied().sum())
}

fn validate_events(
    events: &[ProbeEvent],
    emitted: u64,
    workload_cgroup_id: u64,
) -> Result<(), Box<dyn Error>> {
    let kinds: BTreeSet<_> = events.iter().map(|event| event.kind).collect();
    for required in [
        EVENT_CONNECT4,
        EVENT_CONNECT6,
        EVENT_SOCKOPS,
        EVENT_INGRESS,
        EVENT_EGRESS,
    ] {
        if !kinds.contains(&required) {
            return Err(io::Error::other(format!("missing event kind {required}")).into());
        }
    }
    for connect in events
        .iter()
        .filter(|event| matches!(event.kind, EVENT_CONNECT4 | EVENT_CONNECT6))
    {
        let correlated: Vec<_> = events
            .iter()
            .filter(|event| {
                event.socket_cookie == connect.socket_cookie && event.family == connect.family
            })
            .collect();
        let request = correlated.iter().any(|event| {
            event.kind == EVENT_EGRESS
                && (payload(event).starts_with(b"GET ") || payload(event).starts_with(b"POST "))
        });
        let response = correlated
            .iter()
            .any(|event| event.kind == EVENT_INGRESS && payload(event).starts_with(b"HTTP/1.1 "));
        if connect.pid_tgid == 0
            || !correlated.iter().any(|event| event.kind == EVENT_SOCKOPS)
            || !correlated.iter().any(|event| event.kind == EVENT_INGRESS)
            || !correlated.iter().any(|event| event.kind == EVENT_EGRESS)
            || !request
            || !response
        {
            return Err(io::Error::other(format!(
                "cookie {} family {} lacks complete connect/lifecycle/request/response correlation",
                connect.socket_cookie, connect.family
            ))
            .into());
        }
    }
    if !events.iter().any(|event| event.tcp_sequence != 0) {
        return Err(io::Error::other("TCP sequence not observed").into());
    }
    if !events
        .iter()
        .any(|event| event.kind == EVENT_SOCKOPS && matches!(event.direction, 1 | 2))
        || !events
            .iter()
            .any(|event| event.kind == EVENT_SOCKOPS && event.direction == 3)
    {
        return Err(
            io::Error::other("established and state-change evidence not both observed").into(),
        );
    }
    if events
        .iter()
        .filter(|event| matches!(event.kind, EVENT_CONNECT4 | EVENT_CONNECT6))
        .any(|event| event.cgroup_id != workload_cgroup_id)
    {
        return Err(
            io::Error::other("connect hook cgroup ID did not match descendant workload").into(),
        );
    }
    let connect_cookies: BTreeSet<_> = events
        .iter()
        .filter(|event| matches!(event.kind, EVENT_CONNECT4 | EVENT_CONNECT6))
        .map(|event| event.socket_cookie)
        .filter(|cookie| *cookie != 0)
        .collect();
    if connect_cookies.len() < 2
        || !events.iter().any(|event| {
            matches!(event.kind, EVENT_SOCKOPS | EVENT_INGRESS | EVENT_EGRESS)
                && connect_cookies.contains(&event.socket_cookie)
        })
    {
        return Err(io::Error::other("socket cookie did not correlate candidate hooks").into());
    }
    if emitted < events.len() as u64 {
        return Err(io::Error::other("per-CPU event count regressed").into());
    }
    Ok(())
}

fn validate_continuations(continuations: &[ContinuationEvidence]) -> Result<(), Box<dyn Error>> {
    if !exact_32k_reconstruction(continuations) {
        return Err(io::Error::other(
            "two ordered 16 KiB continuations did not reconstruct 32 KiB request payload",
        )
        .into());
    }
    Ok(())
}

fn exact_32k_reconstruction(continuations: &[ContinuationEvidence]) -> bool {
    let mut groups: BTreeMap<(u64, u32), Vec<&ContinuationEvidence>> = BTreeMap::new();
    for continuation in continuations
        .iter()
        .filter(|continuation| continuation.payload.iter().all(|byte| *byte == b'Q'))
    {
        groups
            .entry((
                continuation.header.socket_cookie,
                continuation.header.tcp_sequence,
            ))
            .or_default()
            .push(continuation);
    }
    groups.into_values().any(|mut request| {
        request.sort_by_key(|continuation| continuation.header.payload_offset);
        request.len() == 2
            && request.iter().all(|continuation| {
                continuation.header.direction == 2 && continuation.header.observed_len == 32 * 1024
            })
            && request[0].header.payload_offset == 0
            && request[1].header.payload_offset == 16 * 1024
            && request
                .iter()
                .flat_map(|continuation| continuation.payload.iter().copied())
                .eq(std::iter::repeat_n(b'Q', 32 * 1024))
    })
}

#[allow(clippy::too_many_lines)] // One flat constructor mirrors retained JSON evidence.
fn build_report(
    events: &[ProbeEvent],
    continuations: &[ContinuationEvidence],
    loss_samples: &[LossSampleEvidence],
    emitted: u64,
    lost: u64,
    cgroup: &CgroupGuard,
    offload_interface: &str,
) -> Result<FeasibilityReport, Box<dyn Error>> {
    let connect_cookies: BTreeSet<_> = events
        .iter()
        .filter(|event| matches!(event.kind, EVENT_CONNECT4 | EVENT_CONNECT6))
        .map(|event| event.socket_cookie)
        .filter(|cookie| *cookie != 0)
        .collect();
    Ok(FeasibilityReport {
        report_version: 1,
        disposable_harness: true,
        kernel: command_output("uname", &["-r"])?,
        architecture: command_output("uname", &["-m"])?,
        cgroup_v2: fs::read_to_string("/proc/filesystems")?.contains("cgroup2"),
        selected_cgroup_id: cgroup.path.metadata()?.ino(),
        workload_cgroup_id: cgroup.workload.metadata()?.ino(),
        selected_direct_tgids: direct_tgids(&cgroup.path)?,
        workload_direct_tgids: direct_tgids(&cgroup.workload)?,
        descendant_cgroup_count: descendant_cgroup_count(&cgroup.path)?,
        observed_cgroup_ids: events.iter().map(|event| event.cgroup_id).collect(),
        btf: Path::new("/sys/kernel/btf/vmlinux").is_file(),
        effective_capabilities: effective_capabilities()?,
        aya_version: "0.14.0",
        aya_ebpf_version: "0.2.1",
        programs: vec![
            "cgroup/connect4",
            "cgroup/connect6",
            "sockops",
            "cgroup_skb/ingress",
            "cgroup_skb/egress",
        ],
        helpers: vec![
            "bpf_ktime_get_ns",
            "bpf_get_current_cgroup_id",
            "bpf_get_socket_cookie",
            "bpf_get_current_pid_tgid (connect4/connect6 only)",
            "bpf_skb_load_bytes",
            "bpf_ringbuf_output/reserve/submit",
            "per-CPU array lookup",
        ],
        privileges: vec![
            "root in feasibility VM",
            "CAP_BPF-equivalent program/map load",
            "CAP_NET_ADMIN-equivalent cgroup attach",
            "writable delegated cgroup v2 subtree",
        ],
        ring_bytes: 8 * 1024 * 1024,
        clock_id: "CLOCK_MONOTONIC/bpf_ktime_get_ns",
        event_bytes: size_of::<ProbeEvent>(),
        payload_bytes_per_event: PAYLOAD_BYTES,
        continuation_bytes: 16 * 1024,
        max_continuations_per_skb: 4,
        ipv4: events.iter().any(|event| event.family == 2),
        ipv6: events.iter().any(|event| event.family == 10),
        stable_socket_cookie: events.iter().any(|event| {
            matches!(event.kind, EVENT_SOCKOPS | EVENT_INGRESS | EVENT_EGRESS)
                && connect_cookies.contains(&event.socket_cookie)
        }),
        candidate_generations: events
            .iter()
            .filter(|event| matches!(event.kind, EVENT_CONNECT4 | EVENT_CONNECT6))
            .map(|event| (event.socket_cookie, event.timestamp_ns))
            .collect(),
        lifecycle_observed: events.iter().any(|event| event.kind == EVENT_SOCKOPS),
        request_payload_observed: events.iter().any(|event| {
            event.kind == EVENT_EGRESS
                && (payload(event).starts_with(b"GET ") || payload(event).starts_with(b"POST "))
        }),
        response_payload_observed: events
            .iter()
            .any(|event| event.kind == EVENT_INGRESS && payload(event).starts_with(b"HTTP/1.1 ")),
        tcp_sequence_observed: events.iter().any(|event| event.tcp_sequence != 0),
        events_emitted: emitted,
        ring_reservation_failures: lost,
        max_observed_payload_bytes: events
            .iter()
            .map(|event| event.observed_len)
            .max()
            .unwrap_or_default(),
        continuation_records: continuations.len(),
        continuations: continuations
            .iter()
            .map(|continuation| ContinuationReport {
                family: continuation.header.family,
                direction: continuation.header.direction,
                socket_cookie: continuation.header.socket_cookie,
                tcp_sequence: continuation.header.tcp_sequence,
                observed_len: continuation.header.observed_len,
                payload_offset: continuation.header.payload_offset,
                payload_len: continuation.header.payload_len,
            })
            .collect(),
        exact_32k_continuation_reconstruction: exact_32k_reconstruction(continuations),
        gso_metadata_observed: events
            .iter()
            .any(|event| event.gso_size > 0 || event.gso_segs > 1),
        large_aggregate_observed: events.iter().any(|event| event.observed_len > 16 * 1024),
        nonlinear_skb_load_succeeded: exact_32k_reconstruction(continuations)
            && events.iter().any(|event| event.linear_len < event.skb_len),
        offload_features: offload_features(offload_interface)?,
        delayed_loss_interval_observed: loss_samples.iter().any(|sample| {
            sample
                .interval_end_ns
                .saturating_sub(sample.interval_start_ns)
                >= 200_000_000
        }),
        pre_first_sample_loss_observed_in_live_run: loss_samples
            .first()
            .is_some_and(|sample| sample.ring_reservation_failures > 0),
        loss_model_scenarios: vec![
            "pre_first_positive",
            "zero_delta_no_wal_growth",
            "positive_delta",
            "delayed_interval",
            "counter_reset",
            "map_replacement",
            "clock_mismatch",
            "partial_read",
            "final_sample_failure",
        ],
        mandatory_final_sample: loss_samples
            .last()
            .is_some_and(|sample| sample.final_sample),
        loss_samples: loss_samples
            .iter()
            .map(|sample| LossSampleEvidence {
                interval_start_ns: sample.interval_start_ns,
                interval_end_ns: sample.interval_end_ns,
                emitted: sample.emitted,
                ring_reservation_failures: sample.ring_reservation_failures,
                cpu_count: sample.cpu_count,
                final_sample: sample.final_sample,
            })
            .collect(),
        events: events.iter().map(event_evidence).collect(),

        supported: vec![
            "Linux 6.8 aarch64 with cgroup v2 and BTF",
            "plaintext client-initiated IPv4 and IPv6 TCP",
            "veth GSO/GRO metadata and nonlinear payload visibility; separate 32 KiB aggregate via two ordered 16 KiB continuations",
            "socket-cookie correlation across connect, sock_ops, and cgroup_skb",
            "100 ms-or-later cumulative complete per-CPU loss sampling",
        ],
        unsupported: vec![
            "TLS plaintext",
            "current PID/TGID from sock_ops or cgroup_skb context",
            "current cgroup helper from cgroup_skb as socket-owner identity",
            "directional half-close inference",
            "reset versus orderly close from sock_ops state alone",
            "payload continuation ABI",
        ],
    })
}

fn validate_report_contract(report: &FeasibilityReport) -> Result<(), Box<dyn Error>> {
    if report.ring_bytes != 8 * 1024 * 1024
        || report.payload_bytes_per_event != PAYLOAD_BYTES
        || report.continuation_bytes != 16 * 1024
        || report.max_continuations_per_skb != 4
    {
        return Err(io::Error::other(
            "report constants differ from kernel capture feasibility spec bounds",
        )
        .into());
    }
    if !matches!(report.architecture.as_str(), "aarch64" | "x86_64")
        || !report.cgroup_v2
        || !report.btf
        || !report.ipv4
        || !report.ipv6
        || !report.stable_socket_cookie
        || report.candidate_generations.len() < 3
        || !report.lifecycle_observed
        || !report.request_payload_observed
        || !report.response_payload_observed
        || !report.tcp_sequence_observed
        || !report.exact_32k_continuation_reconstruction
        || !report.large_aggregate_observed
        || !report.nonlinear_skb_load_succeeded
        || !report.offload_features.values().all(|enabled| *enabled)
        || report.ring_reservation_failures == 0
        || !report.delayed_loss_interval_observed
        || report.loss_model_scenarios.len() != 9
        || !report.mandatory_final_sample
    {
        return Err(io::Error::other(
            "Kernel capture feasibility report omitted required evidence",
        )
        .into());
    }
    let version = report
        .kernel
        .split(['.', '-'])
        .take(2)
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()?;
    if version.as_slice() < &[6, 1] {
        return Err(io::Error::other("kernel is older than supported Linux 6.1").into());
    }
    Ok(())
}

fn event_evidence(event: &ProbeEvent) -> EventEvidence {
    EventEvidence {
        kind: match event.kind {
            EVENT_CONNECT4 => "connect4",
            EVENT_CONNECT6 => "connect6",
            EVENT_SOCKOPS => "sock_ops",
            EVENT_INGRESS => "ingress",
            EVENT_EGRESS => "egress",
            _ => "unknown",
        },
        family: event.family,
        direction: event.direction,
        socket_cookie: event.socket_cookie,
        cgroup_id: event.cgroup_id,
        pid_tgid: event.pid_tgid,
        local_addr: event.local_addr,
        remote_addr: event.remote_addr,
        local_port: event.local_port,
        remote_port: event.remote_port,
        tcp_sequence: event.tcp_sequence,
        lifecycle_signal: match (event.kind, event.direction, event.state) {
            (EVENT_SOCKOPS, 1 | 2, _) => "established",
            (EVENT_SOCKOPS, 3, _) => "state_changed",
            (EVENT_SOCKOPS, _, _) => "unknown",
            _ => "not_lifecycle",
        },
        previous_state: ((event.flags >> 8) & 0xff) as u8,
        state: event.state,
        observed_len: event.observed_len,
        captured_len: event.captured_len,
        gso_size: event.gso_size,
        gso_segs: event.gso_segs,
        skb_len: event.skb_len,
        linear_len: event.linear_len,
        truncated: event.flags & FLAG_TRUNCATED != 0,
        contains_request: event.kind == EVENT_EGRESS
            && (payload(event).starts_with(b"GET ") || payload(event).starts_with(b"POST ")),
        contains_response: event.kind == EVENT_INGRESS && payload(event).starts_with(b"HTTP/1.1 "),
    }
}

fn payload(event: &ProbeEvent) -> &[u8] {
    let captured_len = usize::try_from(event.captured_len)
        .unwrap_or(PAYLOAD_BYTES)
        .min(PAYLOAD_BYTES);
    &event.payload[..captured_len]
}

fn spawn_direct_members(cgroup: &Path) -> Result<ChildGuard, Box<dyn Error>> {
    let mut children = Vec::new();
    for _ in 0..2 {
        let child = Command::new("sleep").arg("60").spawn()?;
        fs::write(cgroup.join("cgroup.procs"), child.id().to_string())?;
        children.push(child);
    }
    let groups: BTreeSet<_> = children
        .iter()
        .map(|child| process_group(child.id()))
        .collect::<Result<_, _>>()?;
    if groups.len() != 1 {
        return Err(
            io::Error::other("direct TGIDs did not share expected POSIX process group").into(),
        );
    }
    Ok(ChildGuard(children))
}

fn process_group(pid: u32) -> Result<u32, Box<dyn Error>> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let after_name = stat
        .rsplit_once(')')
        .ok_or_else(|| io::Error::other("malformed process stat"))?
        .1;
    after_name
        .split_whitespace()
        .nth(2)
        .ok_or_else(|| io::Error::other("process group missing"))?
        .parse()
        .map_err(Into::into)
}

fn validate_scope(
    cgroup: &CgroupGuard,
    expected_direct_tgids: &BTreeSet<u32>,
) -> Result<(), Box<dyn Error>> {
    if direct_tgids(&cgroup.path)?
        .into_iter()
        .collect::<BTreeSet<_>>()
        != *expected_direct_tgids
    {
        return Err(io::Error::other("selected parent direct TGID set mismatch").into());
    }
    if direct_tgids(&cgroup.workload)? != [std::process::id()] {
        return Err(io::Error::other("descendant workload TGID set mismatch").into());
    }
    if descendant_cgroup_count(&cgroup.path)? != 1 {
        return Err(io::Error::other("selected subtree descendant count mismatch").into());
    }
    let mut exited = Command::new("true").spawn()?;
    let exited_pid = exited.id();
    exited.wait()?;
    if resolve_tgid(exited_pid).is_ok() {
        return Err(io::Error::other("exited PID resolution did not fail closed").into());
    }
    Ok(())
}

fn direct_tgids(cgroup: &Path) -> Result<Vec<u32>, Box<dyn Error>> {
    let mut tgids = BTreeSet::new();
    for line in fs::read_to_string(cgroup.join("cgroup.procs"))?.lines() {
        let pid: u32 = line.parse()?;
        tgids.insert(resolve_tgid(pid)?);
    }
    Ok(tgids.into_iter().collect())
}

fn resolve_tgid(pid: u32) -> Result<u32, Box<dyn Error>> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    status
        .lines()
        .find_map(|line| line.strip_prefix("Tgid:\t"))
        .ok_or_else(|| io::Error::other(format!("Tgid missing for PID {pid}")))?
        .trim()
        .parse()
        .map_err(Into::into)
}

fn descendant_cgroup_count(root: &Path) -> Result<usize, Box<dyn Error>> {
    let mut count = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                count += 1;
                pending.push(entry.path());
            }
        }
    }
    Ok(count)
}

fn current_cgroup_path() -> Result<PathBuf, Box<dyn Error>> {
    let membership = fs::read_to_string("/proc/self/cgroup")?;
    let relative = membership
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .ok_or_else(|| io::Error::other("unified cgroup path unavailable"))?;
    Ok(Path::new("/sys/fs/cgroup").join(relative.trim_start_matches('/')))
}

fn require_supported_host() -> Result<(), Box<dyn Error>> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "privileged feasibility test must run as root",
        )
        .into());
    }
    if !Path::new("/sys/fs/cgroup/cgroup.controllers").is_file() {
        return Err(io::Error::other("cgroup v2 unavailable").into());
    }
    if !Path::new("/sys/kernel/btf/vmlinux").is_file() {
        return Err(io::Error::other("kernel BTF unavailable").into());
    }
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String, Box<dyn Error>> {
    let mut output = String::with_capacity(64);
    for byte in Sha256::digest(fs::read(path)?) {
        std::fmt::Write::write_fmt(&mut output, format_args!("{byte:02x}"))
            .expect("writing to String cannot fail");
    }
    Ok(output)
}

fn feasibility_object() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../ebpf-feasibility/target/bpfel-unknown-none/release/chronicle-ebpf-feasibility")
}

fn report_path() -> PathBuf {
    std::env::var_os("CHRONICLE_FEASIBILITY_REPORT").map_or_else(
        || {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../target/capture-feasibility/feasibility-report.json")
        },
        PathBuf::from,
    )
}

fn offload_features(interface: &str) -> Result<BTreeMap<String, bool>, Box<dyn Error>> {
    let output = command_output("ethtool", &["-k", interface])?;
    let wanted = [
        "generic-segmentation-offload",
        "generic-receive-offload",
        "tcp-segmentation-offload",
    ];
    let features: BTreeMap<_, _> = output
        .lines()
        .filter_map(|line| {
            let (name, value) = line.trim().split_once(':')?;
            wanted
                .contains(&name)
                .then(|| (name.to_owned(), value.trim().starts_with("on")))
        })
        .collect();
    if features.len() != wanted.len() {
        return Err(io::Error::other("loopback offload feature report incomplete").into());
    }
    Ok(features)
}

fn command_output(command: &str, arguments: &[&str]) -> Result<String, Box<dyn Error>> {
    let output = Command::new(command).args(arguments).output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!("{command} failed")).into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn effective_capabilities() -> Result<String, Box<dyn Error>> {
    fs::read_to_string("/proc/self/status")?
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("CapEff unavailable").into())
}
