//! Privileged signal-handling acceptance against the current public record
//! surface: SIGINT/SIGTERM graceful finalization and forced termination on a
//! second signal. Runs the public `record --cgroup PATH --duration 60` form
//! with `--data-dir`; the pre-0.1 one-shot forms were removed.

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

/// The recording metadata file under the public data-directory layout:
/// `recordings/<recording-id>/recording.json`.
fn recording_metadata(data_dir: &Path) -> PathBuf {
    let recordings = data_dir.join("recordings");
    for entry in fs::read_dir(&recordings).expect("read recordings root") {
        let entry = entry.expect("recordings entry");
        if entry.file_type().expect("entry type").is_dir() {
            let metadata = entry.path().join("recording.json");
            if metadata.is_file() {
                return metadata;
            }
        }
    }
    panic!(
        "recording metadata was not created under {}",
        data_dir.display()
    );
}

fn wait_for_metadata(data_dir: &Path) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if fs::read_dir(data_dir.join("recordings"))
            .is_ok_and(|mut entries| entries.next().is_some())
        {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    panic!("recording directory was not created");
}

fn signal(child: &Child, signal: &str) {
    let status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("send signal");
    assert!(status.success(), "kill {signal} failed");
}

fn recording(cgroup: &CgroupGuard, data_dir: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_chronicle"))
        .args([
            "--format",
            "json",
            "--data-dir",
            data_dir.to_str().expect("data dir is UTF-8"),
            "record",
            "--cgroup",
            cgroup.path.to_str().expect("cgroup path is UTF-8"),
            "--duration",
            "60",
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
    let data_dir =
        std::env::temp_dir().join(format!("chronicle-signal-data-{}", uuid::Uuid::new_v4()));
    let chronicle = recording(&cgroup, &data_dir);
    wait_for_metadata(&data_dir);
    signal(&chronicle, signal_name);
    let output = chronicle.wait_with_output().expect("wait chronicle");
    assert!(output.status.success());
    let summary: serde_json::Value = serde_json::from_slice(&output.stdout).expect("summary JSON");
    assert_eq!(summary["status"], "completed");
    assert_eq!(summary["shutdown_reason"], reason);
    assert!(summary["duration_ms"].is_u64());
    assert!(summary["operations"].is_u64());
    assert!(summary["counters"]["committed"].is_object());

    let metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(recording_metadata(&data_dir)).expect("read metadata"))
            .expect("metadata JSON");
    assert_eq!(metadata["status"], "completed");
    assert_eq!(metadata["shutdown_reason"], reason);
    fs::remove_dir_all(data_dir).expect("remove data dir");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8 aarch64"]
fn sigint_finalizes_completed() {
    assert_graceful_signal("-INT", "user_interrupt");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8 aarch64"]
fn sigterm_finalizes_completed() {
    assert_graceful_signal("-TERM", "termination_signal");
}

#[test]
#[ignore = "requires root, cgroup v2, BTF, and Ubuntu 24.04/Linux 6.8 aarch64"]
fn second_signal_forces_termination() {
    // Public contract: a second signal forces immediate termination (130/143)
    // without a finalization marker. The persisted state is reconciled by
    // crash recovery (checkpoint-kill-restart / wal-recovery scenarios); the
    // one-shot runtime that wrote an explicit forced-termination marker was
    // removed before 0.1.0.
    let cgroup = CgroupGuard::create().expect("create dedicated cgroup");
    let data_dir =
        std::env::temp_dir().join(format!("chronicle-forced-data-{}", uuid::Uuid::new_v4()));
    let mut chronicle = recording(&cgroup, &data_dir);
    wait_for_metadata(&data_dir);
    signal(&chronicle, "-INT");
    signal(&chronicle, "-TERM");
    let status = chronicle.wait().expect("wait chronicle");
    assert!(matches!(status.code(), Some(130 | 143)));
    fs::remove_dir_all(data_dir).expect("remove data dir");
}
