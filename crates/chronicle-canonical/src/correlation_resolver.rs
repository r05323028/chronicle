//! Deterministic three-phase correlation resolver (`correlation-resolver`).
//!
//! Phase A1 propagates sparse scenario-support sets to a fixed point without
//! materializing outcomes; Phase A2 derives canonical ownership witnesses from
//! the closed support structure and immutable relation indexes, then
//! materializes `Resolved` / `Ambiguous` / `Uncorrelated` exactly once; Phase
//! B constructs direct causal-parent edges through a fixed normative sequence
//! with deterministic cycle safety.

use crate::correlation::{CanonicalOperationRef, scenario_id_v1};
use crate::{
    CorrelationConfidence, CorrelationEvidence, CorrelationEvidenceKind, CorrelationGraph,
    CorrelationResolution, InteractionRoleResolution, RecordingId, RelativeTimeNanos, Scenario,
    ScenarioCandidate, SelectedCausalEdge,
};
use chronicle_common::ScenarioId;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

/// Target total retained correlation evidence per resolution slot or ambiguity
/// candidate. Required semantic witnesses are never truncated; optional fill
/// uses remaining capacity, so long semantic proofs may exceed this target.
pub const CORRELATION_RETENTION_TARGET: usize = 64;

/// Lifetime view of one canonical operation supplied alongside its role and
/// correlation evidence. Temporal values stay contextual in this revision:
/// nothing in the resolver compares lifetimes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationCorrelationView {
    pub started_at_offset: RelativeTimeNanos,
    pub completed_at_offset: Option<RelativeTimeNanos>,
}

/// Complete resolver input for one canonical operation.
///
/// `role` (with its nested role evidence) and `evidence` (the correlation
/// channel) are distinct input channels. Role evidence never participates in
/// correlation predicates; correlation evidence is the only indexed input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CorrelationInput {
    pub reference: CanonicalOperationRef,
    pub operation_view: OperationCorrelationView,
    pub role: InteractionRoleResolution,
    pub evidence: Vec<CorrelationEvidence>,
}

impl CorrelationInput {
    pub fn new(
        reference: CanonicalOperationRef,
        operation_view: OperationCorrelationView,
        role: InteractionRoleResolution,
        evidence: Vec<CorrelationEvidence>,
    ) -> Self {
        Self {
            reference,
            operation_view,
            role,
            evidence,
        }
    }
}

/// Explicit provider-neutral join carrier mapping full operation references to
/// supplied role resolutions and correlation-evidence collections. Canonical
/// sessions alone are not complete resolver input; `chronicle-etl` composes
/// the join, this type carries it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CorrelationContext {
    pub role_resolutions: BTreeMap<CanonicalOperationRef, InteractionRoleResolution>,
    pub evidence: BTreeMap<CanonicalOperationRef, Vec<CorrelationEvidence>>,
}

/// Typed, fail-closed resolver errors. Ordinary evidentiary situations —
/// missing relationships, ambiguity, uncertain parenthood, cycles — are
/// normal outcomes, never errors.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CorrelationResolverError {
    #[error("duplicate operation reference {reference:?}")]
    DuplicateReference { reference: CanonicalOperationRef },
    #[error("operation reference {reference:?} belongs to recording {found}, expected {expected}")]
    RecordingScopeMismatch {
        reference: CanonicalOperationRef,
        expected: RecordingId,
        found: RecordingId,
    },
    #[error("invalid role resolution for {reference:?}: {message}")]
    InvalidRole {
        reference: CanonicalOperationRef,
        message: String,
    },
    #[error("invalid correlation evidence for {reference:?}: {message}")]
    InvalidCorrelationEvidence {
        reference: CanonicalOperationRef,
        message: String,
    },
    #[error(
        "caller-supplied ScenarioRoot evidence is reserved for the resolver ({channel}) at {reference:?}"
    )]
    ReservedScenarioRootInput {
        reference: CanonicalOperationRef,
        channel: &'static str,
    },
    #[error("impossible internal graph invariant after construction")]
    InternalInvariant(String),
}

const CHANNEL_CORRELATION: &str = "correlation-evidence";
const CHANNEL_ROLE_KNOWN: &str = "role-known-evidence";
const CHANNEL_ROLE_UNKNOWN: &str = "role-unknown-evidence";
const CHANNEL_ROLE_AMBIGUOUS: &str = "role-ambiguous-candidate-evidence";

/// Input reservation: caller-supplied `ScenarioRoot` items are invalid anywhere
/// in caller-controlled input, in EITHER evidence channel. Scanning role
/// evidence for the forbidden value does not convert it into correlation input.
fn reject_reserved_scenario_root(
    reference: CanonicalOperationRef,
    role: &InteractionRoleResolution,
    evidence: &[CorrelationEvidence],
) -> Result<(), CorrelationResolverError> {
    let is_root = |item: &CorrelationEvidence| {
        matches!(item.kind, CorrelationEvidenceKind::ScenarioRoot { .. })
    };
    if evidence.iter().any(is_root) {
        return Err(CorrelationResolverError::ReservedScenarioRootInput {
            reference,
            channel: CHANNEL_CORRELATION,
        });
    }
    match role {
        InteractionRoleResolution::Known { evidence, .. } if evidence.iter().any(is_root) => {
            Err(CorrelationResolverError::ReservedScenarioRootInput {
                reference,
                channel: CHANNEL_ROLE_KNOWN,
            })
        }
        InteractionRoleResolution::Unknown { evidence } if evidence.iter().any(is_root) => {
            Err(CorrelationResolverError::ReservedScenarioRootInput {
                reference,
                channel: CHANNEL_ROLE_UNKNOWN,
            })
        }
        InteractionRoleResolution::Ambiguous { candidates } => {
            for candidate in candidates {
                if candidate.evidence.iter().any(is_root) {
                    return Err(CorrelationResolverError::ReservedScenarioRootInput {
                        reference,
                        channel: CHANNEL_ROLE_AMBIGUOUS,
                    });
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Immutable relation indexes built ONCE from the complete validated
/// correlation-channel evidence. Ordered structures keep evaluation
/// deterministic and indexed lookups replace global scans.
#[derive(Debug, Default)]
struct RelationIndexes {
    /// `(provider, trace_id)` -> operations carrying a trace relationship
    /// with that identity.
    by_trace: BTreeMap<(String, String), BTreeSet<CanonicalOperationRef>>,
    /// `(provider, trace_id, span_id)` -> operations EXPOSING that span.
    by_span: BTreeMap<(String, String, String), BTreeSet<CanonicalOperationRef>>,
}

impl RelationIndexes {
    fn build(inputs: &BTreeMap<CanonicalOperationRef, CorrelationInput>) -> Self {
        let mut indexes = Self::default();
        for (reference, input) in inputs {
            for item in &input.evidence {
                if let CorrelationEvidenceKind::TraceRelationship {
                    provider,
                    trace_id,
                    span_id,
                    parent_span_id,
                } = &item.kind
                {
                    indexes
                        .by_trace
                        .entry((provider.clone(), trace_id.clone()))
                        .or_default()
                        .insert(*reference);
                    if let Some(span_id) = span_id
                        && !span_id.is_empty()
                    {
                        indexes
                            .by_span
                            .entry((provider.clone(), trace_id.clone(), span_id.clone()))
                            .or_default()
                            .insert(*reference);
                    }
                    let _ = parent_span_id;
                }
            }
        }
        indexes
    }
}

/// Directional declared-parenthood check: does THIS operation declare
/// `candidate` as its span parent (`reference.parent_span_id ==
/// candidate.span_id` under one provider+trace identity)? `ExplicitParentSpan`
/// is directional — a reverse declaration by the candidate never creates a
/// relationship, never suppresses `SharedTraceIdentity`, and never routes
/// inherited support through the transitive channel.
fn declares_parent(
    reference: &CanonicalOperationRef,
    candidate: &CanonicalOperationRef,
    declared_parents: &BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>>,
) -> bool {
    declared_parents
        .get(reference)
        .is_some_and(|parents| parents.contains(candidate))
}

/// Sparse per-operation support state. Memberships exist only for discovered
/// positive relationships — never an eager operation-times-scenario matrix —
/// and no transitive witness PATHS are stored here (they can be exponential).
#[derive(Clone, Debug, Default)]
struct Support {
    scenarios: BTreeSet<ScenarioId>,
    /// Subset of `scenarios` reached by a DIRECT `SharedTraceIdentity`
    /// relationship of the operation itself.
    direct: BTreeSet<ScenarioId>,
}

/// One resolved trace-relationship fact extracted from correlation evidence.
#[derive(Clone, Debug)]
struct TraceFact {
    provider: String,
    trace_id: String,
    parent_span_id: Option<String>,
    /// Position of the owning `CorrelationEvidence` item inside the input.
    item_index: usize,
}

fn trace_facts(evidence: &[CorrelationEvidence]) -> Vec<TraceFact> {
    let mut facts = Vec::new();
    for (index, item) in evidence.iter().enumerate() {
        if let CorrelationEvidenceKind::TraceRelationship {
            provider,
            trace_id,
            span_id: _,
            parent_span_id,
        } = &item.kind
        {
            facts.push(TraceFact {
                provider: provider.clone(),
                trace_id: trace_id.clone(),
                parent_span_id: parent_span_id.clone(),
                item_index: index,
            });
        }
    }
    facts
}

/// Typed total canonical evidence key. Each field is a typed ordered part,
/// so string boundaries are structural and Rust's lexicographic `String` order
/// preserves proper-prefix ordering (`"a" < "aa"`).
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
enum CanonicalEvidenceKeyPart {
    Text(String),
    OptionalText(Option<String>),
    U64(u64),
    OptionalU64(Option<u64>),
    OptionalConnectionUuid(Option<[u8; 16]>),
    Reference(CanonicalOperationRef),
    CandidateScope(Option<CanonicalOperationRef>),
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CanonicalEvidenceKey(Vec<CanonicalEvidenceKeyPart>);

/// Total canonical evidence key covering the ENTIRE value so distinct
/// serialized evidence items can never tie: kind discriminator, every semantic
/// field of the kind in declaration order (strings by UTF-8 bytes, options as
/// `None < Some`), the candidate full-reference tuple when candidate-relative,
/// then `provenance.source`, then `provenance.observation`.
#[allow(clippy::too_many_lines)] // One flat match keeps declaration-field order visible.
fn canonical_evidence_key(
    item: &CorrelationEvidence,
    candidate_scope: Option<&CanonicalOperationRef>,
) -> CanonicalEvidenceKey {
    fn push_str(key: &mut CanonicalEvidenceKey, value: &str) {
        key.0.push(CanonicalEvidenceKeyPart::Text(value.to_owned()));
    }
    fn push_opt_str(key: &mut CanonicalEvidenceKey, value: Option<&String>) {
        key.0
            .push(CanonicalEvidenceKeyPart::OptionalText(value.cloned()));
    }
    fn push_opt_u64(key: &mut CanonicalEvidenceKey, value: Option<&u64>) {
        key.0
            .push(CanonicalEvidenceKeyPart::OptionalU64(value.copied()));
    }
    fn push_opt_uuid(key: &mut CanonicalEvidenceKey, value: Option<&crate::ConnectionId>) {
        key.0.push(CanonicalEvidenceKeyPart::OptionalConnectionUuid(
            value.map(|value| *value.as_uuid().as_bytes()),
        ));
    }
    fn push_ref(key: &mut CanonicalEvidenceKey, reference: &CanonicalOperationRef) {
        key.0.push(CanonicalEvidenceKeyPart::Reference(*reference));
    }

    let mut key = CanonicalEvidenceKey(Vec::new());
    match &item.kind {
        CorrelationEvidenceKind::TraceRelationship {
            provider,
            trace_id,
            span_id,
            parent_span_id,
        } => {
            push_str(&mut key, "trace_relationship");
            push_str(&mut key, provider);
            push_str(&mut key, trace_id);
            push_opt_str(&mut key, span_id.as_ref());
            push_opt_str(&mut key, parent_span_id.as_ref());
        }
        CorrelationEvidenceKind::ScenarioRoot { root } => {
            push_str(&mut key, "scenario_root");
            push_ref(&mut key, root);
        }
        CorrelationEvidenceKind::ProtocolOwnership {
            protocol,
            ownership,
        } => {
            push_str(&mut key, "protocol_ownership");
            push_str(&mut key, protocol.as_str());
            push_str(
                &mut key,
                match ownership {
                    crate::ApplicationOwnership::PassiveApplicationServer => {
                        "passive_application_server"
                    }
                    crate::ApplicationOwnership::ActiveOutbound => "active_outbound",
                },
            );
        }
        CorrelationEvidenceKind::ExecutionTaskLineage { task, worker } => {
            push_str(&mut key, "execution_task_lineage");
            push_str(&mut key, task);
            push_opt_str(&mut key, worker.as_ref());
        }
        CorrelationEvidenceKind::ProcessThreadGeneration {
            process_id,
            thread_id,
            process_generation,
            thread_generation,
        } => {
            push_str(&mut key, "process_thread_generation");
            push_opt_u64(&mut key, process_id.as_ref());
            push_opt_u64(&mut key, thread_id.as_ref());
            push_opt_u64(&mut key, process_generation.as_ref());
            push_opt_u64(&mut key, thread_generation.as_ref());
        }
        CorrelationEvidenceKind::ConnectionSocketGeneration {
            connection_id,
            socket_id,
            generation,
        } => {
            push_str(&mut key, "connection_socket_generation");
            push_opt_uuid(&mut key, connection_id.as_ref());
            push_opt_str(&mut key, socket_id.as_ref());
            push_opt_u64(&mut key, generation.as_ref());
        }
        CorrelationEvidenceKind::ProtocolStream {
            stream_id,
            generation,
        } => {
            push_str(&mut key, "protocol_stream");
            push_str(&mut key, stream_id);
            push_opt_u64(&mut key, generation.as_ref());
        }
        CorrelationEvidenceKind::TemporalLifetime { start, end } => {
            push_str(&mut key, "temporal_lifetime");
            key.0.push(CanonicalEvidenceKeyPart::U64(start.0));
            push_opt_u64(&mut key, end.map(|end| end.0).as_ref());
        }
        CorrelationEvidenceKind::WireDirection { direction } => {
            push_str(&mut key, "wire_direction");
            push_str(
                &mut key,
                match direction {
                    crate::Direction::ClientToServer => "client_to_server",
                    crate::Direction::ServerToClient => "server_to_client",
                },
            );
        }
        CorrelationEvidenceKind::SocketRole { role } => {
            push_str(&mut key, "socket_role");
            push_str(
                &mut key,
                match role {
                    crate::SocketRoleEvidence::Active => "active",
                    crate::SocketRoleEvidence::Passive => "passive",
                },
            );
        }
        CorrelationEvidenceKind::Custom {
            namespace,
            key: field,
            value,
        } => {
            push_str(&mut key, "custom");
            push_str(&mut key, namespace);
            push_str(&mut key, field);
            push_str(&mut key, value);
        }
    }
    key.0.push(CanonicalEvidenceKeyPart::CandidateScope(
        candidate_scope.copied(),
    ));
    push_str(&mut key, item.provenance.source.as_str());
    push_opt_str(&mut key, item.provenance.observation.as_ref());
    key
}

/// Resolve correlation for one recording over complete validated inputs.
///
/// Fails closed on broken inputs (duplicate references, scope mismatch,
/// invalid roles/evidence, caller-supplied `ScenarioRoot` anywhere in either
/// channel). Ordinary evidentiary situations resolve normally.
///
/// Output satisfies all correlation graph structural/domain invariants and
/// succeeds under `validate_against_sessions(...)` with the canonical sessions
/// owning its referenced operations. Context-free `validate()` intentionally
/// still fails non-empty graphs, unchanged from the foundation contract.
///
/// # Panics
/// Never panics on any input: broken inputs are typed errors, and internal
/// `expect`s guard invariants already established by admission validation and
/// monotonic closure.
#[allow(clippy::too_many_lines)] // One linear three-phase pipeline; splitting
// it across helpers would scatter the phase comments that carry the contract.
pub fn resolve_correlation(
    recording_id: RecordingId,
    inputs: Vec<CorrelationInput>,
) -> Result<CorrelationGraph, CorrelationResolverError> {
    // ---- Admission: deterministic order, duplicate/scope/validation checks ----
    let mut ordered: BTreeMap<CanonicalOperationRef, CorrelationInput> = BTreeMap::new();
    for input in inputs {
        let reference = input.reference;
        if ordered.contains_key(&reference) {
            return Err(CorrelationResolverError::DuplicateReference { reference });
        }
        if reference.recording_id != recording_id {
            return Err(CorrelationResolverError::RecordingScopeMismatch {
                reference,
                expected: recording_id,
                found: reference.recording_id,
            });
        }
        // Input reservation runs FIRST so a smuggled ScenarioRoot surfaces as
        // the typed reservation error even when the surrounding role state
        // would independently fail validation.
        reject_reserved_scenario_root(reference, &input.role, &input.evidence)?;
        input
            .role
            .validate()
            .map_err(|error| CorrelationResolverError::InvalidRole {
                reference,
                message: error.to_string(),
            })?;
        for item in &input.evidence {
            item.validate().map_err(|error| {
                CorrelationResolverError::InvalidCorrelationEvidence {
                    reference,
                    message: error.to_string(),
                }
            })?;
        }
        ordered.insert(reference, input);
    }

    // ---- Immutable relation indexes over FULL validated correlation evidence.
    let indexes = RelationIndexes::build(&ordered);

    // Declared parenthood adjacency over the FULL validated correlation
    // channel, built once: child -> candidates whose exposed span matches the
    // child's parent_span_id under the same provider+trace identity. Declared
    // parenthood is a transitive-support channel; a bare SharedTraceIdentity
    // (no declared parenthood between the pair) is direct scenario-level
    // support.
    let mut declared_parents: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
        BTreeMap::new();
    for (reference, input) in &ordered {
        for fact in trace_facts(&input.evidence) {
            let Some(parent_span_id) = fact.parent_span_id.clone() else {
                continue;
            };
            if parent_span_id.is_empty() {
                continue;
            }
            if let Some(candidates) =
                indexes
                    .by_span
                    .get(&(fact.provider, fact.trace_id, parent_span_id))
            {
                for candidate in candidates {
                    if candidate != reference {
                        declared_parents
                            .entry(*reference)
                            .or_default()
                            .insert(*candidate);
                    }
                }
            }
        }
    }

    let mut roots: BTreeMap<CanonicalOperationRef, EstablishedRoot> = BTreeMap::new();
    let mut support: BTreeMap<CanonicalOperationRef, Support> = BTreeMap::new();
    for (reference, input) in &ordered {
        if input.role.is_root_eligible() {
            let scenario = Scenario::new(scenario_id_v1(reference), *reference);
            let mut entry = Support::default();
            entry.scenarios.insert(scenario.id);
            entry.direct.insert(scenario.id);
            support.insert(*reference, entry);
            roots.insert(*reference, EstablishedRoot { scenario });
        }
    }

    // ---- Phase A1: sparse support closure by snapshot rounds.
    // Each round reads only the previous-round snapshot; support grows
    // monotonically and terminates when a round adds no new membership.
    loop {
        let snapshot = support.clone();
        let mut grew = false;
        for (reference, input) in &ordered {
            if roots.contains_key(reference) {
                continue; // pinned to its own scenario forever
            }
            let facts = trace_facts(&input.evidence);
            let mut inherited_direct = BTreeSet::new();
            let mut inherited_transitive = BTreeSet::new();
            for fact in &facts {
                // SharedTraceIdentity: union the candidate's snapshot support
                // as DIRECT scenario-level support.
                if let Some(candidates) = indexes
                    .by_trace
                    .get(&(fact.provider.clone(), fact.trace_id.clone()))
                {
                    for candidate in candidates {
                        if candidate == reference {
                            continue;
                        }
                        // Declared parenthood between the pair routes the
                        // inheritance through the transitive channel below.
                        if declares_parent(reference, candidate, &declared_parents) {
                            continue;
                        }
                        if let Some(candidate_support) = snapshot.get(candidate) {
                            for scenario in &candidate_support.scenarios {
                                inherited_direct.insert(*scenario);
                            }
                        }
                    }
                }
                // ExplicitParentSpan: inherit the candidate's snapshot support
                // transitively through declared span parenthood.
                if let Some(parent_span_id) = &fact.parent_span_id
                    && !parent_span_id.is_empty()
                    && let Some(candidates) = indexes.by_span.get(&(
                        fact.provider.clone(),
                        fact.trace_id.clone(),
                        parent_span_id.clone(),
                    ))
                {
                    for candidate in candidates {
                        if candidate == reference {
                            continue;
                        }
                        if let Some(candidate_support) = snapshot.get(candidate) {
                            for scenario in &candidate_support.scenarios {
                                inherited_transitive.insert(*scenario);
                            }
                        }
                    }
                }
            }
            let entry = support.entry(*reference).or_default();
            // The bounded per-membership semantic flag is recorded exactly once,
            // when the membership is created; later incidental relationships
            // never upgrade an existing membership's class.
            for scenario in inherited_direct {
                if entry.scenarios.insert(scenario) {
                    entry.direct.insert(scenario);
                    grew = true;
                }
            }
            for scenario in inherited_transitive {
                if entry.scenarios.insert(scenario) {
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }

    // ---- Confidence refresh over the CLOSED support structure.
    // The capability defines `Exact` by the operation's own direct
    // scenario-level relationships among FINAL supporters, so the
    // creation-round flag is refreshed exactly once against the closed graph.
    // State stays bounded: N times S sparse memberships plus one flag set per
    // membership — never witness-path storage.
    let mut refreshed_direct: BTreeMap<CanonicalOperationRef, BTreeSet<ScenarioId>> =
        BTreeMap::new();
    for (reference, input) in &ordered {
        if roots.contains_key(reference) {
            continue; // roots stay pinned to their own scenario
        }
        let mut fresh: BTreeSet<ScenarioId> = BTreeSet::new();
        for fact in trace_facts(&input.evidence) {
            if let Some(candidates) = indexes
                .by_trace
                .get(&(fact.provider.clone(), fact.trace_id.clone()))
            {
                for candidate in candidates {
                    if candidate == reference
                        || declares_parent(reference, candidate, &declared_parents)
                    {
                        continue;
                    }
                    if let Some(candidate_support) = support.get(candidate) {
                        fresh.extend(candidate_support.scenarios.iter().copied());
                    }
                }
            }
        }
        refreshed_direct.insert(*reference, fresh);
    }
    for (reference, fresh) in refreshed_direct {
        if let Some(entry) = support.get_mut(&reference) {
            entry.direct = fresh;
        }
    }

    // ---- Post-closure canonical ownership witness derivation reads the same
    // immutable declared-parenthood adjacency built before Phase A1.
    let parent_edges = &declared_parents;

    // ---- Phase A2: materialize outcomes exactly once from closed support.
    // Canonical witnesses are derived AFTER closure from the immutable relation
    // indexes plus final support sets — never from first-discovery state.
    let mut resolutions: BTreeMap<CanonicalOperationRef, CorrelationResolution> = BTreeMap::new();
    let mut scenario_members: BTreeMap<ScenarioId, BTreeSet<CanonicalOperationRef>> =
        BTreeMap::new();
    for (reference, root) in &roots {
        scenario_members
            .entry(root.scenario.id)
            .or_default()
            .insert(*reference);
    }

    for (reference, input) in &ordered {
        if roots.contains_key(reference) {
            let witness = Witness {
                own_indices: Vec::new(),
                foreign: vec![CorrelationEvidence::new(
                    CorrelationEvidenceKind::ScenarioRoot { root: *reference },
                )],
            };
            let evidence = apply_retention(witness, &input.evidence);
            resolutions.insert(
                *reference,
                CorrelationResolution::Resolved {
                    scenario: roots[reference].scenario.id,
                    confidence: CorrelationConfidence::Exact,
                    evidence,
                },
            );
            continue;
        }
        let entry = support.get(reference).cloned().unwrap_or_default();
        if entry.scenarios.is_empty() {
            let evidence = uncorrelated_retained(&input.evidence);
            resolutions.insert(*reference, CorrelationResolution::Uncorrelated { evidence });
            continue;
        }
        if entry.scenarios.len() == 1 {
            let scenario_id = *entry.scenarios.iter().next().expect("single scenario");
            scenario_members
                .entry(scenario_id)
                .or_default()
                .insert(*reference);
            let witness = derive_ownership_witness(
                *reference,
                scenario_id,
                &input.evidence,
                &support,
                &indexes,
                parent_edges,
                &ordered,
            );
            let confidence = if entry.direct.contains(&scenario_id) {
                CorrelationConfidence::Exact
            } else {
                CorrelationConfidence::Strong
            };
            let evidence = apply_retention(witness, &input.evidence);
            resolutions.insert(
                *reference,
                CorrelationResolution::Resolved {
                    scenario: scenario_id,
                    confidence,
                    evidence,
                },
            );
            continue;
        }
        let mut candidates = Vec::new();
        for scenario_id in &entry.scenarios {
            let witness = derive_ownership_witness(
                *reference,
                *scenario_id,
                &input.evidence,
                &support,
                &indexes,
                parent_edges,
                &ordered,
            );
            candidates.push(ScenarioCandidate {
                scenario: *scenario_id,
                evidence: apply_retention(witness, &input.evidence),
            });
        }
        resolutions.insert(*reference, CorrelationResolution::Ambiguous { candidates });
    }

    // ---- Phase B: fixed normative construction per finally resolved scenario.
    let causal_edges = construct_selected_edges(&ordered, &indexes, &scenario_members, &roots);

    // ---- Assembly. Foundation validation confirms what construction guarantees.
    let mut scenarios: Vec<Scenario> = roots
        .values()
        .map(|root| {
            let mut scenario = Scenario::new(root.scenario.id, root.scenario.root);
            scenario.members = scenario_members.get(&scenario.id).map_or_else(
                || vec![scenario.root],
                |members| members.iter().copied().collect(),
            );
            scenario
        })
        .collect();
    scenarios.sort_by_key(|scenario| scenario.id.as_uuid());

    let mut graph = CorrelationGraph::new(recording_id);
    for (reference, input) in &ordered {
        let resolution = resolutions.remove(reference).ok_or_else(|| {
            CorrelationResolverError::InternalInvariant("missing resolution".into())
        })?;
        graph
            .admit_operation(*reference, input.role.clone(), resolution)
            .map_err(|error| CorrelationResolverError::InternalInvariant(error.to_string()))?;
    }
    for scenario in scenarios {
        graph
            .add_scenario(scenario)
            .map_err(|error| CorrelationResolverError::InternalInvariant(error.to_string()))?;
    }
    for edge in causal_edges {
        graph.add_causal_edge(edge);
    }
    Ok(graph)
}

/// Semantic ownership witness for one (operation, scenario) support entry:
/// a canonical DIRECT `SharedTraceIdentity` item when direct support exists,
/// otherwise the canonical transitive `ExplicitParentSpan` proof path.
#[derive(Default)]
struct Witness {
    /// Indices into the operation's own correlation-evidence collection.
    own_indices: Vec<usize>,
    /// Required semantic items not indexed into current operation evidence,
    /// including foreign transitive-proof items and resolver-owned root items.
    foreign: Vec<CorrelationEvidence>,
}

#[allow(clippy::too_many_lines)] // Flat witness selection keeps class rules visible.
fn derive_ownership_witness(
    reference: CanonicalOperationRef,
    scenario_id: ScenarioId,
    evidence: &[CorrelationEvidence],
    support: &BTreeMap<CanonicalOperationRef, Support>,
    indexes: &RelationIndexes,
    parent_edges: &BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>>,
    ordered: &BTreeMap<CanonicalOperationRef, CorrelationInput>,
) -> Witness {
    // Direct class: canonical minimal child-side TraceRelationship item sharing
    // trace identity with some candidate whose closed support contains S.
    let mut best_direct: Option<(CanonicalEvidenceKey, usize)> = None;
    for (index, item) in evidence.iter().enumerate() {
        if let CorrelationEvidenceKind::TraceRelationship {
            provider, trace_id, ..
        } = &item.kind
        {
            let Some(candidates) = indexes.by_trace.get(&(provider.clone(), trace_id.clone()))
            else {
                continue;
            };
            let mut scope: Option<CanonicalOperationRef> = None;
            for candidate in candidates {
                // Declared parenthood between the pair routes that relation
                // through the transitive channel; it never counts here.
                if *candidate != reference
                    && !declares_parent(&reference, candidate, parent_edges)
                    && support.get(candidate).is_some_and(|candidate_support| {
                        candidate_support.scenarios.contains(&scenario_id)
                    })
                {
                    scope = Some(match scope {
                        Some(existing) if existing < *candidate => existing,
                        _ => *candidate,
                    });
                }
            }
            if let Some(scope) = scope {
                let key = canonical_evidence_key(item, Some(&scope));
                if best_direct.as_ref().is_none_or(|(best, _)| key < *best) {
                    best_direct = Some((key, index));
                }
            }
        }
    }
    if let Some((_, index)) = best_direct {
        return Witness {
            own_indices: vec![index],
            foreign: Vec::new(),
        };
    }
    // Transitive class: canonical best simple path under
    // (hop count, reference sequence), then canonical per-relation keys.
    // The proof terminates at a DIRECT-origin supporter (pinned root or
    // direct-flagged member), so the retained path demonstrates the complete
    // inheritance chain rather than stopping at another transitive member.
    let targets: BTreeSet<CanonicalOperationRef> = support
        .iter()
        .filter(|(candidate, candidate_support)| {
            **candidate != reference
                && candidate_support.scenarios.contains(&scenario_id)
                && candidate_support.direct.contains(&scenario_id)
        })
        .map(|(candidate, _)| *candidate)
        .collect();
    let Some(path) = canonical_transitive_path(reference, &targets, parent_edges) else {
        return Witness::default();
    };
    let mut witness = Witness::default();
    let mut from = reference;
    for to in path {
        let mut best_hop: Option<(
            CanonicalEvidenceKey,
            usize,
            bool,
            Option<CorrelationEvidence>,
        )> = None;
        if let Some(input) = ordered.get(&from) {
            for (index, item) in input.evidence.iter().enumerate() {
                if let CorrelationEvidenceKind::TraceRelationship {
                    provider,
                    trace_id,
                    span_id: _,
                    parent_span_id: Some(parent_span_id),
                } = &item.kind
                {
                    if parent_span_id.is_empty() {
                        continue;
                    }
                    let realizes = indexes
                        .by_span
                        .get(&(provider.clone(), trace_id.clone(), parent_span_id.clone()))
                        .is_some_and(|operations| operations.contains(&to));
                    if !realizes {
                        continue;
                    }
                    let key = canonical_evidence_key(item, Some(&to));
                    let own = from == reference;
                    let better = match &best_hop {
                        None => true,
                        Some((best, _, _, _)) => key < *best,
                    };
                    if better {
                        best_hop = Some((key, index, own, Some(item.clone())));
                    }
                }
            }
        }
        match best_hop {
            Some((_, index, true, _)) => witness.own_indices.push(index),
            Some((_, _, false, item)) => {
                witness
                    .foreign
                    .push(item.expect("foreign hop item retained"));
            }
            None => return Witness::default(),
        }
        from = to;
    }
    witness
}

/// Retention happens after ownership semantics: required semantic witnesses are
/// kept in full; `CORRELATION_RETENTION_TARGET` is the ordinary total target,
/// and optional contextual fill uses only its remaining capacity.
fn apply_retention(
    witness: Witness,
    all_evidence: &[CorrelationEvidence],
) -> Vec<CorrelationEvidence> {
    let mut retained: Vec<CorrelationEvidence> =
        Vec::with_capacity(witness.own_indices.len() + witness.foreign.len());
    for index in &witness.own_indices {
        retained.push(all_evidence[*index].clone());
    }
    retained.extend(witness.foreign);
    let consumed: BTreeSet<usize> = witness.own_indices.into_iter().collect();
    let mut fill: Vec<(CanonicalEvidenceKey, &CorrelationEvidence)> = all_evidence
        .iter()
        .enumerate()
        .filter(|(index, _)| !consumed.contains(index))
        .map(|(_, item)| (canonical_evidence_key(item, None), item))
        .collect();
    fill.sort_by(|left, right| left.0.cmp(&right.0));
    let fill_capacity = CORRELATION_RETENTION_TARGET.saturating_sub(retained.len());
    for (_, item) in fill.into_iter().take(fill_capacity) {
        retained.push(item.clone());
    }
    retained
}

/// Uncorrelated retention: canonically sorted contextual/input evidence with no
/// manufactured ownership witness.
fn uncorrelated_retained(evidence: &[CorrelationEvidence]) -> Vec<CorrelationEvidence> {
    let mut sorted: Vec<(CanonicalEvidenceKey, &CorrelationEvidence)> = evidence
        .iter()
        .map(|item| (canonical_evidence_key(item, None), item))
        .collect();
    sorted.sort_by(|left, right| left.0.cmp(&right.0));
    sorted
        .into_iter()
        .take(CORRELATION_RETENTION_TARGET)
        .map(|(_, item)| item.clone())
        .collect()
}

/// Deterministic cycle-safe best-path search over simple paths.
/// Labels order by (hop count, lexicographic reference sequence); each node
/// settles at most once and revisiting any reference is pruned before
/// extension, so cyclic raw relation graphs terminate and search state stays
/// proportional to the visited frontier — never to the number of distinct paths.
fn canonical_transitive_path(
    start: CanonicalOperationRef,
    targets: &BTreeSet<CanonicalOperationRef>,
    parent_edges: &BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>>,
) -> Option<Vec<CanonicalOperationRef>> {
    if targets.contains(&start) {
        return None; // direct support exists; transitive search not applicable
    }
    let mut frontier: BTreeSet<(usize, Vec<CanonicalOperationRef>)> = BTreeSet::new();
    let mut best_seen: BTreeMap<CanonicalOperationRef, (usize, Vec<CanonicalOperationRef>)> =
        BTreeMap::new();
    let mut settled: BTreeSet<CanonicalOperationRef> = BTreeSet::new();
    for first in parent_edges.get(&start).into_iter().flatten() {
        let label = (1usize, vec![*first]);
        best_seen.insert(*first, label.clone());
        frontier.insert(label);
    }
    while let Some(label) = frontier.pop_first() {
        let (hops, sequence) = label;
        let current = *sequence.last().expect("label sequence is non-empty");
        if settled.contains(&current) {
            continue;
        }
        settled.insert(current);
        if targets.contains(&current) {
            return Some(sequence);
        }
        for next in parent_edges.get(&current).into_iter().flatten() {
            if settled.contains(next) || sequence.contains(next) {
                continue; // simple paths only; cycles cannot expand unbounded
            }
            let mut next_sequence = sequence.clone();
            next_sequence.push(*next);
            let next_label = (hops + 1, next_sequence);
            // Label order is monotone under extension, so only a STRICTLY
            // better label may enter the frontier. Without this prune the
            // frontier would hold one entry per distinct simple path —
            // exponentially many in diamond-rich graphs.
            let improves = match best_seen.get(next) {
                Some(existing) => *existing > next_label,
                None => true,
            };
            if !improves {
                continue;
            }
            best_seen.insert(*next, next_label.clone());
            frontier.insert(next_label);
        }
    }
    None
}

/// Phase B construction over FINAL memberships, per resolved scenario, in the
/// exact normative sequence: collect correlation-channel relations, invalid-
/// relation pre-filter, unique-parent mapping, complete provisional graph,
/// global cyclic-SCC analysis, internal-edge removal, canonical emission.
fn construct_selected_edges(
    ordered: &BTreeMap<CanonicalOperationRef, CorrelationInput>,
    indexes: &RelationIndexes,
    scenario_members: &BTreeMap<ScenarioId, BTreeSet<CanonicalOperationRef>>,
    roots: &BTreeMap<CanonicalOperationRef, EstablishedRoot>,
) -> Vec<SelectedCausalEdge> {
    let mut edges = Vec::new();
    for (scenario_id, members) in scenario_members {
        let root = roots
            .values()
            .find(|established| established.scenario.id == *scenario_id)
            .expect("every scenario has an established root")
            .scenario
            .root;
        // Steps 1-3: collect, pre-filter, group surviving relations by child.
        let mut by_child: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
            BTreeMap::new();
        let mut by_child_items: BTreeMap<
            (CanonicalOperationRef, CanonicalOperationRef),
            (CanonicalEvidenceKey, CorrelationEvidence),
        > = BTreeMap::new();
        for (child, input) in ordered {
            if !members.contains(child) || *child == root {
                continue;
            }
            for fact in trace_facts(&input.evidence) {
                let Some(parent_span_id) = fact.parent_span_id else {
                    continue;
                };
                if parent_span_id.is_empty() {
                    continue;
                }
                let span_key = (fact.provider.clone(), fact.trace_id.clone(), parent_span_id);
                let Some(candidates) = indexes.by_span.get(&span_key) else {
                    continue;
                };
                // Shared-span ambiguity blocks only direct-parent sufficiency.
                if candidates.len() > 1 {
                    continue;
                }
                for parent in candidates {
                    if parent == child || !members.contains(parent) {
                        continue;
                    }
                    by_child.entry(*child).or_default().insert(*parent);
                    let item = &input.evidence[fact.item_index];
                    let key = canonical_evidence_key(item, Some(parent));
                    let entry = by_child_items
                        .entry((*child, *parent))
                        .or_insert((key.clone(), item.clone()));
                    if key < entry.0 {
                        *entry = (key, item.clone());
                    }
                }
            }
        }
        // Step 4: unique sufficient parents become provisional edges.
        let mut nodes: BTreeSet<CanonicalOperationRef> = BTreeSet::new();
        let mut adjacency: BTreeMap<CanonicalOperationRef, Vec<CanonicalOperationRef>> =
            BTreeMap::new();
        let mut provisional: Vec<(CanonicalOperationRef, CanonicalOperationRef)> = Vec::new();
        for (child, parents) in &by_child {
            if parents.len() == 1 {
                let parent = *parents.iter().next().expect("single parent");
                provisional.push((*child, parent));
                nodes.insert(*child);
                nodes.insert(parent);
                adjacency.entry(parent).or_default().push(*child);
            }
        }
        for children in adjacency.values_mut() {
            children.sort_unstable();
            children.dedup();
        }
        // Steps 5-7: global cyclic SCC analysis over the COMPLETE provisional
        // graph; remove exactly the edges internal to cyclic components.
        let components = compute_sccs(&nodes, &adjacency);
        for (child, parent) in provisional {
            if components.get(&child) == components.get(&parent) {
                continue; // both endpoints inside the same cyclic component
            }
            let item = &by_child_items[&(child, parent)].1;
            edges.push(SelectedCausalEdge::new(
                *scenario_id,
                parent,
                child,
                vec![item.clone()],
            ));
        }
    }
    // Step 8: canonical emission order.
    edges.sort_by_key(|left| (left.child, left.parent));
    edges.dedup_by(|left, right| left.child == right.child && left.parent == right.parent);
    edges
}

struct EstablishedRoot {
    scenario: Scenario,
}

/// Iterative Tarjan strongly-connected components (no recursion).
fn compute_sccs(
    nodes: &BTreeSet<CanonicalOperationRef>,
    adjacency: &BTreeMap<CanonicalOperationRef, Vec<CanonicalOperationRef>>,
) -> BTreeMap<CanonicalOperationRef, usize> {
    let mut counter: usize = 0;
    let mut indices: BTreeMap<CanonicalOperationRef, usize> = BTreeMap::new();
    let mut lowlinks: BTreeMap<CanonicalOperationRef, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<CanonicalOperationRef> = BTreeSet::new();
    let mut stack: Vec<CanonicalOperationRef> = Vec::new();
    let mut component: BTreeMap<CanonicalOperationRef, usize> = BTreeMap::new();
    let mut component_count: usize = 0;

    for start_node in nodes {
        if indices.contains_key(start_node) {
            continue;
        }
        indices.insert(*start_node, counter);
        lowlinks.insert(*start_node, counter);
        counter += 1;
        stack.push(*start_node);
        on_stack.insert(*start_node);
        let mut call_stack: Vec<(CanonicalOperationRef, usize)> = vec![(*start_node, 0)];
        while let Some(frame) = call_stack.last_mut() {
            let node = frame.0;
            let children: &[CanonicalOperationRef] = adjacency
                .get(&node)
                .map_or(&[][..], |children| children.as_slice());
            if frame.1 < children.len() {
                let next = children[frame.1];
                frame.1 += 1;
                if let std::collections::btree_map::Entry::Vacant(e) = indices.entry(next) {
                    e.insert(counter);
                    lowlinks.insert(next, counter);
                    counter += 1;
                    stack.push(next);
                    on_stack.insert(next);
                    call_stack.push((next, 0));
                } else if on_stack.contains(&next) {
                    let next_index = indices[&next];
                    let lowlink = lowlinks.get_mut(&node).expect("visited node lowlinked");
                    if next_index < *lowlink {
                        *lowlink = next_index;
                    }
                }
            } else {
                call_stack.pop();
                if let Some(parent) = call_stack.last().map(|frame| frame.0) {
                    let child_lowlink = lowlinks[&node];
                    let parent_lowlink = lowlinks.get_mut(&parent).expect("parent lowlinked");
                    if child_lowlink < *parent_lowlink {
                        *parent_lowlink = child_lowlink;
                    }
                }
                if lowlinks[&node] == indices[&node] {
                    loop {
                        let member = stack.pop().expect("tarjan stack non-empty");
                        on_stack.remove(&member);
                        component.insert(member, component_count);
                        if member == node {
                            break;
                        }
                    }
                    component_count += 1;
                }
            }
        }
    }
    component
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        CANONICAL_SCHEMA_VERSION, CanonicalConnection, Completeness, CorrelationValidationError,
        InteractionRole, OperationEffect, OperationEpochRange, OperationKind, OperationProvenance,
        PROTOCOL_DATA_SCHEMA_VERSION, PayloadRef, ProtocolData, SCENARIO_ID_V1_SEPARATOR,
        SourceMetadata, SourceProvenance, TimelineEntry,
    };
    use chronicle_common::{Endpoint, EpochId, OperationId, RecordingId, SessionId};
    use std::collections::{BTreeMap, BTreeSet};
    use time::OffsetDateTime;
    use uuid::Uuid;

    // ---------- helpers ----------

    fn uuid_at(seed: u128) -> Uuid {
        Uuid::from_u128(seed)
    }

    /// Distinct scoped reference per seed: recording 1, epoch 2,
    /// per-seed session, operation seeded.
    fn ref_at(seed: u128) -> CanonicalOperationRef {
        CanonicalOperationRef::new(
            RecordingId::from_uuid(uuid_at(1)),
            EpochId::from_uuid(uuid_at(2)),
            SessionId::from_uuid(uuid_at(1000 + seed)),
            OperationId::from_uuid(uuid_at(seed)),
        )
    }

    fn view() -> OperationCorrelationView {
        OperationCorrelationView {
            started_at_offset: RelativeTimeNanos(0),
            completed_at_offset: Some(RelativeTimeNanos(10)),
        }
    }

    fn ingress_role() -> InteractionRoleResolution {
        InteractionRoleResolution::known(
            InteractionRole::Ingress,
            vec![CorrelationEvidence::passive_http()],
        )
    }

    fn egress_role() -> InteractionRoleResolution {
        InteractionRoleResolution::known(
            InteractionRole::Egress,
            vec![CorrelationEvidence::active_http()],
        )
    }

    fn unknown_role() -> InteractionRoleResolution {
        InteractionRoleResolution::unknown(vec![CorrelationEvidence::custom(
            "test", "reason", "fixture",
        )])
    }

    fn ambiguous_ingress_egress_role() -> InteractionRoleResolution {
        InteractionRoleResolution::ambiguous(vec![
            crate::InteractionRoleCandidate {
                role: InteractionRole::Ingress,
                evidence: vec![CorrelationEvidence::passive_http()],
            },
            crate::InteractionRoleCandidate {
                role: InteractionRole::Egress,
                evidence: vec![CorrelationEvidence::active_http()],
            },
        ])
    }

    fn input(
        reference: CanonicalOperationRef,
        role: InteractionRoleResolution,
        evidence: Vec<CorrelationEvidence>,
    ) -> CorrelationInput {
        CorrelationInput::new(reference, view(), role, evidence)
    }

    fn trace(
        provider: &str,
        trace_id: &str,
        span: Option<&str>,
        parent: Option<&str>,
    ) -> CorrelationEvidence {
        CorrelationEvidence::new(CorrelationEvidenceKind::TraceRelationship {
            provider: provider.into(),
            trace_id: trace_id.into(),
            span_id: span.map(Into::into),
            parent_span_id: parent.map(Into::into),
        })
    }

    fn task(task_label: &str) -> CorrelationEvidence {
        CorrelationEvidence::new(CorrelationEvidenceKind::ExecutionTaskLineage {
            task: task_label.into(),
            worker: None,
        })
    }

    fn lifetime(start: u64, end: u64) -> CorrelationEvidence {
        CorrelationEvidence::temporal(RelativeTimeNanos(start), Some(RelativeTimeNanos(end)))
    }

    fn resolve(inputs: Vec<CorrelationInput>) -> CorrelationGraph {
        let recording_id = RecordingId::from_uuid(uuid_at(1));
        resolve_correlation(recording_id, inputs).expect("resolution succeeds")
    }

    fn resolved_of(
        graph: &CorrelationGraph,
        reference: CanonicalOperationRef,
    ) -> (
        &ScenarioId,
        CorrelationConfidence,
        &Vec<CorrelationEvidence>,
    ) {
        match graph.resolution(&reference).expect("resolution present") {
            CorrelationResolution::Resolved {
                scenario,
                confidence,
                evidence,
            } => (scenario, *confidence, evidence),
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    fn assert_uncorrelated(graph: &CorrelationGraph, reference: CanonicalOperationRef) {
        assert!(matches!(
            graph.resolution(&reference),
            Some(CorrelationResolution::Uncorrelated { .. })
        ));
    }

    fn assert_ambiguous(
        graph: &CorrelationGraph,
        reference: CanonicalOperationRef,
        expected: &[ScenarioId],
    ) {
        match graph.resolution(&reference) {
            Some(CorrelationResolution::Ambiguous { candidates }) => {
                let mut got: Vec<ScenarioId> = candidates.iter().map(|c| c.scenario).collect();
                got.sort_by_key(|id| id.as_uuid());
                let mut want = expected.to_vec();
                want.sort_by_key(|id| id.as_uuid());
                assert_eq!(got, want);
            }
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }

    /// Minimal valid canonical session owning exactly `reference`.
    fn session_for(
        reference: CanonicalOperationRef,
        extra_epochs: &[EpochId],
    ) -> crate::CanonicalSession {
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
                epoch_ranges: extra_epochs
                    .iter()
                    .chain(std::iter::once(&reference.owner_epoch_id))
                    .map(|epoch_id| OperationEpochRange {
                        parent_id: None,
                        epoch_id: *epoch_id,
                        epoch_ordinal: None,
                        wal_sequence_range: None,
                    })
                    .collect(),
                ..OperationProvenance::default()
            },
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        let connection_id = chronicle_common::ConnectionId::new();
        let session = crate::CanonicalSession {
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
                protocol: chronicle_common::ProtocolId::new("test"),
                client: Endpoint::new("client", 1),
                server: Endpoint::new("server", 2),
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
        session.validate().expect("fixture session valid");
        session
    }

    // ---------- Task 1: scenario-id-v1 identity ----------

    #[test]
    fn separator_is_exactly_thirty_six_ascii_bytes() {
        assert_eq!(SCENARIO_ID_V1_SEPARATOR.len(), 36);
        assert_eq!(
            std::str::from_utf8(SCENARIO_ID_V1_SEPARATOR).unwrap(),
            "chronicle-correlation/scenario-id/v1"
        );
    }

    #[test]
    fn known_answer_matches_specified_algorithm() {
        // SHA-256("chronicle-correlation/scenario-id/v1" || the four UUID
        // byte arrays below in declaration order), truncated to 16 bytes with
        // version-8 / RFC 4122 variant bits.
        let reference = CanonicalOperationRef::new(
            RecordingId::from_uuid(uuid_at(0x00000000_0000_0000_0000_00000000000a)),
            EpochId::from_uuid(uuid_at(0x00000000_0000_0000_0000_00000000000b)),
            SessionId::from_uuid(uuid_at(0x00000000_0000_0000_0000_00000000000c)),
            OperationId::from_uuid(uuid_at(0x00000000_0000_0000_0000_00000000000d)),
        );
        let derived = scenario_id_v1(&reference);
        assert_eq!(
            derived.as_uuid().to_string(),
            "847fe4da-3422-8bb8-a443-13f672399947"
        );
        let octets = *derived.as_uuid().as_bytes();
        assert_eq!(octets[6] >> 4, 0x8, "UUID version field is 8");
        assert_eq!(octets[8] >> 6, 0b10, "RFC 4122 variant bits");
    }

    #[test]
    fn duplicate_operation_ids_across_sessions_derive_distinct_scenarios() {
        let operation_id = OperationId::from_uuid(uuid_at(0x99999999_8888_7777_6666_555555555555));
        let left = CanonicalOperationRef::new(
            RecordingId::from_uuid(uuid_at(0x11111111_2222_3333_4444_555555555555)),
            EpochId::from_uuid(uuid_at(0xaaaa_aaaa_bbbb_cccc_dddd_eeee_eeee_eeee)),
            SessionId::from_uuid(uuid_at(0x12345678_1234_5678_1234_567812345678)),
            operation_id,
        );
        let right = CanonicalOperationRef::new(
            left.recording_id,
            left.owner_epoch_id,
            SessionId::from_uuid(uuid_at(0x87654321_4321_8765_4321_876587654321)),
            operation_id,
        );
        let left_id = scenario_id_v1(&left);
        let right_id = scenario_id_v1(&right);
        assert_ne!(left_id, right_id);
        assert_eq!(
            left_id.as_uuid().to_string(),
            "bbcaac3b-0c5d-8ef6-8e0e-bfb3ea5bd7d1"
        );
        assert_eq!(
            right_id.as_uuid().to_string(),
            "a9ce98d8-53a2-80a8-94f2-4c1165d33ce2"
        );
    }

    #[test]
    fn derivation_is_stable_across_repetition_and_permutation() {
        let seeds: [u128; 4] = [10, 11, 12, 13];
        let forward: Vec<CorrelationInput> = seeds
            .iter()
            .map(|seed| input(ref_at(*seed), ingress_role(), vec![]))
            .collect();
        let mut reversed = forward.clone();
        reversed.reverse();
        let first = resolve(forward);
        let second = resolve(reversed);
        // Byte-identical graphs: ids, memberships, resolutions, witnesses,
        // edges — not merely selected scenarios.
        assert_eq!(first, second);
        for seed in seeds {
            let reference = ref_at(seed);
            assert_eq!(
                first.resolution(&reference).unwrap().selected_scenario(),
                second.resolution(&reference).unwrap().selected_scenario()
            );
        }
    }

    // ---------- Task 2: ScenarioRoot variant and placement consistency ----------

    fn scenario_root_item(root: CanonicalOperationRef) -> CorrelationEvidence {
        CorrelationEvidence::new(CorrelationEvidenceKind::ScenarioRoot { root })
    }

    #[test]
    fn scenario_root_is_non_temporal_and_serializes_tagged() {
        let item = scenario_root_item(ref_at(10));
        assert!(!item.is_temporal());
        let json = serde_json::to_value(&item).unwrap();
        assert_eq!(json["kind"]["kind"], "scenario_root");
        let roundtrip: CorrelationEvidence = serde_json::from_value(json).unwrap();
        assert_eq!(roundtrip, item);
    }

    // ---------- domain placement validation fixtures ----------

    fn placement_fixture() -> (
        CorrelationGraph,
        CanonicalOperationRef,
        CanonicalOperationRef,
    ) {
        let recording_id = RecordingId::from_uuid(uuid_at(1));
        let root = ref_at(10);
        let member = ref_at(11);
        let scenario_id = scenario_id_v1(&root);
        let mut graph = CorrelationGraph::new(recording_id);
        graph
            .admit_operation(
                root,
                ingress_role(),
                CorrelationResolution::Resolved {
                    scenario: scenario_id,
                    confidence: CorrelationConfidence::Exact,
                    evidence: vec![scenario_root_item(root)],
                },
            )
            .unwrap();
        graph
            .admit_operation(
                member,
                egress_role(),
                CorrelationResolution::resolved(scenario_id, vec![trace("otel", "t", None, None)]),
            )
            .unwrap();
        let mut scenario = Scenario::new(scenario_id, root);
        scenario.members.push(member);
        graph.add_scenario(scenario).unwrap();
        (graph, root, member)
    }

    #[test]
    fn valid_root_placement_passes_full_validation() {
        let (graph, root, member) = placement_fixture();
        let sessions = vec![session_for(root, &[]), session_for(member, &[])];
        assert!(graph.validate_against_sessions(&sessions).is_ok());
    }

    #[test]
    fn wrong_scenario_root_reference_fails_minimal_validation() {
        let (mut graph, root, member) = placement_fixture();
        *graph.resolutions.get_mut(&member).unwrap() = CorrelationResolution::Resolved {
            scenario: scenario_id_v1(&root),
            confidence: CorrelationConfidence::Exact,
            evidence: vec![scenario_root_item(ref_at(99))],
        };
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::InvalidScenarioRootPlacement { .. })
        ));
    }

    #[test]
    fn wrong_operation_placement_fails_validation() {
        let (mut graph, _root, member) = placement_fixture();
        let stranger = ref_at(99);
        graph.role_resolutions.insert(stranger, unknown_role());
        graph
            .resolutions
            .insert(stranger, CorrelationResolution::uncorrelated(vec![]));
        // A ScenarioRoot claim for `root` inside a different operation's outcome.
        let resolution = graph.resolutions.get_mut(&member).unwrap();
        if let CorrelationResolution::Resolved { evidence, .. } = resolution {
            evidence.push(scenario_root_item(ref_at(10)));
        }
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::InvalidScenarioRootPlacement { .. })
        ));
    }

    #[test]
    fn non_root_member_claim_fails_validation() {
        let (mut graph, _root, member) = placement_fixture();
        let resolution = graph.resolutions.get_mut(&member).unwrap();
        if let CorrelationResolution::Resolved { evidence, .. } = resolution {
            // The member claims itself as root; no scenario is rooted at it.
            evidence.push(scenario_root_item(member));
        }
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::InvalidScenarioRootPlacement { .. })
        ));
    }

    #[test]
    fn wrong_scenario_selection_fails_validation() {
        let (mut graph, root, member) = placement_fixture();
        let stranger_root = ref_at(50);
        let other_scenario = Scenario::new(scenario_id_v1(&stranger_root), stranger_root);
        // Root's own resolution now selects the OTHER scenario.
        let scenario_id_v1_of_stranger = other_scenario.id;
        graph.add_scenario(other_scenario).unwrap();
        let resolution = graph.resolutions.get_mut(&root).unwrap();
        *resolution = CorrelationResolution::Resolved {
            scenario: scenario_id_v1_of_stranger,
            confidence: CorrelationConfidence::Exact,
            evidence: vec![scenario_root_item(root)],
        };
        // Keep membership consistent enough to reach the placement check:
        // remove member from old scenario and place under stranger scenario.
        for scenario in &mut graph.scenarios {
            if scenario.id == scenario_id_v1(&root) {
                scenario
                    .members
                    .retain(|m| *m == scenario.root || *m == member);
            }
        }
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::InvalidScenarioRootPlacement { .. })
        ));
    }

    #[test]
    fn uncorrelated_and_ambiguous_and_edge_placements_fail_validation() {
        // Uncorrelated evidence carrying ScenarioRoot fails.
        let recording_id = RecordingId::from_uuid(uuid_at(1));
        let operation = ref_at(30);
        let mut graph = CorrelationGraph::new(recording_id);
        graph
            .admit_operation(
                operation,
                unknown_role(),
                CorrelationResolution::uncorrelated(vec![scenario_root_item(operation)]),
            )
            .unwrap();
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::MisplacedScenarioRootEvidence)
        ));

        // Ambiguous candidate evidence carrying ScenarioRoot fails.
        let ambiguous = ref_at(31);
        let mut graph = CorrelationGraph::new(recording_id);
        graph
            .admit_operation(
                ambiguous,
                unknown_role(),
                CorrelationResolution::ambiguous(vec![
                    crate::ScenarioCandidate {
                        scenario: ScenarioId::from_uuid(uuid_at(77)),
                        evidence: vec![scenario_root_item(ambiguous)],
                    },
                    crate::ScenarioCandidate {
                        scenario: ScenarioId::from_uuid(uuid_at(78)),
                        evidence: vec![trace("otel", "t", None, None)],
                    },
                ]),
            )
            .unwrap();
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::MisplacedScenarioRootEvidence)
        ));

        // Selected-edge evidence carrying ScenarioRoot fails.
        let (mut graph, root, member) = placement_fixture();
        graph.add_causal_edge(SelectedCausalEdge::new(
            scenario_id_v1(&root),
            root,
            member,
            vec![scenario_root_item(root)],
        ));
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::MisplacedScenarioRootEvidence)
        ));
    }

    // ---------- Task 3/4: channels and reservation ----------

    #[test]
    fn role_channel_trace_does_not_correlate_but_correlation_channel_does() {
        let root = ref_at(10);
        let egress = ref_at(20);
        let role_with_trace = InteractionRoleResolution::known(
            InteractionRole::Egress,
            vec![
                CorrelationEvidence::active_http(),
                trace("otel", "T1", Some("span-eg"), None),
            ],
        );

        // Trace ONLY inside role evidence: no support.
        let isolated = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T1", Some("span-root"), None)],
            ),
            input(egress, role_with_trace.clone(), vec![]),
        ]);
        assert_uncorrelated(&isolated, egress);
        assert_eq!(isolated.scenarios.len(), 1);

        // The same relationship moved into correlation evidence correlates.
        let correlated = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T1", Some("span-root"), None)],
            ),
            input(
                egress,
                role_with_trace,
                vec![trace("otel", "T1", Some("span-eg"), Some("span-root"))],
            ),
        ]);
        let (scenario, confidence, _) = resolved_of(&correlated, egress);
        assert_eq!(*scenario, scenario_id_v1(&root));
        assert_eq!(confidence, CorrelationConfidence::Strong);
    }

    #[test]
    fn shared_trace_identity_gives_direct_exact_confidence() {
        let root = ref_at(10);
        let egress = ref_at(20);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T1", Some("span-root"), None)],
            ),
            input(
                egress,
                egress_role(),
                vec![trace("otel", "T1", Some("span-eg"), None)],
            ),
        ]);
        let (scenario, confidence, evidence) = resolved_of(&graph, egress);
        assert_eq!(*scenario, scenario_id_v1(&root));
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert!(matches!(
            evidence[0].kind,
            CorrelationEvidenceKind::TraceRelationship { .. }
        ));
    }

    #[test]
    fn supplied_role_resolutions_survive_byte_for_byte() {
        let root = ref_at(10);
        let egress = ref_at(20);
        let role = InteractionRoleResolution::known(
            InteractionRole::Egress,
            vec![
                trace("w3c", "zzz", None, None),
                CorrelationEvidence::active_http(),
                task("worker-a"),
            ],
        );
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T1", Some("span-root"), None)],
            ),
            input(
                egress,
                role.clone(),
                vec![trace("otel", "T1", Some("span-eg"), None)],
            ),
        ]);
        assert_eq!(graph.role_resolution(&egress), Some(&role));
    }

    #[test]
    fn caller_supplied_scenario_root_is_rejected_in_every_input_path() {
        let operation = ref_at(10);
        let smuggled = scenario_root_item(operation);

        // 1. correlation channel
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![input(operation, ingress_role(), vec![smuggled.clone()])],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::ReservedScenarioRootInput {
                channel: CHANNEL_CORRELATION,
                ..
            }
        ));

        // 2. Known role evidence
        let known_with_smuggled =
            InteractionRoleResolution::known(InteractionRole::Ingress, vec![smuggled]);
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![input(operation, known_with_smuggled, vec![])],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::ReservedScenarioRootInput {
                channel: CHANNEL_ROLE_KNOWN,
                ..
            }
        ));

        // 3. Unknown role evidence
        let unknown_with_smuggled =
            InteractionRoleResolution::unknown(vec![scenario_root_item(operation)]);
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![input(operation, unknown_with_smuggled, vec![])],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::ReservedScenarioRootInput {
                channel: CHANNEL_ROLE_UNKNOWN,
                ..
            }
        ));

        // 4. Ambiguous role candidate evidence
        let ambiguous_with_smuggled = InteractionRoleResolution::ambiguous(vec![
            crate::InteractionRoleCandidate {
                role: InteractionRole::Ingress,
                evidence: vec![scenario_root_item(operation)],
            },
            crate::InteractionRoleCandidate {
                role: InteractionRole::Egress,
                evidence: vec![CorrelationEvidence::active_http()],
            },
        ]);
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![input(operation, ambiguous_with_smuggled, vec![])],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::ReservedScenarioRootInput {
                channel: CHANNEL_ROLE_AMBIGUOUS,
                ..
            }
        ));
    }

    #[test]
    fn duplicate_references_fail_closed() {
        let operation = ref_at(10);
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![
                input(operation, ingress_role(), vec![]),
                input(operation, egress_role(), vec![]),
            ],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::DuplicateReference { .. }
        ));
    }

    #[test]
    fn cross_recording_reference_fails_closed() {
        let foreign = CanonicalOperationRef::new(
            RecordingId::from_uuid(uuid_at(9)),
            EpochId::from_uuid(uuid_at(2)),
            SessionId::from_uuid(uuid_at(1010)),
            OperationId::from_uuid(uuid_at(10)),
        );
        let result = resolve_correlation(
            RecordingId::from_uuid(uuid_at(1)),
            vec![input(foreign, ingress_role(), vec![])],
        );
        assert!(matches!(
            result.unwrap_err(),
            CorrelationResolverError::RecordingScopeMismatch { .. }
        ));
    }

    // ---------- Task 7: root establishment and pinning ----------

    #[test]
    fn roots_sharing_one_trace_stay_independently_owned() {
        let left = ref_at(10);
        let right = ref_at(11);
        let graph = resolve(vec![
            input(
                left,
                ingress_role(),
                vec![trace("otel", "shared", Some("ls"), None)],
            ),
            input(
                right,
                ingress_role(),
                vec![trace("otel", "shared", Some("rs"), None)],
            ),
        ]);
        assert_eq!(graph.scenarios.len(), 2);
        let (left_scenario, confidence, evidence) = resolved_of(&graph, left);
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert_eq!(evidence.len(), 2);
        assert!(matches!(
            evidence[0].kind,
            CorrelationEvidenceKind::ScenarioRoot { .. }
        ));
        assert!(
            evidence
                .iter()
                .skip(1)
                .any(|item| matches!(item.kind, CorrelationEvidenceKind::TraceRelationship { .. }))
        );
        let (right_scenario, _, _) = resolved_of(&graph, right);
        assert_ne!(left_scenario, right_scenario);
        for scenario in &graph.scenarios {
            if scenario.root == left {
                assert_eq!(scenario.members, vec![left]);
            }
        }
    }

    #[test]
    fn egress_unknown_and_ambiguous_never_root_scenarios() {
        let egress = ref_at(20);
        let unknown = ref_at(21);
        let ambiguous = ref_at(22);
        let graph = resolve(vec![
            input(
                egress,
                egress_role(),
                vec![trace("otel", "T", Some("s"), None)],
            ),
            input(unknown, unknown_role(), vec![]),
            input(ambiguous, ambiguous_ingress_egress_role(), vec![]),
        ]);
        assert_eq!(graph.scenarios.len(), 0);
        assert_uncorrelated(&graph, egress);
        // Roles preserved verbatim.
        assert_eq!(graph.role_resolution(&unknown), Some(&unknown_role()));
        assert_eq!(
            graph.role_resolution(&ambiguous),
            Some(&ambiguous_ingress_egress_role())
        );
    }

    #[test]
    fn root_only_scenario_validates_against_sessions_and_context_free_validate_fails() {
        let root = ref_at(10);
        let graph = resolve(vec![input(root, ingress_role(), vec![])]);
        assert_eq!(graph.scenarios.len(), 1);
        assert_eq!(graph.scenarios[0].members, vec![root]);
        let (_, confidence, evidence) = resolved_of(&graph, root);
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert_eq!(evidence.len(), 1);
        assert!(matches!(
            evidence[0].kind,
            CorrelationEvidenceKind::ScenarioRoot { root: evidence_root } if evidence_root == root
        ));
        let session = session_for(root, &[]);
        assert!(
            graph
                .validate_against_sessions(std::slice::from_ref(&session))
                .is_ok()
        );
        assert!(matches!(
            graph.validate(),
            Err(CorrelationValidationError::MissingSessionContext { .. })
        ));
    }

    #[test]
    fn root_retains_correlation_context_after_scenario_root() {
        let root = ref_at(10);
        let role = InteractionRoleResolution::known(
            InteractionRole::Ingress,
            vec![
                CorrelationEvidence::passive_http(),
                CorrelationEvidence::custom("role", "evidence", "preserved"),
            ],
        );
        let build = |reverse: bool| {
            let mut evidence = vec![
                trace("otel", "T", Some("root-span"), None),
                lifetime(1, 2),
                CorrelationEvidence::custom("ctx", "root", "value"),
            ];
            if reverse {
                evidence.reverse();
            }
            resolve_correlation(
                RecordingId::from_uuid(uuid_at(1)),
                vec![input(root, role.clone(), evidence)],
            )
            .expect("root resolution succeeds")
        };
        let forward = build(false);
        let backward = build(true);
        assert_eq!(forward, backward);
        assert_eq!(forward.role_resolution(&root), Some(&role));
        let (scenario, confidence, evidence) = resolved_of(&forward, root);
        assert_eq!(*scenario, scenario_id_v1(&root));
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert!(matches!(
            evidence.first().map(|item| &item.kind),
            Some(CorrelationEvidenceKind::ScenarioRoot { root: evidence_root })
                if evidence_root == &root
        ));
        assert_eq!(evidence.len(), 4);
        assert!(
            evidence
                .iter()
                .skip(1)
                .any(|item| matches!(item.kind, CorrelationEvidenceKind::TraceRelationship { .. }))
        );
        assert!(
            evidence
                .iter()
                .skip(1)
                .any(|item| matches!(item.kind, CorrelationEvidenceKind::TemporalLifetime { .. }))
        );
        assert!(
            evidence
                .iter()
                .skip(1)
                .any(|item| matches!(item.kind, CorrelationEvidenceKind::Custom { .. }))
        );
    }

    // ---------- Task 8: Phase A1 closure ----------

    #[test]
    fn directional_parent_child_inherits_parent_support_transitively() {
        let parent = ref_at(10);
        let child = ref_at(11);
        let graph = resolve(vec![
            input(
                parent,
                ingress_role(),
                vec![trace("otel", "T", Some("parent-span"), None)],
            ),
            input(
                child,
                egress_role(),
                vec![trace("otel", "T", Some("child-span"), Some("parent-span"))],
            ),
        ]);
        let (scenario, confidence, evidence) = resolved_of(&graph, child);
        assert_eq!(*scenario, scenario_id_v1(&parent));
        assert_eq!(confidence, CorrelationConfidence::Strong);
        assert!(matches!(
            evidence.first().map(|item| &item.kind),
            Some(CorrelationEvidenceKind::TraceRelationship {
                parent_span_id: Some(parent_span_id),
                ..
            }) if parent_span_id == "parent-span"
        ));
        assert!(
            graph
                .causal_edges
                .iter()
                .any(|edge| edge.parent == parent && edge.child == child)
        );
    }

    #[test]
    fn reverse_parent_declaration_keeps_shared_trace_direct_and_edge_directional() {
        let root = ref_at(20);
        let declaring_child = ref_at(21);
        let declared_parent = ref_at(22);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "R", Some("root-span"), None)],
            ),
            input(
                declaring_child,
                egress_role(),
                vec![
                    trace("otel", "T", Some("child-span"), Some("parent-span")),
                    trace("otel", "R", Some("root-bridge"), None),
                ],
            ),
            input(
                declared_parent,
                egress_role(),
                vec![trace("otel", "T", Some("parent-span"), None)],
            ),
        ]);

        // The only declaration is child -> parent. It is not a relationship
        // in the reverse parent -> child direction.
        let mut declared_parents: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
            BTreeMap::new();
        declared_parents.insert(declaring_child, BTreeSet::from([declared_parent]));
        assert!(declares_parent(
            &declaring_child,
            &declared_parent,
            &declared_parents
        ));
        assert!(!declares_parent(
            &declared_parent,
            &declaring_child,
            &declared_parents
        ));

        // Reverse evaluation keeps SharedTraceIdentity available, so the
        // parent receives Exact rather than transitive-only Strong support.
        let (_, parent_confidence, parent_evidence) = resolved_of(&graph, declared_parent);
        assert_eq!(parent_confidence, CorrelationConfidence::Exact);
        assert!(matches!(
            parent_evidence.first().map(|item| &item.kind),
            Some(CorrelationEvidenceKind::TraceRelationship {
                parent_span_id: None,
                ..
            })
        ));
        assert!(
            graph
                .causal_edges
                .iter()
                .any(|edge| edge.parent == declared_parent && edge.child == declaring_child)
        );
        assert!(
            !graph
                .causal_edges
                .iter()
                .any(|edge| edge.parent == declaring_child && edge.child == declared_parent)
        );
    }

    #[test]
    fn transitive_chain_resolves_strong_with_parent_span_edges() {
        let root = ref_at(10);
        let middle = ref_at(11);
        let leaf = ref_at(12);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                middle,
                egress_role(),
                vec![trace("otel", "T", Some("ms"), Some("rs"))],
            ),
            input(
                leaf,
                egress_role(),
                vec![trace("otel", "T", Some("ls"), Some("ms"))],
            ),
        ]);
        let (scenario, confidence, _) = resolved_of(&graph, middle);
        assert_eq!(*scenario, scenario_id_v1(&root));
        // The middle declares the root as parent, but the leaf's reverse
        // declaration must not suppress SharedTraceIdentity for middle.
        // Closed support therefore retains direct support -> Exact.
        assert_eq!(confidence, CorrelationConfidence::Exact);
        // The leaf declares the middle as parent, while the root is an
        // undeclared same-trace supporter -> Exact through SharedTraceIdentity.
        let (_, leaf_confidence, _) = resolved_of(&graph, leaf);
        assert_eq!(leaf_confidence, CorrelationConfidence::Exact);
        // Stage B edges: R->M, M->L.
        assert_eq!(graph.causal_edges.len(), 2);
        assert_eq!(graph.causal_edges[0].parent, root);
        assert_eq!(graph.causal_edges[0].child, middle);
        assert_eq!(graph.causal_edges[1].parent, middle);
        assert_eq!(graph.causal_edges[1].child, leaf);
    }

    #[test]
    fn short_path_and_long_path_converge_to_ambiguity() {
        let root_a = ref_at(10);
        let root_b = ref_at(11);
        let m1 = ref_at(12); // one hop from A
        let l1 = ref_at(13); // long chain to B
        let l2 = ref_at(14);
        let target = ref_at(30);
        let graph = resolve(vec![
            input(
                root_a,
                ingress_role(),
                vec![trace("otel", "TA", Some("ra"), None)],
            ),
            input(
                root_b,
                ingress_role(),
                vec![trace("otel", "TB", Some("rb"), None)],
            ),
            input(
                m1,
                egress_role(),
                vec![trace("otel", "TA", Some("m1"), Some("ra"))],
            ),
            input(
                l1,
                egress_role(),
                vec![trace("otel", "TB", Some("l1"), Some("rb"))],
            ),
            input(
                l2,
                egress_role(),
                vec![trace("otel", "TB", Some("l2"), Some("l1"))],
            ),
            input(
                target,
                egress_role(),
                vec![
                    trace("otel", "TA", Some("z"), Some("m1")),
                    trace("otel", "TB", None, Some("l2")),
                ],
            ),
        ]);
        assert_ambiguous(
            &graph,
            target,
            &[scenario_id_v1(&root_a), scenario_id_v1(&root_b)],
        );
    }

    #[test]
    fn same_task_string_alone_resolves_nothing() {
        let left = ref_at(10);
        let right = ref_at(11);
        let worker = ref_at(30);
        let shared = vec![task("worker-1")];
        let graph = resolve(vec![
            input(left, ingress_role(), shared.clone()),
            input(right, ingress_role(), shared.clone()),
            input(worker, egress_role(), shared),
        ]);
        assert_uncorrelated(&graph, worker);
    }

    #[test]
    fn temporal_overlap_alone_selects_nothing_and_nonoverlap_eliminates_nothing() {
        let root = ref_at(10);
        let overlapping = ref_at(30);
        let disjoint_later = ref_at(31);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None), lifetime(0, 5)],
            ),
            // Overlapping lifetime only: stays uncorrelated.
            input(overlapping, egress_role(), vec![lifetime(1, 9)]),
            // Disjoint lifetime AFTER the parent completed: trace still binds.
            input(
                disjoint_later,
                egress_role(),
                vec![
                    lifetime(100, 200),
                    trace("otel", "T", Some("x"), Some("rs")),
                ],
            ),
        ]);
        assert_uncorrelated(&graph, overlapping);
        let (scenario, _, _) = resolved_of(&graph, disjoint_later);
        assert_eq!(*scenario, scenario_id_v1(&root));
    }

    #[test]
    fn concurrent_ingresses_resolve_independently() {
        let ingress_a = ref_at(10);
        let ingress_b = ref_at(11);
        let ingress_c = ref_at(12);
        let egress_x = ref_at(30);
        let egress_y = ref_at(31);
        let egress_w = ref_at(32);
        let graph = resolve(vec![
            input(
                ingress_a,
                ingress_role(),
                vec![trace("otel", "TA", Some("sa"), None)],
            ),
            input(
                ingress_b,
                ingress_role(),
                vec![trace("otel", "TB", Some("sb"), None)],
            ),
            input(
                egress_x,
                egress_role(),
                vec![trace("otel", "TA", Some("sx"), None)],
            ),
            input(
                egress_y,
                egress_role(),
                vec![trace("otel", "TB", Some("sy"), None)],
            ),
            input(
                ingress_c,
                ingress_role(),
                vec![trace("otel", "TC", Some("sc"), None)],
            ),
            input(
                egress_w,
                egress_role(),
                vec![trace("otel", "TC", Some("sw"), None)],
            ),
        ]);
        assert_eq!(resolved_of(&graph, egress_x).0, &scenario_id_v1(&ingress_a));
        assert_eq!(resolved_of(&graph, egress_y).0, &scenario_id_v1(&ingress_b));
        assert_eq!(resolved_of(&graph, egress_w).0, &scenario_id_v1(&ingress_c));
        for scenario in &graph.scenarios {
            if scenario.id == scenario_id_v1(&ingress_a) {
                assert_eq!(scenario.members.len(), 2);
            }
        }
    }

    #[test]
    fn cross_epoch_membership_resolves_through_full_references() {
        let recording_id = RecordingId::from_uuid(uuid_at(1));
        let epoch_n = EpochId::from_uuid(uuid_at(2));
        let epoch_successor = EpochId::from_uuid(uuid_at(3));
        let root_session = SessionId::from_uuid(uuid_at(1010));
        let child_session = SessionId::from_uuid(uuid_at(2010));
        let root = CanonicalOperationRef::new(
            recording_id,
            epoch_n,
            root_session,
            OperationId::from_uuid(uuid_at(10)),
        );
        let child = CanonicalOperationRef::new(
            recording_id,
            epoch_successor,
            child_session,
            OperationId::from_uuid(uuid_at(30)),
        );
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                child,
                egress_role(),
                vec![trace("otel", "T", Some("cs"), Some("rs"))],
            ),
        ]);
        assert_eq!(resolved_of(&graph, child).0, &scenario_id_v1(&root));
        assert_eq!(graph.causal_edges.len(), 1);
        assert_eq!(graph.causal_edges[0].parent, root);
        assert_eq!(graph.causal_edges[0].child, child);
        let root_session_fixture = session_for(root, &[]);
        let mut child_session_fixture = session_for(child, &[epoch_n]);
        child_session_fixture.source_provenance.epoch_id = Some(epoch_successor);
        let sessions = vec![root_session_fixture, session_for(child, &[])];
        // The fixture builder already stamps the completion-owner range; the
        // historical epoch N range is provenance only.
        let _ = child_session_fixture;
        assert!(graph.validate_against_sessions(&sessions).is_ok());
    }

    // ---------- Task 9: post-closure proof derivation ----------

    #[test]
    fn canonical_transitive_path_prefers_shortest_then_lexical_sequence() {
        let start = ref_at(10);
        let x = ref_at(11);
        let y = ref_at(12);
        let target = ref_at(30);
        let targets = BTreeSet::from([target]);

        // Insert the longer route first. The two valid paths are:
        // start -> x -> target
        // start -> y -> x -> target
        let mut longer_first: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
            BTreeMap::new();
        longer_first.entry(start).or_default().insert(y);
        longer_first.entry(y).or_default().insert(x);
        longer_first.entry(x).or_default().insert(target);
        longer_first.entry(start).or_default().insert(x);
        assert_eq!(
            canonical_transitive_path(start, &targets, &longer_first),
            Some(vec![x, target])
        );

        // Reverse construction order must produce the same shortest proof.
        let mut shorter_first: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
            BTreeMap::new();
        shorter_first.entry(start).or_default().insert(x);
        shorter_first.entry(x).or_default().insert(target);
        shorter_first.entry(start).or_default().insert(y);
        shorter_first.entry(y).or_default().insert(x);
        assert_eq!(
            canonical_transitive_path(start, &targets, &shorter_first),
            Some(vec![x, target])
        );

        let low = ref_at(40);
        let high = ref_at(41);
        let mut lexical: BTreeMap<CanonicalOperationRef, BTreeSet<CanonicalOperationRef>> =
            BTreeMap::new();
        // Both routes have two hops; low is the lexicographically smaller
        // CanonicalOperationRef sequence despite being inserted second.
        lexical.entry(start).or_default().insert(high);
        lexical.entry(high).or_default().insert(target);
        lexical.entry(start).or_default().insert(low);
        lexical.entry(low).or_default().insert(target);
        assert_eq!(
            canonical_transitive_path(start, &targets, &lexical),
            Some(vec![low, target])
        );
    }

    #[test]
    fn late_shorter_path_wins_after_support_closure() {
        let build = |reverse: bool| {
            let root = ref_at(10);
            let p2 = ref_at(11);
            let p1 = ref_at(12);
            let x1 = ref_at(13);
            let x2 = ref_at(14);
            let x3 = ref_at(15);
            let z = ref_at(16);
            let target = ref_at(30);
            let mut inputs = vec![
                // Bidirectional long route keeps its intermediate ownership
                // purely transitive under directional parent semantics:
                // target -> p1 -> p2 -> root.
                input(
                    root,
                    ingress_role(),
                    vec![
                        trace("otel", "L1", Some("r1"), Some("p2_1")),
                        trace("otel", "L4", Some("r4"), None),
                    ],
                ),
                input(
                    p2,
                    egress_role(),
                    vec![
                        trace("otel", "L1", Some("p2_1"), Some("r1")),
                        trace("otel", "L2", Some("p2_2"), Some("p1_2")),
                    ],
                ),
                input(
                    p1,
                    egress_role(),
                    vec![
                        trace("otel", "L2", Some("p1_2"), Some("p2_2")),
                        trace("otel", "L3", Some("p1_3"), Some("target_3")),
                    ],
                ),
                input(
                    target,
                    egress_role(),
                    vec![
                        trace("otel", "L3", Some("target_3"), Some("p1_3")),
                        trace("otel", "L8", None, Some("z8")),
                    ],
                ),
                // x1 -> x2 -> x3 becomes supported later; z then gains
                // DIRECT support through its shared L7 identity.
                input(
                    x1,
                    egress_role(),
                    vec![
                        trace("otel", "L4", Some("x1_4"), Some("r4")),
                        trace("otel", "L5", Some("x1_5"), None),
                    ],
                ),
                input(
                    x2,
                    egress_role(),
                    vec![
                        trace("otel", "L5", Some("x2_5"), Some("x1_5")),
                        trace("otel", "L6", Some("x2_6"), None),
                    ],
                ),
                input(
                    x3,
                    egress_role(),
                    vec![
                        trace("otel", "L6", Some("x3_6"), Some("x2_6")),
                        trace("otel", "L7", Some("x3_7"), None),
                    ],
                ),
                input(
                    z,
                    egress_role(),
                    vec![
                        trace("otel", "L7", Some("z7"), None),
                        trace("otel", "L8", Some("z8"), None),
                    ],
                ),
            ];
            if reverse {
                inputs.reverse();
            }
            resolve(inputs)
        };
        let forward = build(false);
        let backward = build(true);
        assert_eq!(forward, backward);

        let (scenario, confidence, evidence) = resolved_of(&forward, ref_at(30));
        assert_eq!(*scenario, scenario_id_v1(&ref_at(10)));
        assert_eq!(confidence, CorrelationConfidence::Strong);
        // z becomes direct only after the late x-chain closes. Final A2 proof
        // selection therefore chooses target -> z, not the earlier long route.
        assert!(matches!(
            evidence.first().map(|item| &item.kind),
            Some(CorrelationEvidenceKind::TraceRelationship {
                parent_span_id: Some(parent_span_id),
                ..
            }) if parent_span_id == "z8"
        ));
    }

    #[test]
    fn equal_length_paths_break_by_lexicographic_reference_sequence() {
        // Midpoints bridge two trace identities: they declare parenthood into
        // the root on TR and expose spans on TB for the target. The target
        // therefore has ONLY declared-parenthood inflow (no bare trace share
        // with any supporter), keeping its proof class purely transitive.
        let root = ref_at(10);
        let m_low = ref_at(11); // smaller operation uuid than m_high
        let m_high = ref_at(12);
        let target = ref_at(30);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "TR", Some("rs"), None)],
            ),
            input(
                m_low,
                egress_role(),
                vec![
                    trace("otel", "TR", Some("low_tr"), Some("rs")),
                    trace("otel", "TB", Some("low"), None),
                ],
            ),
            input(
                m_high,
                egress_role(),
                vec![
                    trace("otel", "TR", Some("high_tr"), Some("rs")),
                    trace("otel", "TB", Some("high"), None),
                ],
            ),
            input(
                target,
                egress_role(),
                vec![
                    trace("otel", "TB", None, Some("high")),
                    trace("otel", "TB", None, Some("low")),
                ],
            ),
        ]);
        let (_, confidence, evidence) = resolved_of(&graph, target);
        assert_eq!(confidence, CorrelationConfidence::Strong);
        match &evidence[0].kind {
            CorrelationEvidenceKind::TraceRelationship { parent_span_id, .. } => {
                // Lexicographically smaller candidate reference wins regardless
                // of evidence presentation order ("high" was listed first).
                assert_eq!(parent_span_id.as_deref(), Some("low"));
            }
            other => panic!("expected trace witness, got {other:?}"),
        }
    }

    #[test]
    fn cyclic_raw_relation_graph_terminates_closure_and_derivation() {
        let root = ref_at(10);
        let b = ref_at(11);
        let c = ref_at(12);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            // B claims BOTH the root and C as its parent span (raw cycle B<->C).
            input(
                b,
                egress_role(),
                vec![
                    trace("otel", "T", Some("bs"), Some("rs")),
                    trace("otel", "T", Some("bs2"), Some("cs")),
                ],
            ),
            input(
                c,
                egress_role(),
                vec![trace("otel", "T", Some("cs"), Some("bs"))],
            ),
        ]);
        // Closure terminated; both members resolved into the scenario.
        assert_eq!(resolved_of(&graph, b).0, &scenario_id_v1(&root));
        assert_eq!(resolved_of(&graph, c).0, &scenario_id_v1(&root));
    }

    #[test]
    fn diamond_rich_graph_stays_bounded_and_correct() {
        let root = ref_at(10);
        let mut inputs = vec![input(
            root,
            ingress_role(),
            vec![trace("otel", "T", Some("L0a"), None)],
        )];
        // 3-wide, 12-level diamonds create 3^12 = 531,441 distinct simple
        // routes; bounded search state must keep this instant.
        let levels = 12;
        let width = 3;
        let mut seed: u128 = 100;
        let mut previous_spans: Vec<String> = vec!["L0a".into(), "L0b".into()];
        // Seed level 0 second node hangs off the root too.
        inputs.push(input(
            ref_at(11),
            egress_role(),
            vec![trace("otel", "T", Some("L0b"), Some("L0a"))],
        ));
        for level in 1..=levels {
            let mut current_spans = Vec::new();
            for index in 0..width {
                seed += 1;
                let span = format!("L{level}_{index}");
                let mut evidence = Vec::new();
                for parent in &previous_spans {
                    evidence.push(trace("otel", "T", None, Some(parent.as_str())));
                }
                evidence.push(trace("otel", "T", Some(span.as_str()), None));
                inputs.push(input(ref_at(seed), egress_role(), evidence));
                current_spans.push(span);
            }
            previous_spans = current_spans;
        }
        let graph = resolve(inputs);
        let scenario = scenario_id_v1(&root);
        for operation_seed in 101..=seed {
            let (got, _, _) = resolved_of(&graph, ref_at(operation_seed));
            assert_eq!(*got, scenario);
        }
        assert_eq!(resolved_of(&graph, ref_at(11)).0, &scenario);
        assert_eq!(graph.scenarios.len(), 1);
    }

    // ---------- Task 13/14/15: Phase B construction and cycle safety ----------

    #[test]
    fn multi_parent_child_gets_no_edge_and_corrected_case_emits_only_b_to_c() {
        // Corrected regression: A->B, C->B, B->C where A is the scenario root.
        // B has two sufficient parents {A, C} so BOTH incoming edges vanish
        // before mapping; C keeps its single parent B; the emitted edge set is
        // exactly {B->C}; no artificial B<->C SCC forms.
        let root = ref_at(10);
        let b = ref_at(11);
        let c = ref_at(12);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("ra"), None)],
            ),
            input(
                b,
                egress_role(),
                vec![
                    trace("otel", "T", Some("bs"), Some("ra")),
                    trace("otel", "T", None, Some("cs")),
                ],
            ),
            input(
                c,
                egress_role(),
                vec![trace("otel", "T", Some("cs"), Some("bs"))],
            ),
        ]);
        assert_eq!(resolved_of(&graph, b).0, &scenario_id_v1(&root));
        assert_eq!(resolved_of(&graph, c).0, &scenario_id_v1(&root));
        assert_eq!(graph.causal_edges.len(), 1);
        assert_eq!(graph.causal_edges[0].parent, b);
        assert_eq!(graph.causal_edges[0].child, c);
    }

    #[test]
    fn two_node_cycle_loses_both_internal_edges() {
        let root = ref_at(10);
        let x = ref_at(11);
        let y = ref_at(12);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                x,
                egress_role(),
                vec![trace("otel", "T", Some("xs"), Some("ys"))],
            ),
            input(
                y,
                egress_role(),
                vec![trace("otel", "T", Some("ys"), Some("xs"))],
            ),
        ]);
        // Both remain Resolved members.
        assert_eq!(resolved_of(&graph, x).0, &scenario_id_v1(&root));
        assert_eq!(resolved_of(&graph, y).0, &scenario_id_v1(&root));
        assert!(graph.causal_edges.is_empty());
        // Direct-parent evidence stays inspectable in resolutions.
        let (_, _, evidence) = resolved_of(&graph, x);
        assert!(!evidence.is_empty());
    }

    #[test]
    fn three_node_cycle_loses_all_internal_edges() {
        let root = ref_at(10);
        let x = ref_at(11);
        let y = ref_at(12);
        let z = ref_at(13);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                x,
                egress_role(),
                vec![trace("otel", "T", Some("xs"), Some("zs"))],
            ),
            input(
                y,
                egress_role(),
                vec![trace("otel", "T", Some("ys"), Some("xs"))],
            ),
            input(
                z,
                egress_role(),
                vec![trace("otel", "T", Some("zs"), Some("ys"))],
            ),
        ]);
        for operation in [x, y, z] {
            assert_eq!(resolved_of(&graph, operation).0, &scenario_id_v1(&root));
        }
        assert!(graph.causal_edges.is_empty());
    }

    #[test]
    fn cycle_with_acyclic_outgoing_structure_keeps_outgoing_edges() {
        // B->C, C->B internal cycle plus C->E and E->F acyclic structure.
        let root = ref_at(10);
        let b = ref_at(11);
        let c = ref_at(12);
        let e = ref_at(13);
        let f = ref_at(14);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                b,
                egress_role(),
                vec![trace("otel", "T", Some("bs"), Some("cs"))],
            ),
            input(
                c,
                egress_role(),
                vec![trace("otel", "T", Some("cs"), Some("bs"))],
            ),
            input(
                e,
                egress_role(),
                vec![trace("otel", "T", Some("es"), Some("cs"))],
            ),
            input(
                f,
                egress_role(),
                vec![trace("otel", "T", Some("fs"), Some("es"))],
            ),
        ]);
        let mut got: Vec<(CanonicalOperationRef, CanonicalOperationRef)> = graph
            .causal_edges
            .iter()
            .map(|edge| (edge.parent, edge.child))
            .collect();
        got.sort();
        assert_eq!(got, vec![(c, e), (e, f)]);
        // Membership never changed.
        assert_eq!(resolved_of(&graph, b).0, &scenario_id_v1(&root));
    }

    #[test]
    fn shared_target_span_blocks_only_the_edge_not_support() {
        let root = ref_at(10);
        let x = ref_at(11);
        let y = ref_at(12);
        let child = ref_at(30);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                x,
                egress_role(),
                vec![trace("otel", "T", Some("shared"), Some("rs"))],
            ),
            input(
                y,
                egress_role(),
                vec![trace("otel", "T", Some("shared"), Some("rs"))],
            ),
            input(
                child,
                egress_role(),
                vec![trace("otel", "T", Some("z"), Some("shared"))],
            ),
        ]);
        // Support propagation worked normally through the shared-span members.
        assert_eq!(resolved_of(&graph, child).0, &scenario_id_v1(&root));
        // No selected edge targets the shared span.
        assert!(graph.causal_edges.iter().all(|edge| edge.child != child));
        assert!(
            graph
                .causal_edges
                .iter()
                .all(|edge| edge.child != x && edge.child != y || edge.parent == root)
        );
    }

    #[test]
    fn self_parent_relation_produces_no_edge_without_error() {
        let root = ref_at(10);
        let a = ref_at(11);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                a,
                egress_role(),
                vec![
                    trace("otel", "T", Some("as"), Some("rs")),
                    // Self relation: span equals its own parent span.
                    trace("otel", "T", Some("loop"), Some("loop")),
                ],
            ),
        ]);
        assert_eq!(resolved_of(&graph, a).0, &scenario_id_v1(&root));
        assert_eq!(graph.causal_edges.len(), 1);
        assert_eq!(graph.causal_edges[0].parent, root);
        assert_eq!(graph.causal_edges[0].child, a);
        // The self relation evidence remains inspectable as non-selected context.
        let (_, _, evidence) = resolved_of(&graph, a);
        assert!(evidence.iter().any(|item| matches!(
            &item.kind,
            CorrelationEvidenceKind::TraceRelationship { span_id, parent_span_id, .. }
                if span_id.as_deref() == Some("loop") && parent_span_id.as_deref() == Some("loop")
        )));
    }

    #[test]
    fn root_as_child_relation_produces_no_edge_without_mutation() {
        let root = ref_at(10);
        let member = ref_at(11);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![
                    trace("otel", "T", Some("rs"), None),
                    // Root claims member's span as its own parent: relation M->R rejected.
                    trace("otel", "T", Some("r2"), Some("ms")),
                ],
            ),
            input(
                member,
                egress_role(),
                vec![trace("otel", "T", Some("ms"), Some("rs"))],
            ),
        ]);
        let (scenario, confidence, evidence) = resolved_of(&graph, root);
        assert_eq!(*scenario, scenario_id_v1(&root));
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert!(matches!(
            evidence[0].kind,
            CorrelationEvidenceKind::ScenarioRoot { .. }
        ));
        assert_eq!(graph.scenarios[0].root, root);
        assert!(graph.causal_edges.iter().all(|edge| edge.child != root));
    }

    #[test]
    fn ambiguous_operations_never_appear_in_selected_edges() {
        let root_a = ref_at(10);
        let root_b = ref_at(11);
        let target = ref_at(30);
        let graph = resolve(vec![
            input(
                root_a,
                ingress_role(),
                vec![trace("otel", "TA", Some("sa"), None)],
            ),
            input(
                root_b,
                ingress_role(),
                vec![trace("otel", "TB", Some("sb"), None)],
            ),
            input(
                target,
                egress_role(),
                vec![
                    trace("otel", "TA", Some("z"), None),
                    trace("otel", "TB", Some("z2"), None),
                ],
            ),
        ]);
        assert_ambiguous(
            &graph,
            target,
            &[scenario_id_v1(&root_a), scenario_id_v1(&root_b)],
        );
        assert!(
            graph
                .causal_edges
                .iter()
                .all(|edge| edge.child != target && edge.parent != target)
        );
    }

    #[test]
    fn stage_b_is_independent_of_presentation_order() {
        let build = |reverse: bool| {
            let mut inputs = vec![
                input(
                    ref_at(10),
                    ingress_role(),
                    vec![trace("otel", "T", Some("rs"), None)],
                ),
                input(
                    ref_at(11),
                    egress_role(),
                    vec![trace("otel", "T", Some("bs"), Some("cs"))],
                ),
                input(
                    ref_at(12),
                    egress_role(),
                    vec![trace("otel", "T", Some("cs"), Some("bs"))],
                ),
                input(
                    ref_at(13),
                    egress_role(),
                    vec![trace("otel", "T", Some("es"), Some("cs"))],
                ),
                input(
                    ref_at(14),
                    egress_role(),
                    vec![trace("otel", "T", Some("fs"), Some("es"))],
                ),
            ];
            if reverse {
                inputs.reverse();
            }
            resolve(inputs)
        };
        let forward = build(false);
        let backward = build(true);
        let mut forward_edges: Vec<_> = forward
            .causal_edges
            .iter()
            .map(|edge| (edge.parent, edge.child))
            .collect();
        let mut backward_edges: Vec<_> = backward
            .causal_edges
            .iter()
            .map(|edge| (edge.parent, edge.child))
            .collect();
        forward_edges.sort();
        backward_edges.sort();
        assert_eq!(forward_edges, backward_edges);
        assert_eq!(forward.scenarios, backward.scenarios);
    }

    // ---------- Task 9/17: retention ----------

    #[test]
    fn exact_resolution_keeps_direct_witness_over_transitive_path() {
        let root = ref_at(10);
        let middle = ref_at(11);
        let target = ref_at(30);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "TZ", Some("rz"), None)],
            ),
            // Middle declares into the root on TZ (the transitive path).
            input(
                middle,
                egress_role(),
                vec![trace("otel", "TZ", Some("mz"), Some("rz"))],
            ),
            // A second member exposes TD and declares into the root on TZ, so
            // the target's TD share is a direct relation to a supporter that
            // is NOT its declared parenthood neighbor.
            input(
                ref_at(12),
                egress_role(),
                vec![
                    trace("otel", "TZ", Some("m2tz"), Some("rz")),
                    trace("otel", "TD", Some("m2d"), None),
                ],
            ),
            input(
                target,
                egress_role(),
                vec![
                    // Declared parenthood into middle on TZ (transitive channel)...
                    trace("otel", "TZ", Some("zz"), Some("mz")),
                    // ...and a bare SharedTraceIdentity on TD (direct).
                    trace("otel", "TD", Some("z9"), None),
                ],
            ),
        ]);
        let (_, confidence, evidence) = resolved_of(&graph, target);
        assert_eq!(confidence, CorrelationConfidence::Exact);
        match &evidence[0].kind {
            CorrelationEvidenceKind::TraceRelationship {
                trace_id,
                parent_span_id,
                ..
            } => {
                assert_eq!(trace_id, "TD");
                assert_eq!(parent_span_id, &None); // direct share, not the transitive item
            }
            other => panic!("expected direct witness, got {other:?}"),
        }
    }

    #[test]
    fn canonical_evidence_key_orders_prefixes_and_contextual_fill_permutations() {
        let a = trace("a", "trace", Some("span"), None);
        let aa = trace("aa", "trace", Some("span"), None);
        assert!(canonical_evidence_key(&a, None) < canonical_evidence_key(&aa, None));

        let otel = trace("otel", "trace", Some("span"), None);
        let otel2 = trace("otel2", "trace", Some("span"), None);
        assert!(canonical_evidence_key(&otel, None) < canonical_evidence_key(&otel2, None));

        let build = |reverse: bool| {
            let root = ref_at(10);
            let target = ref_at(30);
            let prefix_a = CorrelationEvidence::custom("ctx", "a", "value");
            let prefix_aa = CorrelationEvidence::custom("ctx", "aa", "value");
            let mut evidence = vec![
                trace("otel", "T", Some("target"), None),
                prefix_aa,
                prefix_a,
            ];
            if reverse {
                evidence.reverse();
            }
            resolve(vec![
                input(
                    root,
                    ingress_role(),
                    vec![trace("otel", "T", Some("root"), None)],
                ),
                input(target, egress_role(), evidence),
            ])
        };
        let forward = build(false);
        let backward = build(true);
        assert_eq!(forward, backward);
        let (_, _, evidence) = resolved_of(&forward, ref_at(30));
        assert!(matches!(
            &evidence[1].kind,
            CorrelationEvidenceKind::Custom { key, .. } if key == "a"
        ));
        assert!(matches!(
            &evidence[2].kind,
            CorrelationEvidenceKind::Custom { key, .. } if key == "aa"
        ));
    }

    #[test]
    fn strong_short_proof_uses_only_remaining_retention_capacity() {
        let root = ref_at(10);
        let target = ref_at(30);
        let mut evidence = vec![trace("otel", "T", Some("target"), Some("root"))];
        evidence.extend(
            (0..80).map(|index| CorrelationEvidence::custom("ctx", format!("k{index:03}"), "v")),
        );
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("root"), None)],
            ),
            input(target, egress_role(), evidence),
        ]);
        let (_, confidence, evidence) = resolved_of(&graph, target);
        assert_eq!(confidence, CorrelationConfidence::Strong);
        assert_eq!(evidence.len(), CORRELATION_RETENTION_TARGET);
        assert_eq!(
            evidence
                .iter()
                .filter(|item| matches!(item.kind, CorrelationEvidenceKind::Custom { .. }))
                .count(),
            CORRELATION_RETENTION_TARGET - 1
        );
        assert!(matches!(
            evidence.first().map(|item| &item.kind),
            Some(CorrelationEvidenceKind::TraceRelationship {
                parent_span_id: Some(parent_span_id),
                ..
            }) if parent_span_id == "root"
        ));
    }

    #[test]
    fn cap_pressure_changes_no_semantics_and_fill_is_permutation_stable() {
        let build = |reverse: bool| {
            let root = ref_at(10);
            let target = ref_at(30);
            let mut contextual: Vec<CorrelationEvidence> = (0..80u32)
                .map(|index| CorrelationEvidence::custom("ctx", format!("k{index:03}"), "v"))
                .collect();
            if reverse {
                contextual.reverse();
            }
            let inputs = vec![
                input(
                    root,
                    ingress_role(),
                    vec![trace("otel", "T", Some("rs"), None)],
                ),
                input(target, egress_role(), {
                    let mut items = vec![trace("otel", "T", Some("z"), None)];
                    items.extend(contextual);
                    items
                }),
            ];
            resolve(inputs)
        };
        let forward = build(false);
        let backward = build(true);
        let (_, confidence_a, evidence_a) = resolved_of(&forward, ref_at(30));
        let (_, confidence_b, evidence_b) = resolved_of(&backward, ref_at(30));
        assert_eq!(confidence_a, CorrelationConfidence::Exact);
        assert_eq!(confidence_b, confidence_a);
        assert_eq!(
            evidence_a, evidence_b,
            "retained output must be permutation-stable"
        );
        assert_eq!(evidence_a.len(), CORRELATION_RETENTION_TARGET);
        assert_eq!(
            evidence_a
                .iter()
                .filter(|item| matches!(item.kind, CorrelationEvidenceKind::Custom { .. }))
                .count(),
            CORRELATION_RETENTION_TARGET - 1
        );
        // The semantic witness survived the cap.
        assert!(evidence_a.iter().any(|item| matches!(
            &item.kind,
            CorrelationEvidenceKind::TraceRelationship { span_id, .. } if span_id.as_deref() == Some("z")
        )));
    }

    #[test]
    fn stage_b_reads_full_input_beyond_retained_output_target() {
        let root = ref_at(10);
        let parent = ref_at(11);
        let child = ref_at(30);
        let mut child_evidence = vec![trace("otel", "direct", Some("child-direct"), None)];
        child_evidence.extend(
            (0..80).map(|index| CorrelationEvidence::custom("ctx", format!("k{index:03}"), "v")),
        );
        // This relation sorts after the contextual custom items and is omitted
        // from child's retained output, but Stage B must still consume it.
        child_evidence.push(trace(
            "otel",
            "edge",
            Some("child-edge"),
            Some("parent-edge"),
        ));
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![
                    trace("otel", "direct", Some("root-direct"), None),
                    trace("otel", "parent", Some("root-parent"), None),
                ],
            ),
            input(
                parent,
                egress_role(),
                vec![
                    trace("otel", "parent", Some("parent-root"), Some("root-parent")),
                    trace("otel", "edge", Some("parent-edge"), None),
                ],
            ),
            input(child, egress_role(), child_evidence),
        ]);
        assert!(
            graph
                .causal_edges
                .iter()
                .any(|edge| edge.parent == parent && edge.child == child)
        );
        let (_, confidence, evidence) = resolved_of(&graph, child);
        assert_eq!(confidence, CorrelationConfidence::Exact);
        assert_eq!(evidence.len(), CORRELATION_RETENTION_TARGET);
        assert!(!evidence.iter().any(|item| matches!(
            &item.kind,
            CorrelationEvidenceKind::TraceRelationship {
                parent_span_id: Some(parent_span_id),
                ..
            } if parent_span_id == "parent-edge"
        )));
    }

    #[test]
    fn semantic_transitive_witness_over_64_hops_is_not_truncated() {
        const HOPS: usize = 70;
        let build = |reverse: bool| {
            let root = ref_at(10);
            let mut inputs = vec![input(
                root,
                ingress_role(),
                vec![trace("otel", "chain-0", Some("span-0"), Some("span-1"))],
            )];
            for index in 1..=HOPS {
                let mut evidence = Vec::new();
                let link = index - 1;
                let trace_id = format!("chain-{link}");
                let span = format!("span-{index}");
                let parent_span = format!("span-{link}");
                evidence.push(trace("otel", &trace_id, Some(&span), Some(&parent_span)));
                if index < HOPS {
                    let next_trace_id = format!("chain-{index}");
                    let next_parent = format!("span-{}", index + 1);
                    evidence.push(trace(
                        "otel",
                        &next_trace_id,
                        Some(&span),
                        Some(&next_parent),
                    ));
                } else {
                    evidence.push(task("optional-context"));
                }
                inputs.push(input(ref_at(10 + index as u128), egress_role(), evidence));
            }
            if reverse {
                inputs.reverse();
            }
            resolve(inputs)
        };
        let forward = build(false);
        let backward = build(true);
        assert_eq!(forward, backward);

        let target = ref_at(10 + HOPS as u128);
        let (scenario, confidence, evidence) = resolved_of(&forward, target);
        assert_eq!(*scenario, scenario_id_v1(&ref_at(10)));
        assert_eq!(confidence, CorrelationConfidence::Strong);
        assert_eq!(evidence.len(), HOPS);
        assert!(evidence.len() > CORRELATION_RETENTION_TARGET);
        assert!(evidence.iter().all(|item| {
            !matches!(
                item.kind,
                CorrelationEvidenceKind::ExecutionTaskLineage { .. }
            )
        }));
    }

    #[test]
    fn uncorrelated_retention_has_no_manufactured_witness() {
        let worker = ref_at(30);
        let graph = resolve(vec![input(
            worker,
            egress_role(),
            vec![task("w"), lifetime(1, 2)],
        )]);
        match graph.resolution(&worker) {
            Some(CorrelationResolution::Uncorrelated { evidence }) => {
                assert_eq!(evidence.len(), 2);
                assert!(evidence.iter().all(|item| !matches!(
                    item.kind,
                    CorrelationEvidenceKind::ScenarioRoot { .. }
                )));
            }
            other => panic!("expected uncorrelated, got {other:?}"),
        }
    }

    // ---------- Task 15/19: validator contract ----------

    #[test]
    fn witness_ties_break_by_provenance_then_observation_deterministically() {
        // Two equivalent direct witnesses differing ONLY in provenance.source
        // must retain the canonically smaller one under every permutation.
        let build = |flip: bool| {
            let root = ref_at(10);
            let target = ref_at(30);
            let mut item_a = trace("otel", "T", Some("z"), None);
            item_a.provenance.source = "alpha".into();
            item_a.provenance.observation = Some("later".into());
            let mut item_b = trace("otel", "T", Some("z"), None);
            item_b.provenance.source = "beta".into();
            item_b.provenance.observation = Some("early".into());
            let mut items = vec![item_b.clone(), item_a.clone()];
            if flip {
                items.reverse();
            }
            resolve(vec![
                input(
                    root,
                    ingress_role(),
                    vec![trace("otel", "T", Some("rs"), None)],
                ),
                input(target, egress_role(), items),
            ])
        };
        let forward = build(false);
        let backward = build(true);
        assert_eq!(
            forward.resolutions[&ref_at(30)],
            backward.resolutions[&ref_at(30)]
        );
        match &forward.resolutions[&ref_at(30)] {
            CorrelationResolution::Resolved { evidence, .. } => {
                // The canonical DIRECT witness leads; retention may still fill
                // remaining capacity with the unused input item.
                assert_eq!(evidence[0].provenance.source, "alpha");
                assert_eq!(evidence[0].provenance.observation.as_deref(), Some("later"));
                assert!(evidence.len() <= CORRELATION_RETENTION_TARGET);
            }
            other => panic!("expected resolved, got {other:?}"),
        }
    }

    #[test]
    fn resolver_output_passes_validate_against_sessions_for_full_chains() {
        let root = ref_at(10);
        let middle = ref_at(11);
        let leaf = ref_at(12);
        let graph = resolve(vec![
            input(
                root,
                ingress_role(),
                vec![trace("otel", "T", Some("rs"), None)],
            ),
            input(
                middle,
                egress_role(),
                vec![trace("otel", "T", Some("ms"), Some("rs"))],
            ),
            input(
                leaf,
                egress_role(),
                vec![trace("otel", "T", Some("ls"), Some("ms"))],
            ),
        ]);
        let sessions = vec![
            session_for(root, &[]),
            session_for(middle, &[]),
            session_for(leaf, &[]),
        ];
        assert!(graph.validate_against_sessions(&sessions).is_ok());
    }
}
