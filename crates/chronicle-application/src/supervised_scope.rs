//! Linux supervised cgroup-v2 scope for command-mode record/replay.
//!
//! A supervised scope is a cgroup-v2 directory under a user-delegated root.
//! Chronicle creates it, moves the target process in, and owns its lifecycle:
//! TERM, bounded wait, KILL (via `cgroup.kill`), bounded wait-empty, and
//! destroy. Hard bounds: TERM 5 s, KILL wait 5 s, absolute cleanup 15 s.
//! There is no process-group fallback: the cgroup is the only ownership
//! boundary, and non-Linux targets are a typed unsupported error.
//!
//! Access separation is preflighted: no ancestor of the delegated root (up to
//! the cgroup-v2 hierarchy root) may be writable by the invoking credentials,
//! so a dropped-privilege target cannot add unrelated processes to or escape
//! through a writable membership-control ancestor.

use std::fs::{self, DirBuilder, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::DirBuilderExt;

use rustix::process::{Pid, Signal, kill_process};

pub const SCOPE_TERM_GRACE: Duration = Duration::from_secs(5);
pub const SCOPE_KILL_WAIT: Duration = Duration::from_secs(5);
pub const SCOPE_CLEANUP_ABSOLUTE: Duration = Duration::from_secs(15);
pub const SCOPE_POLL_INTERVAL: Duration = Duration::from_millis(20);
const CGROUP_PROCS: &str = "cgroup.procs";
const CGROUP_KILL: &str = "cgroup.kill";
const CGROUP_ID: &str = "cgroup.id";

#[derive(Debug, Error)]
pub enum ScopeError {
    #[error("supervised scope requires Linux")]
    UnsupportedPlatform,
    #[error("cgroup v2 hierarchy is unavailable")]
    NotV2,
    #[error(
        "no writable delegated cgroup root is available for this user; run as root or configure a delegated scope (or use explicit --pid/--cgroup/--target modes)"
    )]
    NotDelegated,
    #[error(
        "cgroup scope access separation is violated: an ancestor of the delegated root is writable by the invoking credentials; the target could escape membership control"
    )]
    AccessSeparation,
    #[error("scope name is unsafe: {0}")]
    UnsafeName(String),
    #[error("scope destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("scope creation failed: {0}")]
    CreateFailed(std::io::Error),
    #[error("cannot read cgroup id: {0}")]
    CgroupIdUnavailable(String),
    #[error("cannot move pid {pid} into scope: {error}")]
    MoveFailed { pid: u32, error: std::io::Error },
    #[error("scope identity changed: expected {expected:?}, observed {observed:?}")]
    IdentityChanged {
        expected: ScopeIdentity,
        observed: ScopeIdentity,
    },
    #[error("scope still populated; cannot destroy {0}")]
    Populated(PathBuf),
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

impl From<ScopeError> for crate::ApplicationError {
    fn from(error: ScopeError) -> Self {
        match error {
            // Live-preflight class: unsupported environment maps to exit 4.
            ScopeError::UnsupportedPlatform => {
                Self::UnsupportedLivePreflight("supervised scope requires Linux")
            }
            ScopeError::NotV2 => {
                Self::UnsupportedLivePreflight("cgroup v2 hierarchy is unavailable")
            }
            ScopeError::NotDelegated => {
                Self::UnsupportedLivePreflight("no writable delegated cgroup root is available")
            }
            ScopeError::AccessSeparation => {
                Self::UnsupportedLivePreflight("cgroup scope access separation is violated")
            }
            ScopeError::UnsafeName(message) | ScopeError::CgroupIdUnavailable(message) => {
                Self::InvalidConfig(message)
            }
            ScopeError::DestinationExists(path) => {
                Self::InvalidConfig(format!("scope destination exists: {}", path.display()))
            }
            ScopeError::CreateFailed(error) | ScopeError::Io(error) => Self::Io(error),
            ScopeError::MoveFailed { pid, error } => {
                Self::InvalidConfig(format!("cannot move pid {pid} into scope: {error}"))
            }
            ScopeError::IdentityChanged { .. } => {
                Self::RecordingMetadataValidation("supervised scope identity changed".into())
            }
            ScopeError::Populated(path) => Self::InvalidConfig(format!(
                "supervised scope still populated: {}",
                path.display()
            )),
        }
    }
}

/// Stable scope identity: canonical cgroup path plus the kernel cgroup id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeIdentity {
    pub canonical_path: PathBuf,
    pub cgroup_id: u64,
}

/// Outcome of a bounded scope cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// Scope emptied on TERM (or was already empty) and was destroyed.
    Clean,
    /// TERM did not empty the scope; KILL did, and the scope was destroyed.
    Killed,
    /// Scope remained populated past the absolute deadline. Remaining pids are
    /// orphan evidence; the scope was not destroyed and is not empty.
    TimedOut { remaining: Vec<u32> },
}

/// Injected time source so deadline transitions are testable without sleeping.
pub trait ScopeClock {
    fn now(&self) -> Instant;
}

/// Production clock.
pub struct RealClock;

impl ScopeClock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// Linux supervised scope. `#[cfg]`-gated: non-Linux callers get a typed
/// [`ScopeError::UnsupportedPlatform`] from the exported constructors.
#[derive(Debug)]
pub struct SupervisedScope {
    path: PathBuf,
    identity: ScopeIdentity,
}

impl SupervisedScope {
    pub fn identity(&self) -> &ScopeIdentity {
        &self.identity
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Re-read the kernel identity and fail if the path or id moved. Called
    /// immediately before capture attachment and before cleanup.
    pub fn revalidate(&self) -> Result<ScopeIdentity, ScopeError> {
        let observed = read_identity(&self.path)?;
        if observed != self.identity {
            return Err(ScopeError::IdentityChanged {
                expected: self.identity.clone(),
                observed,
            });
        }
        Ok(self.identity.clone())
    }

    /// Move a process (or thread) into the scope by writing its pid to
    /// `cgroup.procs`. The kernel re-parents the process; no signal is sent.
    pub fn move_process(&self, pid: u32) -> Result<(), ScopeError> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.path.join(CGROUP_PROCS))
            .map_err(|error| ScopeError::MoveFailed { pid, error })?;
        file.write_all(format!("{pid}\n").as_bytes())
            .map_err(|error| ScopeError::MoveFailed { pid, error })?;
        Ok(())
    }

    /// Current scope members (deduplicated, sorted). A process that exits is
    /// removed from `cgroup.procs` by the kernel.
    pub fn members(&self) -> Result<Vec<u32>, ScopeError> {
        let contents = match fs::read_to_string(self.path.join(CGROUP_PROCS)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
            Ok(contents) => contents,
        };
        let mut members: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for line in contents.lines() {
            if let Ok(pid) = line.trim().parse::<u32>() {
                members.insert(pid);
            }
        }
        Ok(members.into_iter().collect())
    }

    /// Best-effort signal to every current member. Races with process exit are
    /// ignored; the KILL phase uses the authoritative `cgroup.kill` file.
    pub fn signal_all(&self, signal: Signal) -> Result<usize, ScopeError> {
        let members = self.members()?;
        for pid in &members {
            if let Some(pid) = Pid::from_raw(i32::try_from(*pid).unwrap_or(i32::MAX)) {
                let _ = kill_process(pid, signal);
            }
        }
        Ok(members.len())
    }

    /// Kill every member via the cgroup v2 `cgroup.kill` file (authoritative;
    /// opened write-only without create/truncate), falling back to per-pid
    /// SIGKILL only when the file is absent.
    pub fn kill_all(&self) -> Result<(), ScopeError> {
        let kill_path = self.path.join(CGROUP_KILL);
        match OpenOptions::new().write(true).open(&kill_path) {
            Ok(mut file) => {
                file.write_all(b"1")?;
                Ok(())
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                for pid in self.members()? {
                    if let Some(pid) = Pid::from_raw(i32::try_from(pid).unwrap_or(i32::MAX)) {
                        let _ = kill_process(pid, Signal::KILL);
                    }
                }
                Ok(())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Poll `cgroup.procs` until empty or `deadline`. `poll` is the real
    /// sleep between polls; tests pass `Duration::ZERO` with a fake clock.
    pub fn wait_empty_until(
        &self,
        deadline: Instant,
        clock: &dyn ScopeClock,
        poll: Duration,
    ) -> Result<bool, ScopeError> {
        loop {
            if self.members()?.is_empty() {
                return Ok(true);
            }
            if clock.now() >= deadline {
                return Ok(false);
            }
            if !poll.is_zero() {
                std::thread::sleep(poll);
            }
        }
    }

    /// Destroy the scope directory. Only an empty scope may be destroyed; a
    /// populated scope is orphan evidence and fails with [`ScopeError::Populated`].
    /// Real cgroup-v2 directories remove cleanly once empty; filesystem-backed
    /// hosts (tests) leave helper files behind, which are removed first.
    pub fn destroy(&self) -> Result<(), ScopeError> {
        if !self.members()?.is_empty() {
            return Err(ScopeError::Populated(self.path.clone()));
        }
        match fs::remove_dir(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                for name in [CGROUP_PROCS, CGROUP_KILL, CGROUP_ID] {
                    let _ = fs::remove_file(self.path.join(name));
                }
                match fs::remove_dir(&self.path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {
                        Err(ScopeError::Populated(self.path.clone()))
                    }
                    Err(error) => Err(error.into()),
                }
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Bounded cleanup: TERM, wait up to 5 s, KILL, wait up to 5 s, destroy.
    /// The absolute deadline is 15 s from start; a scope that is still
    /// populated at the deadline returns [`CleanupOutcome::TimedOut`] with
    /// orphan pids and is left in place (never claimed clean).
    pub fn cleanup(
        &self,
        clock: &dyn ScopeClock,
        poll: Duration,
    ) -> Result<CleanupOutcome, ScopeError> {
        let start = clock.now();
        let absolute = start + SCOPE_CLEANUP_ABSOLUTE;
        let term_deadline = start + SCOPE_TERM_GRACE;
        self.signal_all(Signal::TERM)?;
        if self.wait_empty_until(term_deadline, clock, poll)? {
            self.destroy()?;
            return Ok(CleanupOutcome::Clean);
        }
        if clock.now() >= absolute {
            return self.timed_out_outcome();
        }
        self.kill_all()?;
        let kill_deadline = (start + SCOPE_TERM_GRACE + SCOPE_KILL_WAIT).min(absolute);
        if self.wait_empty_until(kill_deadline, clock, poll)? {
            self.destroy()?;
            return Ok(CleanupOutcome::Killed);
        }
        self.timed_out_outcome()
    }

    fn timed_out_outcome(&self) -> Result<CleanupOutcome, ScopeError> {
        Ok(CleanupOutcome::TimedOut {
            remaining: self.members()?,
        })
    }
}

fn read_identity(path: &Path) -> Result<ScopeIdentity, ScopeError> {
    let canonical_path = fs::canonicalize(path)?;
    let contents = fs::read_to_string(path.join(CGROUP_ID))
        .map_err(|error| ScopeError::CgroupIdUnavailable(error.to_string()))?;
    let cgroup_id = contents
        .trim()
        .parse::<u64>()
        .map_err(|error| ScopeError::CgroupIdUnavailable(error.to_string()))?;
    Ok(ScopeIdentity {
        canonical_path,
        cgroup_id,
    })
}

fn validate_scope_name(name: &str) -> Result<(), ScopeError> {
    if name.is_empty()
        || name.len() > 64
        || name.starts_with('.')
        || name
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte == b'/')
    {
        return Err(ScopeError::UnsafeName(name.to_owned()));
    }
    if name == "." || name == ".." {
        return Err(ScopeError::UnsafeName(name.to_owned()));
    }
    Ok(())
}

/// Is `path` writable by `uid` under a conservative mode-based check? Group
/// and other write bits are treated as writable because uid→gid membership
/// cannot be resolved without process context.
fn writable_by(metadata: &fs::Metadata, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let mode = metadata.mode();
    if metadata.uid() == uid {
        mode & 0o200 != 0
    } else {
        mode & 0o022 != 0
    }
}

/// Is the ancestor's `cgroup.procs` membership-control file writable by `uid`?
/// The file is the actual write vector for adding processes to or escaping a
/// scope, so directory modes alone are insufficient evidence of separation.
fn membership_control_writable(ancestor: &Path, uid: u32) -> Result<bool, ScopeError> {
    let procs = ancestor.join(CGROUP_PROCS);
    match fs::symlink_metadata(&procs) {
        Ok(metadata) if !metadata.file_type().is_symlink() => Ok(writable_by(&metadata, uid)),
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

/// Access-separation preflight: the delegated root must be writable by `uid`,
/// and every ancestor up to and including `hierarchy_root` must not be. This
/// guarantees the dropped-privilege target cannot reach membership control of
/// an ancestor scope.
pub fn preflight_scope_access(
    hierarchy_root: &Path,
    delegated_root: &Path,
    uid: u32,
) -> Result<(), ScopeError> {
    if !hierarchy_root.is_absolute() || !delegated_root.is_absolute() {
        return Err(ScopeError::UnsafeName(delegated_root.display().to_string()));
    }
    if !delegated_root.starts_with(hierarchy_root) {
        return Err(ScopeError::NotDelegated);
    }
    if hierarchy_root
        .components()
        .any(|component| !matches!(component, Component::RootDir | Component::Normal(_)))
    {
        return Err(ScopeError::UnsafeName(hierarchy_root.display().to_string()));
    }
    let metadata = fs::symlink_metadata(delegated_root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(ScopeError::NotDelegated);
    }
    if !writable_by(&metadata, uid) {
        return Err(ScopeError::NotDelegated);
    }
    // Every ancestor up to and including the hierarchy root must be
    // non-writable by the invoking credentials — both the directory itself and
    // its cgroup.procs membership-control file (the actual write vector for
    // adding or escaping processes).
    let mut ancestor = delegated_root.parent();
    while let Some(path) = ancestor {
        let metadata = fs::symlink_metadata(path)?;
        if writable_by(&metadata, uid) || membership_control_writable(path, uid)? {
            return Err(ScopeError::AccessSeparation);
        }
        if path == hierarchy_root {
            return Ok(());
        }
        ancestor = path.parent();
    }
    // Delegated root was inside hierarchy_root, so the walk must have hit it.
    Err(ScopeError::NotV2)
}

/// Create (or open) a supervised scope named `name` under `delegated_root`.
/// Preflight must pass first; the directory is created privately (0o700).
pub fn create_supervised_scope(
    hierarchy_root: &Path,
    delegated_root: &Path,
    name: &str,
    uid: u32,
) -> Result<SupervisedScope, ScopeError> {
    preflight_scope_access(hierarchy_root, delegated_root, uid)?;
    validate_scope_name(name)?;
    let path = delegated_root.join(name);
    match DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(ScopeError::DestinationExists(path));
        }
        Err(error) => return Err(ScopeError::CreateFailed(error)),
    }
    let identity = match read_identity(&path) {
        Ok(identity) => identity,
        Err(error) => {
            // Roll back the directory we just created: a scope without a
            // kernel identity is not a usable scope.
            let _ = fs::remove_dir(&path);
            return Err(error);
        }
    };
    Ok(SupervisedScope { path, identity })
}

/// Open an existing scope directory (used by the fake-kernel test host).
pub fn open_scope(path: &Path) -> Result<SupervisedScope, ScopeError> {
    Ok(SupervisedScope {
        identity: read_identity(path)?,
        path: path.to_path_buf(),
    })
}

/// Discover the delegated cgroup root for the current process: walk up from
/// `/proc/self/cgroup` to the first directory writable by `uid`. Root
/// privileges use the hierarchy root itself. Ancestor safety is validated by
/// [`preflight_scope_access`].
#[cfg(target_os = "linux")]
pub fn discover_delegated_root(uid: u32) -> Result<(PathBuf, PathBuf), ScopeError> {
    use std::os::unix::fs::MetadataExt;
    let hierarchy = PathBuf::from("/sys/fs/cgroup");
    if !hierarchy.is_dir() {
        return Err(ScopeError::NotV2);
    }
    if uid == 0 {
        return Ok((hierarchy.clone(), hierarchy));
    }
    let contents = fs::read_to_string("/proc/self/cgroup").map_err(|_| ScopeError::NotV2)?;
    let current = contents
        .lines()
        .find_map(|line| {
            let (controllers, path) = line.split_once(':')?;
            if controllers.is_empty() {
                Some(path.trim_start_matches('/'))
            } else {
                None
            }
        })
        .ok_or(ScopeError::NotV2)?;
    let mut candidate = hierarchy.join(current);
    loop {
        if fs::symlink_metadata(&candidate).is_ok_and(|metadata| writable_by(&metadata, uid)) {
            return Ok((hierarchy, candidate));
        }
        if !candidate.pop() {
            break;
        }
        if candidate == hierarchy {
            break;
        }
    }
    Err(ScopeError::NotDelegated)
}

// Non-Linux: typed unsupported — there is no /proc cgroup hierarchy to walk.
#[cfg(not(target_os = "linux"))]
pub fn discover_delegated_root(_uid: u32) -> Result<(PathBuf, PathBuf), ScopeError> {
    Err(ScopeError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Mutex;

    /// Deterministic clock: each `now()` advances by `step`, so deadline
    /// transitions fire after a fixed number of polls.
    struct FakeClock {
        now: Mutex<Instant>,
        step: Duration,
    }

    impl FakeClock {
        fn new(step: Duration) -> Self {
            Self {
                now: Mutex::new(Instant::now()),
                step,
            }
        }
    }

    impl ScopeClock for FakeClock {
        fn now(&self) -> Instant {
            let mut guard = self.now.lock().unwrap();
            let current = *guard;
            *guard = current + self.step;
            current
        }
    }

    struct FakeHost {
        hierarchy: PathBuf,
        delegated: PathBuf,
        resistant: std::sync::Arc<std::sync::atomic::AtomicBool>,
        watchers: Vec<(
            std::sync::Arc<std::sync::atomic::AtomicBool>,
            std::thread::JoinHandle<()>,
        )>,
    }

    impl FakeHost {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("chronicle-scope-{}", uuid::Uuid::new_v4()));
            let hierarchy = root.join("cgroup");
            let delegated = hierarchy.join("delegated");
            fs::create_dir_all(&delegated).unwrap();
            // Hierarchy root not writable by anyone (simulating root-owned).
            fs::set_permissions(&hierarchy, fs::Permissions::from_mode(0o555)).unwrap();
            // Delegated root writable by anyone (simulating user chown).
            fs::set_permissions(&delegated, fs::Permissions::from_mode(0o777)).unwrap();
            Self {
                hierarchy,
                delegated,
                resistant: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                watchers: Vec::new(),
            }
        }

        fn kernel_scope(&mut self, name: &str, id: u64) -> PathBuf {
            let path = self.delegated.join(name);
            fs::create_dir_all(&path).unwrap();
            fs::write(path.join(CGROUP_ID), id.to_string()).unwrap();
            fs::write(path.join(CGROUP_PROCS), "").unwrap();
            fs::write(path.join(CGROUP_KILL), "").unwrap();
            // Fake kernel: watch the kill file; when written, kill members by
            // clearing cgroup.procs unless the host is resistant.
            let procs = path.join(CGROUP_PROCS);
            let kill = path.join(CGROUP_KILL);
            let resistant = self.resistant.clone();
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let stop_handle = stop.clone();
            let watcher = std::thread::spawn(move || {
                // A real kernel kills exactly once; clearing at most once
                // avoids recreating cgroup.procs while destroy() removes it.
                let mut cleared = false;
                while !stop_handle.load(std::sync::atomic::Ordering::Relaxed) {
                    if !cleared
                        && fs::read_to_string(&kill).is_ok_and(|contents| contents.trim() == "1")
                        && !resistant.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        cleared = true;
                        let _ = fs::write(&procs, "");
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
            });
            self.watchers.push((stop, watcher));
            path
        }

        fn populate(scope: &Path, pids: &[u32]) {
            fs::write(
                scope.join(CGROUP_PROCS),
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
        }

        fn set_resistant(&mut self, resistant: bool) {
            self.resistant
                .store(resistant, std::sync::atomic::Ordering::Relaxed);
        }
    }

    impl Drop for FakeHost {
        fn drop(&mut self) {
            for (stop, handle) in self.watchers.drain(..) {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
                let _ = handle.join();
            }
            let _ = fs::set_permissions(&self.hierarchy, fs::Permissions::from_mode(0o755));
            let _ = fs::remove_dir_all(self.hierarchy.parent().unwrap());
        }
    }

    fn test_uid() -> u32 {
        use std::os::unix::fs::MetadataExt;
        fs::metadata("/").unwrap().uid() + 1
    }

    #[test]
    fn create_and_open_scope_roundtrip() {
        let mut host = FakeHost::new();
        // Pre-made kernel scope opens with a stable identity.
        let path = host.kernel_scope("app", 42);
        let scope = open_scope(&path).unwrap();
        assert_eq!(scope.identity().cgroup_id, 42);
        assert!(scope.identity().canonical_path.is_absolute());
        assert!(scope.members().unwrap().is_empty());
        scope.destroy().unwrap();
        assert!(!path.exists());

        // create_supervised_scope on a pre-made scope is rejected as existing.
        let path = host.kernel_scope("app2", 7);
        assert!(path.is_dir());
        let err = create_supervised_scope(&host.hierarchy, &host.delegated, "app2", test_uid())
            .unwrap_err();
        assert!(matches!(err, ScopeError::DestinationExists(_)));

        // create_supervised_scope creates the dir, fails on the missing
        // kernel cgroup.id, and rolls the directory back.
        let err = create_supervised_scope(&host.hierarchy, &host.delegated, "fresh", test_uid())
            .unwrap_err();
        assert!(matches!(err, ScopeError::CgroupIdUnavailable(_)));
        assert!(!host.delegated.join("fresh").exists());
    }

    #[test]
    fn move_process_and_members_roundtrip() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 3);
        let scope = open_scope(&path).unwrap();
        scope.move_process(999_999).unwrap();
        scope.move_process(999_998).unwrap();
        scope.move_process(999_999).unwrap();
        // Deduplicated and sorted.
        assert_eq!(scope.members().unwrap(), vec![999_998, 999_999]);
    }

    #[test]
    fn cleanup_clean_when_scope_already_empty() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 1);
        let scope = open_scope(&path).unwrap();
        let clock = FakeClock::new(Duration::from_millis(100));
        let outcome = scope.cleanup(&clock, Duration::ZERO).unwrap();
        assert_eq!(outcome, CleanupOutcome::Clean);
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_escalates_term_to_kill_and_destroys() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 2);
        let scope = open_scope(&path).unwrap();
        FakeHost::populate(&path, &[999_999]);
        let clock = FakeClock::new(Duration::from_millis(100));
        // TERM is sent to the (fake) member and ignored; the scope stays
        // populated past the 5s term deadline, so cleanup escalates: the fake
        // kernel reacts to cgroup.kill by clearing members, wait-empty
        // resolves, and the scope is destroyed. A 1 ms poll lets the fake
        // kernel watcher react without racing the busy poll loop.
        let outcome = scope.cleanup(&clock, Duration::from_millis(1)).unwrap();
        assert_eq!(outcome, CleanupOutcome::Killed);
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_timed_out_reports_orphan_evidence_and_leaves_scope() {
        let mut host = FakeHost::new();
        host.set_resistant(true);
        let path = host.kernel_scope("app", 4);
        let scope = open_scope(&path).unwrap();
        FakeHost::populate(&path, &[999_999]);
        let clock = FakeClock::new(Duration::from_millis(100));
        let outcome = scope.cleanup(&clock, Duration::ZERO).unwrap();
        match outcome {
            CleanupOutcome::TimedOut { remaining } => assert_eq!(remaining, vec![999_999]),
            other => panic!("expected TimedOut, got {other:?}"),
        }
        // Scope is left in place and populated: orphan evidence, never claimed clean.
        assert!(path.is_dir());
        assert_eq!(scope.members().unwrap(), vec![999_999]);
    }

    #[test]
    fn access_separation_rejects_writable_ancestor() {
        let host = FakeHost::new();
        fs::set_permissions(&host.hierarchy, fs::Permissions::from_mode(0o777)).unwrap();
        let err = preflight_scope_access(&host.hierarchy, &host.delegated, test_uid()).unwrap_err();
        assert!(matches!(err, ScopeError::AccessSeparation));
        fs::set_permissions(&host.hierarchy, fs::Permissions::from_mode(0o555)).unwrap();
    }

    #[test]
    fn access_separation_rejects_writable_membership_control_file() {
        let host = FakeHost::new();
        // The hierarchy directory itself is not writable, but its cgroup.procs
        // membership-control file is: the target could add processes to or
        // escape through the ancestor scope.
        fs::set_permissions(&host.hierarchy, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(host.hierarchy.join(CGROUP_PROCS), "1\n").unwrap();
        fs::set_permissions(
            host.hierarchy.join(CGROUP_PROCS),
            fs::Permissions::from_mode(0o666),
        )
        .unwrap();
        fs::set_permissions(&host.hierarchy, fs::Permissions::from_mode(0o555)).unwrap();
        let err = preflight_scope_access(&host.hierarchy, &host.delegated, test_uid()).unwrap_err();
        assert!(matches!(err, ScopeError::AccessSeparation));
        fs::set_permissions(&host.hierarchy, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_file(host.hierarchy.join(CGROUP_PROCS));
    }

    #[test]
    fn not_delegated_root_rejected() {
        let host = FakeHost::new();
        fs::set_permissions(&host.delegated, fs::Permissions::from_mode(0o555)).unwrap();
        let err = preflight_scope_access(&host.hierarchy, &host.delegated, test_uid()).unwrap_err();
        assert!(matches!(err, ScopeError::NotDelegated));
        fs::set_permissions(&host.delegated, fs::Permissions::from_mode(0o777)).unwrap();
    }

    #[test]
    fn unsafe_scope_names_rejected() {
        let host = FakeHost::new();
        for name in ["", "a/b", "..", ".", ".hidden", "x\x1by", &"a".repeat(65)] {
            let err = create_supervised_scope(&host.hierarchy, &host.delegated, name, test_uid())
                .unwrap_err();
            assert!(matches!(err, ScopeError::UnsafeName(_)), "name {name:?}");
        }
    }

    #[test]
    fn identity_change_detected() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 9);
        let scope = open_scope(&path).unwrap();
        assert_eq!(scope.revalidate().unwrap().cgroup_id, 9);
        // Kernel id changed (scope replaced).
        fs::write(path.join(CGROUP_ID), "10").unwrap();
        let err = scope.revalidate().unwrap_err();
        assert!(matches!(err, ScopeError::IdentityChanged { .. }));
    }

    #[test]
    fn destroy_rejects_populated_scope() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 11);
        let scope = open_scope(&path).unwrap();
        FakeHost::populate(&path, &[999_999]);
        assert!(matches!(scope.destroy(), Err(ScopeError::Populated(_))));
        assert!(path.is_dir());
    }

    #[test]
    fn wait_empty_respects_deadline_transitions() {
        let mut host = FakeHost::new();
        let path = host.kernel_scope("app", 12);
        let scope = open_scope(&path).unwrap();
        FakeHost::populate(&path, &[999_999]);
        // Deadline 500ms away with 100ms auto-advance steps: empty is never
        // reached, so it must stop at the deadline.
        let clock = FakeClock::new(Duration::from_millis(100));
        let deadline = clock.now() + Duration::from_millis(500);
        assert!(
            !scope
                .wait_empty_until(deadline, &clock, Duration::ZERO)
                .unwrap()
        );
        // Once the file is cleared, the next wait resolves immediately.
        FakeHost::populate(&path, &[]);
        let deadline = clock.now() + Duration::from_millis(500);
        assert!(
            scope
                .wait_empty_until(deadline, &clock, Duration::ZERO)
                .unwrap()
        );
    }
}
