use crate::replay_inspect::{ReplaySessionResult, ReplayStatus};
use chronicle_capture::CaptureError;
use chronicle_etl::EtlError;
use chronicle_protocol::{ProtocolError, TransportErrorCategory};
use chronicle_replay::{ReplayError, ReplayTargetError};
use chronicle_storage::StorageError;
use chronicle_wal::WalError;
use std::path::PathBuf;
use thiserror::Error;

#[cfg(target_os = "linux")]
use chronicle_capture_ebpf::EbpfCaptureError;

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[cfg(target_os = "linux")]
    #[error(transparent)]
    Ebpf(#[from] EbpfCaptureError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Etl(#[from] EtlError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    ReplayTarget(#[from] ReplayTargetError),
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("runtime Authorization credential is missing from environment variable {environment}")]
    MissingReplayCredential { environment: String },
    #[error("runtime Authorization credential has invalid HTTP field-value bytes")]
    InvalidReplayCredential,
    #[error("JSON serialization failed: {0}")]
    JsonSerialization(#[from] serde_json::Error),
    #[error("encoded fixture WAL is {bytes} bytes; one-segment limit is {limit} bytes")]
    FixtureWalTooLarge { bytes: u64, limit: u64 },
    #[error("capture flags {0:#x} cannot fit in WAL record flags")]
    CaptureFlagsOutOfRange(u32),
    #[error("WAL directory already exists: {0}")]
    WalDestinationExists(PathBuf),
    #[error("configuration parse failed: {0}")]
    Config(#[from] toml::de::Error),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("recording metadata failed validation: {0}")]
    RecordingMetadataValidation(String),
    #[error("existing published session {session_id} does not match recording output")]
    PublishedRecordingMismatch {
        session_id: chronicle_common::SessionId,
    },
    #[error("recording ETL checkpoint contradicts recovered WAL snapshot")]
    CheckpointContradiction,
    #[error("production recording preflight failed: {0}")]
    ProductionPreflight(&'static str),
    #[error(
        "unsafe data directory path '{0}': must be absolute with no symlink or non-directory components"
    )]
    UnsafeDataDir(PathBuf),
    #[error(
        "data directory resolution is unsupported on this platform; pass --data-dir or set CHRONICLE_DATA_DIR"
    )]
    UnsupportedDataDirResolution,
    #[error("recording catalog invalid: {0}")]
    CatalogInvalid(String),
    #[error("recording catalog has {count} entries; limit is {limit}")]
    CatalogEntryLimit { count: usize, limit: usize },
    #[error("recording catalog path is unsafe: {0}")]
    CatalogUnsafePath(PathBuf),
    #[error("invalid recording name '{0}': {1}")]
    InvalidRecordingName(String, String),
    #[error("recording '{0}' was not found")]
    RecordingNotFound(String),
    #[error("recording name '{0}' is already in use")]
    RecordingNameCollision(String),
    #[error("chronicle data domain is already owned by another process")]
    DomainOwned,
    #[error("domain lock is unsupported on this platform")]
    UnsupportedDomainLock,
    #[error("domain lock path is unsafe")]
    DomainLockUnsafePath,
    #[error("unsupported live supervised scope: {0}")]
    UnsupportedLivePreflight(&'static str),
    #[error("replay target readiness failed: {0}")]
    ReplayReadiness(String),
    #[error("target bootstrap, hardening, or exec failed")]
    TargetLaunchFailed,
    #[error(
        "incompatible domain lock mapping: data directory {data_dir} and lock root {lock_root} resolve different lock paths on the same filesystem; multiple differently locked Chronicle domains on one filesystem are unsupported"
    )]
    IncompatibleDomainLockMapping {
        data_dir: PathBuf,
        lock_root: PathBuf,
    },
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

/// Stable replay-result exit-code classification for outer adapters.
/// Operates on the application-owned result contract so adapters never touch
/// the replay-owned outcome taxonomy.
pub fn replay_result_exit_code(result: &ReplaySessionResult) -> i32 {
    match result.outcome {
        ReplayStatus::Completed | ReplayStatus::CompletedWithSkips | ReplayStatus::DryRun => 0,
        ReplayStatus::StoppedPolicy | ReplayStatus::StoppedInvalidSession => 4,
        ReplayStatus::StoppedTransport => 5,
        ReplayStatus::StoppedVerification => 6,
    }
}

/// Stable application-error exit-code classification for outer adapters.
pub fn application_error_exit_code(error: &ApplicationError) -> i32 {
    match error {
        ApplicationError::InvalidRecordingName(_, _) => 2,
        ApplicationError::UnsupportedLivePreflight(_)
        | ApplicationError::ReplayReadiness(_)
        | ApplicationError::ReplayTarget(_)
        | ApplicationError::Replay(ReplayError::PreflightDenied) => 4,
        ApplicationError::Protocol(ProtocolError::Transport { .. })
        | ApplicationError::Replay(ReplayError::Protocol(ProtocolError::Transport {
            category:
                TransportErrorCategory::Refused
                | TransportErrorCategory::Timeout
                | TransportErrorCategory::Disconnect
                | TransportErrorCategory::Io,
            ..
        })) => 5,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_errors_map_to_stable_exit_family() {
        let transport = ApplicationError::Protocol(ProtocolError::Transport {
            category: TransportErrorCategory::Timeout,
            message: "timeout".into(),
        });
        for (error, expected) in [
            (
                ApplicationError::InvalidRecordingName("name".into(), "invalid".into()),
                2,
            ),
            (ApplicationError::RecordingNotFound("missing".into()), 3),
            (ApplicationError::TargetLaunchFailed, 3),
            (ApplicationError::UnsupportedLivePreflight("unsupported"), 4),
            (transport, 5),
        ] {
            assert_eq!(application_error_exit_code(&error), expected);
        }
    }
}
