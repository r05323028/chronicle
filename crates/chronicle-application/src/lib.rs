//! Application services and configuration. CLI remains an outer adapter.

use chronicle_capture::{CaptureError, CaptureSource, FixtureCaptureSource, encode_event};
use chronicle_etl::{EtlError, EtlIssue, EtlOutput, EtlPipeline};
use chronicle_protocol::{ProtocolError, ProtocolRegistry};
use chronicle_session::SessionLimits;
use chronicle_storage::{FilesystemSessionStore, PublishSession, StorageError};
use chronicle_wal::{
    RecordKind, SegmentedWalWriter, WalCheckpoint, WalError, WalReader, WalRecord, WalWriter,
    encode_record,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
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
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error(transparent)]
    Etl(#[from] EtlError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("inspect JSON encoding failed: {0}")]
    InspectJson(#[from] serde_json::Error),
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
    #[error("{0} command is not implemented in current scaffold")]
    NotImplemented(&'static str),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedWal {
    pub first_sequence: Option<u64>,
    pub checkpoint: Option<WalCheckpoint>,
    pub record_count: usize,
}

pub fn write_capture_to_wal(
    source: &mut impl CaptureSource,
    directory: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordedWal, ApplicationError> {
    let directory = directory.as_ref();
    if directory.exists() {
        return Err(ApplicationError::WalDestinationExists(
            directory.to_path_buf(),
        ));
    }

    let mut records = Vec::new();
    let mut encoded_bytes = 0_u64;
    while let Some(event) = source.next_event()? {
        let flags = u16::try_from(event.flags.0)
            .map_err(|_| ApplicationError::CaptureFlagsOutOfRange(event.flags.0))?;
        let record = WalRecord {
            kind: RecordKind::CaptureEvent,
            flags,
            sequence: event.monotonic_sequence,
            payload: encode_event(&event)?,
        };
        encoded_bytes = encoded_bytes
            .saturating_add(u64::try_from(encode_record(&record)?.len()).unwrap_or(u64::MAX));
        records.push(record);
    }
    if encoded_bytes > max_segment_bytes {
        return Err(ApplicationError::FixtureWalTooLarge {
            bytes: encoded_bytes,
            limit: max_segment_bytes,
        });
    }

    let mut writer = SegmentedWalWriter::create(directory, max_segment_bytes)?;
    let first_sequence = records.first().map(|record| record.sequence);
    let mut checkpoint = None;
    for record in &records {
        checkpoint = Some(writer.append(record)?);
    }
    writer.flush()?;
    Ok(RecordedWal {
        first_sequence,
        checkpoint,
        record_count: records.len(),
    })
}

pub fn process_single_wal(
    path: impl AsRef<Path>,
    first_sequence: u64,
    registry: &ProtocolRegistry,
    session_id: chronicle_common::SessionId,
) -> Result<(EtlOutput, WalCheckpoint), ApplicationError> {
    let mut reader = WalReader::new(fs::File::open(path)?, first_sequence);
    let output =
        EtlPipeline::new(SessionLimits::default()).process(&mut reader, registry, session_id)?;
    Ok((output, reader.checkpoint()))
}

const MAX_RECORD_ISSUES: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordIssueSummary {
    pub code: String,
    pub sequence: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectOperationSummary {
    pub sequence: u64,
    pub kind: String,
    pub effect: String,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub incomplete: bool,
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectConnectionSummary {
    pub protocol: String,
    pub client: chronicle_common::Endpoint,
    pub server: chronicle_common::Endpoint,
    pub incomplete: bool,
    pub truncated: bool,
    pub operations: Vec<InspectOperationSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectSessionResult {
    pub session_id: chronicle_common::SessionId,
    pub complete: bool,
    pub replayable: bool,
    pub blockers: Vec<String>,
    pub issue_codes: Vec<String>,
    pub connections: Vec<InspectConnectionSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordFixtureResult {
    pub session_id: chronicle_common::SessionId,
    pub checkpoint: WalCheckpoint,
    pub issues: Vec<RecordIssueSummary>,
    pub replayability: Vec<String>,
    pub complete: bool,
}

/// Runs fixture capture through durable WAL and ETL before atomically publishing its session.
pub fn record_fixture(
    source: &mut FixtureCaptureSource,
    root: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordFixtureResult, ApplicationError> {
    let root = root.as_ref();
    let session_id = chronicle_common::SessionId::new();
    let wal_directory = root.join("wal").join(session_id.to_string());
    let recorded = write_capture_to_wal(source, &wal_directory, max_segment_bytes)?;
    let (output, checkpoint) = process_single_wal(
        wal_directory.join(chronicle_wal::segment_file_name(
            recorded.first_sequence.unwrap_or(1),
        )),
        recorded.first_sequence.unwrap_or(1),
        &chronicle_protocol_builtins::registry()?,
        session_id,
    )?;
    let issues = issue_summaries(&output.issues);
    let replayability = replayability_reasons(&output);
    let complete = session_complete(&output);
    FilesystemSessionStore::new(root).publish(PublishSession {
        session: output.session,
        checkpoint: Some(format!(
            "{}:{}:{}",
            checkpoint.segment_first_sequence, checkpoint.byte_offset, checkpoint.next_sequence
        )),
        issues: issues.iter().map(|issue| issue.code.clone()).collect(),
        replayability: replayability.clone(),
        complete,
    })?;
    Ok(RecordFixtureResult {
        session_id,
        checkpoint,
        issues,
        replayability,
        complete,
    })
}

fn issue_summaries(issues: &[EtlIssue]) -> Vec<RecordIssueSummary> {
    let mut summaries: Vec<_> = issues
        .iter()
        .take(MAX_RECORD_ISSUES)
        .map(|issue| RecordIssueSummary {
            code: match issue.kind {
                chronicle_etl::EtlIssueKind::MalformedCaptureEvent => "malformed_capture_event",
                chronicle_etl::EtlIssueKind::CaptureSequenceMismatch => "capture_sequence_mismatch",
                chronicle_etl::EtlIssueKind::PartialWalTail => "partial_wal_tail",
                chronicle_etl::EtlIssueKind::UnknownProtocol => "unknown_protocol",
                chronicle_etl::EtlIssueKind::ProtocolDecode => "protocol_decode",
            }
            .into(),
            sequence: issue.sequence,
        })
        .collect();
    if issues.len() > MAX_RECORD_ISSUES {
        summaries.push(RecordIssueSummary {
            code: "issue_summary_truncated".into(),
            sequence: None,
        });
    }
    summaries
}

fn replayability_reasons(output: &EtlOutput) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    if !output.issues.is_empty() {
        reasons.insert("etl_issues".into());
    }
    for connection in &output.session.connections {
        if connection.incomplete || connection.truncated {
            reasons.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if operation.incomplete || operation.truncated {
                reasons.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                reasons.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                reasons.insert(format!("warning:{}", warning.code));
            }
        }
    }
    reasons.into_iter().collect()
}

fn session_complete(output: &EtlOutput) -> bool {
    output.issues.is_empty() && canonical_session_complete(&output.session)
}

fn canonical_session_complete(session: &chronicle_canonical::CanonicalSession) -> bool {
    session.connections.iter().all(|connection| {
        !connection.incomplete
            && !connection.truncated
            && connection.operations.iter().all(|operation| {
                !operation.incomplete
                    && !operation.truncated
                    && operation.recorded_response.is_some()
            })
    })
}

/// Produces inspect-safe metadata without reading artifact body bytes.
pub fn inspect_session(
    root: impl AsRef<Path>,
    session_id: chronicle_common::SessionId,
) -> Result<InspectSessionResult, ApplicationError> {
    let stored = FilesystemSessionStore::new(root.as_ref()).inspect_with_metadata(session_id)?;
    let mut blockers: BTreeSet<_> = stored.replayability.into_iter().collect();
    for connection in &stored.session.connections {
        if connection.incomplete || connection.truncated {
            blockers.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if operation.incomplete || operation.truncated {
                blockers.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                blockers.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                blockers.insert(format!("warning:{}", warning.code));
            }
        }
    }
    let complete = stored.complete && canonical_session_complete(&stored.session);
    if !complete {
        blockers.insert("incomplete_capture".into());
    }
    let mut issue_codes = stored.issues;
    issue_codes.sort();
    let connections = stored
        .session
        .connections
        .iter()
        .map(|connection| InspectConnectionSummary {
            protocol: connection.protocol.to_string(),
            client: connection.client.clone(),
            server: connection.server.clone(),
            incomplete: connection.incomplete,
            truncated: connection.truncated,
            operations: connection
                .operations
                .iter()
                .map(|operation| InspectOperationSummary {
                    sequence: operation.sequence,
                    kind: format!("{:?}", operation.kind).to_lowercase(),
                    effect: format!("{:?}", operation.effect).to_lowercase(),
                    request_bytes: payload_size(&operation.request),
                    response_bytes: operation.recorded_response.as_ref().and_then(payload_size),
                    incomplete: operation.incomplete,
                    truncated: operation.truncated,
                    warnings: operation
                        .warnings
                        .iter()
                        .map(|warning| warning.code.clone())
                        .collect(),
                })
                .collect(),
        })
        .collect();
    Ok(InspectSessionResult {
        session_id,
        complete,
        replayable: complete && blockers.is_empty(),
        blockers: blockers.into_iter().collect(),
        issue_codes,
        connections,
    })
}

#[derive(Serialize)]
struct InspectJsonV1<'a> {
    version: u8,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

/// Renders stable, body-free inspect output for interactive use.
pub fn render_inspect_human(inspected: &InspectSessionResult) -> String {
    let mut output = format!(
        "session_id: {}\ncomplete: {}\nreplayable: {}\nblockers: {}\nissues: {}\n",
        inspected.session_id,
        inspected.complete,
        inspected.replayable,
        joined_or_none(&inspected.blockers),
        joined_or_none(&inspected.issue_codes),
    );
    for connection in &inspected.connections {
        output.push_str(&format!(
            "connection: {} {}:{} -> {}:{} incomplete={} truncated={}\n",
            connection.protocol,
            connection.client.host,
            connection.client.port,
            connection.server.host,
            connection.server.port,
            connection.incomplete,
            connection.truncated,
        ));
        for operation in &connection.operations {
            output.push_str(&format!(
                "operation: sequence={} kind={} effect={} request_bytes={} response_bytes={} incomplete={} truncated={} warnings={}\n",
                operation.sequence,
                operation.kind,
                operation.effect,
                optional_size(operation.request_bytes),
                optional_size(operation.response_bytes),
                operation.incomplete,
                operation.truncated,
                joined_or_none(&operation.warnings),
            ));
        }
    }
    output
}

/// Renders stable inspect JSON v1 containing only summary metadata.
pub fn render_inspect_json(inspected: &InspectSessionResult) -> Result<String, ApplicationError> {
    Ok(serde_json::to_string(&InspectJsonV1 {
        version: 1,
        result: inspected,
    })?)
}

fn joined_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none".into()
    } else {
        values.join(",")
    }
}

fn optional_size(size: Option<u64>) -> String {
    size.map_or_else(|| "unknown".into(), |size| size.to_string())
}

fn payload_size(payload: &chronicle_canonical::PayloadRef) -> Option<u64> {
    match payload {
        chronicle_canonical::PayloadRef::Inline { bytes, .. } => Some(bytes.len() as u64),
        chronicle_canonical::PayloadRef::Object { size, .. }
        | chronicle_canonical::PayloadRef::Artifact { size, .. } => Some(*size),
        chronicle_canonical::PayloadRef::Redacted { original_size, .. } => *original_size,
        chronicle_canonical::PayloadRef::Missing { .. } => None,
    }
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

    #[test]
    fn record_fixture_runs_wal_etl_and_atomic_publish() {
        let root = std::env::temp_dir().join(format!("chronicle-record-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        let result = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        assert!(result.issues.is_empty());
        assert!(result.checkpoint.next_sequence > 1);
        let session = FilesystemSessionStore::new(&root)
            .inspect(result.session_id)
            .unwrap();
        assert_eq!(session.id, result.session_id);
        assert_eq!(session.connections.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_reads_published_session_after_fixture_and_wal_removal() {
        let root = std::env::temp_dir().join(format!("chronicle-inspect-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        std::fs::remove_dir_all(root.join("wal")).unwrap();

        let inspected = inspect_session(&root, recorded.session_id).unwrap();
        assert!(inspected.complete);
        assert!(inspected.replayable);
        assert!(inspected.blockers.is_empty());
        assert_eq!(inspected.connections[0].operations.len(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_rendering_is_deterministic_and_excludes_header_values() {
        let root =
            std::env::temp_dir().join(format!("chronicle-inspect-render-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/duplicate-header.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        let inspected = inspect_session(&root, recorded.session_id).unwrap();

        let human = render_inspect_human(&inspected);
        let json = render_inspect_json(&inspected).unwrap();
        assert_eq!(human, render_inspect_human(&inspected));
        assert_eq!(json, render_inspect_json(&inspected).unwrap());
        for rendered in [&human, &json] {
            assert!(!rendered.contains("x-tag"));
            assert!(!rendered.contains("two"));
        }
        assert!(json.starts_with("{\"version\":1,"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_marks_truncated_fixture_non_replayable() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-inspect-truncated-{}",
            uuid::Uuid::new_v4()
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/truncated.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();

        let inspected = inspect_session(&root, recorded.session_id).unwrap();
        assert!(!inspected.complete);
        assert!(!inspected.replayable);
        assert!(!inspected.blockers.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inspect_session_propagates_missing_and_corrupt_sessions() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-inspect-invalid-{}",
            uuid::Uuid::new_v4()
        ));
        assert!(matches!(
            inspect_session(&root, chronicle_common::SessionId::new()),
            Err(ApplicationError::Storage(StorageError::NotFound(_)))
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();
        let recorded = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        std::fs::write(
            root.join("sessions")
                .join(recorded.session_id.to_string())
                .join("manifest.json"),
            b"corrupt",
        )
        .unwrap();
        assert!(matches!(
            inspect_session(&root, recorded.session_id),
            Err(ApplicationError::Storage(StorageError::Validation(_)))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn record_fixture_marks_truncated_capture_non_replayable() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-record-truncated-{}",
            uuid::Uuid::new_v4()
        ));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/truncated.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        let result = record_fixture(&mut source, &root, 1024 * 1024).unwrap();
        assert!(!result.complete);
        assert!(!result.replayability.is_empty());
        let manifest = std::fs::read_to_string(
            root.join("sessions")
                .join(result.session_id.to_string())
                .join("manifest.json"),
        )
        .unwrap();
        assert!(manifest.contains("\"complete\":false"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn issue_summaries_are_bounded_and_omit_messages() {
        let issues: Vec<_> = (0..=MAX_RECORD_ISSUES)
            .map(|sequence| EtlIssue {
                sequence: Some(sequence as u64),
                kind: chronicle_etl::EtlIssueKind::ProtocolDecode,
                message: "secret header value".into(),
            })
            .collect();
        let summaries = issue_summaries(&issues);
        assert_eq!(summaries.len(), MAX_RECORD_ISSUES + 1);
        assert!(
            summaries
                .iter()
                .all(|summary| !summary.code.contains("secret"))
        );
        assert_eq!(summaries.last().unwrap().code, "issue_summary_truncated");
    }

    #[test]
    fn record_fixture_does_not_publish_when_wal_preflight_fails() {
        let root =
            std::env::temp_dir().join(format!("chronicle-record-fail-{}", uuid::Uuid::new_v4()));
        let fixture = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/http/basic-session.json"
        ))
        .unwrap();
        let mut source = FixtureCaptureSource::from_json(&fixture).unwrap();

        assert!(matches!(
            record_fixture(&mut source, &root, 1),
            Err(ApplicationError::FixtureWalTooLarge { .. })
        ));
        assert!(!root.join("sessions").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capture_source_is_written_and_read_back_through_wal() {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, TransportProtocol};
        use chronicle_wal::{ReadOutcome, WalReader, segment_file_name};
        use std::fs::File;

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"fixture".to_vec(),
            process: None,
            container: None,
            file_descriptor: Some(7),
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        let recorded = write_capture_to_wal(&mut source, &directory, 1024).unwrap();
        assert_eq!(recorded.record_count, 1);
        assert_eq!(recorded.first_sequence, Some(1));

        let mut reader =
            WalReader::new(File::open(directory.join(segment_file_name(1))).unwrap(), 1);
        assert!(
            matches!(reader.next_record().unwrap(), ReadOutcome::Record(record) if record.sequence == 1)
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oversized_capture_is_rejected_before_wal_directory_creation() {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, TransportProtocol};

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-wal-limit-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: vec![0; 128],
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        let error = write_capture_to_wal(&mut source, &directory, 64).unwrap_err();
        assert!(matches!(error, ApplicationError::FixtureWalTooLarge { .. }));
        assert!(!directory.exists());
    }

    #[test]
    fn single_wal_process_returns_checkpoint_after_last_valid_record() {
        use chronicle_capture::{
            CAPTURE_EVENT_SCHEMA_VERSION, CaptureEvent, CaptureFlags, InMemoryCaptureSource,
        };
        use chronicle_common::{ConnectionKey, Direction, Endpoint, SessionId, TransportProtocol};
        use chronicle_wal::segment_file_name;

        let directory =
            std::env::temp_dir().join(format!("chronicle-app-etl-{}", uuid::Uuid::new_v4()));
        let event = CaptureEvent {
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            monotonic_sequence: 1,
            wall_time: None,
            connection: ConnectionKey::new(
                Endpoint::new("fixture-client", 41000),
                Endpoint::new("recorded.invalid", 8080),
                TransportProtocol::Tcp,
            ),
            direction: Direction::ClientToServer,
            payload: b"FAKE request".to_vec(),
            process: None,
            container: None,
            file_descriptor: None,
            truncated: false,
            flags: CaptureFlags::default(),
        };
        let mut source = InMemoryCaptureSource::new([event]);
        write_capture_to_wal(&mut source, &directory, 1024).unwrap();
        let (output, checkpoint) = process_single_wal(
            directory.join(segment_file_name(1)),
            1,
            &chronicle_protocol_builtins::registry().unwrap(),
            SessionId::new(),
        )
        .unwrap();
        assert_eq!(checkpoint.next_sequence, 2);
        assert_eq!(output.session.connections.len(), 1);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
