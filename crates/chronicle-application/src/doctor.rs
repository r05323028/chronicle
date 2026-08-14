//! Non-destructive doctor probes and actionable remediation.

use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DoctorStatus {
    Supported,
    SupportedWithWarnings,
    Unsupported,
    NotChecked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorProbe {
    pub required: bool,
    pub status: DoctorStatus,
    pub code: String,
    pub message: String,
    pub remediation: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DoctorReport {
    pub version: u16,
    pub status: DoctorStatus,
    pub probes: Vec<DoctorProbe>,
}

impl DoctorReport {
    pub fn new(probes: Vec<DoctorProbe>) -> Self {
        let status = aggregate_doctor_status(&probes);
        Self {
            version: DOCTOR_REPORT_VERSION,
            status,
            probes,
        }
    }

    pub const fn exit_code(&self) -> i32 {
        doctor_exit_code(self.status)
    }
}

pub fn aggregate_doctor_status(probes: &[DoctorProbe]) -> DoctorStatus {
    if probes
        .iter()
        .any(|probe| probe.required && probe.status == DoctorStatus::Unsupported)
    {
        DoctorStatus::Unsupported
    } else if probes
        .iter()
        .any(|probe| probe.required && probe.status == DoctorStatus::NotChecked)
    {
        DoctorStatus::NotChecked
    } else if probes.iter().any(|probe| {
        probe.status == DoctorStatus::SupportedWithWarnings
            || (!probe.required
                && matches!(
                    probe.status,
                    DoctorStatus::Unsupported | DoctorStatus::NotChecked
                ))
    }) {
        DoctorStatus::SupportedWithWarnings
    } else {
        DoctorStatus::Supported
    }
}

pub const fn doctor_exit_code(status: DoctorStatus) -> i32 {
    match status {
        DoctorStatus::Unsupported | DoctorStatus::NotChecked => 4,
        DoctorStatus::Supported | DoctorStatus::SupportedWithWarnings => 0,
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DoctorOptions {
    pub wal_dir: Option<PathBuf>,
    pub output: Option<PathBuf>,
    pub state_root: Option<PathBuf>,
    pub config: Option<PathBuf>,
}

/// Runs only local, non-attaching probes. Supplied paths are inspected but never created.
pub fn doctor_report(options: &DoctorOptions) -> DoctorReport {
    let mut probes = capture_doctor_probes();
    probes.extend([
        path_doctor_probe(options.wal_dir.as_deref(), "wal_dir", "WAL directory"),
        path_doctor_probe(options.output.as_deref(), "output", "output path"),
        path_doctor_probe(
            options.state_root.as_deref(),
            "state_root",
            "recorder state root",
        ),
        recorder_metadata_doctor_probe(options.state_root.as_deref()),
        recorder_owner_doctor_probe(options.state_root.as_deref()),
        recorder_config_doctor_probe(options.config.as_deref()),
        wal_manifest_doctor_probe(options.wal_dir.as_deref()),
        etl_checkpoint_doctor_probe(options.wal_dir.as_deref()),
        protocol_doctor_probe(),
        DoctorProbe {
            required: false,
            status: DoctorStatus::Supported,
            code: "replay.target_not_checked".into(),
            message: "replay target is command-specific and was not contacted".into(),
            remediation: "supply --target to replay; doctor never contacts targets".into(),
        },
    ]);
    DoctorReport::new(probes)
}

/// Validate replay-related application configuration without contacting a target
/// or exposing credential values.
pub fn replay_config_doctor_probe(config: &ReplayConfig) -> DoctorProbe {
    if let Some(environment) = config.authorization_env.as_deref() {
        let valid_name = !environment.is_empty()
            && environment.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            });
        if !valid_name {
            return DoctorProbe {
                required: false,
                status: DoctorStatus::Unsupported,
                code: "replay.configuration".into(),
                message: "replay authorization environment-variable name is invalid".into(),
                remediation: "set replay.authorization_env to a valid environment-variable name"
                    .into(),
            };
        }
        if std::env::var_os(environment).is_none() {
            return DoctorProbe {
                required: false,
                status: DoctorStatus::NotChecked,
                code: "replay.configuration".into(),
                message: "configured replay authorization environment variable is not set".into(),
                remediation: "set the configured authorization environment variable before authenticated replay"
                    .into(),
            };
        }
    }
    if !config.dry_run
        || config.allow_reads
        || config.allow_writes
        || config.allow_publication
        || !config.target_mappings.is_empty()
    {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::SupportedWithWarnings,
            code: "replay.configuration".into(),
            message: "configuration contains replay target or authorization grants that public replay ignores"
                .into(),
            remediation: "use explicit replay --target, --execute, --allow-host, and effect gates"
                .into(),
        };
    }
    DoctorProbe {
        required: false,
        status: DoctorStatus::Supported,
        code: "replay.configuration".into(),
        message: "replay configuration preserves deny-by-default behavior".into(),
        remediation: "none".into(),
    }
}

fn recorder_metadata_doctor_probe(path: Option<&Path>) -> DoctorProbe {
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "recorder.metadata".into(),
            message: "recorder state root was not supplied".into(),
            remediation: "supply --state-root to inspect recorder metadata".into(),
        };
    };
    match load_recorder_metadata(path) {
        Ok(_) => DoctorProbe {
            required: true,
            status: DoctorStatus::Supported,
            code: "recorder.metadata".into(),
            message: "recorder metadata and checksum are valid".into(),
            remediation: "none".into(),
        },
        Err(_) if !path.join(RECORDER_METADATA_FILE).exists() => DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "recorder.metadata".into(),
            message: "recorder metadata is not present".into(),
            remediation: "start recorder before checking active metadata".into(),
        },
        Err(error) => DoctorProbe {
            required: true,
            status: DoctorStatus::Unsupported,
            code: "recorder.metadata".into(),
            message: "recorder metadata is invalid".into(),
            remediation: format!("recover or quarantine recorder metadata ({error})"),
        },
    }
}

fn recorder_owner_doctor_probe(path: Option<&Path>) -> DoctorProbe {
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "recorder.owner".into(),
            message: "recorder state root was not supplied".into(),
            remediation: "supply --state-root to inspect recorder ownership".into(),
        };
    };
    match RecorderLease::state_is_owned(path) {
        Ok(true) => DoctorProbe {
            required: false,
            status: DoctorStatus::SupportedWithWarnings,
            code: "recorder.owner".into(),
            message: "recorder state lease is currently held".into(),
            remediation: "do not mutate recorder state outside the owner".into(),
        },
        Ok(false) => DoctorProbe {
            required: false,
            status: DoctorStatus::Supported,
            code: "recorder.owner".into(),
            message: "recorder state lease is available".into(),
            remediation: "none".into(),
        },
        Err(error) => DoctorProbe {
            required: true,
            status: DoctorStatus::Unsupported,
            code: "recorder.owner".into(),
            message: "recorder state lease could not be inspected".into(),
            remediation: error.to_string(),
        },
    }
}

fn recorder_config_doctor_probe(path: Option<&Path>) -> DoctorProbe {
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "recorder.config".into(),
            message: "recorder configuration was not supplied".into(),
            remediation: "supply --config to validate recorder configuration".into(),
        };
    };
    match RecorderConfigV1::from_file(path).and_then(|config| config.normalize()) {
        Ok(config) => DoctorProbe {
            required: true,
            status: DoctorStatus::Supported,
            code: "recorder.config".into(),
            message: format!(
                "recorder configuration validated: {}",
                config.stable_digest().unwrap_or_default()
            ),
            remediation: "none".into(),
        },
        Err(error) => DoctorProbe {
            required: true,
            status: DoctorStatus::Unsupported,
            code: "recorder.config".into(),
            message: "recorder configuration is invalid".into(),
            remediation: error.to_string(),
        },
    }
}

fn wal_manifest_doctor_probe(path: Option<&Path>) -> DoctorProbe {
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "wal.manifest".into(),
            message: "WAL path was not supplied".into(),
            remediation: "supply --wal-dir".into(),
        };
    };
    match chronicle_wal::load_manifest(path) {
        Ok(_) => DoctorProbe {
            required: true,
            status: DoctorStatus::Supported,
            code: "wal.manifest".into(),
            message: "WAL manifest validates".into(),
            remediation: "none".into(),
        },
        Err(_error) if !path.join(chronicle_wal::WAL_MANIFEST_FILE).exists() => DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "wal.manifest".into(),
            message: "WAL manifest is not present".into(),
            remediation: "start or recover recorder".into(),
        },
        Err(error) => DoctorProbe {
            required: true,
            status: DoctorStatus::Unsupported,
            code: "wal.manifest".into(),
            message: "WAL manifest is invalid".into(),
            remediation: error.to_string(),
        },
    }
}

fn etl_checkpoint_doctor_probe(path: Option<&Path>) -> DoctorProbe {
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "etl.checkpoint".into(),
            message: "WAL path was not supplied".into(),
            remediation: "supply --wal-dir".into(),
        };
    };
    let checkpoint = path.join("etl-checkpoint.json");
    match chronicle_etl::read_checkpoint(&checkpoint) {
        Ok(_) => DoctorProbe {
            required: true,
            status: DoctorStatus::Supported,
            code: "etl.checkpoint".into(),
            message: "incremental ETL checkpoint validates".into(),
            remediation: "none".into(),
        },
        Err(_) if !checkpoint.exists() => DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code: "etl.checkpoint".into(),
            message: "incremental ETL checkpoint is not present".into(),
            remediation: "run recorder ETL before checking checkpoint".into(),
        },
        Err(error) => DoctorProbe {
            required: true,
            status: DoctorStatus::Unsupported,
            code: "etl.checkpoint".into(),
            message: "incremental ETL checkpoint is invalid".into(),
            remediation: error.to_string(),
        },
    }
}

fn protocol_doctor_probe() -> DoctorProbe {
    let status = if chronicle_protocol_builtins::registry().is_ok() {
        DoctorStatus::Supported
    } else {
        DoctorStatus::Unsupported
    };
    DoctorProbe {
        required: false,
        status,
        code: "replay.protocol_capabilities".into(),
        message: "registered replay protocol capabilities inspected without network access".into(),
        remediation: "build with supported protocol adapters".into(),
    }
}

fn path_doctor_probe(path: Option<&Path>, name: &str, label: &str) -> DoctorProbe {
    let code = format!("storage.{name}");
    let Some(path) = path else {
        return DoctorProbe {
            required: false,
            status: DoctorStatus::NotChecked,
            code,
            message: format!("{label} was not supplied"),
            remediation: format!("supply --{} to check this path", name.replace('_', "-")),
        };
    };

    let directory = match fs::metadata(path) {
        Ok(metadata) => metadata.is_dir().then_some(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().unwrap_or_else(|| Path::new("."));
            let parent = if parent.as_os_str().is_empty() {
                Path::new(".")
            } else {
                parent
            };
            fs::metadata(parent)
                .ok()
                .filter(std::fs::Metadata::is_dir)
                .map(|_| parent)
        }
        Err(_) => None,
    };

    let status = directory.map_or(DoctorStatus::Unsupported, |directory| {
        let Ok(stats) = rustix::fs::statvfs(directory) else {
            return DoctorStatus::Unsupported;
        };
        let available_bytes = stats.f_bavail.saturating_mul(stats.f_frsize);
        if available_bytes == 0 || !probe_directory_io(directory) {
            DoctorStatus::Unsupported
        } else if available_bytes < DEFAULT_MAX_WAL_BYTES {
            DoctorStatus::SupportedWithWarnings
        } else {
            DoctorStatus::Supported
        }
    });
    DoctorProbe {
        required: false,
        status,
        code,
        message: format!("{label} inspected without creating or overwriting it"),
        remediation: "choose a new path below a writable existing directory".into(),
    }
}

fn probe_directory_io(directory: &Path) -> bool {
    let temporary = directory.join(format!(".chronicle-doctor-{}", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let mut created = false;
    let io_result = options.open(&temporary).and_then(|mut file| {
        created = true;
        file.try_lock()?;
        file.write_all(b"doctor-data")?;
        file.write_all(b"doctor-commit-marker")?;
        file.sync_all()?;
        let expected_bytes = (b"doctor-data".len() + b"doctor-commit-marker".len()) as u64;
        (file.metadata()?.len() == expected_bytes)
            .then_some(())
            .ok_or_else(|| std::io::Error::other("doctor probe byte accounting failed"))
    });
    let publication_result =
        io_result.is_ok() && probe_no_replace_publication(directory, &temporary);
    let cleanup_result = if temporary.exists() {
        created && fs::remove_file(&temporary).is_ok() && !temporary.exists()
    } else {
        true
    };
    publication_result && cleanup_result
}

#[cfg(any(target_os = "linux", target_vendor = "apple"))]
fn probe_no_replace_publication(directory: &Path, temporary: &Path) -> bool {
    let published = directory.join(format!(
        ".chronicle-doctor-published-{}",
        uuid::Uuid::new_v4()
    ));
    let renamed = renameat_with(CWD, temporary, CWD, &published, RenameFlags::NOREPLACE).is_ok();
    let synced = renamed
        && File::open(directory)
            .and_then(|directory| directory.sync_all())
            .is_ok();
    let cleaned = if renamed {
        fs::remove_file(&published).is_ok() && !published.exists()
    } else {
        true
    };
    renamed && synced && cleaned
}

#[cfg(not(any(target_os = "linux", target_vendor = "apple")))]
fn probe_no_replace_publication(_directory: &Path, _temporary: &Path) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn capture_doctor_probes() -> Vec<DoctorProbe> {
    let checks = chronicle_capture_ebpf::probe_embedded();
    [
        ("capture.platform", true, checks.platform),
        ("capture.architecture", true, checks.architecture),
        ("capture.cgroup_v2", true, checks.cgroup_v2),
        ("capture.btf", true, checks.btf),
        ("capture.object", true, checks.embedded_object),
        ("capture.programs", true, checks.required_programs),
        ("capture.attach", false, checks.attach),
        ("capture.cap_bpf", true, checks.cap_bpf),
        ("capture.cap_net_admin", true, checks.cap_net_admin),
    ]
    .into_iter()
    .map(|(code, required, check)| {
        let (status, message) = match check {
            chronicle_capture_ebpf::PreflightCheck::Available => {
                (DoctorStatus::Supported, "available")
            }
            chronicle_capture_ebpf::PreflightCheck::Unavailable(reason) => {
                (DoctorStatus::Unsupported, reason)
            }
            chronicle_capture_ebpf::PreflightCheck::NotChecked(reason) => {
                (DoctorStatus::NotChecked, reason)
            }
        };
        DoctorProbe {
            required,
            status,
            code: code.into(),
            message: message.into(),
            remediation: capture_probe_remediation(code, status).into(),
        }
    })
    .collect()
}

#[cfg(target_os = "linux")]
fn capture_probe_remediation(code: &str, status: DoctorStatus) -> &'static str {
    if status == DoctorStatus::Supported {
        return "none";
    }
    match code {
        "capture.platform" => "run Chronicle on supported Linux",
        "capture.architecture" => "use a supported little-endian Linux architecture and kernel",
        "capture.cgroup_v2" => "enable and mount the unified cgroup v2 hierarchy",
        "capture.btf" => "install a kernel with BTF enabled and /sys/kernel/btf/vmlinux available",
        "capture.object" | "capture.programs" => {
            "rebuild Chronicle on a supported little-endian Linux target so the embedded capture programs are included"
        }
        "capture.cap_bpf" => "grant CAP_BPF to the Chronicle supervisor",
        "capture.cap_net_admin" => "grant CAP_NET_ADMIN to the Chronicle supervisor",
        "capture.attach" => {
            "rerun recording with an explicit PID or cgroup selector to verify attachment"
        }
        _ => "resolve the reported local eBPF prerequisite before recording",
    }
}

#[cfg(not(target_os = "linux"))]
fn capture_doctor_probes() -> Vec<DoctorProbe> {
    vec![DoctorProbe {
        required: true,
        status: DoctorStatus::Unsupported,
        code: "capture.platform".into(),
        message: "Linux eBPF live capture is unavailable on this platform".into(),
        remediation: "build and run Chronicle on a supported Linux host".into(),
    }]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProductionRecordingBounds {
    pub duration_seconds: u64,
    pub segment_bytes: u64,
    pub max_wal_bytes: u64,
}

impl Default for ProductionRecordingBounds {
    fn default() -> Self {
        Self {
            duration_seconds: DEFAULT_RECORDING_DURATION_SECONDS,
            segment_bytes: DEFAULT_SEGMENT_BYTES,
            max_wal_bytes: DEFAULT_MAX_WAL_BYTES,
        }
    }
}

/// Signal state shared by platform adapters and synchronous recording.
#[derive(Clone, Debug, Default)]
pub struct ProductionSignalStop(Arc<AtomicU8>);

impl ProductionSignalStop {
    /// Requests graceful shutdown. Returns false when this is a second signal.
    pub fn request_interrupt(&self) -> bool {
        self.request(1)
    }

    /// Requests graceful shutdown. Returns false when this is a second signal.
    pub fn request_termination(&self) -> bool {
        self.request(2)
    }

    pub fn shutdown_reason(&self) -> Option<ShutdownReason> {
        match self.0.load(Ordering::SeqCst) {
            1 => Some(ShutdownReason::UserInterrupt),
            2 => Some(ShutdownReason::TerminationSignal),
            _ => None,
        }
    }

    fn request(&self, signal: u8) -> bool {
        self.0
            .compare_exchange(0, signal, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}
