use crate::{CanonicalOperation, CanonicalSession, RelativeTimeNanos};
use chronicle_common::{
    ConnectionId, Direction, EpochId, OperationId, ProtocolId, RecordingId, ScenarioId, SessionId,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const MAX_CORRELATION_NAMESPACE_BYTES: usize = 64;
pub const MAX_CORRELATION_KEY_BYTES: usize = 128;
pub const MAX_CORRELATION_VALUE_BYTES: usize = 1024;

/// Application-relative ownership of one logical canonical operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionRole {
    Ingress,
    Egress,
}

/// Evidence provenance stays Chronicle-owned and provider-neutral.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceProvenance {
    pub source: String,
    pub observation: Option<String>,
}

impl Default for EvidenceProvenance {
    fn default() -> Self {
        Self {
            source: "chronicle".into(),
            observation: None,
        }
    }
}

impl EvidenceProvenance {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            observation: None,
        }
    }
}

/// Application ownership evidence used by deterministic role normalization.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationOwnership {
    PassiveApplicationServer,
    ActiveOutbound,
}

/// Socket role is evidence only; it is not an interaction or scenario identity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketRoleEvidence {
    Active,
    Passive,
}

/// Tagged, framework-neutral correlation evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CorrelationEvidenceKind {
    TraceRelationship {
        provider: String,
        trace_id: String,
        span_id: Option<String>,
        parent_span_id: Option<String>,
    },
    ProtocolOwnership {
        protocol: ProtocolId,
        ownership: ApplicationOwnership,
    },
    ExecutionTaskLineage {
        task: String,
        worker: Option<String>,
    },
    ProcessThreadGeneration {
        process_id: Option<u64>,
        thread_id: Option<u64>,
        process_generation: Option<u64>,
        thread_generation: Option<u64>,
    },
    ConnectionSocketGeneration {
        connection_id: Option<ConnectionId>,
        socket_id: Option<String>,
        generation: Option<u64>,
    },
    ProtocolStream {
        stream_id: String,
        generation: Option<u64>,
    },
    TemporalLifetime {
        start: RelativeTimeNanos,
        end: Option<RelativeTimeNanos>,
    },
    WireDirection {
        direction: Direction,
    },
    SocketRole {
        role: SocketRoleEvidence,
    },
    Custom {
        namespace: String,
        key: String,
        value: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationEvidence {
    pub kind: CorrelationEvidenceKind,
    pub provenance: EvidenceProvenance,
}

impl CorrelationEvidence {
    pub fn new(kind: CorrelationEvidenceKind) -> Self {
        Self {
            kind,
            provenance: EvidenceProvenance::default(),
        }
    }

    pub fn with_provenance(kind: CorrelationEvidenceKind, provenance: EvidenceProvenance) -> Self {
        Self { kind, provenance }
    }

    pub fn trace(provider: impl Into<String>, trace_id: impl Into<String>) -> Self {
        Self::new(CorrelationEvidenceKind::TraceRelationship {
            provider: provider.into(),
            trace_id: trace_id.into(),
            span_id: None,
            parent_span_id: None,
        })
    }

    pub fn passive_http() -> Self {
        Self::new(CorrelationEvidenceKind::ProtocolOwnership {
            protocol: ProtocolId::new("http"),
            ownership: ApplicationOwnership::PassiveApplicationServer,
        })
    }

    pub fn active_http() -> Self {
        Self::new(CorrelationEvidenceKind::ProtocolOwnership {
            protocol: ProtocolId::new("http"),
            ownership: ApplicationOwnership::ActiveOutbound,
        })
    }

    pub fn active_dependency(protocol: impl Into<String>) -> Self {
        Self::new(CorrelationEvidenceKind::ProtocolOwnership {
            protocol: ProtocolId::new(protocol),
            ownership: ApplicationOwnership::ActiveOutbound,
        })
    }

    pub fn temporal(start: RelativeTimeNanos, end: Option<RelativeTimeNanos>) -> Self {
        Self::new(CorrelationEvidenceKind::TemporalLifetime { start, end })
    }

    pub fn wire_direction(direction: Direction) -> Self {
        Self::new(CorrelationEvidenceKind::WireDirection { direction })
    }

    pub fn socket_role(role: SocketRoleEvidence) -> Self {
        Self::new(CorrelationEvidenceKind::SocketRole { role })
    }

    pub fn custom(
        namespace: impl Into<String>,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        Self::new(CorrelationEvidenceKind::Custom {
            namespace: namespace.into(),
            key: key.into(),
            value: value.into(),
        })
    }

    pub fn is_temporal(&self) -> bool {
        matches!(self.kind, CorrelationEvidenceKind::TemporalLifetime { .. })
    }

    pub fn application_ownership(&self) -> Option<ApplicationOwnership> {
        match self.kind {
            CorrelationEvidenceKind::ProtocolOwnership { ownership, .. } => Some(ownership),
            _ => None,
        }
    }

    pub fn validate(&self) -> Result<(), CorrelationValidationError> {
        if self.provenance.source.is_empty() {
            return Err(CorrelationValidationError::InvalidEvidence(
                "evidence provenance source must not be empty".into(),
            ));
        }
        match &self.kind {
            CorrelationEvidenceKind::TraceRelationship {
                provider, trace_id, ..
            } if provider.is_empty() || trace_id.is_empty() => {
                Err(CorrelationValidationError::InvalidEvidence(
                    "trace evidence requires provider and opaque trace_id".into(),
                ))
            }
            CorrelationEvidenceKind::ProtocolOwnership { protocol, .. }
                if protocol.as_str().is_empty() =>
            {
                Err(CorrelationValidationError::InvalidEvidence(
                    "protocol ownership evidence requires a protocol".into(),
                ))
            }
            CorrelationEvidenceKind::ExecutionTaskLineage { task, .. } if task.is_empty() => {
                Err(CorrelationValidationError::InvalidEvidence(
                    "execution/task evidence requires a task".into(),
                ))
            }
            CorrelationEvidenceKind::ProcessThreadGeneration {
                process_id,
                thread_id,
                process_generation,
                thread_generation,
            } if process_id.is_none()
                && thread_id.is_none()
                && process_generation.is_none()
                && thread_generation.is_none() =>
            {
                Err(CorrelationValidationError::InvalidEvidence(
                    "process/thread evidence requires a value".into(),
                ))
            }
            CorrelationEvidenceKind::ConnectionSocketGeneration {
                connection_id,
                socket_id,
                generation,
            } if connection_id.is_none() && socket_id.is_none() && generation.is_none() => {
                Err(CorrelationValidationError::InvalidEvidence(
                    "connection/socket evidence requires a value".into(),
                ))
            }
            CorrelationEvidenceKind::ProtocolStream { stream_id, .. } if stream_id.is_empty() => {
                Err(CorrelationValidationError::InvalidEvidence(
                    "protocol stream evidence requires a stream_id".into(),
                ))
            }
            CorrelationEvidenceKind::TemporalLifetime { start, end }
                if end.is_some_and(|end| end < *start) =>
            {
                Err(CorrelationValidationError::InvalidEvidence(
                    "temporal evidence end must not precede start".into(),
                ))
            }
            CorrelationEvidenceKind::Custom {
                namespace,
                key,
                value,
            } if namespace.is_empty()
                || key.is_empty()
                || value.is_empty()
                || namespace.len() > MAX_CORRELATION_NAMESPACE_BYTES
                || key.len() > MAX_CORRELATION_KEY_BYTES
                || value.len() > MAX_CORRELATION_VALUE_BYTES =>
            {
                Err(CorrelationValidationError::InvalidEvidence(
                    "custom evidence requires bounded namespace, key, and value".into(),
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InteractionRoleCandidate {
    pub role: InteractionRole,
    pub evidence: Vec<CorrelationEvidence>,
}

pub type RoleCandidate = InteractionRoleCandidate;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InteractionRoleResolution {
    Known {
        role: InteractionRole,
        evidence: Vec<CorrelationEvidence>,
    },
    Unknown {
        evidence: Vec<CorrelationEvidence>,
    },
    Ambiguous {
        candidates: Vec<InteractionRoleCandidate>,
    },
}

impl InteractionRoleResolution {
    pub fn known(role: InteractionRole, evidence: Vec<CorrelationEvidence>) -> Self {
        Self::Known { role, evidence }
    }

    pub fn unknown(evidence: Vec<CorrelationEvidence>) -> Self {
        Self::Unknown { evidence }
    }

    pub fn ambiguous(candidates: Vec<InteractionRoleCandidate>) -> Self {
        Self::Ambiguous { candidates }
    }

    /// Normalize only explicit application-relative ownership evidence.
    /// Wire direction, socket role, timing, and infrastructure evidence remain
    /// attached evidence and never select a role.
    pub fn from_application_evidence(evidence: Vec<CorrelationEvidence>) -> Self {
        let mut passive = Vec::new();
        let mut active = Vec::new();
        let mut neutral = Vec::new();
        for item in evidence {
            match item.application_ownership() {
                Some(ApplicationOwnership::PassiveApplicationServer) => passive.push(item),
                Some(ApplicationOwnership::ActiveOutbound) => active.push(item),
                None => neutral.push(item),
            }
        }
        if passive.is_empty() && active.is_empty() {
            neutral.shrink_to_fit();
            return Self::Unknown { evidence: neutral };
        }
        if passive.is_empty() {
            active.extend(neutral);
            return Self::Known {
                role: InteractionRole::Egress,
                evidence: active,
            };
        }
        if active.is_empty() {
            passive.extend(neutral);
            return Self::Known {
                role: InteractionRole::Ingress,
                evidence: passive,
            };
        }
        let passive_evidence = passive.into_iter().chain(neutral.iter().cloned()).collect();
        let active_evidence = active.into_iter().chain(neutral).collect();
        Self::Ambiguous {
            candidates: vec![
                InteractionRoleCandidate {
                    role: InteractionRole::Ingress,
                    evidence: passive_evidence,
                },
                InteractionRoleCandidate {
                    role: InteractionRole::Egress,
                    evidence: active_evidence,
                },
            ],
        }
    }

    pub fn normalize(evidence: Vec<CorrelationEvidence>) -> Self {
        Self::from_application_evidence(evidence)
    }

    pub fn is_root_eligible(&self) -> bool {
        matches!(
            self,
            Self::Known {
                role: InteractionRole::Ingress,
                ..
            }
        )
    }

    pub fn known_role(&self) -> Option<InteractionRole> {
        match self {
            Self::Known { role, .. } => Some(*role),
            Self::Unknown { .. } | Self::Ambiguous { .. } => None,
        }
    }

    pub fn validate(&self) -> Result<(), CorrelationValidationError> {
        match self {
            Self::Known { evidence, .. } => {
                require_evidence(evidence, "known role requires supporting evidence")
            }
            Self::Unknown { evidence } => validate_evidence(evidence),
            Self::Ambiguous { candidates } => {
                if candidates.len() < 2 {
                    return Err(CorrelationValidationError::InvalidRoleCandidates(
                        "ambiguous role requires at least two candidates".into(),
                    ));
                }
                let mut roles = BTreeSet::new();
                for candidate in candidates {
                    if !roles.insert(candidate.role) {
                        return Err(CorrelationValidationError::InvalidRoleCandidates(
                            "ambiguous role candidates must have distinct roles".into(),
                        ));
                    }
                    require_evidence(
                        &candidate.evidence,
                        "ambiguous role candidates require supporting evidence",
                    )?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CorrelationConfidence {
    Exact,
    Strong,
    Inferred,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioCandidate {
    pub scenario: ScenarioId,
    pub evidence: Vec<CorrelationEvidence>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CorrelationResolution {
    Resolved {
        scenario: ScenarioId,
        confidence: CorrelationConfidence,
        evidence: Vec<CorrelationEvidence>,
    },
    Ambiguous {
        candidates: Vec<ScenarioCandidate>,
    },
    Uncorrelated {
        evidence: Vec<CorrelationEvidence>,
    },
}

impl CorrelationResolution {
    pub fn resolved(scenario: ScenarioId, evidence: Vec<CorrelationEvidence>) -> Self {
        Self::Resolved {
            scenario,
            confidence: CorrelationConfidence::Exact,
            evidence,
        }
    }

    pub fn ambiguous(candidates: Vec<ScenarioCandidate>) -> Self {
        Self::Ambiguous { candidates }
    }

    pub fn uncorrelated(evidence: Vec<CorrelationEvidence>) -> Self {
        Self::Uncorrelated { evidence }
    }

    pub fn selected_scenario(&self) -> Option<ScenarioId> {
        match self {
            Self::Resolved { scenario, .. } => Some(*scenario),
            Self::Ambiguous { .. } | Self::Uncorrelated { .. } => None,
        }
    }

    pub fn validate(&self) -> Result<(), CorrelationValidationError> {
        match self {
            Self::Resolved { evidence, .. } => {
                require_evidence(
                    evidence,
                    "resolved correlation requires supporting evidence",
                )?;
                if evidence.iter().all(CorrelationEvidence::is_temporal) {
                    return Err(CorrelationValidationError::TemporalOnlyResolution);
                }
                Ok(())
            }
            Self::Ambiguous { candidates } => {
                if candidates.len() < 2 {
                    return Err(CorrelationValidationError::InvalidCorrelationCandidates(
                        "ambiguous correlation requires at least two candidates".into(),
                    ));
                }
                let mut scenarios = BTreeSet::new();
                for candidate in candidates {
                    if !scenarios.insert(candidate.scenario) {
                        return Err(CorrelationValidationError::InvalidCorrelationCandidates(
                            "ambiguous correlation candidates must be distinct".into(),
                        ));
                    }
                    require_evidence(
                        &candidate.evidence,
                        "ambiguous correlation candidates require supporting evidence",
                    )?;
                }
                Ok(())
            }
            Self::Uncorrelated { evidence } => validate_evidence(evidence),
        }
    }
}

/// Durable lookup scope for one canonical logical operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CanonicalOperationRef {
    pub recording_id: RecordingId,
    pub owner_epoch_id: EpochId,
    pub session_id: SessionId,
    pub operation_id: OperationId,
}

impl CanonicalOperationRef {
    pub const fn new(
        recording_id: RecordingId,
        owner_epoch_id: EpochId,
        session_id: SessionId,
        operation_id: OperationId,
    ) -> Self {
        Self {
            recording_id,
            owner_epoch_id,
            session_id,
            operation_id,
        }
    }

    /// Resolve only against the explicitly supplied canonical session scope.
    /// No bare `OperationId` lookup or identifier-based lineage inference is used.
    pub fn resolve_in_session<'a>(
        &self,
        session: &'a CanonicalSession,
    ) -> Result<&'a CanonicalOperation, CorrelationValidationError> {
        if session.source_provenance.recording_id != Some(self.recording_id) {
            return Err(CorrelationValidationError::ReferenceRecordingMismatch {
                expected: self.recording_id,
                found: session.source_provenance.recording_id,
            });
        }
        if session.source_provenance.epoch_id != Some(self.owner_epoch_id) {
            return Err(CorrelationValidationError::ReferenceEpochMismatch {
                expected: self.owner_epoch_id,
                found: session.source_provenance.epoch_id,
            });
        }
        if session.id != self.session_id {
            return Err(CorrelationValidationError::ReferenceSessionMismatch {
                expected: self.session_id,
                found: session.id,
            });
        }
        let mut matches = session
            .connections
            .iter()
            .flat_map(|connection| connection.operations.iter())
            .filter(|operation| operation.id == self.operation_id);
        let Some(operation) = matches.next() else {
            return Err(CorrelationValidationError::OperationNotFound {
                operation_id: self.operation_id,
            });
        };
        if matches.next().is_some() {
            return Err(CorrelationValidationError::DuplicateOperationOccurrence {
                operation_id: self.operation_id,
            });
        }
        if operation.provenance.completion_owner_epoch != Some(self.owner_epoch_id) {
            return Err(CorrelationValidationError::OperationOwnerMismatch {
                operation_id: self.operation_id,
                expected: self.owner_epoch_id,
                found: operation.provenance.completion_owner_epoch,
            });
        }
        if !operation
            .provenance
            .epoch_ranges
            .iter()
            .any(|range| range.epoch_id == self.owner_epoch_id)
        {
            return Err(CorrelationValidationError::OperationOwnerMissingRange {
                operation_id: self.operation_id,
                owner_epoch: self.owner_epoch_id,
            });
        }
        Ok(operation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scenario {
    pub id: ScenarioId,
    pub root: CanonicalOperationRef,
    pub members: Vec<CanonicalOperationRef>,
}

impl Scenario {
    pub fn new(id: ScenarioId, root: CanonicalOperationRef) -> Self {
        Self {
            id,
            root,
            members: vec![root],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedCausalEdge {
    pub scenario: ScenarioId,
    pub parent: CanonicalOperationRef,
    pub child: CanonicalOperationRef,
    pub evidence: Vec<CorrelationEvidence>,
}

impl SelectedCausalEdge {
    pub fn new(
        scenario: ScenarioId,
        parent: CanonicalOperationRef,
        child: CanonicalOperationRef,
        evidence: Vec<CorrelationEvidence>,
    ) -> Self {
        Self {
            scenario,
            parent,
            child,
            evidence,
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CorrelationValidationError {
    #[error("invalid correlation evidence: {0}")]
    InvalidEvidence(String),
    #[error("invalid interaction-role candidates: {0}")]
    InvalidRoleCandidates(String),
    #[error("invalid correlation candidates: {0}")]
    InvalidCorrelationCandidates(String),
    #[error("correlation resolution is supported only by temporal evidence")]
    TemporalOnlyResolution,
    #[error("reference recording lineage mismatch: expected {expected}, found {found:?}")]
    ReferenceRecordingMismatch {
        expected: RecordingId,
        found: Option<RecordingId>,
    },
    #[error("reference epoch lineage mismatch: expected {expected}, found {found:?}")]
    ReferenceEpochMismatch {
        expected: EpochId,
        found: Option<EpochId>,
    },
    #[error("reference session mismatch: expected {expected}, found {found}")]
    ReferenceSessionMismatch {
        expected: SessionId,
        found: SessionId,
    },
    #[error("operation {operation_id} is not present exactly once in its scoped session")]
    OperationNotFound { operation_id: OperationId },
    #[error("operation {operation_id} occurs more than once in its scoped session")]
    DuplicateOperationOccurrence { operation_id: OperationId },
    #[error(
        "operation {operation_id} completion owner mismatch: expected {expected}, found {found:?}"
    )]
    OperationOwnerMismatch {
        operation_id: OperationId,
        expected: EpochId,
        found: Option<EpochId>,
    },
    #[error("operation {operation_id} completion owner {owner_epoch} is absent from epoch ranges")]
    OperationOwnerMissingRange {
        operation_id: OperationId,
        owner_epoch: EpochId,
    },
    #[error("canonical session {session_id} is missing from validation context")]
    MissingSessionContext { session_id: SessionId },
    #[error("validation context contains duplicate canonical session {session_id}")]
    DuplicateSessionContext { session_id: SessionId },
    #[error("selected causal edge is supported only by temporal evidence")]
    TemporalOnlyEdgeEvidence,
    #[error("reference {reference:?} belongs to {expected}, not graph recording {found}")]
    ReferenceRecordingScopeMismatch {
        reference: CanonicalOperationRef,
        expected: RecordingId,
        found: RecordingId,
    },
    #[error("operation {reference:?} is missing a role resolution")]
    MissingRoleResolution { reference: CanonicalOperationRef },
    #[error("operation {reference:?} is missing a correlation resolution")]
    MissingCorrelationResolution { reference: CanonicalOperationRef },
    #[error("operation {reference:?} was admitted more than once")]
    DuplicateOperation { reference: CanonicalOperationRef },
    #[error("scenario {id} was admitted more than once")]
    DuplicateScenario { id: ScenarioId },
    #[error("scenario {id} is missing")]
    MissingScenario { id: ScenarioId },
    #[error("scenario {scenario} root {reference:?} is not a known ingress")]
    InvalidScenarioRoot {
        scenario: ScenarioId,
        reference: CanonicalOperationRef,
    },
    #[error("scenario {scenario} root {reference:?} is not a member")]
    RootNotMember {
        scenario: ScenarioId,
        reference: CanonicalOperationRef,
    },
    #[error("scenario {scenario} membership contains invalid operation {reference:?}")]
    InvalidMembership {
        scenario: ScenarioId,
        reference: CanonicalOperationRef,
    },
    #[error("scenario {scenario} contains duplicate member {reference:?}")]
    DuplicateMembership {
        scenario: ScenarioId,
        reference: CanonicalOperationRef,
    },
    #[error("resolved operation {reference:?} does not have exactly one selected membership")]
    ResolvedMembershipMismatch { reference: CanonicalOperationRef },
    #[error("causal edge endpoint {reference:?} is not admitted")]
    EdgeMissingEndpoint { reference: CanonicalOperationRef },
    #[error("causal edge {parent:?} -> {child:?} crosses selected scenarios")]
    EdgeScenarioMismatch {
        parent: Box<CanonicalOperationRef>,
        child: Box<CanonicalOperationRef>,
    },
    #[error("causal edge cannot point from {parent:?} to itself")]
    SelfEdge { parent: CanonicalOperationRef },
    #[error("operation {child:?} has more than one selected parent")]
    MultipleSelectedParents { child: CanonicalOperationRef },
    #[error("selected causal edges contain a cycle")]
    CausalCycle,
}

fn validate_evidence(evidence: &[CorrelationEvidence]) -> Result<(), CorrelationValidationError> {
    evidence.iter().try_for_each(CorrelationEvidence::validate)
}

fn require_evidence(
    evidence: &[CorrelationEvidence],
    message: &'static str,
) -> Result<(), CorrelationValidationError> {
    if evidence.is_empty() {
        return Err(CorrelationValidationError::InvalidEvidence(message.into()));
    }
    validate_evidence(evidence)
}

fn validate_selected_edge_evidence(
    evidence: &[CorrelationEvidence],
) -> Result<(), CorrelationValidationError> {
    require_evidence(
        evidence,
        "selected causal edge requires relationship evidence",
    )?;
    if evidence.iter().all(CorrelationEvidence::is_temporal) {
        return Err(CorrelationValidationError::TemporalOnlyEdgeEvidence);
    }
    Ok(())
}

/// Recording-scoped owner of role state, correlation outcomes, scenarios, and
/// selected causal edges. It is deliberately separate from Canonical Session v1.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CorrelationGraph {
    pub recording_id: RecordingId,
    pub role_resolutions: BTreeMap<CanonicalOperationRef, InteractionRoleResolution>,
    pub scenarios: Vec<Scenario>,
    pub resolutions: BTreeMap<CanonicalOperationRef, CorrelationResolution>,
    pub causal_edges: Vec<SelectedCausalEdge>,
}

impl CorrelationGraph {
    pub fn new(recording_id: RecordingId) -> Self {
        Self {
            recording_id,
            role_resolutions: BTreeMap::new(),
            scenarios: Vec::new(),
            resolutions: BTreeMap::new(),
            causal_edges: Vec::new(),
        }
    }

    pub fn from_operations<I>(
        recording_id: RecordingId,
        operations: I,
    ) -> Result<Self, CorrelationValidationError>
    where
        I: IntoIterator<
            Item = (
                CanonicalOperationRef,
                InteractionRoleResolution,
                CorrelationResolution,
            ),
        >,
    {
        let mut graph = Self::new(recording_id);
        for (reference, role, resolution) in operations {
            graph.admit_operation(reference, role, resolution)?;
        }
        Ok(graph)
    }

    pub fn admit_operation(
        &mut self,
        reference: CanonicalOperationRef,
        role: InteractionRoleResolution,
        resolution: CorrelationResolution,
    ) -> Result<(), CorrelationValidationError> {
        if reference.recording_id != self.recording_id {
            return Err(
                CorrelationValidationError::ReferenceRecordingScopeMismatch {
                    reference,
                    expected: self.recording_id,
                    found: reference.recording_id,
                },
            );
        }
        role.validate()?;
        resolution.validate()?;
        if self.role_resolutions.contains_key(&reference)
            || self.resolutions.contains_key(&reference)
        {
            return Err(CorrelationValidationError::DuplicateOperation { reference });
        }
        self.role_resolutions.insert(reference, role);
        self.resolutions.insert(reference, resolution);
        Ok(())
    }

    pub fn add_scenario(&mut self, scenario: Scenario) -> Result<(), CorrelationValidationError> {
        if self
            .scenarios
            .iter()
            .any(|existing| existing.id == scenario.id)
        {
            return Err(CorrelationValidationError::DuplicateScenario { id: scenario.id });
        }
        self.scenarios.push(scenario);
        Ok(())
    }

    pub fn add_causal_edge(&mut self, edge: SelectedCausalEdge) {
        self.causal_edges.push(edge);
    }

    pub fn role_resolution(
        &self,
        reference: &CanonicalOperationRef,
    ) -> Option<&InteractionRoleResolution> {
        self.role_resolutions.get(reference)
    }

    pub fn resolution(&self, reference: &CanonicalOperationRef) -> Option<&CorrelationResolution> {
        self.resolutions.get(reference)
    }

    /// Validate all graph invariants. References require explicit canonical-session context.
    pub fn validate(&self) -> Result<(), CorrelationValidationError> {
        self.validate_against_sessions(&[])
    }

    /// Validate graph structure and resolve every admitted reference against its
    /// explicitly supplied canonical session. No bare operation lookup or
    /// identifier-based lineage inference is permitted.
    pub fn validate_against_sessions(
        &self,
        sessions: &[CanonicalSession],
    ) -> Result<(), CorrelationValidationError> {
        self.validate_structure()?;
        let mut sessions_by_id = BTreeMap::new();
        for session in sessions {
            if sessions_by_id.insert(session.id, session).is_some() {
                return Err(CorrelationValidationError::DuplicateSessionContext {
                    session_id: session.id,
                });
            }
        }
        for reference in self.role_resolutions.keys() {
            let Some(session) = sessions_by_id.get(&reference.session_id).copied() else {
                return Err(CorrelationValidationError::MissingSessionContext {
                    session_id: reference.session_id,
                });
            };
            reference.resolve_in_session(session)?;
        }
        Ok(())
    }

    fn validate_structure(&self) -> Result<(), CorrelationValidationError> {
        for reference in self.role_resolutions.keys() {
            self.validate_reference_scope(reference)?;
            if !self.resolutions.contains_key(reference) {
                return Err(CorrelationValidationError::MissingCorrelationResolution {
                    reference: *reference,
                });
            }
        }
        for reference in self.resolutions.keys() {
            self.validate_reference_scope(reference)?;
            if !self.role_resolutions.contains_key(reference) {
                return Err(CorrelationValidationError::MissingRoleResolution {
                    reference: *reference,
                });
            }
        }
        for role in self.role_resolutions.values() {
            role.validate()?;
        }
        for resolution in self.resolutions.values() {
            resolution.validate()?;
        }

        let mut scenarios = BTreeMap::new();
        for scenario in &self.scenarios {
            if scenarios.insert(scenario.id, scenario).is_some() {
                return Err(CorrelationValidationError::DuplicateScenario { id: scenario.id });
            }
        }

        let mut selected_memberships: BTreeMap<CanonicalOperationRef, usize> = BTreeMap::new();
        for (reference, resolution) in &self.resolutions {
            if let CorrelationResolution::Resolved { scenario, .. } = resolution
                && !scenarios.contains_key(scenario)
            {
                return Err(CorrelationValidationError::MissingScenario { id: *scenario });
            }
            let _ = reference;
        }

        for scenario in &self.scenarios {
            if !scenario.members.contains(&scenario.root) {
                return Err(CorrelationValidationError::RootNotMember {
                    scenario: scenario.id,
                    reference: scenario.root,
                });
            }
            let Some(root_role) = self.role_resolutions.get(&scenario.root) else {
                return Err(CorrelationValidationError::InvalidScenarioRoot {
                    scenario: scenario.id,
                    reference: scenario.root,
                });
            };
            let root_is_selected = matches!(
                self.resolutions.get(&scenario.root),
                Some(CorrelationResolution::Resolved {
                    scenario: selected,
                    ..
                }) if *selected == scenario.id
            );
            if !root_role.is_root_eligible() || !root_is_selected {
                return Err(CorrelationValidationError::InvalidScenarioRoot {
                    scenario: scenario.id,
                    reference: scenario.root,
                });
            }
            let mut members = BTreeSet::new();
            for reference in &scenario.members {
                if !members.insert(*reference) {
                    return Err(CorrelationValidationError::DuplicateMembership {
                        scenario: scenario.id,
                        reference: *reference,
                    });
                }
                let valid_member = matches!(
                    self.resolutions.get(reference),
                    Some(CorrelationResolution::Resolved {
                        scenario: selected,
                        ..
                    }) if *selected == scenario.id
                );
                if !valid_member || !self.role_resolutions.contains_key(reference) {
                    return Err(CorrelationValidationError::InvalidMembership {
                        scenario: scenario.id,
                        reference: *reference,
                    });
                }
                *selected_memberships.entry(*reference).or_default() += 1;
            }
        }

        for (reference, resolution) in &self.resolutions {
            if matches!(resolution, CorrelationResolution::Resolved { .. })
                && selected_memberships.get(reference).copied() != Some(1)
            {
                return Err(CorrelationValidationError::ResolvedMembershipMismatch {
                    reference: *reference,
                });
            }
        }

        self.validate_edges(&scenarios)
    }

    fn validate_reference_scope(
        &self,
        reference: &CanonicalOperationRef,
    ) -> Result<(), CorrelationValidationError> {
        if reference.recording_id != self.recording_id {
            return Err(
                CorrelationValidationError::ReferenceRecordingScopeMismatch {
                    reference: *reference,
                    expected: self.recording_id,
                    found: reference.recording_id,
                },
            );
        }
        Ok(())
    }

    fn validate_edges(
        &self,
        scenarios: &BTreeMap<ScenarioId, &Scenario>,
    ) -> Result<(), CorrelationValidationError> {
        let mut parents = BTreeSet::new();
        let mut adjacency: BTreeMap<CanonicalOperationRef, Vec<CanonicalOperationRef>> =
            BTreeMap::new();
        for edge in &self.causal_edges {
            if !scenarios.contains_key(&edge.scenario) {
                return Err(CorrelationValidationError::MissingScenario { id: edge.scenario });
            }
            for reference in [edge.parent, edge.child] {
                if !self.resolutions.contains_key(&reference) {
                    return Err(CorrelationValidationError::EdgeMissingEndpoint { reference });
                }
            }
            if edge.parent == edge.child {
                return Err(CorrelationValidationError::SelfEdge {
                    parent: edge.parent,
                });
            }
            let parent_scenario = self.resolutions[&edge.parent].selected_scenario();
            let child_scenario = self.resolutions[&edge.child].selected_scenario();
            if parent_scenario != Some(edge.scenario) || child_scenario != Some(edge.scenario) {
                return Err(CorrelationValidationError::EdgeScenarioMismatch {
                    parent: Box::new(edge.parent),
                    child: Box::new(edge.child),
                });
            }
            if !parents.insert(edge.child) {
                return Err(CorrelationValidationError::MultipleSelectedParents {
                    child: edge.child,
                });
            }
            adjacency.entry(edge.parent).or_default().push(edge.child);
        }
        if has_cycle(&adjacency) {
            return Err(CorrelationValidationError::CausalCycle);
        }
        for edge in &self.causal_edges {
            validate_selected_edge_evidence(&edge.evidence)?;
        }
        Ok(())
    }
}

fn has_cycle(adjacency: &BTreeMap<CanonicalOperationRef, Vec<CanonicalOperationRef>>) -> bool {
    fn visit(
        node: CanonicalOperationRef,
        adjacency: &BTreeMap<CanonicalOperationRef, Vec<CanonicalOperationRef>>,
        visiting: &mut BTreeSet<CanonicalOperationRef>,
        visited: &mut BTreeSet<CanonicalOperationRef>,
    ) -> bool {
        if visiting.contains(&node) {
            return true;
        }
        if !visited.insert(node) {
            return false;
        }
        visiting.insert(node);
        for child in adjacency.get(&node).into_iter().flatten() {
            if visit(*child, adjacency, visiting, visited) {
                return true;
            }
        }
        visiting.remove(&node);
        false
    }

    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    adjacency
        .keys()
        .any(|node| visit(*node, adjacency, &mut visiting, &mut visited))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CANONICAL_SCHEMA_VERSION, CanonicalConnection, Completeness, OperationEffect,
        OperationEpochRange, OperationKind, OperationProvenance, PROTOCOL_DATA_SCHEMA_VERSION,
        PayloadRef, ProtocolData, SourceMetadata, SourceProvenance, TimelineEntry,
    };
    use time::OffsetDateTime;

    fn reference(recording_id: RecordingId, operation_id: OperationId) -> CanonicalOperationRef {
        CanonicalOperationRef::new(recording_id, EpochId::new(), SessionId::new(), operation_id)
    }

    fn evidence() -> CorrelationEvidence {
        CorrelationEvidence::custom("test", "reason", "fixture")
    }

    fn ingress() -> InteractionRoleResolution {
        InteractionRoleResolution::known(InteractionRole::Ingress, vec![evidence()])
    }

    fn egress() -> InteractionRoleResolution {
        InteractionRoleResolution::known(InteractionRole::Egress, vec![evidence()])
    }

    fn resolved(scenario: ScenarioId) -> CorrelationResolution {
        CorrelationResolution::resolved(scenario, vec![evidence()])
    }

    fn graph_with(
        recording_id: RecordingId,
        entries: &[(
            CanonicalOperationRef,
            InteractionRoleResolution,
            CorrelationResolution,
        )],
    ) -> CorrelationGraph {
        CorrelationGraph::from_operations(recording_id, entries.iter().cloned()).unwrap()
    }

    fn session_for(reference: CanonicalOperationRef) -> CanonicalSession {
        let operation = crate::CanonicalOperation {
            id: reference.operation_id,
            sequence: 1,
            started_at_offset: RelativeTimeNanos(0),
            completed_at_offset: Some(RelativeTimeNanos(1)),
            kind: OperationKind::Request,
            effect: OperationEffect::Read,
            request: PayloadRef::Missing {
                reason: "test".into(),
            },
            recorded_response: None,
            attributes: BTreeMap::new(),
            protocol_data: ProtocolData {
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: None,
                bytes: Vec::new(),
            },
            provenance: OperationProvenance {
                completion_owner_epoch: Some(reference.owner_epoch_id),
                epoch_ranges: vec![OperationEpochRange {
                    parent_id: None,
                    epoch_id: reference.owner_epoch_id,
                    epoch_ordinal: None,
                    wal_sequence_range: None,
                }],
                ..OperationProvenance::default()
            },
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        let connection_id = ConnectionId::new();
        let session = CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: reference.session_id,
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: Some(OffsetDateTime::UNIX_EPOCH),
            source: SourceMetadata::default(),
            source_provenance: SourceProvenance {
                recording_id: Some(reference.recording_id),
                epoch_id: Some(reference.owner_epoch_id),
                ..SourceProvenance::default()
            },
            connections: vec![CanonicalConnection {
                id: connection_id,
                protocol: ProtocolId::new("test"),
                client: chronicle_common::Endpoint::new("client", 1),
                server: chronicle_common::Endpoint::new("server", 2),
                attributes: BTreeMap::new(),
                operations: vec![operation],
            }],
            connection_completeness: BTreeMap::from([(connection_id, Completeness::Complete)]),
            operation_completeness: BTreeMap::from([(
                reference.operation_id,
                Completeness::Complete,
            )]),
            timeline: vec![TimelineEntry {
                connection_id,
                operation_id: reference.operation_id,
                offset: RelativeTimeNanos(0),
            }],
            replay: Default::default(),
            replay_attributes: BTreeMap::new(),
        };
        session.validate().unwrap();
        session
    }

    #[test]
    fn provider_trace_maps_to_opaque_chronicle_evidence() {
        let evidence = CorrelationEvidence::trace("w3c", "opaque-trace-value");
        assert!(evidence.validate().is_ok());
        let CorrelationEvidenceKind::TraceRelationship {
            provider, trace_id, ..
        } = evidence.kind
        else {
            panic!("expected trace relationship evidence");
        };
        assert_eq!(provider, "w3c");
        assert_eq!(trace_id, "opaque-trace-value");
    }

    #[test]
    fn role_normalization_uses_application_ownership_only() {
        assert!(matches!(
            InteractionRoleResolution::normalize(vec![CorrelationEvidence::passive_http()]),
            InteractionRoleResolution::Known {
                role: InteractionRole::Ingress,
                ..
            }
        ));
        assert!(matches!(
            InteractionRoleResolution::normalize(vec![CorrelationEvidence::active_http()]),
            InteractionRoleResolution::Known {
                role: InteractionRole::Egress,
                ..
            }
        ));
        assert!(matches!(
            InteractionRoleResolution::normalize(vec![
                CorrelationEvidence::active_dependency("postgres"),
                CorrelationEvidence::wire_direction(Direction::ServerToClient),
            ]),
            InteractionRoleResolution::Known {
                role: InteractionRole::Egress,
                ..
            }
        ));
        assert!(matches!(
            InteractionRoleResolution::normalize(vec![
                CorrelationEvidence::wire_direction(Direction::ClientToServer),
                CorrelationEvidence::socket_role(SocketRoleEvidence::Passive),
            ]),
            InteractionRoleResolution::Unknown { .. }
        ));
    }

    #[test]
    fn conflicting_role_evidence_stays_candidate_specific() {
        let resolution = InteractionRoleResolution::normalize(vec![
            CorrelationEvidence::passive_http(),
            CorrelationEvidence::active_http(),
        ]);
        let InteractionRoleResolution::Ambiguous { candidates } = resolution else {
            panic!("expected ambiguous role");
        };
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates[0].role, InteractionRole::Ingress);
        assert_eq!(candidates[1].role, InteractionRole::Egress);
        assert!(candidates[0].evidence.iter().all(|item| {
            item.application_ownership() == Some(ApplicationOwnership::PassiveApplicationServer)
        }));
        assert!(candidates[1].evidence.iter().all(|item| {
            item.application_ownership() == Some(ApplicationOwnership::ActiveOutbound)
        }));
    }

    #[test]
    fn invalid_role_candidate_cardinality_is_rejected() {
        for candidates in [
            Vec::new(),
            vec![InteractionRoleCandidate {
                role: InteractionRole::Ingress,
                evidence: vec![evidence()],
            }],
            vec![
                InteractionRoleCandidate {
                    role: InteractionRole::Ingress,
                    evidence: vec![evidence()],
                },
                InteractionRoleCandidate {
                    role: InteractionRole::Ingress,
                    evidence: vec![evidence()],
                },
            ],
            vec![
                InteractionRoleCandidate {
                    role: InteractionRole::Ingress,
                    evidence: Vec::new(),
                },
                InteractionRoleCandidate {
                    role: InteractionRole::Egress,
                    evidence: vec![evidence()],
                },
            ],
        ] {
            assert!(
                InteractionRoleResolution::ambiguous(candidates)
                    .validate()
                    .is_err()
            );
        }
        assert!(
            InteractionRoleResolution::known(InteractionRole::Ingress, vec![evidence()])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn correlation_candidates_require_supporting_evidence() {
        let scenario_a = ScenarioId::new();
        let scenario_b = ScenarioId::new();
        assert!(matches!(
            CorrelationResolution::ambiguous(vec![
                ScenarioCandidate {
                    scenario: scenario_a,
                    evidence: Vec::new(),
                },
                ScenarioCandidate {
                    scenario: scenario_b,
                    evidence: vec![evidence()],
                },
            ])
            .validate(),
            Err(CorrelationValidationError::InvalidEvidence(_))
        ));
        assert!(matches!(
            CorrelationResolution::resolved(ScenarioId::new(), Vec::new()).validate(),
            Err(CorrelationValidationError::InvalidEvidence(_))
        ));
    }

    #[test]
    fn graph_preserves_independent_role_and_correlation_states() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let ingress_ref = reference(recording_id, OperationId::new());
        let egress_ref = reference(recording_id, OperationId::new());
        let unknown_ref = reference(recording_id, OperationId::new());
        let ambiguous_ref = reference(recording_id, OperationId::new());
        let graph = graph_with(
            recording_id,
            &[
                (ingress_ref, ingress(), resolved(scenario)),
                (
                    egress_ref,
                    egress(),
                    CorrelationResolution::ambiguous(vec![
                        ScenarioCandidate {
                            scenario,
                            evidence: vec![evidence()],
                        },
                        ScenarioCandidate {
                            scenario: ScenarioId::new(),
                            evidence: vec![CorrelationEvidence::trace("fixture", "trace-b")],
                        },
                    ]),
                ),
                (
                    unknown_ref,
                    InteractionRoleResolution::unknown(vec![]),
                    CorrelationResolution::uncorrelated(vec![]),
                ),
                (
                    ambiguous_ref,
                    InteractionRoleResolution::normalize(vec![
                        CorrelationEvidence::passive_http(),
                        CorrelationEvidence::active_http(),
                    ]),
                    resolved(scenario),
                ),
            ],
        );
        assert_eq!(
            graph.role_resolution(&ambiguous_ref).unwrap().known_role(),
            None
        );
        assert_eq!(
            graph
                .resolution(&ambiguous_ref)
                .unwrap()
                .selected_scenario(),
            Some(scenario)
        );
        assert!(graph.validate_structure().is_err());
        let mut valid = graph;
        valid
            .add_scenario(Scenario::new(scenario, ingress_ref))
            .unwrap();
        valid.scenarios[0].members.push(ambiguous_ref);
        assert!(valid.validate_structure().is_ok());
    }

    #[test]
    fn only_known_ingress_is_root_and_egress_can_be_child() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let child = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (child, egress(), resolved(scenario)),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        graph.scenarios[0].members.push(child);
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario,
            root,
            child,
            vec![evidence()],
        ));
        assert!(graph.validate_structure().is_ok());

        graph.scenarios[0].root = child;
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::InvalidScenarioRoot { .. })
        ));
    }

    #[test]
    fn scoped_reference_checks_recording_epoch_session_and_unique_operation() {
        let recording_id = RecordingId::new();
        let operation_id = OperationId::new();
        let reference = reference(recording_id, operation_id);
        let session = session_for(reference);
        assert_eq!(
            reference.resolve_in_session(&session).unwrap().id,
            operation_id
        );

        let mut wrong_recording = session.clone();
        wrong_recording.source_provenance.recording_id = Some(RecordingId::new());
        assert!(matches!(
            reference.resolve_in_session(&wrong_recording),
            Err(CorrelationValidationError::ReferenceRecordingMismatch { .. })
        ));
        let mut wrong_epoch = session.clone();
        wrong_epoch.source_provenance.epoch_id = Some(EpochId::new());
        assert!(matches!(
            reference.resolve_in_session(&wrong_epoch),
            Err(CorrelationValidationError::ReferenceEpochMismatch { .. })
        ));
        let mut wrong_session = session.clone();
        wrong_session.id = SessionId::new();
        assert!(matches!(
            reference.resolve_in_session(&wrong_session),
            Err(CorrelationValidationError::ReferenceSessionMismatch { .. })
        ));
        let mut missing = session.clone();
        missing.connections[0].operations.clear();
        assert!(matches!(
            reference.resolve_in_session(&missing),
            Err(CorrelationValidationError::OperationNotFound { .. })
        ));

        let mut missing_owner = session.clone();
        missing_owner.connections[0].operations[0]
            .provenance
            .completion_owner_epoch = None;
        assert!(matches!(
            reference.resolve_in_session(&missing_owner),
            Err(CorrelationValidationError::OperationOwnerMismatch { found: None, .. })
        ));

        let mut missing_range = session.clone();
        missing_range.connections[0].operations[0]
            .provenance
            .epoch_ranges
            .clear();
        assert!(matches!(
            reference.resolve_in_session(&missing_range),
            Err(CorrelationValidationError::OperationOwnerMissingRange { .. })
        ));
    }

    #[test]
    fn graph_validation_requires_explicit_session_lineage() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let reference = reference(recording_id, OperationId::new());
        let mut graph = graph_with(recording_id, &[(reference, ingress(), resolved(scenario))]);
        graph
            .add_scenario(Scenario::new(scenario, reference))
            .unwrap();
        let session = session_for(reference);

        assert!(graph.validate_structure().is_ok());
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::MissingSessionContext { .. })
        ));
        assert!(matches!(
            graph.validate_against_sessions(&[]),
            Err(CorrelationValidationError::MissingSessionContext { .. })
        ));
        assert!(
            graph
                .validate_against_sessions(std::slice::from_ref(&session))
                .is_ok()
        );

        let mut wrong_epoch = session.clone();
        wrong_epoch.source_provenance.epoch_id = Some(EpochId::new());
        assert!(matches!(
            graph.validate_against_sessions(std::slice::from_ref(&wrong_epoch)),
            Err(CorrelationValidationError::ReferenceEpochMismatch { .. })
        ));
    }

    #[test]
    fn equal_operation_ids_remain_distinct_when_scoped() {
        let operation_id = OperationId::new();
        let left = reference(RecordingId::new(), operation_id);
        let right = reference(RecordingId::new(), operation_id);
        assert_ne!(left, right);
        let mut graph = CorrelationGraph::new(left.recording_id);
        graph
            .admit_operation(left, ingress(), CorrelationResolution::uncorrelated(vec![]))
            .unwrap();
        assert!(
            graph
                .admit_operation(
                    right,
                    ingress(),
                    CorrelationResolution::uncorrelated(vec![])
                )
                .is_err()
        );
    }

    #[test]
    fn supplied_membership_does_not_derive_from_interleaving() {
        let recording_id = RecordingId::new();
        let a = ScenarioId::new();
        let b = ScenarioId::new();
        let c = ScenarioId::new();
        let op1 = reference(recording_id, OperationId::new());
        let op2 = reference(recording_id, OperationId::new());
        let op3 = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (op1, ingress(), resolved(a)),
                (op2, egress(), resolved(b)),
                (op3, egress(), resolved(c)),
            ],
        );
        graph.add_scenario(Scenario::new(a, op1)).unwrap();
        graph.add_scenario(Scenario::new(b, op2)).unwrap();
        graph.add_scenario(Scenario::new(c, op3)).unwrap();
        assert!(graph.validate_structure().is_err());
        graph
            .role_resolutions
            .get_mut(&op2)
            .unwrap()
            .clone_from(&ingress());
        graph
            .role_resolutions
            .get_mut(&op3)
            .unwrap()
            .clone_from(&ingress());
        assert!(graph.validate_structure().is_ok());
    }

    #[test]
    fn ambiguous_and_uncorrelated_operations_have_no_membership_or_edges() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let uncertain = reference(recording_id, OperationId::new());
        let uncorrelated = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (
                    uncertain,
                    ingress(),
                    CorrelationResolution::ambiguous(vec![
                        ScenarioCandidate {
                            scenario,
                            evidence: vec![evidence()],
                        },
                        ScenarioCandidate {
                            scenario: ScenarioId::new(),
                            evidence: vec![CorrelationEvidence::trace("fixture", "trace-c")],
                        },
                    ]),
                ),
                (
                    uncorrelated,
                    InteractionRoleResolution::unknown(vec![]),
                    CorrelationResolution::uncorrelated(vec![CorrelationEvidence::temporal(
                        RelativeTimeNanos(1),
                        Some(RelativeTimeNanos(2)),
                    )]),
                ),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        assert!(graph.validate_structure().is_ok());
        assert!(!graph.scenarios[0].members.contains(&uncertain));
        assert!(!graph.scenarios[0].members.contains(&uncorrelated));
    }

    #[test]
    fn selected_edges_reject_shape_and_scenario_errors() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let other = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let child = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (child, egress(), resolved(scenario)),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        graph.scenarios[0].members.push(child);
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, root, root, vec![]));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::SelfEdge { .. })
        ));

        graph.causal_edges.clear();
        graph.add_causal_edge(SelectedCausalEdge::new(other, root, child, vec![]));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::MissingScenario { .. })
        ));

        graph.causal_edges.clear();
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario,
            root,
            CanonicalOperationRef::new(
                recording_id,
                EpochId::new(),
                SessionId::new(),
                OperationId::new(),
            ),
            vec![],
        ));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::EdgeMissingEndpoint { .. })
        ));
    }

    #[test]
    fn selected_edges_require_non_temporal_evidence() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let child = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (child, egress(), resolved(scenario)),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        graph.scenarios[0].members.push(child);
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, root, child, Vec::new()));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::InvalidEvidence(_))
        ));

        graph.causal_edges.clear();
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario,
            root,
            child,
            vec![CorrelationEvidence::temporal(
                RelativeTimeNanos(0),
                Some(RelativeTimeNanos(1)),
            )],
        ));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::TemporalOnlyEdgeEvidence)
        ));
    }

    #[test]
    fn selected_edges_reject_cycles_and_multiple_parents() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let left = reference(recording_id, OperationId::new());
        let right = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (left, egress(), resolved(scenario)),
                (right, egress(), resolved(scenario)),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        graph.scenarios[0].members.extend([left, right]);
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, root, left, vec![]));
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, root, right, vec![]));
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, left, right, vec![]));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::MultipleSelectedParents { .. })
        ));

        graph.causal_edges.truncate(2);
        graph.add_causal_edge(SelectedCausalEdge::new(scenario, left, root, vec![]));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::CausalCycle)
        ));
    }

    #[test]
    fn all_supported_active_dependency_protocols_are_egress() {
        for protocol in ["postgresql", "mysql", "redis"] {
            assert!(matches!(
                InteractionRoleResolution::normalize(vec![CorrelationEvidence::active_dependency(
                    protocol
                )]),
                InteractionRoleResolution::Known {
                    role: InteractionRole::Egress,
                    ..
                }
            ));
        }
    }

    #[test]
    fn bidirectional_payload_and_socket_direction_do_not_change_role() {
        let ingress = InteractionRoleResolution::normalize(vec![
            CorrelationEvidence::passive_http(),
            CorrelationEvidence::wire_direction(Direction::ClientToServer),
            CorrelationEvidence::wire_direction(Direction::ServerToClient),
            CorrelationEvidence::socket_role(SocketRoleEvidence::Passive),
        ]);
        assert_eq!(ingress.known_role(), Some(InteractionRole::Ingress));
        let egress = InteractionRoleResolution::normalize(vec![
            CorrelationEvidence::active_http(),
            CorrelationEvidence::wire_direction(Direction::ClientToServer),
            CorrelationEvidence::wire_direction(Direction::ServerToClient),
            CorrelationEvidence::socket_role(SocketRoleEvidence::Active),
        ]);
        assert_eq!(egress.known_role(), Some(InteractionRole::Egress));
        assert_eq!(
            InteractionRoleResolution::normalize(vec![
                CorrelationEvidence::wire_direction(Direction::ClientToServer),
                CorrelationEvidence::wire_direction(Direction::ServerToClient),
            ])
            .known_role(),
            None
        );
    }

    #[test]
    fn unknown_and_ambiguous_roles_cannot_be_roots() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        for role in [
            InteractionRoleResolution::unknown(vec![evidence()]),
            InteractionRoleResolution::normalize(vec![
                CorrelationEvidence::passive_http(),
                CorrelationEvidence::active_http(),
            ]),
        ] {
            let root = reference(recording_id, OperationId::new());
            let mut graph = graph_with(recording_id, &[(root, role, resolved(scenario))]);
            graph.add_scenario(Scenario::new(scenario, root)).unwrap();
            assert!(matches!(
                graph.validate_structure(),
                Err(CorrelationValidationError::InvalidScenarioRoot { .. })
            ));
        }
    }

    #[test]
    fn scoped_operation_ids_can_repeat_within_recording() {
        let recording_id = RecordingId::new();
        let operation_id = OperationId::new();
        let first = reference(recording_id, operation_id);
        let second = CanonicalOperationRef::new(
            recording_id,
            EpochId::new(),
            SessionId::new(),
            operation_id,
        );
        let graph = graph_with(
            recording_id,
            &[
                (
                    first,
                    ingress(),
                    CorrelationResolution::uncorrelated(vec![]),
                ),
                (
                    second,
                    ingress(),
                    CorrelationResolution::uncorrelated(vec![]),
                ),
            ],
        );
        assert_eq!(graph.role_resolutions.len(), 2);
        assert!(graph.validate_structure().is_ok());
    }

    #[test]
    fn infrastructure_and_temporal_values_remain_evidence() {
        let evidence_items = vec![
            CorrelationEvidence::new(CorrelationEvidenceKind::ExecutionTaskLineage {
                task: "worker-1".into(),
                worker: Some("thread-pool".into()),
            }),
            CorrelationEvidence::new(CorrelationEvidenceKind::ProcessThreadGeneration {
                process_id: Some(7),
                thread_id: Some(8),
                process_generation: Some(1),
                thread_generation: Some(2),
            }),
            CorrelationEvidence::new(CorrelationEvidenceKind::ConnectionSocketGeneration {
                connection_id: Some(ConnectionId::new()),
                socket_id: Some("socket-1".into()),
                generation: Some(1),
            }),
            CorrelationEvidence::new(CorrelationEvidenceKind::ProtocolStream {
                stream_id: "stream-1".into(),
                generation: Some(1),
            }),
            CorrelationEvidence::temporal(RelativeTimeNanos(1), Some(RelativeTimeNanos(2))),
        ];
        assert!(validate_evidence(&evidence_items).is_ok());
        let recording_id = RecordingId::new();
        let operation = reference(recording_id, OperationId::new());
        let graph = graph_with(
            recording_id,
            &[(
                operation,
                InteractionRoleResolution::unknown(evidence_items.clone()),
                CorrelationResolution::uncorrelated(evidence_items),
            )],
        );
        assert!(graph.validate_structure().is_ok());
        assert!(graph.role_resolution(&operation).is_some());
    }

    #[test]
    fn cross_scenario_edges_and_duplicate_membership_are_rejected() {
        let recording_id = RecordingId::new();
        let scenario_a = ScenarioId::new();
        let scenario_b = ScenarioId::new();
        let root_a = reference(recording_id, OperationId::new());
        let root_b = reference(recording_id, OperationId::new());
        let mut graph = graph_with(
            recording_id,
            &[
                (root_a, ingress(), resolved(scenario_a)),
                (root_b, ingress(), resolved(scenario_b)),
            ],
        );
        graph
            .add_scenario(Scenario::new(scenario_a, root_a))
            .unwrap();
        graph
            .add_scenario(Scenario::new(scenario_b, root_b))
            .unwrap();
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario_a,
            root_a,
            root_b,
            vec![evidence()],
        ));
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::EdgeScenarioMismatch { .. })
        ));
        graph.causal_edges.clear();
        graph.scenarios[0].members.push(root_a);
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::DuplicateMembership { .. })
        ));
    }

    #[test]
    fn ambiguous_role_stays_non_root_after_resolved_correlation() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let root = reference(recording_id, OperationId::new());
        let ambiguous = reference(recording_id, OperationId::new());
        let role = InteractionRoleResolution::normalize(vec![
            CorrelationEvidence::passive_http(),
            CorrelationEvidence::active_http(),
        ]);
        let mut graph = graph_with(
            recording_id,
            &[
                (root, ingress(), resolved(scenario)),
                (ambiguous, role, resolved(scenario)),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, root)).unwrap();
        graph.scenarios[0].members.push(ambiguous);
        assert!(graph.validate_structure().is_ok());
        graph.scenarios[0].root = ambiguous;
        assert!(matches!(
            graph.validate_structure(),
            Err(CorrelationValidationError::InvalidScenarioRoot { .. })
        ));
    }

    #[test]
    fn temporal_only_resolved_outcome_is_rejected() {
        assert!(matches!(
            CorrelationResolution::resolved(
                ScenarioId::new(),
                vec![CorrelationEvidence::temporal(
                    RelativeTimeNanos(0),
                    Some(RelativeTimeNanos(1)),
                )],
            )
            .validate(),
            Err(CorrelationValidationError::TemporalOnlyResolution)
        ));
    }

    #[test]
    fn cross_epoch_references_and_scenario_id_are_stable() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let epoch_n = EpochId::new();
        let epoch_successor = EpochId::new();
        let session_n = SessionId::new();
        let parent_session_id = SessionId::new();
        let child_session_id = SessionId::new();
        let parent = CanonicalOperationRef::new(
            recording_id,
            epoch_successor,
            parent_session_id,
            OperationId::new(),
        );
        let child = CanonicalOperationRef::new(
            recording_id,
            epoch_successor,
            child_session_id,
            OperationId::new(),
        );
        let historical =
            CanonicalOperationRef::new(recording_id, epoch_n, session_n, OperationId::new());

        let mut parent_session = session_for(parent);
        parent_session.connections[0].operations[0]
            .provenance
            .epoch_ranges
            .insert(
                0,
                OperationEpochRange {
                    parent_id: None,
                    epoch_id: epoch_n,
                    epoch_ordinal: Some(0),
                    wal_sequence_range: None,
                },
            );
        parent_session.validate().unwrap();
        let child_session = session_for(child);
        let historical_session = session_for(historical);
        let sessions = vec![parent_session, child_session, historical_session];

        let mut graph = graph_with(
            recording_id,
            &[
                (parent, ingress(), resolved(scenario)),
                (child, egress(), resolved(scenario)),
                (
                    historical,
                    InteractionRoleResolution::unknown(vec![]),
                    CorrelationResolution::uncorrelated(vec![]),
                ),
            ],
        );
        graph.add_scenario(Scenario::new(scenario, parent)).unwrap();
        graph.scenarios[0].members.push(child);
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario,
            parent,
            child,
            vec![evidence()],
        ));
        assert!(graph.validate_against_sessions(&sessions).is_ok());
        assert_eq!(graph.scenarios[0].id, scenario);
        assert_eq!(parent.owner_epoch_id, epoch_successor);
        assert!(
            sessions[0].connections[0].operations[0]
                .provenance
                .epoch_ranges
                .iter()
                .any(|range| range.epoch_id == epoch_n)
        );
        assert_ne!(parent.owner_epoch_id, historical.owner_epoch_id);
        assert_ne!(parent.session_id, child.session_id);
    }

    #[test]
    fn correlation_does_not_change_completeness() {
        let recording_id = RecordingId::new();
        let scenario = ScenarioId::new();
        let operation_ref = reference(recording_id, OperationId::new());
        let mut session = session_for(operation_ref);
        session
            .operation_completeness
            .insert(operation_ref.operation_id, Completeness::Incomplete);
        let completeness_before = session.operation_state(&operation_ref.operation_id);
        let mut graph = graph_with(
            recording_id,
            &[(operation_ref, ingress(), resolved(scenario))],
        );
        graph
            .add_scenario(Scenario::new(scenario, operation_ref))
            .unwrap();
        assert!(graph.validate_structure().is_ok());
        assert_eq!(
            graph
                .resolution(&operation_ref)
                .unwrap()
                .selected_scenario(),
            Some(scenario)
        );
        assert_eq!(
            session.operation_state(&operation_ref.operation_id),
            completeness_before
        );
        assert!(!crate::OperationCompletion::Incomplete.is_replayable());
    }
}
