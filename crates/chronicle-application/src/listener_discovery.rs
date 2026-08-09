//! Stable loopback-listener discovery for command-mode replay.
//!
//! Ownership evidence comes from current cgroup members plus each process's
//! start time and socket FDs. Global `/proc/net/tcp{,6}` tables contribute only
//! socket state/address; namespace tables alone never establish ownership.

use crate::supervised_scope::SupervisedScope;
use chronicle_replay::ReplayPlan;
use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::Path;
use std::time::{Duration, Instant};
use thiserror::Error;

pub const LISTENER_READINESS_DEADLINE: Duration = Duration::from_secs(30);
pub const LISTENER_POLL_INTERVAL: Duration = Duration::from_millis(100);
pub const LISTENER_MAX_UNSTABLE_SNAPSHOTS: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum AddressFamily {
    V4,
    V6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ListenerRequirement {
    port: u16,
    family: Option<AddressFamily>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SocketOwner {
    pid: u32,
    start_time: u64,
    fd: u32,
    inode: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListenerSnapshot {
    members: Vec<u32>,
    owners: Vec<SocketOwner>,
    candidates: Vec<SocketAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredListener {
    pub address: SocketAddr,
    snapshot: ListenerSnapshot,
}

impl DiscoveredListener {
    pub fn origin(&self) -> String {
        match self.address {
            SocketAddr::V4(address) => format!("http://{address}"),
            SocketAddr::V6(address) => {
                format!("http://[{}]:{}", address.ip(), address.port())
            }
        }
    }

    pub fn host(&self) -> String {
        self.address.ip().to_string()
    }
}

#[derive(Debug, Error)]
pub enum ListenerDiscoveryError {
    #[error(
        "target exited before opening a matching loopback listener; use portable --target mode for an already-running application"
    )]
    TargetExited,
    #[error(
        "no matching loopback listener became ready within 30 seconds; bind the target to the recorded port or use portable --target mode"
    )]
    NoMatchingListener,
    #[error(
        "multiple matching loopback listeners found: {0:?}; bind one exact listener or use portable --target mode"
    )]
    Ambiguous(Vec<SocketAddr>),
    #[error(
        "listener ownership evidence changed more than five times; stabilize the target or use portable --target mode"
    )]
    Unstable,
    #[error("listener ownership evidence changed before replay; no traffic was sent")]
    EvidenceChanged,
    #[error("cannot inspect listener ownership evidence: {0}")]
    Evidence(String),
}

impl From<ListenerDiscoveryError> for crate::ApplicationError {
    fn from(error: ListenerDiscoveryError) -> Self {
        Self::ReplayReadiness(error.to_string())
    }
}

pub(crate) trait ListenerClock {
    fn now(&self) -> Instant;
    fn sleep(&self, duration: Duration);
}

pub(crate) struct RealListenerClock;

impl ListenerClock for RealListenerClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

pub(crate) fn listener_requirements(plan: &ReplayPlan) -> BTreeSet<ListenerRequirement> {
    plan.operations()
        .iter()
        .filter(|operation| operation.is_allowed() && operation.protocol().as_str() == "http/1.1")
        .map(|operation| {
            let endpoint = operation.recorded_target();
            ListenerRequirement {
                port: endpoint.port,
                family: parse_family(&endpoint.host),
            }
        })
        .collect()
}

fn parse_family(host: &str) -> Option<AddressFamily> {
    host.trim_matches(['[', ']'])
        .parse::<IpAddr>()
        .ok()
        .map(|address| match address {
            IpAddr::V4(_) => AddressFamily::V4,
            IpAddr::V6(_) => AddressFamily::V6,
        })
}

pub(crate) fn discover_listener(
    scope: &SupervisedScope,
    proc_root: &Path,
    requirements: &BTreeSet<ListenerRequirement>,
    deadline: Instant,
) -> Result<DiscoveredListener, ListenerDiscoveryError> {
    discover_listener_with(
        || read_snapshot(scope, proc_root, requirements),
        &RealListenerClock,
        deadline,
    )
}

fn discover_listener_with(
    mut snapshot: impl FnMut() -> Result<ListenerSnapshot, ListenerDiscoveryError>,
    clock: &dyn ListenerClock,
    deadline: Instant,
) -> Result<DiscoveredListener, ListenerDiscoveryError> {
    let mut unstable = 0;
    loop {
        if clock.now() >= deadline {
            return Err(ListenerDiscoveryError::NoMatchingListener);
        }
        let first = match snapshot() {
            Err(ListenerDiscoveryError::EvidenceChanged) => {
                unstable += 1;
                if unstable > LISTENER_MAX_UNSTABLE_SNAPSHOTS {
                    return Err(ListenerDiscoveryError::Unstable);
                }
                continue;
            }
            other => other?,
        };
        if first.members.is_empty() {
            return Err(ListenerDiscoveryError::TargetExited);
        }
        if first.candidates.is_empty() {
            clock
                .sleep(LISTENER_POLL_INTERVAL.min(deadline.saturating_duration_since(clock.now())));
            continue;
        }
        let second = match snapshot() {
            Err(ListenerDiscoveryError::EvidenceChanged) => {
                unstable += 1;
                if unstable > LISTENER_MAX_UNSTABLE_SNAPSHOTS {
                    return Err(ListenerDiscoveryError::Unstable);
                }
                continue;
            }
            other => other?,
        };
        if first != second {
            unstable += 1;
            if unstable > LISTENER_MAX_UNSTABLE_SNAPSHOTS {
                return Err(ListenerDiscoveryError::Unstable);
            }
            continue;
        }
        if clock.now() >= deadline {
            return Err(ListenerDiscoveryError::NoMatchingListener);
        }
        return match first.candidates.as_slice() {
            [address] => Ok(DiscoveredListener {
                address: *address,
                snapshot: first,
            }),
            candidates => Err(ListenerDiscoveryError::Ambiguous(candidates.to_vec())),
        };
    }
}

pub(crate) fn revalidate_listener(
    scope: &SupervisedScope,
    proc_root: &Path,
    requirements: &BTreeSet<ListenerRequirement>,
    discovered: &DiscoveredListener,
) -> Result<(), ListenerDiscoveryError> {
    let current = read_snapshot(scope, proc_root, requirements)?;
    if current == discovered.snapshot && current.candidates == [discovered.address] {
        Ok(())
    } else {
        Err(ListenerDiscoveryError::EvidenceChanged)
    }
}

fn read_snapshot(
    scope: &SupervisedScope,
    proc_root: &Path,
    requirements: &BTreeSet<ListenerRequirement>,
) -> Result<ListenerSnapshot, ListenerDiscoveryError> {
    scope
        .revalidate()
        .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
    let members = scope
        .members()
        .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
    let mut owners = Vec::new();
    for pid in &members {
        let process = proc_root.join(pid.to_string());
        let start_time = read_start_time(&process.join("stat"))?;
        let entries =
            fs::read_dir(process.join("fd")).map_err(|error| map_evidence_error(&error))?;
        for entry in entries {
            let entry = entry.map_err(|error| map_evidence_error(&error))?;
            let Ok(fd) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            let target = match fs::read_link(entry.path()) {
                Ok(target) => target,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(ListenerDiscoveryError::EvidenceChanged);
                }
                Err(error) => return Err(map_evidence_error(&error)),
            };
            if let Some(inode) = parse_socket_inode(&target.to_string_lossy()) {
                owners.push(SocketOwner {
                    pid: *pid,
                    start_time,
                    fd,
                    inode,
                });
            }
        }
    }
    owners.sort();
    let mut listeners = read_listener_table(&proc_root.join("net/tcp"), AddressFamily::V4)?;
    listeners.extend(read_listener_table(
        &proc_root.join("net/tcp6"),
        AddressFamily::V6,
    )?);
    let owned_inodes: BTreeSet<_> = owners.iter().map(|owner| owner.inode).collect();
    let candidates = listeners
        .into_iter()
        .filter(|(inode, address)| {
            owned_inodes.contains(inode)
                && address.ip().is_loopback()
                && matches_requirement(*address, requirements)
        })
        .map(|(_, address)| address)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(ListenerSnapshot {
        members,
        owners,
        candidates,
    })
}

fn read_start_time(path: &Path) -> Result<u64, ListenerDiscoveryError> {
    let contents = fs::read_to_string(path).map_err(|error| map_evidence_error(&error))?;
    let after_name = contents
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .ok_or_else(|| ListenerDiscoveryError::Evidence("invalid /proc pid stat".into()))?;
    after_name
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| ListenerDiscoveryError::Evidence("missing process start time".into()))?
        .parse::<u64>()
        .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))
}

fn parse_socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

fn read_listener_table(
    path: &Path,
    family: AddressFamily,
) -> Result<Vec<(u64, SocketAddr)>, ListenerDiscoveryError> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(map_evidence_error(&error)),
    };
    contents
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() <= 9 || fields[3] != "0A" {
                return None;
            }
            Some(parse_listener_row(fields[1], fields[9], family))
        })
        .collect()
}

fn parse_listener_row(
    local: &str,
    inode: &str,
    family: AddressFamily,
) -> Result<(u64, SocketAddr), ListenerDiscoveryError> {
    let (address, port) = local
        .split_once(':')
        .ok_or_else(|| ListenerDiscoveryError::Evidence("invalid TCP listener row".into()))?;
    let port = u16::from_str_radix(port, 16)
        .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
    let inode = inode
        .parse::<u64>()
        .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
    let ip = match family {
        AddressFamily::V4 => {
            let value = u32::from_str_radix(address, 16)
                .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
            IpAddr::V4(Ipv4Addr::from(value.to_le_bytes()))
        }
        AddressFamily::V6 => {
            if address.len() != 32 {
                return Err(ListenerDiscoveryError::Evidence(
                    "invalid IPv6 TCP listener row".into(),
                ));
            }
            let mut bytes = [0_u8; 16];
            for (index, chunk) in address.as_bytes().chunks_exact(8).enumerate() {
                let chunk = std::str::from_utf8(chunk)
                    .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
                let value = u32::from_str_radix(chunk, 16)
                    .map_err(|error| ListenerDiscoveryError::Evidence(error.to_string()))?;
                bytes[index * 4..index * 4 + 4].copy_from_slice(&value.to_le_bytes());
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
    };
    Ok((inode, SocketAddr::new(ip, port)))
}

fn matches_requirement(address: SocketAddr, requirements: &BTreeSet<ListenerRequirement>) -> bool {
    let family = match address {
        SocketAddr::V4(_) => AddressFamily::V4,
        SocketAddr::V6(_) => AddressFamily::V6,
    };
    requirements.iter().any(|requirement| {
        requirement.port == address.port()
            && requirement.family.is_none_or(|expected| expected == family)
    })
}

fn map_evidence_error(error: &io::Error) -> ListenerDiscoveryError {
    if error.kind() == io::ErrorKind::NotFound {
        ListenerDiscoveryError::EvidenceChanged
    } else {
        ListenerDiscoveryError::Evidence(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeClock {
        now: Cell<Instant>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                now: Cell::new(Instant::now()),
            }
        }
    }

    impl ListenerClock for FakeClock {
        fn now(&self) -> Instant {
            self.now.get()
        }

        fn sleep(&self, duration: Duration) {
            self.now.set(self.now.get() + duration);
        }
    }

    fn snapshot(candidates: &[SocketAddr]) -> ListenerSnapshot {
        ListenerSnapshot {
            members: vec![10],
            owners: vec![SocketOwner {
                pid: 10,
                start_time: 20,
                fd: 3,
                inode: 30,
            }],
            candidates: candidates.to_vec(),
        }
    }

    #[test]
    fn stable_unique_listener_preserves_exact_family() {
        let clock = FakeClock::new();
        let ipv6: SocketAddr = "[::1]:8080".parse().unwrap();
        let result = discover_listener_with(
            || Ok(snapshot(&[ipv6])),
            &clock,
            clock.now() + Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(result.address, ipv6);
        assert_eq!(result.origin(), "http://[::1]:8080");
    }

    #[test]
    fn absent_listener_stops_at_deadline() {
        let clock = FakeClock::new();
        let error = discover_listener_with(
            || Ok(snapshot(&[])),
            &clock,
            clock.now() + Duration::from_millis(250),
        )
        .unwrap_err();
        assert!(matches!(error, ListenerDiscoveryError::NoMatchingListener));
        assert!(clock.now() <= Instant::now() + Duration::from_secs(1));
    }

    #[test]
    fn stable_ambiguity_fails_without_selection() {
        let clock = FakeClock::new();
        let candidates = [
            "127.0.0.1:8080".parse().unwrap(),
            "127.0.0.1:8081".parse().unwrap(),
        ];
        let error = discover_listener_with(
            || Ok(snapshot(&candidates)),
            &clock,
            clock.now() + Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(matches!(error, ListenerDiscoveryError::Ambiguous(_)));
    }

    #[test]
    fn unstable_evidence_is_bounded() {
        let clock = FakeClock::new();
        let mut calls = 0;
        let error = discover_listener_with(
            || {
                calls += 1;
                let port = 8000 + calls;
                Ok(snapshot(&[format!("127.0.0.1:{port}").parse().unwrap()]))
            },
            &clock,
            clock.now() + Duration::from_secs(30),
        )
        .unwrap_err();
        assert!(matches!(error, ListenerDiscoveryError::Unstable));
        assert!(calls <= (LISTENER_MAX_UNSTABLE_SNAPSHOTS + 1) * 2);
    }

    #[test]
    fn opposite_family_same_port_does_not_match() {
        let requirements = BTreeSet::from([ListenerRequirement {
            port: 8080,
            family: Some(AddressFamily::V6),
        }]);
        assert!(!matches_requirement(
            "127.0.0.1:8080".parse().unwrap(),
            &requirements
        ));
        assert!(matches_requirement(
            "[::1]:8080".parse().unwrap(),
            &requirements
        ));
    }

    #[test]
    fn proc_rows_decode_ipv4_and_ipv6() {
        assert_eq!(
            parse_listener_row("0100007F:1F90", "42", AddressFamily::V4).unwrap(),
            (42, "127.0.0.1:8080".parse().unwrap())
        );
        assert_eq!(
            parse_listener_row(
                "00000000000000000000000001000000:1F90",
                "43",
                AddressFamily::V6
            )
            .unwrap(),
            (43, "[::1]:8080".parse().unwrap())
        );
    }

    #[test]
    fn socket_inode_parser_rejects_non_socket_links() {
        assert_eq!(parse_socket_inode("socket:[123]"), Some(123));
        assert_eq!(parse_socket_inode("pipe:[123]"), None);
    }
}
