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
    /// Retained compatibility flag: true only for fully replayable sessions.
    pub replayable: bool,
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
    let session_id = chronicle_common::SessionId::new();
    let wal_directory = root.join("wal").join(session_id.to_string());
    let recorded = write_capture_to_wal(source, &wal_directory, max_segment_bytes)?;
    let (output, checkpoint) = process_fixture_wal(
        &wal_directory,
        recorded.recording_id,
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
        replayable: replayability == Replayability::FullyReplayable,
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
        "session_id: {}\ncomplete: {}\nreplayable: {}\nreplayability: {:?}\noperations: executable={} non_executable={}\nintegrity_valid: {}\nsource_status: {:?}\nsource_reason: {}\nblockers: {}\nissues: {}\n",
        inspected.session_id,
        inspected.complete,
        inspected.replayable,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReplayOperationSummary {
    pub operation_id: String,
    pub decision: String,
    pub state: OperationExecutionState,
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
                    OperationExecutionState::Completed => counts.completed += 1,
                    OperationExecutionState::Failed => counts.failed += 1,
                    OperationExecutionState::NotAttempted => counts.unattempted += 1,
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
    pub replayability: Replayability,
    pub outcome: ReplayOutcome,
    pub dry_run: bool,
    pub preflight_denied: bool,
    pub transport_failed: bool,
    pub counts: ReplayCounts,
    pub operations: Vec<ReplayOperationSummary>,
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
    drop(writer);
    let scan = scan_wal(directory, recording_id, DEFAULT_MAX_RECORD_BYTES)
        .map_err(|error| ApplicationError::InvalidConfig(error.to_string()))?;
    let authority = scan.authority;
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

/// Replays hydrated filesystem session through explicit loopback-only options.
pub async fn replay_session(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    options: &LoopbackReplayOptions,
) -> Result<ReplaySessionResult, ApplicationError> {
    replay_session_with_plan(root, session_id, config, options, |_| {}).await
}

/// Replays a session after exposing its complete plan, before any network I/O.
pub async fn replay_session_with_plan<F>(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    options: &LoopbackReplayOptions,
    on_plan: F,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
{
    replay_session_with_plan_guard(root, session_id, config, options, on_plan, || Ok(())).await
}

/// Same sole replay path with one zero-traffic guard immediately before any
/// execution context is authorized. Command mode uses it to revalidate owned
/// listener evidence; explicit-target callers use [`replay_session_with_plan`].
pub async fn replay_session_with_plan_guard<F, G>(
    root: impl AsRef<Path>,
    session_id: &str,
    config: &ReplayConfig,
    options: &LoopbackReplayOptions,
    on_plan: F,
    before_execute: G,
) -> Result<ReplaySessionResult, ApplicationError>
where
    F: FnOnce(&ReplaySessionResult),
    G: FnOnce() -> Result<(), ApplicationError>,
{
    let session = hydrate_replay_session(root.as_ref(), session_id)?;
    let (targets, policy) = replay_command_inputs(&session, options)?;
    let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap)?;
    on_plan(&replay_plan_result(session.id.to_string(), &plan));
    let mut context = ReplayContext::default();
    if options.execute && plan.is_executable() {
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
            state: result.state,
            attempted: result.state != OperationExecutionState::NotAttempted,
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
        replayability: plan.replayability(),
        outcome: execution.outcome,
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
            state: OperationExecutionState::NotAttempted,
            attempted: false,
            verification: "not_run".into(),
            category: String::new(),
            transport_error: String::new(),
        })
        .collect();
    ReplaySessionResult {
        session_id,
        replayability: plan.replayability(),
        outcome: if plan.is_dry_run() {
            ReplayOutcome::DryRun
        } else if plan.is_executable() {
            ReplayOutcome::Completed
        } else if plan.has_policy_denial() {
            ReplayOutcome::StoppedPolicy
        } else {
            ReplayOutcome::StoppedInvalidSession
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
