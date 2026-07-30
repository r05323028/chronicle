//! Replay planning, target mapping, execution, and verification orchestration.

use chronicle_canonical::{
    CanonicalOperation, CanonicalSession, Completeness, OperationEffect, RelativeTimeNanos,
};
use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId};
use chronicle_protocol::{
    ProtocolError, ProtocolRegistry, ReplayContext, VerificationResult, VerificationStatus,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use thiserror::Error;

pub const MAX_REPLAY_OPERATIONS: usize = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingMode {
    Preserve,
    Asap,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetRule {
    pub protocol: Option<ProtocolId>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub connection_id: Option<ConnectionId>,
    pub target: Endpoint,
}

impl TargetRule {
    fn matches(
        &self,
        protocol: &ProtocolId,
        endpoint: &Endpoint,
        connection_id: ConnectionId,
    ) -> bool {
        self.protocol.as_ref().is_none_or(|value| value == protocol)
            && self
                .host
                .as_ref()
                .is_none_or(|value| value == &endpoint.host)
            && self.port.is_none_or(|value| value == endpoint.port)
            && self
                .connection_id
                .is_none_or(|value| value == connection_id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TargetMap {
    pub rules: Vec<TargetRule>,
}

impl TargetMap {
    pub fn resolve(
        &self,
        protocol: &ProtocolId,
        endpoint: &Endpoint,
        connection_id: ConnectionId,
    ) -> Option<&Endpoint> {
        self.rules
            .iter()
            .find(|rule| rule.matches(protocol, endpoint, connection_id))
            .map(|rule| &rule.target)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)] // Independent safety gates stay explicit and default-deny.
pub struct ReplayPolicy {
    pub dry_run: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub allow_publication: bool,
    pub allow_authentication: bool,
    pub allow_unknown: bool,
    pub block_recorded_destination: bool,
    /// Explicit execution opt-in. HTTP adapters also require loopback IP targets.
    pub execution_authorized: bool,
}

impl Default for ReplayPolicy {
    fn default() -> Self {
        Self {
            dry_run: true,
            allow_reads: false,
            allow_writes: false,
            allow_publication: false,
            allow_authentication: false,
            allow_unknown: false,
            block_recorded_destination: true,
            execution_authorized: false,
        }
    }
}

impl ReplayPolicy {
    pub fn allows(&self, effect: OperationEffect) -> bool {
        match effect {
            OperationEffect::Read => self.allow_reads,
            OperationEffect::Write => self.allow_writes,
            OperationEffect::Publish => self.allow_publication,
            OperationEffect::Authentication => self.allow_authentication,
            OperationEffect::Unknown => self.allow_unknown,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopbackTarget {
    endpoint: Endpoint,
    ip: IpAddr,
}

impl LoopbackTarget {
    pub fn parse(origin: &str) -> Result<Self, ReplayTargetError> {
        let authority = origin
            .strip_prefix("http://")
            .ok_or(ReplayTargetError::PlainHttpRequired)?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        let address: SocketAddr = authority
            .parse()
            .map_err(|_| ReplayTargetError::InvalidOrigin)?;
        if !address.ip().is_loopback() {
            return Err(ReplayTargetError::LoopbackRequired);
        }
        Ok(Self {
            endpoint: Endpoint::new(address.ip().to_string(), address.port()),
            ip: address.ip(),
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopbackReplayOptions {
    pub target: Option<String>,
    pub allow_hosts: Vec<String>,
    pub execute: bool,
    pub allow_reads: bool,
    pub allow_writes: bool,
    pub timing: TimingMode,
}

impl LoopbackReplayOptions {
    pub fn validate_target(&self) -> Result<LoopbackTarget, ReplayTargetError> {
        if self.timing != TimingMode::Asap {
            return Err(ReplayTargetError::ImmediateTimingRequired);
        }
        let target = LoopbackTarget::parse(
            self.target
                .as_deref()
                .ok_or(ReplayTargetError::TargetRequired)?,
        )?;
        let allowed_hosts: Result<Vec<IpAddr>, _> = self
            .allow_hosts
            .iter()
            .map(|host| host.parse::<IpAddr>())
            .collect();
        if !allowed_hosts
            .map_err(|_| ReplayTargetError::InvalidAllowHost)?
            .contains(&target.ip)
        {
            return Err(ReplayTargetError::AllowHostMismatch);
        }
        Ok(target)
    }

    pub fn target_map_for(
        &self,
        session: &CanonicalSession,
    ) -> Result<TargetMap, ReplayTargetError> {
        let target = self.validate_target()?;
        Ok(TargetMap {
            rules: session
                .connections
                .iter()
                .map(|connection| TargetRule {
                    protocol: None,
                    host: None,
                    port: None,
                    connection_id: Some(connection.id),
                    target: target.endpoint.clone(),
                })
                .collect(),
        })
    }

    pub fn policy(&self) -> ReplayPolicy {
        ReplayPolicy {
            dry_run: !self.execute,
            allow_reads: self.allow_reads,
            allow_writes: self.allow_writes,
            execution_authorized: self.execute,
            ..ReplayPolicy::default()
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReplayTargetError {
    #[error("replay target is required")]
    TargetRequired,
    #[error("replay target must use plain http")]
    PlainHttpRequired,
    #[error("replay target must be an IP-literal origin with port")]
    InvalidOrigin,
    #[error("replay target must use a loopback IP address")]
    LoopbackRequired,
    #[error("--allow-host must be an IP literal")]
    InvalidAllowHost,
    #[error("--allow-host must exactly match replay target")]
    AllowHostMismatch,
    #[error("only immediate replay timing is supported")]
    ImmediateTimingRequired,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayDecision {
    Allowed,
    Denied(ReplayDenial),
    Unsupported,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayDenial {
    MissingTarget,
    RecordedDestination,
    ExecutionNotAuthorized,
    EffectNotAllowed(OperationEffect),
}

#[derive(Clone, Debug)]
pub struct PlannedOperation {
    connection_id: ConnectionId,
    protocol: ProtocolId,
    target: Option<Endpoint>,
    recorded_target: Endpoint,
    decision: ReplayDecision,
    scheduled_offset: RelativeTimeNanos,
    completeness: Completeness,
    operation: CanonicalOperation,
}

impl PlannedOperation {
    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub fn protocol(&self) -> &ProtocolId {
        &self.protocol
    }

    pub fn target(&self) -> Option<&Endpoint> {
        self.target.as_ref()
    }

    pub fn decision(&self) -> &ReplayDecision {
        &self.decision
    }

    pub fn is_allowed(&self) -> bool {
        self.decision == ReplayDecision::Allowed
    }

    pub fn recorded_target(&self) -> &Endpoint {
        &self.recorded_target
    }

    pub fn scheduled_offset(&self) -> RelativeTimeNanos {
        self.scheduled_offset
    }

    pub fn completeness(&self) -> &Completeness {
        &self.completeness
    }

    pub fn operation(&self) -> &CanonicalOperation {
        &self.operation
    }
}

#[derive(Clone, Debug)]
pub struct ReplayPlan {
    timing: TimingMode,
    dry_run: bool,
    invalid_session: bool,
    operations: Vec<PlannedOperation>,
}

impl ReplayPlan {
    pub fn timing(&self) -> TimingMode {
        self.timing
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    pub fn operations(&self) -> &[PlannedOperation] {
        &self.operations
    }

    /// True when at least one operation can execute and no executable candidate is policy denied.
    pub fn is_executable(&self) -> bool {
        !self.invalid_session && self.has_executable() && !self.has_policy_denial()
    }

    pub fn has_executable(&self) -> bool {
        self.operations.iter().any(PlannedOperation::is_allowed)
    }

    pub fn has_policy_denial(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| matches!(operation.decision, ReplayDecision::Denied(_)))
    }

    pub fn replayability(&self) -> Replayability {
        if self.invalid_session {
            return Replayability::NotReplayable;
        }
        let executable = self
            .operations
            .iter()
            .filter(|operation| operation.is_allowed())
            .count();
        match (executable, self.operations.len()) {
            (0, _) => Replayability::NotReplayable,
            (count, total) if count == total => Replayability::FullyReplayable,
            _ => Replayability::PartiallyReplayable,
        }
    }
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("no explicit replay target mapping for connection {connection_id}")]
    MissingTarget { connection_id: ConnectionId },
    #[error("replay policy denied {effect:?} operation {operation_id}")]
    PolicyDenied {
        operation_id: OperationId,
        effect: OperationEffect,
    },
    #[error("replay target equals recorded destination for connection {connection_id}")]
    RecordedDestination { connection_id: ConnectionId },
    #[error("dry-run plans cannot execute")]
    DryRun,
    #[error("replay plan contains denied or unsupported operations")]
    PreflightDenied,
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
}

pub struct ReplayPlanner;

impl ReplayPlanner {
    pub fn plan(
        session: &CanonicalSession,
        targets: &TargetMap,
        policy: &ReplayPolicy,
        timing: TimingMode,
    ) -> Result<ReplayPlan, ReplayError> {
        let invalid_session = session
            .connections
            .iter()
            .map(|connection| connection.operations.len())
            .sum::<usize>()
            > MAX_REPLAY_OPERATIONS;
        let mut operations = Vec::new();
        for connection in &session.connections {
            let (target, connection_decision) =
                match targets.resolve(&connection.protocol, &connection.server, connection.id) {
                    None => (
                        None,
                        Some(ReplayDecision::Denied(ReplayDenial::MissingTarget)),
                    ),
                    Some(target)
                        if policy.block_recorded_destination && target == &connection.server =>
                    {
                        (
                            Some(target.clone()),
                            Some(ReplayDecision::Denied(ReplayDenial::RecordedDestination)),
                        )
                    }
                    Some(target) => (Some(target.clone()), None),
                };
            let connection_complete =
                session.connection_state(&connection.id) == Completeness::Complete;
            for operation in &connection.operations {
                let completeness = session.operation_state(&operation.id);
                let decision = if invalid_session
                    || !connection_complete
                    || completeness != Completeness::Complete
                    || operation.recorded_response.is_none()
                    || operation
                        .attributes
                        .get("chronicle.replayable")
                        .is_some_and(|value| value == "false")
                {
                    ReplayDecision::Unsupported
                } else if connection.protocol.as_str() == "http/1.1" && !policy.execution_authorized
                {
                    ReplayDecision::Denied(ReplayDenial::ExecutionNotAuthorized)
                } else if operation
                    .attributes
                    .get("chronicle.replay.requires_runtime_authorization")
                    .is_some_and(|value| value == "true")
                    && !policy.allow_authentication
                {
                    ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(
                        OperationEffect::Authentication,
                    ))
                } else {
                    connection_decision.clone().unwrap_or_else(|| {
                        if policy.allows(operation.effect) {
                            ReplayDecision::Allowed
                        } else {
                            ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(operation.effect))
                        }
                    })
                };
                operations.push(PlannedOperation {
                    connection_id: connection.id,
                    protocol: connection.protocol.clone(),
                    target: target.clone(),
                    recorded_target: connection.server.clone(),
                    decision,
                    scheduled_offset: match timing {
                        TimingMode::Preserve => operation.started_at_offset,
                        TimingMode::Asap => RelativeTimeNanos(0),
                    },
                    completeness,
                    operation: operation.clone(),
                });
            }
        }
        operations
            .sort_by_key(|operation| (operation.scheduled_offset, operation.operation.sequence));
        Ok(ReplayPlan {
            timing,
            dry_run: policy.dry_run,
            invalid_session,
            operations,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReplayOutcome {
    Completed,
    CompletedWithSkips,
    DryRun,
    StoppedPolicy,
    StoppedInvalidSession,
    StoppedTransport,
    StoppedVerification,
}

impl ReplayOutcome {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationExecutionState {
    Completed,
    Failed,
    NotAttempted,
}

impl OperationExecutionState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::NotAttempted => "not_attempted",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Replayability {
    FullyReplayable,
    PartiallyReplayable,
    NotReplayable,
}

#[derive(Clone, Debug)]
pub struct OperationReplayResult {
    pub operation_id: OperationId,
    pub state: OperationExecutionState,
    pub verification: Option<VerificationResult>,
    pub transport_error: Option<chronicle_protocol::TransportErrorCategory>,
}

#[derive(Clone, Debug)]
pub struct ReplayExecution {
    pub outcome: ReplayOutcome,
    pub results: Vec<OperationReplayResult>,
}

pub struct ReplayExecutor<'a> {
    registry: &'a ProtocolRegistry,
}

impl<'a> ReplayExecutor<'a> {
    pub fn new(registry: &'a ProtocolRegistry) -> Self {
        Self { registry }
    }

    pub async fn execute(
        &self,
        plan: &ReplayPlan,
        contexts: &BTreeMap<ProtocolId, ReplayContext>,
    ) -> Result<ReplayExecution, ReplayError> {
        if plan.dry_run || !plan.is_executable() {
            let (outcome, category) = if plan.dry_run {
                (ReplayOutcome::DryRun, "dry_run")
            } else if plan.has_policy_denial() {
                (ReplayOutcome::StoppedPolicy, "preflight_denied")
            } else {
                (ReplayOutcome::StoppedInvalidSession, "non_executable")
            };
            return Ok(ReplayExecution {
                outcome,
                results: plan
                    .operations
                    .iter()
                    .map(|planned| no_run_result(planned, category))
                    .collect(),
            });
        }
        let mut results = Vec::with_capacity(plan.operations.len());
        let mut outcome = ReplayOutcome::Completed;
        let mut stopped = false;
        for planned in &plan.operations {
            if !planned.is_allowed() {
                results.push(no_run_result(planned, "non_executable"));
                if outcome == ReplayOutcome::Completed {
                    outcome = ReplayOutcome::CompletedWithSkips;
                }
                continue;
            }
            if stopped {
                results.push(no_run_result(planned, "unattempted_after_stop"));
                continue;
            }
            let target = planned.target.as_ref().ok_or(ReplayError::MissingTarget {
                connection_id: planned.connection_id,
            })?;
            let registration = self
                .registry
                .get(&planned.protocol)
                .ok_or_else(|| ProtocolError::NotRegistered(planned.protocol.clone()))?;
            let adapter = registration
                .replay_adapter
                .as_ref()
                .ok_or(ProtocolError::CapabilityUnavailable("replay"))?;
            let verifier = registration
                .verifier
                .as_ref()
                .ok_or(ProtocolError::CapabilityUnavailable("verification"))?;
            let context = contexts.get(&planned.protocol).cloned().unwrap_or_default();
            let mut connection = match adapter.connect(target, &context).await {
                Ok(connection) => connection,
                Err(ProtocolError::Transport { category, .. }) => {
                    results.push(transport_result(planned, category));
                    outcome = ReplayOutcome::StoppedTransport;
                    stopped = true;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let observed = match connection.execute(&planned.operation).await {
                Ok(observed) => observed,
                Err(ProtocolError::Transport { category, .. }) => {
                    results.push(transport_result(planned, category));
                    outcome = ReplayOutcome::StoppedTransport;
                    stopped = true;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let verification = verifier.verify(&planned.operation, &observed);
            stopped = verification.status != VerificationStatus::Passed;
            if stopped {
                outcome = ReplayOutcome::StoppedVerification;
            }
            results.push(OperationReplayResult {
                operation_id: planned.operation.id,
                state: OperationExecutionState::Completed,
                verification: Some(verification),
                transport_error: None,
            });
        }
        Ok(ReplayExecution { outcome, results })
    }
}

fn no_run_result(planned: &PlannedOperation, category: &str) -> OperationReplayResult {
    OperationReplayResult {
        operation_id: planned.operation.id,
        state: OperationExecutionState::NotAttempted,
        verification: Some(verification(
            VerificationStatus::Skipped,
            "verification was not run",
            category,
        )),
        transport_error: None,
    }
}

fn transport_result(
    planned: &PlannedOperation,
    category: chronicle_protocol::TransportErrorCategory,
) -> OperationReplayResult {
    OperationReplayResult {
        operation_id: planned.operation.id,
        state: OperationExecutionState::Failed,
        verification: None,
        transport_error: Some(category),
    }
}

fn verification(status: VerificationStatus, summary: &str, category: &str) -> VerificationResult {
    VerificationResult {
        status,
        summary: summary.into(),
        details: BTreeMap::from([("category".into(), category.into())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{
        Attributes, CANONICAL_SCHEMA_VERSION, CanonicalConnection, CanonicalSession, OperationKind,
        PayloadRef, ProtocolData, ReplayMetadata, SourceMetadata,
    };
    use chronicle_common::{OperationId, SessionId};
    use chronicle_protocol::{
        CapabilityStatus, ProtocolCapabilities, ProtocolRegistration, ReplayAdapter,
        VerificationStatus,
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use time::OffsetDateTime;

    fn session(effect: OperationEffect) -> CanonicalSession {
        let connection_id = ConnectionId::new();
        let operation_id = OperationId::new();
        CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            source: SourceMetadata::default(),
            source_provenance: Default::default(),
            connections: vec![CanonicalConnection {
                id: connection_id,
                protocol: ProtocolId::new("fake"),
                client: Endpoint::new("client", 1),
                server: Endpoint::new("production", 2),
                attributes: Attributes::new(),
                operations: vec![CanonicalOperation {
                    id: operation_id,
                    sequence: 1,
                    started_at_offset: RelativeTimeNanos(0),
                    completed_at_offset: None,
                    kind: OperationKind::Request,
                    effect,
                    request: PayloadRef::Missing {
                        reason: "test".into(),
                    },
                    recorded_response: Some(PayloadRef::Inline {
                        content_type: None,
                        bytes: Vec::new(),
                    }),
                    attributes: Attributes::new(),
                    protocol_data: ProtocolData {
                        schema_version: 1,
                        media_type: None,
                        bytes: Vec::new(),
                    },
                    redactions: Vec::new(),
                    warnings: Vec::new(),
                }],
            }],
            connection_completeness: [(connection_id, Completeness::Complete)].into(),
            operation_completeness: [(operation_id, Completeness::Complete)].into(),
            timeline: Vec::new(),
            replay: ReplayMetadata::default(),
            replay_attributes: Attributes::new(),
        }
    }

    #[test]
    fn target_mapping_is_mandatory() {
        let plan = ReplayPlanner::plan(
            &session(OperationEffect::Read),
            &TargetMap::default(),
            &ReplayPolicy::default(),
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(plan.operations()[0].target(), None);
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Denied(ReplayDenial::MissingTarget)
        );
    }

    #[test]
    fn default_policy_denies_operations() {
        let session = session(OperationEffect::Write);
        let connection = &session.connections[0];
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("test", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy::default(),
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(OperationEffect::Write))
        );
    }

    #[tokio::test]
    async fn planner_constructs_read_only_plan_and_dry_run_cannot_execute() {
        let session = session(OperationEffect::Read);
        let connection = &session.connections[0];
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("test", 2),
            }],
        };
        let policy = ReplayPolicy {
            allow_reads: true,
            ..ReplayPolicy::default()
        };
        let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap).unwrap();

        assert!(plan.is_dry_run());
        assert_eq!(plan.timing(), TimingMode::Asap);
        assert!(plan.is_executable());
        assert_eq!(plan.operations().len(), 1);
        assert_eq!(
            plan.operations()[0].target(),
            Some(&Endpoint::new("test", 2))
        );
        let results = ReplayExecutor::new(&ProtocolRegistry::new())
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(results.outcome, ReplayOutcome::DryRun);
        assert_eq!(results.results.len(), 1);
        assert_eq!(
            results.results[0].state,
            OperationExecutionState::NotAttempted
        );
        assert_eq!(
            results.results[0].verification.as_ref().unwrap().status,
            VerificationStatus::Skipped
        );
    }

    #[test]
    fn replay_contract_enums_have_stable_json_names() {
        for (value, expected) in [
            (ReplayOutcome::Completed, "\"completed\""),
            (
                ReplayOutcome::CompletedWithSkips,
                "\"completed_with_skips\"",
            ),
            (ReplayOutcome::DryRun, "\"dry_run\""),
            (ReplayOutcome::StoppedPolicy, "\"stopped_policy\""),
            (
                ReplayOutcome::StoppedInvalidSession,
                "\"stopped_invalid_session\"",
            ),
            (ReplayOutcome::StoppedTransport, "\"stopped_transport\""),
            (
                ReplayOutcome::StoppedVerification,
                "\"stopped_verification\"",
            ),
        ] {
            assert_eq!(serde_json::to_string(&value).unwrap(), expected);
            assert_eq!(
                serde_json::from_str::<ReplayOutcome>(expected).unwrap(),
                value
            );
        }
        assert_eq!(
            serde_json::to_string(&OperationExecutionState::NotAttempted).unwrap(),
            "\"not_attempted\""
        );
        assert_eq!(
            serde_json::to_string(&Replayability::PartiallyReplayable).unwrap(),
            "\"partially_replayable\""
        );
    }

    #[test]
    fn loopback_target_rejects_non_origin_and_non_loopback_inputs() {
        let ipv4 = LoopbackTarget::parse("http://127.0.0.1:8080/").unwrap();
        assert_eq!(ipv4.endpoint(), &Endpoint::new("127.0.0.1", 8080));
        let ipv6 = LoopbackTarget::parse("http://[::1]:8080").unwrap();
        assert_eq!(ipv6.endpoint(), &Endpoint::new("::1", 8080));

        for origin in [
            "https://127.0.0.1:8080",
            "http://localhost:8080",
            "http://127.0.0.1:8080/path",
            "http://127.0.0.1:8080?query",
            "http://127.0.0.1:8080#fragment",
            "http://user@127.0.0.1:8080",
            "http://127.0.0.1",
            "http://0.0.0.0:8080",
            "http://192.0.2.1:8080",
            "http://224.0.0.1:8080",
        ] {
            assert!(LoopbackTarget::parse(origin).is_err(), "{origin}");
        }
    }

    #[test]
    fn loopback_options_require_matching_allow_host_and_asap_timing() {
        let options = LoopbackReplayOptions {
            target: Some("http://127.0.0.1:8080".into()),
            allow_hosts: vec!["127.0.0.2".into()],
            execute: false,
            allow_reads: false,
            allow_writes: false,
            timing: TimingMode::Asap,
        };
        assert!(matches!(
            options.validate_target(),
            Err(ReplayTargetError::AllowHostMismatch)
        ));

        let preserve = LoopbackReplayOptions {
            allow_hosts: vec!["127.0.0.1".into()],
            timing: TimingMode::Preserve,
            ..options
        };
        assert!(matches!(
            preserve.validate_target(),
            Err(ReplayTargetError::ImmediateTimingRequired)
        ));

        let dry_run = LoopbackReplayOptions {
            target: Some("http://127.0.0.1:8080".into()),
            allow_hosts: vec!["127.0.0.1".into()],
            execute: false,
            allow_reads: false,
            allow_writes: false,
            timing: TimingMode::Asap,
        };
        assert!(dry_run.policy().dry_run);
        assert!(!dry_run.policy().allows(OperationEffect::Read));
        let execute = LoopbackReplayOptions {
            execute: true,
            allow_reads: true,
            ..dry_run
        };
        assert!(!execute.policy().dry_run);
        assert!(execute.policy().allows(OperationEffect::Read));
        assert!(!execute.policy().allows(OperationEffect::Write));
    }

    #[test]
    fn plan_retains_read_write_and_unknown_decisions() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        let mut write = connection.operations[0].clone();
        write.id = OperationId::new();
        write.sequence = 2;
        write.effect = OperationEffect::Write;
        let mut unknown = write.clone();
        unknown.id = OperationId::new();
        unknown.sequence = 3;
        unknown.effect = OperationEffect::Unknown;
        let connection_id = connection.id;
        let write_id = write.id;
        let unknown_id = unknown.id;
        connection.operations.extend([write, unknown]);
        session
            .operation_completeness
            .insert(write_id, Completeness::Complete);
        session
            .operation_completeness
            .insert(unknown_id, Completeness::Complete);
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection_id),
                target: Endpoint::new("test", 2),
            }],
        };
        let policy = ReplayPolicy {
            dry_run: false,
            allow_reads: true,
            ..ReplayPolicy::default()
        };

        let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap).unwrap();

        assert_eq!(plan.operations().len(), 3);
        assert_eq!(plan.operations()[0].decision(), &ReplayDecision::Allowed);
        assert_eq!(
            plan.operations()[1].decision(),
            &ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(OperationEffect::Write))
        );
        assert_eq!(
            plan.operations()[2].decision(),
            &ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(OperationEffect::Unknown))
        );
        assert!(!plan.is_executable());
    }

    #[test]
    fn plan_denies_operations_requiring_runtime_authorization() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        connection.operations[0].attributes.insert(
            "chronicle.replay.requires_runtime_authorization".into(),
            "true".into(),
        );
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("127.0.0.1", 2),
            }],
        };
        let policy = ReplayPolicy {
            dry_run: false,
            allow_reads: true,
            execution_authorized: true,
            ..ReplayPolicy::default()
        };
        let plan = ReplayPlanner::plan(&session, &targets, &policy, TimingMode::Asap).unwrap();
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Denied(ReplayDenial::EffectNotAllowed(
                OperationEffect::Authentication
            ))
        );
    }

    struct ConnectSpy {
        id: ProtocolId,
        connects: Arc<AtomicUsize>,
    }

    struct FailingConnect {
        id: ProtocolId,
        connects: Arc<AtomicUsize>,
        fail_at: usize,
    }

    impl ReplayAdapter for FailingConnect {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn connect<'a>(
            &'a self,
            _target: &'a Endpoint,
            _context: &'a ReplayContext,
        ) -> chronicle_protocol::BoxFuture<
            'a,
            Result<Box<dyn chronicle_protocol::ReplayConnection>, ProtocolError>,
        > {
            Box::pin(async move {
                if self.connects.fetch_add(1, Ordering::Relaxed) == self.fail_at {
                    return Err(ProtocolError::Transport {
                        category: chronicle_protocol::TransportErrorCategory::Refused,
                        message: "scripted refusal".into(),
                    });
                }
                Ok(Box::new(PassingConnection) as Box<dyn chronicle_protocol::ReplayConnection>)
            })
        }
    }

    struct PassingConnection;

    impl chronicle_protocol::ReplayConnection for PassingConnection {
        fn execute<'a>(
            &'a mut self,
            operation: &'a CanonicalOperation,
        ) -> chronicle_protocol::BoxFuture<
            'a,
            Result<chronicle_protocol::ObservedResponse, ProtocolError>,
        > {
            Box::pin(async move {
                Ok(chronicle_protocol::ObservedResponse {
                    payload: operation.recorded_response.clone(),
                    protocol_data: None,
                    attributes: BTreeMap::new(),
                    error_category: None,
                })
            })
        }
    }

    struct PassingVerifier {
        id: ProtocolId,
    }

    impl chronicle_protocol::Verifier for PassingVerifier {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn verify(
            &self,
            _operation: &CanonicalOperation,
            _observed: &chronicle_protocol::ObservedResponse,
        ) -> VerificationResult {
            verification(VerificationStatus::Passed, "scripted pass", "passed")
        }
    }

    struct FailingSecondVerifier {
        id: ProtocolId,
        verifications: Arc<AtomicUsize>,
    }

    impl chronicle_protocol::Verifier for FailingSecondVerifier {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn verify(
            &self,
            _operation: &CanonicalOperation,
            _observed: &chronicle_protocol::ObservedResponse,
        ) -> VerificationResult {
            let status = if self.verifications.fetch_add(1, Ordering::Relaxed) == 1 {
                VerificationStatus::Failed
            } else {
                VerificationStatus::Passed
            };
            verification(status, "scripted verification", "scripted")
        }
    }

    impl ReplayAdapter for ConnectSpy {
        fn protocol(&self) -> &ProtocolId {
            &self.id
        }

        fn connect<'a>(
            &'a self,
            _target: &'a Endpoint,
            _context: &'a ReplayContext,
        ) -> chronicle_protocol::BoxFuture<
            'a,
            Result<Box<dyn chronicle_protocol::ReplayConnection>, ProtocolError>,
        > {
            Box::pin(async move {
                self.connects.fetch_add(1, Ordering::Relaxed);
                Err(ProtocolError::Replay("connect spy called".into()))
            })
        }
    }

    #[tokio::test]
    async fn denied_preflight_blocks_connects_and_preserves_unsupported_entries() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        connection.protocol = ProtocolId::new("http/1.1");
        let mut unsupported = connection.operations[0].clone();
        unsupported.id = OperationId::new();
        unsupported.sequence = 2;
        unsupported
            .attributes
            .insert("chronicle.replayable".into(), "false".into());
        let unsupported_id = unsupported.id;
        connection.operations.push(unsupported);
        session
            .operation_completeness
            .insert(unsupported_id, Completeness::Partial);
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("127.0.0.1", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Denied(ReplayDenial::ExecutionNotAuthorized)
        );
        assert_eq!(
            plan.operations()[1].decision(),
            &ReplayDecision::Unsupported
        );

        let connects = Arc::new(AtomicUsize::new(0));
        let mut registry = ProtocolRegistry::new();
        registry
            .register(ProtocolRegistration {
                id: ProtocolId::new("http/1.1"),
                display_name: "connect spy",
                capabilities: ProtocolCapabilities {
                    detection: CapabilityStatus::Unavailable,
                    decoding: CapabilityStatus::Unavailable,
                    canonicalization: CapabilityStatus::Unavailable,
                    replay: CapabilityStatus::Available,
                    verification: CapabilityStatus::Unavailable,
                },
                detector: None,
                decoder_factory: None,
                canonicalizer: None,
                replay_adapter: Some(Arc::new(ConnectSpy {
                    id: ProtocolId::new("http/1.1"),
                    connects: connects.clone(),
                })),
                verifier: None,
            })
            .unwrap();
        let results = ReplayExecutor::new(&registry)
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(results.outcome, ReplayOutcome::StoppedPolicy);
        assert_eq!(connects.load(Ordering::Relaxed), 0);
        assert_eq!(results.results.len(), 2);
        assert!(
            results
                .results
                .iter()
                .all(|result| result.state == OperationExecutionState::NotAttempted)
        );
    }

    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn execution_stops_on_transport_or_verification_and_accounts_for_skips() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        let mut second = connection.operations[0].clone();
        second.id = OperationId::new();
        second.sequence = 2;
        let second_id = second.id;
        let mut third = second.clone();
        third.id = OperationId::new();
        third.sequence = 3;
        let third_id = third.id;
        let mut unsupported = third.clone();
        unsupported.id = OperationId::new();
        unsupported.sequence = 4;
        unsupported
            .attributes
            .insert("chronicle.replayable".into(), "false".into());
        let unsupported_id = unsupported.id;
        connection.operations.extend([second, third, unsupported]);
        session.operation_completeness.extend([
            (second_id, Completeness::Complete),
            (third_id, Completeness::Complete),
            (unsupported_id, Completeness::Partial),
        ]);
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("127.0.0.1", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();
        for (fail_at, expected_states) in [
            (
                0,
                [
                    OperationExecutionState::Failed,
                    OperationExecutionState::NotAttempted,
                    OperationExecutionState::NotAttempted,
                    OperationExecutionState::NotAttempted,
                ],
            ),
            (
                1,
                [
                    OperationExecutionState::Completed,
                    OperationExecutionState::Failed,
                    OperationExecutionState::NotAttempted,
                    OperationExecutionState::NotAttempted,
                ],
            ),
        ] {
            let connects = Arc::new(AtomicUsize::new(0));
            let mut registry = ProtocolRegistry::new();
            registry
                .register(ProtocolRegistration {
                    id: ProtocolId::new("fake"),
                    display_name: "transport spy",
                    capabilities: ProtocolCapabilities::unavailable(),
                    detector: None,
                    decoder_factory: None,
                    canonicalizer: None,
                    replay_adapter: Some(Arc::new(FailingConnect {
                        id: ProtocolId::new("fake"),
                        connects: connects.clone(),
                        fail_at,
                    })),
                    verifier: Some(Arc::new(PassingVerifier {
                        id: ProtocolId::new("fake"),
                    })),
                })
                .unwrap();

            let execution = ReplayExecutor::new(&registry)
                .execute(&plan, &BTreeMap::new())
                .await
                .unwrap();
            assert_eq!(execution.outcome, ReplayOutcome::StoppedTransport);
            assert_eq!(connects.load(Ordering::Relaxed), fail_at + 1);
            assert_eq!(execution.results.len(), 4);
            assert_eq!(
                execution
                    .results
                    .iter()
                    .map(|result| result.state)
                    .collect::<Vec<_>>(),
                expected_states
            );
            assert_eq!(
                execution.results[2]
                    .verification
                    .as_ref()
                    .unwrap()
                    .details
                    .get("category"),
                Some(&"unattempted_after_stop".into())
            );
            assert_eq!(
                execution.results[3]
                    .verification
                    .as_ref()
                    .unwrap()
                    .details
                    .get("category"),
                Some(&"non_executable".into())
            );
        }

        let connects = Arc::new(AtomicUsize::new(0));
        let verifications = Arc::new(AtomicUsize::new(0));
        let mut registry = ProtocolRegistry::new();
        registry
            .register(ProtocolRegistration {
                id: ProtocolId::new("fake"),
                display_name: "verification spy",
                capabilities: ProtocolCapabilities::unavailable(),
                detector: None,
                decoder_factory: None,
                canonicalizer: None,
                replay_adapter: Some(Arc::new(FailingConnect {
                    id: ProtocolId::new("fake"),
                    connects: connects.clone(),
                    fail_at: usize::MAX,
                })),
                verifier: Some(Arc::new(FailingSecondVerifier {
                    id: ProtocolId::new("fake"),
                    verifications: verifications.clone(),
                })),
            })
            .unwrap();
        let execution = ReplayExecutor::new(&registry)
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(execution.outcome, ReplayOutcome::StoppedVerification);
        assert_eq!(connects.load(Ordering::Relaxed), 2);
        assert_eq!(verifications.load(Ordering::Relaxed), 2);
        assert_eq!(
            execution
                .results
                .iter()
                .map(|result| result.state)
                .collect::<Vec<_>>(),
            vec![
                OperationExecutionState::Completed,
                OperationExecutionState::Completed,
                OperationExecutionState::NotAttempted,
                OperationExecutionState::NotAttempted,
            ]
        );

        let mut registry = ProtocolRegistry::new();
        registry
            .register(ProtocolRegistration {
                id: ProtocolId::new("fake"),
                display_name: "partial replay",
                capabilities: ProtocolCapabilities::unavailable(),
                detector: None,
                decoder_factory: None,
                canonicalizer: None,
                replay_adapter: Some(Arc::new(FailingConnect {
                    id: ProtocolId::new("fake"),
                    connects: Arc::new(AtomicUsize::new(0)),
                    fail_at: usize::MAX,
                })),
                verifier: Some(Arc::new(PassingVerifier {
                    id: ProtocolId::new("fake"),
                })),
            })
            .unwrap();
        let execution = ReplayExecutor::new(&registry)
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(execution.outcome, ReplayOutcome::CompletedWithSkips);
        assert!(
            execution.results[..3]
                .iter()
                .all(|result| result.state == OperationExecutionState::Completed)
        );
        assert_eq!(
            execution.results[3].verification.as_ref().unwrap().status,
            VerificationStatus::Skipped
        );
    }

    #[test]
    fn http_execution_requires_loopback_options_authorization() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        connection.protocol = ProtocolId::new("http/1.1");
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("127.0.0.1", 8080),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Denied(ReplayDenial::ExecutionNotAuthorized)
        );
    }

    #[test]
    fn planner_rejects_non_replayable_operations_before_execution() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        connection.operations[0]
            .attributes
            .insert("chronicle.replayable".into(), "false".into());
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("test", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(
            plan.operations()[0].decision(),
            &ReplayDecision::Unsupported
        );
    }

    #[tokio::test]
    async fn over_limit_imported_session_never_connects_or_omits_operations() {
        let mut session = session(OperationEffect::Read);
        let template = session.connections[0].operations[0].clone();
        let extra: Vec<_> = (2..=u64::try_from(MAX_REPLAY_OPERATIONS + 1).unwrap())
            .map(|sequence| {
                let mut operation = template.clone();
                operation.id = OperationId::new();
                operation.sequence = sequence;
                operation
            })
            .collect();
        session.operation_completeness.extend(
            extra
                .iter()
                .map(|operation| (operation.id, Completeness::Complete)),
        );
        session.connections[0].operations.extend(extra);
        let connection = &session.connections[0];
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("127.0.0.1", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                execution_authorized: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();
        assert_eq!(plan.operations().len(), MAX_REPLAY_OPERATIONS + 1);
        assert!(!plan.is_executable());
        assert_eq!(plan.replayability(), Replayability::NotReplayable);

        let connects = Arc::new(AtomicUsize::new(0));
        let mut registry = ProtocolRegistry::new();
        registry
            .register(ProtocolRegistration {
                id: ProtocolId::new("fake"),
                display_name: "connect spy",
                capabilities: ProtocolCapabilities::unavailable(),
                detector: None,
                decoder_factory: None,
                canonicalizer: None,
                replay_adapter: Some(Arc::new(ConnectSpy {
                    id: ProtocolId::new("fake"),
                    connects: connects.clone(),
                })),
                verifier: None,
            })
            .unwrap();
        let execution = ReplayExecutor::new(&registry)
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap();
        assert_eq!(execution.outcome, ReplayOutcome::StoppedInvalidSession);
        assert_eq!(connects.load(Ordering::Relaxed), 0);
        assert_eq!(execution.results.len(), MAX_REPLAY_OPERATIONS + 1);
        assert!(
            execution
                .results
                .iter()
                .all(|result| result.state == OperationExecutionState::NotAttempted)
        );
    }

    #[test]
    fn planner_keeps_complete_sibling_executable_when_other_is_degraded() {
        let mut session = session(OperationEffect::Read);
        let connection = &mut session.connections[0];
        let mut degraded = connection.operations[0].clone();
        degraded.id = OperationId::new();
        degraded.sequence = 2;
        degraded
            .attributes
            .insert("chronicle.replayable".into(), "false".into());
        let degraded_id = degraded.id;
        connection.operations.push(degraded);
        session
            .operation_completeness
            .insert(degraded_id, Completeness::Partial);
        let targets = TargetMap {
            rules: vec![TargetRule {
                protocol: None,
                host: None,
                port: None,
                connection_id: Some(connection.id),
                target: Endpoint::new("test", 2),
            }],
        };
        let plan = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy {
                dry_run: false,
                allow_reads: true,
                ..ReplayPolicy::default()
            },
            TimingMode::Asap,
        )
        .unwrap();

        assert_eq!(plan.operations()[0].decision(), &ReplayDecision::Allowed);
        assert_eq!(
            plan.operations()[1].decision(),
            &ReplayDecision::Unsupported
        );
        assert!(plan.is_executable());
        assert_eq!(plan.replayability(), Replayability::PartiallyReplayable);
    }

    #[test]
    fn verification_status_models_basic_outcomes() {
        let statuses = [
            VerificationStatus::Passed,
            VerificationStatus::Failed,
            VerificationStatus::Skipped,
            VerificationStatus::Inconclusive,
            VerificationStatus::Unsupported,
        ];
        assert_eq!(statuses.len(), 5);
    }
}
