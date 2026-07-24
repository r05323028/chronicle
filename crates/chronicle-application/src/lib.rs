//! Application services and configuration. CLI remains an outer adapter.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub wal: WalConfig,
    pub capture: CaptureConfig,
    pub postgres: Option<PostgresConfig>,
    pub s3: Option<S3Config>,
    pub protocol_overrides: Vec<ProtocolOverride>,
    pub redaction: RedactionConfig,
    pub replay: ReplayConfig,
}

impl AppConfig {
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ApplicationError> {
        let contents = fs::read_to_string(path)?;
        Ok(toml::from_str(&contents)?)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WalConfig {
    pub directory: PathBuf,
    pub segment_size_bytes: u64,
    pub disk_limit_bytes: u64,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            directory: PathBuf::from("./chronicle-wal"),
            segment_size_bytes: 64 * 1024 * 1024,
            disk_limit_bytes: 10 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CaptureConfig {
    pub process_ids: Vec<u32>,
    pub network_namespaces: Vec<u64>,
    pub ports: Vec<u16>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostgresConfig {
    pub connection_url_env: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3Config {
    pub endpoint: Option<String>,
    pub bucket: String,
    pub region: Option<String>,
    pub path_style: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProtocolOverride {
    pub port: u16,
    pub protocol: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RedactionConfig {
    pub redact_headers: Vec<String>,
    pub drop_payloads: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)] // Independent safety gates remain visible in config.
pub struct ReplayConfig {
    pub target_mappings: Vec<TargetMappingConfig>,
    pub dry_run: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub allow_publication: bool,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        Self {
            target_mappings: Vec::new(),
            dry_run: true,
            allow_reads: false,
            allow_writes: false,
            allow_publication: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetMappingConfig {
    pub protocol: Option<String>,
    pub recorded_host: Option<String>,
    pub recorded_port: Option<u16>,
    pub target_host: String,
    pub target_port: u16,
}

#[derive(Clone, Debug)]
pub enum ApplicationCommand {
    Record,
    Etl,
    Replay {
        session_id: String,
        timing: ReplayTiming,
    },
    Inspect {
        session_id: String,
    },
    Doctor,
}

#[derive(Clone, Copy, Debug)]
pub enum ReplayTiming {
    Preserve,
    Asap,
}

#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("configuration I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration parse failed: {0}")]
    Config(#[from] toml::de::Error),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

pub struct ChronicleApplication {
    config: AppConfig,
}

impl ChronicleApplication {
    pub fn new(config: AppConfig) -> Self {
        Self { config }
    }

    pub fn execute(&self, command: &ApplicationCommand) -> Result<String, ApplicationError> {
        self.validate()?;
        match command {
            ApplicationCommand::Record => Err(ApplicationError::NotImplemented("record")),
            ApplicationCommand::Etl => Err(ApplicationError::NotImplemented("etl")),
            ApplicationCommand::Replay { .. } => Err(ApplicationError::NotImplemented("replay")),
            ApplicationCommand::Inspect { .. } => Err(ApplicationError::NotImplemented("inspect")),
            ApplicationCommand::Doctor => {
                Ok("configuration checks passed; external probes not implemented".into())
            }
        }
    }

    fn validate(&self) -> Result<(), ApplicationError> {
        if self.config.wal.segment_size_bytes <= chronicle_wal::HEADER_LEN as u64 {
            return Err(ApplicationError::InvalidConfig(
                "WAL segment size must exceed record header".into(),
            ));
        }
        if self.config.wal.disk_limit_bytes < self.config.wal.segment_size_bytes {
            return Err(ApplicationError::InvalidConfig(
                "WAL disk limit must be at least one segment".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaffold_commands_return_typed_not_implemented_error() {
        let error = ChronicleApplication::new(AppConfig::default())
            .execute(&ApplicationCommand::Record)
            .unwrap_err();
        assert!(matches!(error, ApplicationError::NotImplemented("record")));
    }

    #[test]
    fn doctor_reports_only_completed_configuration_checks() {
        let output = ChronicleApplication::new(AppConfig::default())
            .execute(&ApplicationCommand::Doctor)
            .unwrap();
        assert_eq!(
            output,
            "configuration checks passed; external probes not implemented"
        );
    }
}
