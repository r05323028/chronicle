//! Linux cgroup-v2 selector preflight. No attachment occurs here.

use std::collections::BTreeSet;
#[cfg(any(target_os = "linux", all(test, unix)))]
use std::path::Path;
use std::path::PathBuf;
use thiserror::Error;

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const CGROUP_ROOT: &str = "/sys/fs/cgroup";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const PROC_ROOT: &str = "/proc";
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const FORBIDDEN_ROOTS: &[&str] = &["/", "/system.slice", "/user.slice", "/machine.slice"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CgroupSelector {
    Explicit(PathBuf),
    Pid(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CgroupSelection {
    pub canonical_path: PathBuf,
    pub cgroup_id: u64,
    pub direct_tgids: BTreeSet<u32>,
    pub descendant_cgroup_ids: BTreeSet<u64>,
    pub shared_scope_acknowledged: bool,
}

/// PID selection identity captured before attachment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PidCgroupSelection {
    pid: u32,
    selection: CgroupSelection,
}

impl PidCgroupSelection {
    pub fn selection(&self) -> &CgroupSelection {
        &self.selection
    }

    /// Re-resolve immediately before or after attachment. Path or stable ID movement fails.
    pub fn revalidate(&self) -> Result<CgroupSelection, CgroupSelectionError> {
        let observed = preflight_cgroup_selection(CgroupSelector::Pid(self.pid), false)?;
        ensure_same_pid_scope(&self.selection, &observed)?;
        Ok(observed)
    }
}

/// Establish first PID scope sample. Call `revalidate` immediately before and after links attach.
pub fn preflight_pid_cgroup_selection(
    pid: u32,
) -> Result<PidCgroupSelection, CgroupSelectionError> {
    Ok(PidCgroupSelection {
        pid,
        selection: preflight_cgroup_selection(CgroupSelector::Pid(pid), false)?,
    })
}

impl CgroupSelection {
    pub fn direct_tgid_count(&self) -> usize {
        self.direct_tgids.len()
    }

    pub fn descendant_cgroup_count(&self) -> usize {
        self.descendant_cgroup_ids.len()
    }

    pub fn is_shared_scope(&self) -> bool {
        self.direct_tgid_count() > 1 || self.descendant_cgroup_count() > 0
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum CgroupSelectionError {
    #[error("cgroup selection requires Linux")]
    UnsupportedPlatform,
    #[error("cgroup v2 host hierarchy is unavailable")]
    Unavailable,
    #[error("cgroup selector is invalid or outside host hierarchy")]
    InvalidSelector,
    #[error("selected cgroup scope is forbidden")]
    ForbiddenScope,
    #[error("selected cgroup scope cannot be read completely")]
    UnreadableScope,
    #[error("process namespace identity is ambiguous")]
    NamespaceAmbiguous,
    #[error("process changed or exited during cgroup enumeration")]
    EnumerationRace,
    #[error("recorder belongs to selected cgroup subtree")]
    RecorderInsideSelection,
    #[error("selected cgroup is shared; explicit acknowledgement is required")]
    SharedScopeRequiresAcknowledgement,
    #[error("selected PID is not sole direct cgroup TGID")]
    PidScopeMismatch,
    #[error("selected PID changed cgroup scope during attachment")]
    PidScopeChanged,
}

fn ensure_same_pid_scope(
    expected: &CgroupSelection,
    observed: &CgroupSelection,
) -> Result<(), CgroupSelectionError> {
    if expected.canonical_path == observed.canonical_path
        && expected.cgroup_id == observed.cgroup_id
    {
        Ok(())
    } else {
        Err(CgroupSelectionError::PidScopeChanged)
    }
}

/// Resolve selector against host cgroup-v2 and procfs before eBPF attachment.
pub fn preflight_cgroup_selection(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
) -> Result<CgroupSelection, CgroupSelectionError> {
    #[cfg(target_os = "linux")]
    {
        preflight_with_roots(
            selector,
            allow_shared_cgroup,
            Path::new(CGROUP_ROOT),
            Path::new(PROC_ROOT),
        )
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (selector, allow_shared_cgroup);
        Err(CgroupSelectionError::UnsupportedPlatform)
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn preflight_with_roots(
    selector: CgroupSelector,
    allow_shared_cgroup: bool,
    cgroup_root: &Path,
    proc_root: &Path,
) -> Result<CgroupSelection, CgroupSelectionError> {
    use std::fs;

    if fs::read_to_string(cgroup_root.join("cgroup.controllers")).is_err() {
        return Err(CgroupSelectionError::Unavailable);
    }
    let cgroup_root =
        fs::canonicalize(cgroup_root).map_err(|_| CgroupSelectionError::Unavailable)?;
    let (selected_path, selected_tgid, explicit_selector) = match selector {
        CgroupSelector::Explicit(path) => (canonical_cgroup_path(&cgroup_root, &path)?, None, true),
        CgroupSelector::Pid(pid) => {
            let path = cgroup_path_for_pid(proc_root, pid)?;
            (
                canonical_cgroup_path(&cgroup_root, &path)?,
                Some(host_tgid(proc_root, pid)?),
                false,
            )
        }
    };
    let relative = selected_path
        .strip_prefix(&cgroup_root)
        .map_err(|_| CgroupSelectionError::InvalidSelector)?;
    let relative = format!("/{}", relative.display()).replace("//", "/");
    if FORBIDDEN_ROOTS.contains(&relative.as_str()) {
        return Err(CgroupSelectionError::ForbiddenScope);
    }

    let selected_cgroup_id = cgroup_id(&selected_path)?;
    let descendant_cgroup_ids = descendant_ids(&selected_path)?;
    let recorder_pid = std::process::id();
    host_tgid(proc_root, recorder_pid)?;
    let recorder_id = cgroup_id(&canonical_cgroup_path(
        &cgroup_root,
        &cgroup_path_for_pid(proc_root, recorder_pid)?,
    )?)?;
    if recorder_id == selected_cgroup_id || descendant_cgroup_ids.contains(&recorder_id) {
        return Err(CgroupSelectionError::RecorderInsideSelection);
    }

    let direct_tgids = fs::read_to_string(selected_path.join("cgroup.procs"))
        .map_err(|_| CgroupSelectionError::UnreadableScope)?
        .lines()
        .map(|line| {
            line.parse::<u32>()
                .map_err(|_| CgroupSelectionError::UnreadableScope)
        })
        .map(|pid| pid.and_then(|pid| host_tgid(proc_root, pid)))
        .collect::<Result<BTreeSet<_>, _>>()?;

    let selection = CgroupSelection {
        canonical_path: selected_path,
        cgroup_id: selected_cgroup_id,
        direct_tgids,
        descendant_cgroup_ids,
        shared_scope_acknowledged: explicit_selector && allow_shared_cgroup,
    };
    if let Some(tgid) = selected_tgid {
        if selection.direct_tgids != BTreeSet::from([tgid]) {
            return Err(CgroupSelectionError::PidScopeMismatch);
        }
    } else if selection.is_shared_scope() && !allow_shared_cgroup {
        return Err(CgroupSelectionError::SharedScopeRequiresAcknowledgement);
    }
    Ok(selection)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn canonical_cgroup_path(root: &Path, path: &Path) -> Result<PathBuf, CgroupSelectionError> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let path =
        std::fs::canonicalize(candidate).map_err(|_| CgroupSelectionError::InvalidSelector)?;
    if path.starts_with(root) {
        Ok(path)
    } else {
        Err(CgroupSelectionError::InvalidSelector)
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn cgroup_path_for_pid(proc_root: &Path, pid: u32) -> Result<PathBuf, CgroupSelectionError> {
    let cgroup = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup"))
        .map_err(|_| CgroupSelectionError::EnumerationRace)?;
    let mut lines = cgroup.lines();
    let Some(path) = lines.next().and_then(|line| line.strip_prefix("0::")) else {
        return Err(CgroupSelectionError::NamespaceAmbiguous);
    };
    if lines.next().is_some() || !path.starts_with('/') {
        return Err(CgroupSelectionError::NamespaceAmbiguous);
    }
    Ok(PathBuf::from(path.trim_start_matches('/')))
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn host_tgid(proc_root: &Path, pid: u32) -> Result<u32, CgroupSelectionError> {
    let status = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status"))
        .map_err(|_| CgroupSelectionError::EnumerationRace)?;
    let tgid = status
        .lines()
        .find_map(|line| line.strip_prefix("Tgid:\t"))
        .ok_or(CgroupSelectionError::EnumerationRace)?
        .parse()
        .map_err(|_| CgroupSelectionError::EnumerationRace)?;
    let nspid = status
        .lines()
        .find_map(|line| line.strip_prefix("NSpid:\t"))
        .ok_or(CgroupSelectionError::NamespaceAmbiguous)?;
    if nspid.split_whitespace().count() != 1 {
        return Err(CgroupSelectionError::NamespaceAmbiguous);
    }
    Ok(tgid)
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn cgroup_id(path: &Path) -> Result<u64, CgroupSelectionError> {
    use std::os::unix::fs::MetadataExt;
    let id = std::fs::metadata(path)
        .map_err(|_| CgroupSelectionError::UnreadableScope)?
        .ino();
    if id == 0 {
        Err(CgroupSelectionError::UnreadableScope)
    } else {
        Ok(id)
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn descendant_ids(path: &Path) -> Result<BTreeSet<u64>, CgroupSelectionError> {
    let mut descendants = BTreeSet::new();
    let mut pending = vec![path.to_path_buf()];
    while let Some(parent) = pending.pop() {
        for entry in std::fs::read_dir(parent).map_err(|_| CgroupSelectionError::UnreadableScope)? {
            let entry = entry.map_err(|_| CgroupSelectionError::UnreadableScope)?;
            let file_type = entry
                .file_type()
                .map_err(|_| CgroupSelectionError::UnreadableScope)?;
            if file_type.is_symlink() {
                return Err(CgroupSelectionError::UnreadableScope);
            }
            if file_type.is_dir() {
                let child = entry.path();
                descendants.insert(cgroup_id(&child)?);
                pending.push(child);
            }
        }
    }
    Ok(descendants)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;

    struct Host {
        root: PathBuf,
        cgroup: PathBuf,
        proc: PathBuf,
    }

    impl Host {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("chronicle-cgroup-{}", uuid::Uuid::new_v4()));
            let cgroup = root.join("cgroup");
            let proc = root.join("proc");
            fs::create_dir_all(&cgroup).unwrap();
            fs::write(cgroup.join("cgroup.controllers"), "cpu memory").unwrap();
            fs::create_dir_all(&proc).unwrap();
            Self { root, cgroup, proc }
        }

        fn group(&self, relative: &str, pids: &[u32]) -> PathBuf {
            let path = self.cgroup.join(relative.trim_start_matches('/'));
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join("cgroup.procs"),
                pids.iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
            .unwrap();
            path
        }

        fn process(&self, pid: u32, tgid: u32, cgroup: &str) {
            let path = self.proc.join(pid.to_string());
            fs::create_dir_all(&path).unwrap();
            fs::write(
                path.join("status"),
                format!("Tgid:\t{tgid}\nNSpid:\t{pid}\n"),
            )
            .unwrap();
            fs::write(path.join("cgroup"), format!("0::{cgroup}\n")).unwrap();
        }

        fn select(
            &self,
            selector: CgroupSelector,
            ack: bool,
        ) -> Result<CgroupSelection, CgroupSelectionError> {
            preflight_with_roots(selector, ack, &self.cgroup, &self.proc)
        }
    }

    impl Drop for Host {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn test_host() -> Host {
        let host = Host::new();
        host.group("recorder", &[]);
        host.process(std::process::id(), std::process::id(), "/recorder");
        host
    }

    #[test]
    fn deduplicates_threads_and_safe_result_is_count_only() {
        let host = test_host();
        let selected = host.group("workload", &[11, 12, 13]);
        for pid in [11, 12, 13] {
            host.process(pid, 10, "/workload");
        }
        let scope = host
            .select(CgroupSelector::Explicit(selected), false)
            .unwrap();
        assert_eq!(scope.direct_tgids, BTreeSet::from([10]));
        assert_eq!(scope.direct_tgid_count(), 1);
        assert_eq!(scope.descendant_cgroup_count(), 0);
        assert!(!format!("{scope:?}").contains("command"));
    }

    #[test]
    fn shared_posix_group_and_descendants_need_acknowledgement() {
        let host = test_host();
        let selected = host.group("workload", &[11, 12]);
        host.group("workload/child", &[]);
        host.process(11, 10, "/workload");
        host.process(12, 20, "/workload");
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected.clone()), false),
            Err(CgroupSelectionError::SharedScopeRequiresAcknowledgement)
        );
        let scope = host
            .select(CgroupSelector::Explicit(selected), true)
            .unwrap();
        assert_eq!(scope.direct_tgid_count(), 2);
        assert_eq!(scope.descendant_cgroup_count(), 1);
        assert!(scope.shared_scope_acknowledged);
    }

    #[test]
    fn pid_requires_exact_direct_tgid_even_with_acknowledgement() {
        let host = test_host();
        host.group("workload", &[11, 12]);
        host.process(11, 10, "/workload");
        host.process(12, 20, "/workload");
        assert_eq!(
            host.select(CgroupSelector::Pid(11), true),
            Err(CgroupSelectionError::PidScopeMismatch)
        );

        let host = test_host();
        host.group("workload", &[11]);
        host.process(11, 10, "/workload");
        let scope = host.select(CgroupSelector::Pid(11), true).unwrap();
        assert_eq!(scope.direct_tgids, BTreeSet::from([10]));
        assert!(!scope.shared_scope_acknowledged);
    }

    #[test]
    fn pid_scope_change_and_pid_exit_fail_closed() {
        let host = test_host();
        host.group("workload", &[11]);
        host.process(11, 10, "/workload");
        let baseline = host.select(CgroupSelector::Pid(11), false).unwrap();
        host.group("moved", &[11]);
        host.process(11, 10, "/moved");
        let moved = host.select(CgroupSelector::Pid(11), false).unwrap();
        assert_eq!(
            ensure_same_pid_scope(&baseline, &moved),
            Err(CgroupSelectionError::PidScopeChanged)
        );
        fs::remove_dir_all(host.proc.join("11")).unwrap();
        assert_eq!(
            host.select(CgroupSelector::Pid(11), false),
            Err(CgroupSelectionError::EnumerationRace)
        );
    }

    #[test]
    fn descendant_only_and_unreadable_scopes_fail_without_explicit_acknowledgement() {
        let host = test_host();
        let selected = host.group("workload", &[]);
        host.group("workload/child", &[]);
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected.clone()), false),
            Err(CgroupSelectionError::SharedScopeRequiresAcknowledgement)
        );
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected), true)
                .unwrap()
                .descendant_cgroup_count(),
            1
        );

        let host = test_host();
        let selected = host.group("workload", &[]);
        fs::remove_file(selected.join("cgroup.procs")).unwrap();
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected), true),
            Err(CgroupSelectionError::UnreadableScope)
        );
    }

    #[test]
    fn recorder_descendant_race_namespace_and_forbidden_scope_fail_closed() {
        let host = test_host();
        let selected = host.group("workload", &[11]);
        host.group("workload/recorder-child", &[]);
        host.process(
            std::process::id(),
            std::process::id(),
            "/workload/recorder-child",
        );
        host.process(11, 10, "/workload");
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected), true),
            Err(CgroupSelectionError::RecorderInsideSelection)
        );

        let host = test_host();
        let selected = host.group("workload", &[11]);
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected), true),
            Err(CgroupSelectionError::EnumerationRace)
        );

        let host = test_host();
        let selected = host.group("workload", &[11]);
        host.process(11, 10, "/workload");
        fs::write(host.proc.join("11/status"), "Tgid:\t10\nNSpid:\t1 11\n").unwrap();
        assert_eq!(
            host.select(CgroupSelector::Explicit(selected), true),
            Err(CgroupSelectionError::NamespaceAmbiguous)
        );

        for forbidden in ["", "system.slice", "user.slice", "machine.slice"] {
            let host = test_host();
            let selected = if forbidden.is_empty() {
                host.cgroup.clone()
            } else {
                host.group(forbidden, &[])
            };
            assert_eq!(
                host.select(CgroupSelector::Explicit(selected), true),
                Err(CgroupSelectionError::ForbiddenScope)
            );
        }
    }
}
