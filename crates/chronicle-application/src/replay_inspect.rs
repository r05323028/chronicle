//! Replay planning/inspection and render helpers plus fixture recording.

use super::*;

pub fn replay_command_inputs(
    session: &chronicle_canonical::CanonicalSession,
    options: &LoopbackReplayOptions,
) -> Result<(TargetMap, ReplayPolicy), ApplicationError> {
    Ok((options.target_map_for(session)?, options.policy()))
}

/// Loads optional runtime Authorization without letting config grant replay permissions.
pub fn replay_context(config: &ReplayConfig) -> Result<ReplayContext, ApplicationError> {
    replay_context_from(config, |environment| std::env::var(environment).ok())
}

pub(crate) fn replay_context_from(
    config: &ReplayConfig,
    lookup: impl FnOnce(&str) -> Option<String>,
) -> Result<ReplayContext, ApplicationError> {
    let Some(environment) = config.authorization_env.as_deref() else {
        return Ok(ReplayContext::default());
    };
    let value = lookup(environment).ok_or_else(|| ApplicationError::MissingReplayCredential {
        environment: environment.into(),
    })?;
    if !value.bytes().all(valid_http_field_value_byte) {
        return Err(ApplicationError::InvalidReplayCredential);
    }
    let mut context = ReplayContext::default();
    context.credentials = [("authorization".into(), SecretBytes::new(value.into_bytes()))]
        .into_iter()
        .collect();
    Ok(context)
}

fn valid_http_field_value_byte(byte: u8) -> bool {
    byte == b'\t' || (0x20..=0x7e).contains(&byte) || byte >= 0x80
}

pub(crate) const MAX_RECORD_ISSUES: usize = 64;

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
    pub method: Option<String>,
    pub target: Option<String>,
    pub response_status: Option<String>,
    pub request_bytes: Option<u64>,
    pub response_bytes: Option<u64>,
    pub completeness: Completeness,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectConnectionSummary {
    pub protocol: String,
    pub client: chronicle_common::Endpoint,
    pub server: chronicle_common::Endpoint,
    pub completeness: Completeness,
    pub operations: Vec<InspectOperationSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct InspectSessionResult {
    pub session_id: chronicle_common::SessionId,
    pub complete: bool,
    /// Authoritative replay classification; the removed pre-0.1 `replayable`
    /// boolean alias is gone (replayable == `FullyReplayable`).
    pub replayability: Replayability,
    pub executable_operations: usize,
    pub non_executable_operations: usize,
    pub integrity_valid: bool,
    pub source_provenance: SourceProvenance,
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
    // A fixture recording is one deterministic single-epoch recording: the
    // WAL identity is the epoch identity and the canonical session id derives
    // from it (matching production SessionId(epoch_id) derivation). Explicit
    // provenance is stamped below; lineage is never inferred from identifier
    // equality. The root is a public data-directory shape (recordings/ +
    // sessions/) so the current --data-dir surface resolves fixture output.
    let recording_id = RecordingId::new();
    let session_id = chronicle_common::SessionId(recording_id.0);
    let wal_directory = root.join("recordings").join(recording_id.to_string());
    let recorded = write_capture_to_wal(source, &wal_directory, recording_id, max_segment_bytes)?;
    let (mut output, checkpoint) = process_fixture_wal(
        &wal_directory,
        recorded.recording_id,
        &chronicle_protocol_builtins::registry()?,
        session_id,
    )?;
    output.session.source_provenance.recording_id = Some(recorded.recording_id);
    output.session.source_provenance.epoch_id = Some(chronicle_common::EpochId::from_uuid(
        recorded.recording_id.0,
    ));
    output.session.source_provenance.epoch_ordinal = Some(0);
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

/// Opens fixture input and delegates all record work to the application service.
pub fn record_fixture_file(
    input: impl AsRef<Path>,
    root: impl AsRef<Path>,
    max_segment_bytes: u64,
) -> Result<RecordFixtureResult, ApplicationError> {
    let bytes = fs::read(input)?;
    let mut source = FixtureCaptureSource::from_json(&bytes)?;
    record_fixture(&mut source, root, max_segment_bytes)
}

pub(crate) fn issue_summaries(issues: &[EtlIssue]) -> Vec<RecordIssueSummary> {
    let mut summaries: Vec<_> = issues
        .iter()
        .take(MAX_RECORD_ISSUES)
        .map(|issue| RecordIssueSummary {
            code: match issue.kind {
                chronicle_etl::EtlIssueKind::MalformedCaptureEvent => "malformed_capture_event",
                chronicle_etl::EtlIssueKind::PartialWalTail => "partial_wal_tail",
                chronicle_etl::EtlIssueKind::UnknownProtocol => "unknown_protocol",
                chronicle_etl::EtlIssueKind::ProtocolDecode => "protocol_decode",
                chronicle_etl::EtlIssueKind::MalformedLossWindow => "malformed_loss_window",
                chronicle_etl::EtlIssueKind::MalformedTerminalWalLoss => {
                    "malformed_terminal_wal_loss"
                }
                chronicle_etl::EtlIssueKind::MalformedCommitMarker => "malformed_commit_marker",
                chronicle_etl::EtlIssueKind::IncompleteSocketEvidence => {
                    "incomplete_socket_evidence"
                }
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

pub(crate) fn replayability_reasons(output: &EtlOutput) -> Vec<String> {
    let mut reasons = BTreeSet::new();
    if !output.issues.is_empty() {
        reasons.insert("etl_issues".into());
    }
    for connection in &output.session.connections {
        if output.session.connection_state(&connection.id) != Completeness::Complete {
            reasons.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if output.session.operation_state(&operation.id) != Completeness::Complete {
                reasons.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                reasons.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                if warning.code != "missing_timestamp" {
                    reasons.insert(format!("warning:{}", warning.code));
                }
            }
        }
    }
    reasons.into_iter().collect()
}

pub(crate) fn session_complete(output: &EtlOutput) -> bool {
    output.issues.is_empty() && canonical_session_complete(&output.session)
}

fn canonical_session_complete(session: &chronicle_canonical::CanonicalSession) -> bool {
    session.connections.iter().all(|connection| {
        session.connection_state(&connection.id) == Completeness::Complete
            && connection.operations.iter().all(|operation| {
                session.operation_state(&operation.id) == Completeness::Complete
                    && operation.recorded_response.is_some()
            })
    })
}

/// Produces inspect-safe metadata without reading artifact body bytes.
#[allow(clippy::too_many_lines)]
pub fn inspect_session(
    root: impl AsRef<Path>,
    session_id: chronicle_common::SessionId,
) -> Result<InspectSessionResult, ApplicationError> {
    let stored = FilesystemSessionStore::new(root.as_ref()).inspect_with_metadata(session_id)?;
    let mut blockers: BTreeSet<_> = stored.replayability.into_iter().collect();
    for connection in &stored.session.connections {
        if stored.session.connection_state(&connection.id) != Completeness::Complete {
            blockers.insert("incomplete_capture".into());
        }
        for operation in &connection.operations {
            if stored.session.operation_state(&operation.id) != Completeness::Complete {
                blockers.insert("incomplete_operation".into());
            }
            if operation.recorded_response.is_none() {
                blockers.insert("missing_recorded_response".into());
            }
            for warning in &operation.warnings {
                if warning.code != "missing_timestamp" {
                    blockers.insert(format!("warning:{}", warning.code));
                }
            }
        }
    }
    let complete = stored.complete && canonical_session_complete(&stored.session);
    if !complete {
        blockers.insert("incomplete_capture".into());
    }
    let (executable_operations, non_executable_operations) = stored
        .session
        .connections
        .iter()
        .flat_map(|connection| &connection.operations)
        .fold((0, 0), |(executable, non_executable), operation| {
            if stored.session.operation_state(&operation.id) == Completeness::Complete
                && operation.recorded_response.is_some()
                && operation
                    .warnings
                    .iter()
                    .all(|warning| warning.code == "missing_timestamp")
            {
                (executable + 1, non_executable)
            } else {
                (executable, non_executable + 1)
            }
        });
    let replayability = match (executable_operations, non_executable_operations) {
        (0, _) => Replayability::NotReplayable,
        (_, 0) => Replayability::FullyReplayable,
        _ => Replayability::PartiallyReplayable,
    };
    if non_executable_operations > 0 {
        for entry in &stored.session.source_provenance.evidence {
            match entry.kind {
                ProvenanceKind::RingLoss => {
                    blockers.insert("ring_loss_window".into());
                }
                ProvenanceKind::WalLimitLoss => {
                    blockers.insert("wal_limit_loss_window".into());
                }
                _ => {}
            }
        }
    }
    let mut issue_codes = stored.issues;
    issue_codes.sort();
    let source_provenance = stored.session.source_provenance.clone();
    let connections = stored
        .session
        .connections
        .iter()
        .map(|connection| InspectConnectionSummary {
            protocol: connection.protocol.to_string(),
            client: connection.client.clone(),
            server: connection.server.clone(),
            completeness: stored.session.connection_state(&connection.id),
            operations: connection
                .operations
                .iter()
                .map(|operation| InspectOperationSummary {
                    sequence: operation.sequence,
                    kind: format!("{:?}", operation.kind).to_lowercase(),
                    effect: format!("{:?}", operation.effect).to_lowercase(),
                    method: operation.attributes.get("http.method").cloned(),
                    target: operation.attributes.get("http.request_target").cloned(),
                    response_status: operation.attributes.get("http.response_status").cloned(),
                    request_bytes: payload_size(&operation.request),
                    response_bytes: operation.recorded_response.as_ref().and_then(payload_size),
                    completeness: stored.session.operation_state(&operation.id),
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
        replayability,
        executable_operations,
        non_executable_operations,
        integrity_valid: true,
        source_provenance,
        blockers: blockers.into_iter().collect(),
        issue_codes,
        connections,
    })
}

#[derive(Serialize)]
struct InspectJson<'a> {
    version: u8,
    #[serde(flatten)]
    result: &'a InspectSessionResult,
}

/// Renders stable, body-free inspect output for interactive use.
pub fn render_inspect_human(inspected: &InspectSessionResult) -> String {
    let mut output = format!(
        "session_id: {}\ncomplete: {}\nreplayability: {:?}\noperations: executable={} non_executable={}\nintegrity_valid: {}\nsource_status: {:?}\nsource_reason: {}\nblockers: {}\nissues: {}\n",
        inspected.session_id,
        inspected.complete,
        inspected.replayability,
        inspected.executable_operations,
        inspected.non_executable_operations,
        inspected.integrity_valid,
        inspected.source_provenance.status,
        inspected
            .source_provenance
            .reason
            .as_deref()
            .unwrap_or("none"),
        joined_or_none(&inspected.blockers),
        joined_or_none(&inspected.issue_codes),
    );
    for connection in &inspected.connections {
        writeln!(
            output,
            "connection: {} {}:{} -> {}:{} completeness={:?}",
            connection.protocol,
            connection.client.host,
            connection.client.port,
            connection.server.host,
            connection.server.port,
            connection.completeness,
        )
        .expect("writing into String cannot fail");
        for operation in &connection.operations {
            writeln!(
                output,
                "operation: sequence={} kind={} effect={} method={} target={} response_status={} request_bytes={} response_bytes={} completeness={:?} warnings={}",
                operation.sequence,
                operation.kind,
                operation.effect,
                operation.method.as_deref().unwrap_or("none"),
                operation.target.as_deref().unwrap_or("none"),
                operation.response_status.as_deref().unwrap_or("none"),
                optional_size(operation.request_bytes),
                optional_size(operation.response_bytes),
                operation.completeness,
                joined_or_none(&operation.warnings),
            )
            .expect("writing into String cannot fail");
        }
    }
    output
}

pub fn render_json<T: Serialize + ?Sized>(value: &T) -> Result<String, ApplicationError> {
    serde_json::to_string(value).map_err(ApplicationError::JsonSerialization)
}

/// Renders stable inspect JSON v1 containing only summary metadata.
pub fn render_inspect_json(inspected: &InspectSessionResult) -> Result<String, ApplicationError> {
    render_json(&InspectJson {
        version: 1,
        result: inspected,
    })
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

/// Application-owned replay outcome classification for outer adapters.
/// Serialized representation matches the historical replay outcome contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayStatus {
    Completed,
    CompletedWithSkips,
    DryRun,
    StoppedPolicy,
    StoppedInvalidSession,
    StoppedTransport,
    StoppedVerification,
}

impl ReplayStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::CompletedWithSkips => "completed_with_skips",
            Self::DryRun => "dry_run",
            Self::StoppedPolicy => "stopped_policy",
            Self::StoppedInvalidSession => "stopped_invalid_session",
            Self::StoppedTransport => "stopped_transport",
            Self::StoppedVerification => "stopped_verification",
        }
    }

    /// Whether the outcome represents a completed (or dry-run) replay.
    pub const fn succeeded(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::CompletedWithSkips | Self::DryRun
        )
    }
}

impl From<ReplayOutcome> for ReplayStatus {
    fn from(value: ReplayOutcome) -> Self {
        match value {
            ReplayOutcome::Completed => Self::Completed,
            ReplayOutcome::CompletedWithSkips => Self::CompletedWithSkips,
            ReplayOutcome::DryRun => Self::DryRun,
            ReplayOutcome::StoppedPolicy => Self::StoppedPolicy,
            ReplayOutcome::StoppedInvalidSession => Self::StoppedInvalidSession,
            ReplayOutcome::StoppedTransport => Self::StoppedTransport,
            ReplayOutcome::StoppedVerification => Self::StoppedVerification,
        }
    }
}

/// Application-owned replayability classification for outer adapters.
/// Serialized representation matches the historical replayability contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayabilityStatus {
    FullyReplayable,
    PartiallyReplayable,
    NotReplayable,
}

impl From<Replayability> for ReplayabilityStatus {
    fn from(value: Replayability) -> Self {
        match value {
            Replayability::FullyReplayable => Self::FullyReplayable,
            Replayability::PartiallyReplayable => Self::PartiallyReplayable,
            Replayability::NotReplayable => Self::NotReplayable,
        }
    }
}

/// Application-owned per-operation execution state for outer adapters.
/// Serialized representation matches the historical execution-state contract.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationStatus {
    Completed,
    Failed,
    NotAttempted,
}

impl OperationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::NotAttempted => "not_attempted",
        }
    }
}

impl From<OperationExecutionState> for OperationStatus {
    fn from(value: OperationExecutionState) -> Self {
        match value {
            OperationExecutionState::Completed => Self::Completed,
            OperationExecutionState::Failed => Self::Failed,
            OperationExecutionState::NotAttempted => Self::NotAttempted,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ParentReplayEpochPlan {
    pub ordinal: u64,
    pub epoch_id: chronicle_common::EpochId,
    pub session_id: Option<chronicle_common::SessionId>,
    pub selected: bool,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ParentReplayPlan {
    pub parent_id: chronicle_common::RecordingId,
    pub epochs: Vec<ParentReplayEpochPlan>,
    pub selected_session_ids: Vec<chronicle_common::SessionId>,
    pub active_tail_omitted: bool,
    pub warnings: Vec<String>,
}

/// Build replay selection from parent-owned topology and published session
/// provenance. Raw WAL and continuation-only evidence never enter selection.
#[allow(clippy::too_many_lines)]
pub fn build_parent_replay_plan(
    data_dir: &Path,
    parent_id: chronicle_common::RecordingId,
) -> Result<ParentReplayPlan, ApplicationError> {
    let summaries = FilesystemSessionStore::new(data_dir)
        .list_summaries(chronicle_storage::SESSION_SUMMARY_LIMIT)
        .map_err(ApplicationError::from)?;
    let run_root = recordings_root(data_dir).join(parent_id.to_string());
    let run_path = run_root.join(RECORDING_RUN_FILE);
    let has_run = match fs::symlink_metadata(&run_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(ApplicationError::CatalogUnsafePath(run_path));
        }
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(ApplicationError::Io(error)),
    };
    if !has_run {
        let mut matches: Vec<_> = summaries
            .into_iter()
            .filter(|summary| {
                summary.recording_id == Some(parent_id)
                    || summary.epoch_id == Some(EpochId::from_uuid(parent_id.0))
            })
            .collect();
        matches.sort_by_key(|summary| (summary.epoch_ordinal.unwrap_or(0), summary.session_id));
        if matches.is_empty() {
            return Err(ApplicationError::RecordingNotFound(
                parent_id.to_cli_string(),
            ));
        }
        let epochs = matches
            .iter()
            .map(|summary| {
                Ok(ParentReplayEpochPlan {
                    ordinal: summary.epoch_ordinal.unwrap_or(0),
                    // Explicit epoch identity only: a session matched by
                    // recording provenance without an epoch identity is
                    // invalid lineage and fails closed rather than
                    // synthesizing an epoch from the parent id.
                    epoch_id: summary.epoch_id.ok_or_else(|| {
                        ApplicationError::RecordingMetadataValidation(
                            "session lacks explicit epoch identity".into(),
                        )
                    })?,
                    session_id: Some(summary.session_id),
                    selected: true,
                    reason: None,
                })
            })
            .collect::<Result<Vec<_>, ApplicationError>>()?;
        let selected_session_ids = matches.iter().map(|summary| summary.session_id).collect();
        return Ok(ParentReplayPlan {
            parent_id,
            epochs,
            selected_session_ids,
            active_tail_omitted: false,
            warnings: Vec::new(),
        });
    }

    let run = RecordingRunV2::load(&run_root)
        .map_err(|error| ApplicationError::RecordingMetadataValidation(error.to_string()))?;
    if run.parent_id != parent_id {
        return Err(ApplicationError::RecordingMetadataValidation(
            "run parent identity conflicts with requested parent".into(),
        ));
    }
    let mut epochs = Vec::with_capacity(run.epochs.len());
    let mut selected_session_ids = Vec::new();
    let mut warnings = run.warnings.clone();
    let mut active_tail_omitted = false;
    for epoch in &run.epochs {
        let summary = summaries.iter().find(|summary| {
            summary.recording_id == Some(parent_id) && summary.epoch_id == Some(epoch.epoch_id)
        });
        let active = epoch.state == EpochCatalogState::Active;
        let terminal_run = matches!(
            run.state,
            RunLifecycleState::Completed | RunLifecycleState::Stopped | RunLifecycleState::Failed
        );
        let selected = epoch.summary.published && summary.is_some() && (!active || terminal_run);
        let reason = if selected {
            let session_id = summary
                .ok_or_else(|| {
                    ApplicationError::RecordingMetadataValidation(format!(
                        "parent epoch {} has no session summary",
                        epoch.ordinal
                    ))
                })?
                .session_id;
            selected_session_ids.push(session_id);
            None
        } else if active && !terminal_run {
            active_tail_omitted = true;
            Some("active_epoch_not_selected".into())
        } else {
            return Err(ApplicationError::RecordingMetadataValidation(format!(
                "parent epoch {} is not a verified published session",
                epoch.ordinal
            )));
        };
        epochs.push(ParentReplayEpochPlan {
            ordinal: epoch.ordinal,
            epoch_id: epoch.epoch_id,
            session_id: summary.map(|summary| summary.session_id),
            selected,
            reason,
        });
    }
    if active_tail_omitted {
        warnings.push("active_epoch_not_selected".into());
    }
    Ok(ParentReplayPlan {
        parent_id,
        epochs,
        selected_session_ids,
        active_tail_omitted,
        warnings,
    })
}

/// Application-owned replay request passed by outer adapters. Plain request
/// data only; replay policy construction stays inside application.
#[derive(Clone, Debug)]
pub struct ReplayRequest {
    pub target: Option<String>,
    pub allow_hosts: Vec<String>,
    pub execute: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub timing: ReplayTiming,
}

impl From<&ReplayRequest> for LoopbackReplayOptions {
    fn from(request: &ReplayRequest) -> Self {
        Self {
            target: request.target.clone(),
            allow_hosts: request.allow_hosts.clone(),
            execute: request.execute,
            allow_reads: request.allow_reads,
            allow_writes: request.allow_writes,
            timing: match request.timing {
                ReplayTiming::Asap => TimingMode::Asap,
                ReplayTiming::Preserve => TimingMode::Preserve,
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplayOperationSummary {
    pub operation_id: String,
    pub decision: String,
    pub state: OperationStatus,
    pub attempted: bool,
    pub verification: String,
    pub category: String,
    pub transport_error: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReplayCounts {
    pub executable: usize,
    pub non_executable: usize,
    pub attempted: usize,
    pub completed: usize,
    pub failed: usize,
    pub unattempted: usize,
    pub verification_passed: usize,
    pub verification_failed: usize,
    pub verification_skipped: usize,
    pub verification_inconclusive: usize,
    pub verification_unsupported: usize,
    pub verification_not_run: usize,
}

impl ReplayCounts {
    pub(crate) fn from_operations(operations: &[ReplayOperationSummary]) -> Self {
        operations
            .iter()
            .fold(Self::default(), |mut counts, operation| {
                if operation.decision == "allowed" {
                    counts.executable += 1;
                } else {
                    counts.non_executable += 1;
                }
                counts.attempted += usize::from(operation.attempted);
                match operation.state {
                    OperationStatus::Completed => counts.completed += 1,
                    OperationStatus::Failed => counts.failed += 1,
                    OperationStatus::NotAttempted => counts.unattempted += 1,
                }
                match operation.verification.as_str() {
                    "passed" => counts.verification_passed += 1,
                    "failed" => counts.verification_failed += 1,
                    "skipped" => counts.verification_skipped += 1,
                    "inconclusive" => counts.verification_inconclusive += 1,
                    "unsupported" => counts.verification_unsupported += 1,
                    _ => counts.verification_not_run += 1,
                }
                counts
            })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplaySessionResult {
    pub session_id: String,
    pub replayability: ReplayabilityStatus,
    pub outcome: ReplayStatus,
    pub dry_run: bool,
    pub preflight_denied: bool,
    pub transport_failed: bool,
    pub counts: ReplayCounts,
    pub operations: Vec<ReplayOperationSummary>,
}

impl ReplaySessionResult {
    /// Application-owned success classification for outer adapters.
    pub const fn succeeded(&self) -> bool {
        self.outcome.succeeded()
    }
}

pub(crate) fn hydrate_replay_session(
    root: &Path,
    session_id: &str,
) -> Result<chronicle_canonical::CanonicalSession, ApplicationError> {
    let parsed = uuid::Uuid::parse_str(session_id)
        .map(chronicle_common::SessionId)
        .map_err(|_| ApplicationError::InvalidConfig("session ID must be a UUID".into()))?;
    Ok(FilesystemSessionStore::new(root).hydrate(parsed)?)
}

pub(crate) fn command_replay_plan(
    session: &chronicle_canonical::CanonicalSession,
    allow_writes: bool,
) -> Result<ReplayPlan, ApplicationError> {
    let targets = TargetMap {
        rules: session
            .connections
            .iter()
            .map(|connection| TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: connection.server.clone(),
            })
            .collect(),
    };
    let policy = ReplayPolicy {
        dry_run: false,
        allow_reads: true,
        allow_writes,
        block_recorded_destination: false,
        execution_authorized: true,
        ..ReplayPolicy::default()
    };
    Ok(ReplayPlanner::plan(
        session,
        &targets,
        &policy,
        TimingMode::Asap,
    )?)
}

/// Target-independent command-mode replay plan. Recorded endpoints fill the
/// planner's mandatory target slots only; destination blocking is deferred to
/// the discovered loopback target. No executor or network context is created.
/// Test-support: builds a production-format WAL directory from a capture fixture
/// so outer adapter contract tests can exercise the ETL compatibility paths
/// without capture/WAL dev dependencies in the CLI crate.
///
/// # Panics
///
/// Panics if the fixture WAL does not contain a committed marker or segment
/// authority needed to build the recording metadata.
pub fn production_wal_from_fixture(
    directory: impl AsRef<Path>,
    fixture: impl AsRef<Path>,
) -> Result<RecordingId, ApplicationError> {
    let directory = directory.as_ref();
    let recording_id = RecordingId::new();
    let fixture_bytes = std::fs::read(fixture.as_ref())?;
    let mut source = FixtureCaptureSource::from_json(&fixture_bytes)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    let mut writer = GroupCommitWalWriter::create(directory, recording_id, MIN_SEGMENT_BYTES, 1, 0)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    while let Some(event) = source
        .next_event()
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
    {
        let flags = match &event.kind {
            CaptureEventKind::PayloadFragment(fragment) => {
                u16::try_from(fragment.flags.0).unwrap_or_default()
            }
            _ => 0,
        };
        writer
            .append(
                RecordKind::CaptureEvent,
                CAPTURE_EVENT_SCHEMA_VERSION,
                flags,
                encode_event(&event)
                    .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?,
                0,
            )
            .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    }
    writer
        .flush(1)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?
        .ok_or_else(|| ApplicationError::InvalidConfig("fixture WAL flush failed".into()))?;
    // The committed writer authority is the recovery-authoritative boundary a
    // fresh scan would find. Re-acquiring the recording lock after dropping
    // the writer races with flock ownership on Linux (a second open of the
    // same file in this process conflicts while any other fd on the inode is
    // still alive, which concurrent test subprocesses can transiently
    // provide), so reuse the writer's own validated authority instead.
    let authority = writer.authority().clone();
    drop(writer);
    write_recording_metadata(
        directory,
        &RecordingMetadata {
            version: RECORDING_METADATA_SCHEMA_VERSION,
            recording_id,
            selector: None,
            status: RecordingStatus::Completed,
            shutdown_reason: Some(ShutdownReason::SourceCompleted),
            last_valid_commit: Some(RecordingCommitBoundary {
                marker_sequence: authority.marker_sequence.expect("marker"),
                durable_through_sequence: authority.durable_through_sequence.expect("boundary"),
                durable_record_count: authority.durable_record_count,
                durable_payload_bytes: authority.durable_payload_bytes,
                segment_ordinal: authority.segment_ordinal.expect("segment"),
            }),
            counters: RecordingCounters {
                committed: RecordByteCount {
                    records: authority.durable_record_count,
                    bytes: authority.durable_payload_bytes,
                },
                ..RecordingCounters::default()
            },
            terminal_wal_loss: None,
            capture: None,
        },
    )?;
    Ok(recording_id)
}

pub fn preplan_command_replay_session(
    root: impl AsRef<Path>,
    session_id: &str,
    allow_writes: bool,
) -> Result<ReplaySessionResult, ApplicationError> {
    let session = hydrate_replay_session(root.as_ref(), session_id)?;
    let plan = command_replay_plan(&session, allow_writes)?;
    Ok(replay_plan_result(session.id.to_string(), &plan))
}

/// Build a transient parent session from verified terminal epoch sessions.
///
/// Aggregate is never persisted and never reads raw WAL/continuation bytes. Each
/// selected epoch must hydrate and validate; duplicate operation IDs, conflicting
/// connection identity, foreign parent provenance, missing sessions, and the bounded
/// operation limit fail closed before replay planning.
#[allow(clippy::too_many_lines)]
fn aggregate_parent_session(
    root: &Path,
    parent_id: chronicle_common::RecordingId,
    session_ids: &[chronicle_common::SessionId],
) -> Result<chronicle_canonical::CanonicalSession, ApplicationError> {
    if session_ids.is_empty() {
        return Err(ApplicationError::InvalidConfig(
            "parent has no selected published epochs".into(),
        ));
    }
    let mut connections: Vec<chronicle_canonical::CanonicalConnection> = Vec::new();
    let mut connection_completeness = BTreeMap::new();
    let mut operation_completeness = BTreeMap::new();
    let mut timeline = Vec::new();
    let mut operation_ids = BTreeSet::new();
    let mut source_provenance = SourceProvenance {
        recording_id: Some(parent_id),
        status: SourceStatus::Completed,
        reason: Some("verified_parent_epoch_aggregate".into()),
        ..SourceProvenance::default()
    };
    let mut started_at: Option<chronicle_common::Timestamp> = None;
    let mut ended_at: Option<chronicle_common::Timestamp> = None;
    let mut epoch_base = 0_u64;
    let mut operation_count = 0_usize;

    for session_id in session_ids {
        let mut session = hydrate_replay_session(root, &session_id.to_string())?;
        session.validate().map_err(|error| {
            ApplicationError::InvalidConfig(format!(
                "parent epoch session {session_id} is invalid: {error}"
            ))
        })?;
        if session
            .source_provenance
            .recording_id
            .is_some_and(|recording_id| recording_id != parent_id)
        {
            return Err(ApplicationError::InvalidConfig(format!(
                "parent epoch session {session_id} belongs to another recording"
            )));
        }
        operation_count = operation_count
            .checked_add(
                session
                    .connections
                    .iter()
                    .map(|connection| connection.operations.len())
                    .sum::<usize>(),
            )
            .ok_or_else(|| {
                ApplicationError::InvalidConfig("parent operation count overflow".into())
            })?;
        if operation_count > chronicle_replay::MAX_REPLAY_OPERATIONS {
            return Err(ApplicationError::InvalidConfig(format!(
                "parent replay exceeds {} operations",
                chronicle_replay::MAX_REPLAY_OPERATIONS
            )));
        }
        started_at =
            Some(started_at.map_or(session.started_at, |value| value.min(session.started_at)));
        ended_at = match (ended_at, session.ended_at) {
            (Some(left), Some(right)) => Some(left.max(right)),
            (None, right) => right,
            (left, None) => left,
        };
        let session_status = session.source_provenance.status;
        source_provenance
            .evidence
            .extend(std::mem::take(&mut session.source_provenance.evidence));
        if session_status != SourceStatus::Completed {
            source_provenance.status = session_status;
        }

        let session_connection_completeness = std::mem::take(&mut session.connection_completeness);
        let session_operation_completeness = std::mem::take(&mut session.operation_completeness);
        let mut local_max = 0_u64;
        for mut connection in session.connections {
            let connection_id = connection.id;
            let connection_state = session_connection_completeness
                .get(&connection_id)
                .copied()
                .unwrap_or_default();
            for operation in &mut connection.operations {
                if !operation_ids.insert(operation.id) {
                    return Err(ApplicationError::InvalidConfig(format!(
                        "parent replay has duplicate operation {}",
                        operation.id
                    )));
                }
                local_max = local_max.max(operation.started_at_offset.0);
                local_max =
                    local_max.max(operation.completed_at_offset.map_or(0, |offset| offset.0));
                operation.started_at_offset = chronicle_canonical::RelativeTimeNanos(
                    operation
                        .started_at_offset
                        .0
                        .checked_add(epoch_base)
                        .ok_or_else(|| {
                            ApplicationError::InvalidConfig(
                                "parent replay timeline overflows".into(),
                            )
                        })?,
                );
                operation.completed_at_offset = operation
                    .completed_at_offset
                    .map(|offset| {
                        offset
                            .0
                            .checked_add(epoch_base)
                            .map(chronicle_canonical::RelativeTimeNanos)
                            .ok_or_else(|| {
                                ApplicationError::InvalidConfig(
                                    "parent replay timeline overflows".into(),
                                )
                            })
                    })
                    .transpose()?;
                operation_completeness.insert(
                    operation.id,
                    session_operation_completeness
                        .get(&operation.id)
                        .copied()
                        .unwrap_or_default(),
                );
            }
            if let Some(existing) = connections
                .iter_mut()
                .find(|value| value.id == connection_id)
            {
                if existing.protocol != connection.protocol
                    || existing.client != connection.client
                    || existing.server != connection.server
                {
                    return Err(ApplicationError::InvalidConfig(format!(
                        "parent replay has conflicting connection identity {connection_id}"
                    )));
                }
                existing.operations.extend(connection.operations);
            } else {
                connections.push(connection);
            }
            connection_completeness
                .entry(connection_id)
                .and_modify(|existing| {
                    if *existing == Completeness::Complete {
                        *existing = connection_state;
                    }
                })
                .or_insert(connection_state);
        }
        for mut entry in session.timeline {
            entry.offset = chronicle_canonical::RelativeTimeNanos(
                entry.offset.0.checked_add(epoch_base).ok_or_else(|| {
                    ApplicationError::InvalidConfig("parent replay timeline overflows".into())
                })?,
            );
            timeline.push(entry);
        }
        epoch_base = epoch_base
            .checked_add(local_max.saturating_add(1))
            .ok_or_else(|| {
                ApplicationError::InvalidConfig("parent replay timeline overflows".into())
            })?;
    }

    let aggregate = chronicle_canonical::CanonicalSession {
        schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
        id: chronicle_common::SessionId(parent_id.0),
        started_at: started_at.ok_or_else(|| {
            ApplicationError::InvalidConfig("parent has no session start timestamp".into())
        })?,
        ended_at,
        source: chronicle_canonical::SourceMetadata::default(),
        source_provenance,
        connections,
        connection_completeness,
        operation_completeness,
        timeline,
        replay: chronicle_canonical::ReplayMetadata::default(),
        replay_attributes: chronicle_canonical::Attributes::new(),
    };
    aggregate.validate().map_err(|error| {
        ApplicationError::InvalidConfig(format!("parent aggregate is invalid: {error}"))
    })?;
    Ok(aggregate)
}

/// Replays a verified parent aggregate through the same loopback-only planner
/// and executor used for one epoch. No aggregate artifact is written.
pub async fn replay_parent_session_with_plan<F>(
    root: impl AsRef<Path>,
    parent_id: chronicle_common::RecordingId,
    session_ids: &[chronicle_common::SessionId],
    config: &ReplayConfig,
    request: &ReplayRequest,
    on_plan: F,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
{
    let session = aggregate_parent_session(root.as_ref(), parent_id, session_ids)?;
    let options: LoopbackReplayOptions = request.into();
    let (targets, policy) = replay_command_inputs(&session, &options)?;
    let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap)?;
    on_plan(&replay_plan_result(parent_id.to_cli_string(), &plan));
    let mut context = ReplayContext::default();
    if request.execute && plan.is_executable() {
        context = replay_context(config)?;
        context.authorize_execution_for(options.validate_target()?.endpoint().clone());
    }
    let contexts = BTreeMap::from([(chronicle_common::ProtocolId::new("http/1.1"), context)]);
    let execution = ReplayExecutor::new(&chronicle_protocol_builtins::registry()?)
        .execute(&plan, &contexts)
        .await?;
    let transport_failed = execution
        .results
        .iter()
        .any(|result| result.transport_error.is_some());
    let operations: Vec<_> = plan
        .operations()
        .iter()
        .zip(execution.results)
        .map(|(planned, result)| ReplayOperationSummary {
            operation_id: result.operation_id.to_string(),
            decision: format!("{:?}", planned.decision()).to_lowercase(),
            state: OperationStatus::from(result.state),
            attempted: OperationStatus::from(result.state) != OperationStatus::NotAttempted,
            verification: result.verification.as_ref().map_or_else(
                || "not_run".into(),
                |verification| format!("{:?}", verification.status).to_lowercase(),
            ),
            category: result
                .verification
                .as_ref()
                .and_then(|verification| verification.details.get("category"))
                .cloned()
                .unwrap_or_default(),
            transport_error: result
                .transport_error
                .map(|category| format!("{category:?}").to_lowercase())
                .unwrap_or_default(),
        })
        .collect();
    Ok(ReplaySessionResult {
        session_id: parent_id.to_cli_string(),
        replayability: ReplayabilityStatus::from(plan.replayability()),
        outcome: ReplayStatus::from(execution.outcome),
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed,
        counts: ReplayCounts::from_operations(&operations),
        operations,
    })
}

/// Replays hydrated filesystem session through explicit loopback-only options.
pub async fn replay_session(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    request: &ReplayRequest,
) -> Result<ReplaySessionResult, ApplicationError> {
    replay_session_with_plan(root, session_id, config, request, |_| {}).await
}

/// Replays a session after exposing its complete plan, before any network I/O.
pub async fn replay_session_with_plan<F>(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    request: &ReplayRequest,
    on_plan: F,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
{
    replay_session_with_plan_guard(root, session_id, config, request, on_plan, || Ok(())).await
}

/// Same sole replay path with one zero-traffic guard immediately before any
/// execution context is authorized. Command mode uses it to revalidate owned
/// listener evidence; explicit-target callers use [`replay_session_with_plan`].
pub async fn replay_session_with_plan_guard<F, G>(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    request: &ReplayRequest,
    on_plan: F,
    before_execute: G,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
    G: FnOnce() -> Result<(), ApplicationError>,
{
    let session = hydrate_replay_session(root.as_ref(), session_id)?;
    let options: LoopbackReplayOptions = request.into();
    let (targets, policy) = replay_command_inputs(&session, &options)?;
    let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap)?;
    on_plan(&replay_plan_result(session.id.to_string(), &plan));
    let mut context = ReplayContext::default();
    if request.execute && plan.is_executable() {
        context = replay_context(config)?;
        before_execute()?;
        context.authorize_execution_for(options.validate_target()?.endpoint().clone());
    }
    let contexts = BTreeMap::from([(chronicle_common::ProtocolId::new("http/1.1"), context)]);
    let execution = ReplayExecutor::new(&chronicle_protocol_builtins::registry()?)
        .execute(&plan, &contexts)
        .await?;
    let transport_failed = execution
        .results
        .iter()
        .any(|result| result.transport_error.is_some());
    let operations: Vec<_> = plan
        .operations()
        .iter()
        .zip(execution.results)
        .map(|(planned, result)| ReplayOperationSummary {
            operation_id: result.operation_id.to_string(),
            decision: format!("{:?}", planned.decision()).to_lowercase(),
            state: OperationStatus::from(result.state),
            attempted: OperationStatus::from(result.state) != OperationStatus::NotAttempted,
            verification: result.verification.as_ref().map_or_else(
                || "not_run".into(),
                |verification| format!("{:?}", verification.status).to_lowercase(),
            ),
            category: result
                .verification
                .as_ref()
                .and_then(|verification| verification.details.get("category"))
                .cloned()
                .unwrap_or_default(),
            transport_error: result
                .transport_error
                .map(|category| format!("{category:?}").to_lowercase())
                .unwrap_or_default(),
        })
        .collect();
    Ok(ReplaySessionResult {
        session_id: session.id.to_string(),
        replayability: ReplayabilityStatus::from(plan.replayability()),
        outcome: ReplayStatus::from(execution.outcome),
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed,
        counts: ReplayCounts::from_operations(&operations),
        operations,
    })
}

pub(crate) fn replay_plan_result(session_id: String, plan: &ReplayPlan) -> ReplaySessionResult {
    let operations: Vec<_> = plan
        .operations()
        .iter()
        .map(|planned| ReplayOperationSummary {
            operation_id: planned.operation().id.to_string(),
            decision: format!("{:?}", planned.decision()).to_lowercase(),
            state: OperationStatus::NotAttempted,
            attempted: false,
            verification: "not_run".into(),
            category: String::new(),
            transport_error: String::new(),
        })
        .collect();
    ReplaySessionResult {
        session_id,
        replayability: ReplayabilityStatus::from(plan.replayability()),
        outcome: if plan.is_dry_run() {
            ReplayStatus::DryRun
        } else if plan.is_executable() {
            ReplayStatus::Completed
        } else if plan.has_policy_denial() {
            ReplayStatus::StoppedPolicy
        } else {
            ReplayStatus::StoppedInvalidSession
        },
        dry_run: plan.is_dry_run(),
        preflight_denied: !plan.is_executable(),
        transport_failed: false,
        counts: ReplayCounts::from_operations(&operations),
        operations,
    }
}

pub fn render_replay_human(result: &ReplaySessionResult) -> String {
    let mut output = format!(
        "session_id: {}\nreplayability: {:?}\noutcome: {}\ndry_run: {}\npreflight_denied: {}\ntransport_failed: {}\ncounts: executable={} non_executable={} attempted={} completed={} failed={} unattempted={} verification_passed={} verification_failed={} verification_skipped={} verification_inconclusive={} verification_unsupported={} verification_not_run={}\n",
        result.session_id,
        result.replayability,
        result.outcome.as_str(),
        result.dry_run,
        result.preflight_denied,
        result.transport_failed,
        result.counts.executable,
        result.counts.non_executable,
        result.counts.attempted,
        result.counts.completed,
        result.counts.failed,
        result.counts.unattempted,
        result.counts.verification_passed,
        result.counts.verification_failed,
        result.counts.verification_skipped,
        result.counts.verification_inconclusive,
        result.counts.verification_unsupported,
        result.counts.verification_not_run,
    );
    for operation in &result.operations {
        writeln!(
            output,
            "operation: id={} decision={} state={} attempted={} verification={} category={} transport_error={}",
            operation.operation_id,
            operation.decision,
            operation.state.as_str(),
            operation.attempted,
            operation.verification,
            optional_text(&operation.category),
            optional_text(&operation.transport_error),
        )
        .expect("writing into String cannot fail");
    }
    output
}

pub fn render_replay_json(result: &ReplaySessionResult) -> Result<String, ApplicationError> {
    render_json(&serde_json::json!({
        "version": REPLAY_REPORT_VERSION,
        "result": result
    }))
}

fn optional_text(value: &str) -> &str {
    if value.is_empty() { "none" } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{
        Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalOperation,
        OperationEffect, OperationKind, OperationProvenance, PROTOCOL_DATA_SCHEMA_VERSION,
        PayloadRef, ProtocolData, RelativeTimeNanos, ReplayMetadata, SourceMetadata, TimelineEntry,
    };
    use chronicle_storage::{FilesystemSessionStore, PublishSession};
    use std::collections::BTreeMap;
    use time::OffsetDateTime;

    fn session(parent_id: chronicle_common::RecordingId) -> chronicle_canonical::CanonicalSession {
        let connection_id = chronicle_common::ConnectionId::new();
        let operation_id = chronicle_common::OperationId::new();
        let operation = CanonicalOperation {
            id: operation_id,
            sequence: 1,
            started_at_offset: RelativeTimeNanos(0),
            completed_at_offset: Some(RelativeTimeNanos(1)),
            kind: OperationKind::Request,
            effect: OperationEffect::Read,
            request: PayloadRef::Inline {
                content_type: Some("text/plain".into()),
                bytes: b"request".to_vec(),
            },
            recorded_response: Some(PayloadRef::Inline {
                content_type: Some("text/plain".into()),
                bytes: b"response".to_vec(),
            }),
            attributes: Attributes::new(),
            protocol_data: ProtocolData {
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: None,
                bytes: Vec::new(),
            },
            provenance: OperationProvenance::default(),
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        chronicle_canonical::CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: chronicle_common::SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: Some(OffsetDateTime::UNIX_EPOCH),
            source: SourceMetadata::default(),
            source_provenance: SourceProvenance {
                recording_id: Some(parent_id),
                epoch_id: Some(chronicle_common::EpochId::new()),
                status: SourceStatus::Completed,
                ..SourceProvenance::default()
            },
            connections: vec![CanonicalConnection {
                id: connection_id,
                protocol: chronicle_common::ProtocolId::new("http/1.1"),
                client: chronicle_common::Endpoint::new("127.0.0.1", 1),
                server: chronicle_common::Endpoint::new("127.0.0.1", 2),
                attributes: Attributes::new(),
                operations: vec![operation],
            }],
            connection_completeness: BTreeMap::from([(connection_id, Completeness::Complete)]),
            operation_completeness: BTreeMap::from([(operation_id, Completeness::Complete)]),
            timeline: vec![TimelineEntry {
                connection_id,
                operation_id,
                offset: RelativeTimeNanos(0),
            }],
            replay: ReplayMetadata::default(),
            replay_attributes: Attributes::new(),
        }
    }

    fn publish(root: &std::path::Path, session: chronicle_canonical::CanonicalSession) {
        FilesystemSessionStore::new(root)
            .publish(PublishSession {
                session,
                checkpoint: None,
                issues: Vec::new(),
                replayability: Vec::new(),
                complete: true,
            })
            .unwrap();
    }

    #[test]
    fn parent_aggregate_orders_verified_epochs_and_normalizes_offsets() {
        let root =
            std::env::temp_dir().join(format!("chronicle-parent-replay-{}", uuid::Uuid::new_v4()));
        let parent_id = chronicle_common::RecordingId::new();
        let first = session(parent_id);
        let first_id = first.id;
        let mut second = session(parent_id);
        let second_id = second.id;
        let shared_connection_id = first.connections[0].id;
        second.connections[0].id = shared_connection_id;
        second.connection_completeness.clear();
        second
            .connection_completeness
            .insert(shared_connection_id, Completeness::Complete);
        second.timeline[0].connection_id = shared_connection_id;
        publish(&root, first);
        publish(&root, second);

        let aggregate = aggregate_parent_session(&root, parent_id, &[first_id, second_id]).unwrap();
        assert_eq!(aggregate.id, chronicle_common::SessionId(parent_id.0));
        assert_eq!(aggregate.connections.len(), 1);
        assert_eq!(aggregate.connections[0].operations.len(), 2);
        assert_eq!(aggregate.timeline.len(), 2);
        assert!(aggregate.connections[0].operations[1].started_at_offset.0 > 0);
        assert!(aggregate.validate().is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_without_explicit_provenance_is_unresolved_for_parent_replay() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-parent-replay-unprovenanced-{}",
            uuid::Uuid::new_v4()
        ));
        let parent_id = chronicle_common::RecordingId::new();
        let mut orphan = session(chronicle_common::RecordingId::new());
        orphan.id = chronicle_common::SessionId(parent_id.0);
        orphan.source_provenance.recording_id = None;
        orphan.source_provenance.epoch_id = None;
        publish(&root, orphan);

        // Identifier equality between the session id and the parent id is
        // not lineage: a session without explicit provenance is unresolved.
        assert!(matches!(
            build_parent_replay_plan(&root, parent_id),
            Err(ApplicationError::RecordingNotFound(_))
        ));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_with_explicit_epoch_identity_resolves_for_parent_replay() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-parent-replay-explicit-{}",
            uuid::Uuid::new_v4()
        ));
        let parent_id = chronicle_common::RecordingId::new();
        let epoch_id = chronicle_common::EpochId::new();
        let mut explicit = session(parent_id);
        explicit.id = chronicle_common::SessionId(epoch_id.as_uuid());
        explicit.source_provenance.recording_id = Some(parent_id);
        explicit.source_provenance.epoch_id = Some(epoch_id);
        explicit.source_provenance.epoch_ordinal = Some(0);
        publish(&root, explicit);

        let plan = build_parent_replay_plan(&root, parent_id).unwrap();
        assert_eq!(plan.epochs.len(), 1);
        assert_eq!(plan.epochs[0].epoch_id, epoch_id);
        assert!(plan.epochs[0].selected);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn parent_aggregate_rejects_foreign_provenance() {
        let root = std::env::temp_dir().join(format!(
            "chronicle-parent-replay-foreign-{}",
            uuid::Uuid::new_v4()
        ));
        let parent_id = chronicle_common::RecordingId::new();
        let foreign = session(chronicle_common::RecordingId::new());
        let foreign_id = foreign.id;
        publish(&root, foreign);
        assert!(matches!(
            aggregate_parent_session(&root, parent_id, &[foreign_id]),
            Err(ApplicationError::InvalidConfig(message)) if message.contains("another recording")
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
