#![cfg(target_os = "linux")]

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

struct CgroupGuard {
    path: PathBuf,
    workload: Child,
}

impl CgroupGuard {
    fn create() -> std::io::Result<Self> {
        let path =
            Path::new("/sys/fs/cgroup").join(format!("chronicle-signal-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        let workload = Command::new("sleep").arg("30").spawn()?;
        fs::write(path.join("cgroup.procs"), workload.id().to_string())?;
        Ok(Self { path, workload })
    }
}

impl Drop for CgroupGuard {
    fn drop(&mut self) {
        let _ = self.workload.kill();
        let _ = self.workload.wait();
        let _ = fs::remove_dir(&self.path);
    }
}

fn wait_for_metadata(wal: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if wal.join("recording.json").is_file() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("recording metadata was not created");
}

fn signal(child: &Child, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("send signal");
    assert!(status.success(), "kill {signal} failed");
}

fn recording(cgroup: &CgroupGuard, wal: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_chronicle"))
        .args([
            "--format",
            "json",
            "record",
            "--source",
            "ebpf",
            "--cgroup",
            cgroup.path.to_str().expect("cgroup path is UTF-8"),
            "--wal-dir",
            wal.to_str().expect("WAL path is UTF-8"),
            "--duration-seconds",
            "60",
            "--segment-bytes",
            "16777216",
            "--max-wal-bytes",
            "16777216",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("start chronicle")
}

fn assert_graceful_signal(signal_name: &str, reason: &str) {
    assert!(matches!(std::env::consts::ARCH, "x86_64" | "aarch64"));
    let release = fs::read_to_string("/proc/sys/kernel/osrelease").expect("kernel release");
    let mut parts = release.trim().split('.');
    let major: u32 = parts.next().unwrap().parse().expect("kernel major");
    let minor: u32 = parts
        .next()
        .unwrap()
        .split('-')
        .next()
        .unwrap()
        .parse()
        .expect("kernel minor");
    assert!(major > 6 || (major == 6 && minor >= 1), "kernel {release}");
    let cgroup = CgroupGuard::create().expect("create dedicated cgroup");
    let wal = std::env::temp_dir().join(format!("chronicle-signal-wal-{}", uuid::Uuid::new_v4()));
    let chronicle = recording(&cgroup, &wal);
    wait_for_metadata(&wal);
    signal(&chronicle, signal_name);
    let output = chronicle.wait_with_output().expect("wait chronicle");
    assert!(output.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).expect("summary JSON");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["shutdown_reason"], reason);
    assert!(summary["last_valid_commit"].is_null() || summary["last_valid_commit"].is_object());
    assert!(summary["physical_wal_bytes"].is_u64());
    assert_eq!(summary["direct_tgid_count"], 1);
    assert_eq!(summary["descendant_cgroup_count"], 0);
    assert_eq!(summary["selected_subtree"], true);
    assert!(summary["configured_bounds"].is_object());
    assert!(summary["effective_bounds"].is_object());
    assert!(summary["counters"]["committed"].is_object());

    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(wal.join("recording.json")).expect("read metadata"))
            .expect("metadata JSON");
    assert_eq!(metadata["status"], "completed");
    assert_eq!(metadata["shutdown_reason"], reason);
    fs::remove_dir_all(wal).expect("remove WAL");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn sigint_finalizes_completed() {
    assert_graceful_signal("-INT", "user_interrupt");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn sigterm_finalizes_completed() {
    assert_graceful_signal("-TERM", "termination_signal");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64"]
fn second_signal_persists_forced_termination() {
    let cgroup = CgroupGuard::create().expect("create dedicated cgroup");
    let wal = std::env::temp_dir().join(format!("chronicle-forced-wal-{}", uuid::Uuid::new_v4()));
    let mut chronicle = recording(&cgroup, &wal);
    wait_for_metadata(&wal);
    signal(&chronicle, "-INT");
    signal(&chronicle, "-TERM");
    let status = chronicle.wait().expect("wait chronicle");
    assert!(matches!(status.code(), Some(130 | 143)));

    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(wal.join("recording.json")).expect("read metadata"))
            .expect("metadata JSON");
    assert_eq!(metadata["status"], "aborted");
    assert_eq!(metadata["shutdown_reason"], "forced_termination");
    fs::remove_dir_all(wal).expect("remove WAL");
}
