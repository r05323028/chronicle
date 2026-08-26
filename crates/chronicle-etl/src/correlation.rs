//! Explicit correlation composition boundary (`chronicle-etl`).
//!
//! Joins published canonical sessions with a caller-supplied provider-neutral
//! `CorrelationContext` and invokes the resolver. This module owns reference
//! verification and join ordering ONLY — zero scenario-selection semantics.
//! It is explicitly invoked on demand; the default publication/checkpoint path
//! never calls it.

use crate::native::{
    BoundNativeExecutionHandoffFact, NativeBindingDiagnostic, NativeBoundaryIndex,
    NativeExecutionHandoffObservation, bind_native_execution_observations,
    native_evidence_by_operation,
};
use chronicle_canonical::{
    CanonicalOperationRef, CanonicalSession, CorrelationContext, CorrelationEvidence,
    CorrelationEvidenceKind, CorrelationGraph, CorrelationInput, CorrelationResolverError,
    InteractionRoleResolution, OperationCorrelationView, resolve_correlation,
};
use chronicle_common::{OperationId, RecordingId, SessionId};
use std::collections::BTreeMap;
use thiserror::Error;

/// Typed, fail-closed composition errors. Evidentiary outcomes (ambiguity,
/// missing relationships) remain normal resolver results, never errors.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CorrelationCompositionError {
    #[error("no canonical sessions supplied")]
    EmptySessionSet,
    #[error("session {session_id} has no recording scope")]
    MissingRecordingScope { session_id: SessionId },
    #[error("validation context contains duplicate canonical session {session_id}")]
    DuplicateSessionContext { session_id: SessionId },
    #[error("sessions span recordings {found}, expected {expected}")]
    ConflictingRecordingScope {
        expected: RecordingId,
        found: RecordingId,
    },
    #[error("operation {operation_id} in session {session_id} has no completion owner epoch")]
    MissingCompletionOwnerEpoch {
        session_id: SessionId,
        operation_id: OperationId,
    },
    #[error("reference verification failed for {reference:?}: {message}")]
    ReferenceVerification {
        reference: Box<CanonicalOperationRef>,
        message: String,
    },
    #[error("admitted operation {reference:?} has no role-resolution context entry")]
    MissingRoleResolution {
        reference: Box<CanonicalOperationRef>,
    },
    #[error("context entry {reference:?} references no verified session operation")]
    OrphanContextEntry {
        reference: Box<CanonicalOperationRef>,
    },
    #[error(
        "caller-supplied ScenarioRoot evidence is reserved for the resolver ({channel}) at {reference:?}"
    )]
    ReservedScenarioRootInput {
        reference: Box<CanonicalOperationRef>,
        channel: &'static str,
    },
    #[error("resolver failed: {0}")]
    Resolver(#[from] CorrelationResolverError),
}

const CHANNEL_CORRELATION: &str = "correlation-evidence";
const CHANNEL_ROLE_KNOWN: &str = "role-known-evidence";
const CHANNEL_ROLE_UNKNOWN: &str = "role-unknown-evidence";
const CHANNEL_ROLE_AMBIGUOUS: &str = "role-ambiguous-candidate-evidence";

/// Caller-supplied `ScenarioRoot` items are invalid anywhere in nested role
/// evidence of the supplied context. Scanning for the forbidden value does not
/// convert role evidence into correlation input.
fn reject_reserved_scenario_root_in_role(
    reference: &CanonicalOperationRef,
    role: &InteractionRoleResolution,
) -> Result<(), CorrelationCompositionError> {
    let is_root = |item: &CorrelationEvidence| {
        matches!(item.kind, CorrelationEvidenceKind::ScenarioRoot { .. })
    };
    let reserved = |channel| CorrelationCompositionError::ReservedScenarioRootInput {
        reference: Box::new(*reference),
        channel,
    };
    match role {
        InteractionRoleResolution::Known { evidence, .. } if evidence.iter().any(is_root) => {
            Err(reserved(CHANNEL_ROLE_KNOWN))
        }
        InteractionRoleResolution::Unknown { evidence } if evidence.iter().any(is_root) => {
            Err(reserved(CHANNEL_ROLE_UNKNOWN))
        }
        InteractionRoleResolution::Ambiguous { candidates } => {
            if candidates.iter().any(|c| c.evidence.iter().any(is_root)) {
                Err(reserved(CHANNEL_ROLE_AMBIGUOUS))
            } else {
                Ok(())
            }
        }
        _ => Ok(()),
    }
}

/// Join published canonical sessions with the supplied correlation context and
/// invoke the deterministic resolver.
///
/// Join rules (fail closed):
/// - every session must carry one shared recording scope;
/// - each admitted session operation must resolve against its own session
///   lineage (full scoped references, completion-owner verification);
/// - an admitted operation with no context role resolution is an error;
/// - a missing context EVIDENCE entry joins as empty evidence (normal);
/// - a context entry referencing no verified session operation is an error;
/// - caller-supplied `ScenarioRoot` items anywhere in either channel of the
///   supplied context are errors (the resolver re-checks its inputs too).
pub fn compose_correlation(
    sessions: &[CanonicalSession],
    context: &CorrelationContext,
) -> Result<CorrelationGraph, CorrelationCompositionError> {
    let Some(first) = sessions.first() else {
        return Err(CorrelationCompositionError::EmptySessionSet);
    };

    // ---- Session identity uniqueness and recording scope agreement.
    let mut seen_sessions: std::collections::BTreeSet<SessionId> = Default::default();
    let mut recording_id: Option<RecordingId> = None;
    for session in sessions {
        if !seen_sessions.insert(session.id) {
            return Err(CorrelationCompositionError::DuplicateSessionContext {
                session_id: session.id,
            });
        }
        match session.source_provenance.recording_id {
            Some(found) => match recording_id {
                None => recording_id = Some(found),
                Some(expected) if expected != found => {
                    return Err(CorrelationCompositionError::ConflictingRecordingScope {
                        expected,
                        found,
                    });
                }
                Some(_) => {}
            },
            None => {
                return Err(CorrelationCompositionError::MissingRecordingScope {
                    session_id: session.id,
                });
            }
        }
    }
    let recording_id = recording_id.ok_or(CorrelationCompositionError::MissingRecordingScope {
        session_id: first.id,
    })?;

    // ---- Verify every admitted operation under its own session lineage.
    let mut verified: BTreeMap<CanonicalOperationRef, OperationCorrelationView> = BTreeMap::new();
    for session in sessions {
        for connection in &session.connections {
            for operation in &connection.operations {
                let Some(owner_epoch) = operation.provenance.completion_owner_epoch else {
                    return Err(CorrelationCompositionError::MissingCompletionOwnerEpoch {
                        session_id: session.id,
                        operation_id: operation.id,
                    });
                };
                let reference =
                    CanonicalOperationRef::new(recording_id, owner_epoch, session.id, operation.id);
                reference.resolve_in_session(session).map_err(|error| {
                    CorrelationCompositionError::ReferenceVerification {
                        reference: Box::new(reference),
                        message: error.to_string(),
                    }
                })?;
                verified.insert(
                    reference,
                    OperationCorrelationView {
                        started_at_offset: operation.started_at_offset,
                        completed_at_offset: operation.completed_at_offset,
                    },
                );
            }
        }
    }

    // ---- Orphan context entries fail closed.
    for reference in context
        .role_resolutions
        .keys()
        .chain(context.evidence.keys())
    {
        if !verified.contains_key(reference) {
            return Err(CorrelationCompositionError::OrphanContextEntry {
                reference: Box::new(*reference),
            });
        }
    }

    // ---- Join by exact full reference; missing evidence entries are empty.
    let mut inputs = Vec::with_capacity(verified.len());
    for (reference, operation_view) in &verified {
        let role = context.role_resolutions.get(reference).ok_or_else(|| {
            CorrelationCompositionError::MissingRoleResolution {
                reference: Box::new(*reference),
            }
        })?;
        reject_reserved_scenario_root_in_role(reference, role)?;
        let evidence = context.evidence.get(reference).cloned().unwrap_or_default();
        if evidence
            .iter()
            .any(|item| matches!(item.kind, CorrelationEvidenceKind::ScenarioRoot { .. }))
        {
            return Err(CorrelationCompositionError::ReservedScenarioRootInput {
                reference: Box::new(*reference),
                channel: CHANNEL_CORRELATION,
            });
        }
        inputs.push(CorrelationInput::new(
            *reference,
            *operation_view,
            role.clone(),
            evidence,
        ));
    }

    resolve_correlation(recording_id, inputs).map_err(CorrelationCompositionError::Resolver)
}

#[derive(Clone, Debug)]
pub struct NativeCorrelationComposition {
    pub graph: CorrelationGraph,
    pub facts: Vec<BoundNativeExecutionHandoffFact>,
    pub diagnostics: Vec<NativeBindingDiagnostic>,
}

/// Bind complete pre-canonical native observations, add only child-side native
/// correlation evidence, and invoke the existing deterministic resolver.
/// Binding ambiguity remains a diagnostic and never becomes resolver ambiguity.
pub fn compose_correlation_with_native(
    sessions: &[CanonicalSession],
    context: &CorrelationContext,
    boundary_index: &NativeBoundaryIndex,
    observations: &[NativeExecutionHandoffObservation],
) -> Result<NativeCorrelationComposition, CorrelationCompositionError> {
    let binding = bind_native_execution_observations(sessions, boundary_index, observations);
    let graph = compose_correlation_with_native_facts(sessions, context, &binding.facts)?;
    Ok(NativeCorrelationComposition {
        graph,
        facts: binding.facts,
        diagnostics: binding.diagnostics,
    })
}

/// Composition-only seam for already-bound facts. Native production callers
/// must enter through `compose_correlation_with_native` and its exact binder.
pub(crate) fn compose_correlation_with_native_facts(
    sessions: &[CanonicalSession],
    context: &CorrelationContext,
    facts: &[BoundNativeExecutionHandoffFact],
) -> Result<CorrelationGraph, CorrelationCompositionError> {
    let mut native_context = context.clone();
    for (reference, evidence) in native_evidence_by_operation(facts) {
        native_context
            .evidence
            .entry(reference)
            .or_default()
            .extend(evidence);
    }
    compose_correlation(sessions, &native_context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_canonical::{CorrelationEvidence, InteractionRole};

    fn ref_at(seed: u128) -> CanonicalOperationRef {
        CanonicalOperationRef::new(
            RecordingId::from_uuid(uuid::Uuid::from_u128(1)),
            chronicle_common::EpochId::from_uuid(uuid::Uuid::from_u128(2)),
            SessionId::from_uuid(uuid::Uuid::from_u128(1000 + seed)),
            OperationId::from_uuid(uuid::Uuid::from_u128(seed)),
        )
    }

    fn session_for(reference: CanonicalOperationRef) -> CanonicalSession {
        use chronicle_canonical as cc;
        use time::OffsetDateTime;
        let operation = cc::CanonicalOperation {
            id: reference.operation_id,
            sequence: 1,
            started_at_offset: cc::RelativeTimeNanos(0),
            completed_at_offset: Some(cc::RelativeTimeNanos(1)),
            kind: cc::OperationKind::Request,
            effect: cc::OperationEffect::Read,
            request: cc::PayloadRef::Missing {
                reason: "test".into(),
            },
            recorded_response: None,
            attributes: BTreeMap::new(),
            protocol_data: cc::ProtocolData {
                schema_version: cc::PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: None,
                bytes: Vec::new(),
            },
            provenance: cc::OperationProvenance {
                completion_owner_epoch: Some(reference.owner_epoch_id),
                epoch_ranges: vec![cc::OperationEpochRange {
                    parent_id: Some(reference.recording_id),
                    epoch_id: reference.owner_epoch_id,
                    epoch_ordinal: None,
                    wal_sequence_range: None,
                }],
                ..cc::OperationProvenance::default()
            },
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        let connection_id = chronicle_common::ConnectionId::new();
        let session = CanonicalSession {
            schema_version: cc::CANONICAL_SCHEMA_VERSION,
            id: reference.session_id,
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: Some(OffsetDateTime::UNIX_EPOCH),
            source: cc::SourceMetadata::default(),
            source_provenance: cc::SourceProvenance {
                recording_id: Some(reference.recording_id),
                epoch_id: Some(reference.owner_epoch_id),
                ..cc::SourceProvenance::default()
            },
            connections: vec![cc::CanonicalConnection {
                id: connection_id,
                protocol: chronicle_common::ProtocolId::new("test"),
                client: chronicle_common::Endpoint::new("client", 1),
                server: chronicle_common::Endpoint::new("server", 2),
                attributes: BTreeMap::new(),
                operations: vec![operation],
            }],
            connection_completeness: BTreeMap::from([(connection_id, cc::Completeness::Complete)]),
            operation_completeness: BTreeMap::from([(
                reference.operation_id,
                cc::Completeness::Complete,
            )]),
            timeline: vec![cc::TimelineEntry {
                connection_id,
                operation_id: reference.operation_id,
                offset: cc::RelativeTimeNanos(0),
            }],
            replay: Default::default(),
            replay_attributes: BTreeMap::new(),
        };
        session.validate().unwrap();
        session
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

    fn trace_item(
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

    #[test]
    fn composition_matches_direct_resolver_invocation() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![session_for(root), session_for(member)];
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(root, ingress_role()), (member, egress_role())]),
            evidence: BTreeMap::from([
                (root, vec![trace_item("otel", "T", Some("rs"), None)]),
                (
                    member,
                    vec![trace_item("otel", "T", Some("ms"), Some("rs"))],
                ),
            ]),
        };
        let composed = compose_correlation(&sessions, &context).unwrap();
        let direct_inputs = vec![
            CorrelationInput::new(
                root,
                OperationCorrelationView {
                    started_at_offset: chronicle_canonical::RelativeTimeNanos(0),
                    completed_at_offset: Some(chronicle_canonical::RelativeTimeNanos(1)),
                },
                ingress_role(),
                vec![trace_item("otel", "T", Some("rs"), None)],
            ),
            CorrelationInput::new(
                member,
                OperationCorrelationView {
                    started_at_offset: chronicle_canonical::RelativeTimeNanos(0),
                    completed_at_offset: Some(chronicle_canonical::RelativeTimeNanos(1)),
                },
                egress_role(),
                vec![trace_item("otel", "T", Some("ms"), Some("rs"))],
            ),
        ];
        let direct = resolve_correlation(ref_at_recording(), direct_inputs).unwrap();
        assert_eq!(composed, direct);
        assert_eq!(composed.scenarios.len(), 1);
        assert_eq!(composed.causal_edges.len(), 1);
    }

    fn ref_at_recording() -> RecordingId {
        RecordingId::from_uuid(uuid::Uuid::from_u128(1))
    }

    #[test]
    fn missing_role_context_fails_closed() {
        let root = ref_at(10);
        let sessions = vec![session_for(root)];
        let error = compose_correlation(&sessions, &CorrelationContext::default()).unwrap_err();
        assert!(matches!(
            error,
            CorrelationCompositionError::MissingRoleResolution { .. }
        ));
    }

    #[test]
    fn missing_evidence_entry_joins_as_empty() {
        let root = ref_at(10);
        let sessions = vec![session_for(root)];
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(root, ingress_role())]),
            evidence: BTreeMap::new(),
        };
        let graph = compose_correlation(&sessions, &context).unwrap();
        // Root-only scenario is a normal outcome, not an error.
        assert_eq!(graph.scenarios.len(), 1);
    }

    #[test]
    fn orphan_context_entry_fails_closed() {
        let root = ref_at(10);
        let orphan = ref_at(99);
        let sessions = vec![session_for(root)];
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(orphan, ingress_role())]),
            evidence: BTreeMap::new(),
        };
        let error = compose_correlation(&sessions, &context).unwrap_err();
        assert!(matches!(
            error,
            CorrelationCompositionError::OrphanContextEntry { .. }
        ));
    }

    #[test]
    fn caller_scenario_root_rejected_in_both_channels_of_context() {
        let root = ref_at(10);
        let sessions = vec![session_for(root)];
        let smuggled = CorrelationEvidence::new(CorrelationEvidenceKind::ScenarioRoot { root });

        // Correlation channel.
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(root, ingress_role())]),
            evidence: BTreeMap::from([(root, vec![smuggled.clone()])]),
        };
        let error = compose_correlation(&sessions, &context).unwrap_err();
        assert!(matches!(
            error,
            CorrelationCompositionError::ReservedScenarioRootInput {
                channel: CHANNEL_CORRELATION,
                ..
            }
        ));

        // Nested role candidate evidence.
        let role_with_smuggled = InteractionRoleResolution::ambiguous(vec![
            chronicle_canonical::InteractionRoleCandidate {
                role: InteractionRole::Ingress,
                evidence: vec![CorrelationEvidence::passive_http(), smuggled],
            },
            chronicle_canonical::InteractionRoleCandidate {
                role: InteractionRole::Egress,
                evidence: vec![CorrelationEvidence::active_http()],
            },
        ]);
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(root, role_with_smuggled)]),
            evidence: BTreeMap::new(),
        };
        let error = compose_correlation(&sessions, &context).unwrap_err();
        assert!(matches!(
            error,
            CorrelationCompositionError::ReservedScenarioRootInput {
                channel: CHANNEL_ROLE_AMBIGUOUS,
                ..
            }
        ));
    }

    fn native_range(sequence: u64) -> chronicle_canonical::WalByteRange {
        chronicle_canonical::WalByteRange {
            segment_ordinal: 0,
            segment_first_sequence: 0,
            frame_byte_offset: sequence,
            wal_sequence: sequence,
            direction: chronicle_common::Direction::ClientToServer,
            payload_byte_offset: 0,
            payload_byte_length: 1,
            gap_before: false,
        }
    }

    fn native_session(mut session: CanonicalSession, sequence: u64) -> CanonicalSession {
        let operation = &mut session.connections[0].operations[0];
        operation.provenance.connection_generation =
            Some(chronicle_canonical::SourceConnectionGeneration::Fixture);
        operation.provenance.wal_ranges = vec![native_range(sequence)];
        for epoch_range in &mut operation.provenance.epoch_ranges {
            epoch_range.wal_sequence_range = Some((sequence, sequence));
        }
        session
    }

    fn native_anchor(
        reference: CanonicalOperationRef,
        sequence: u64,
        generation: crate::NativeExecutionContextGeneration,
    ) -> crate::NativeOperationAnchor {
        crate::NativeOperationAnchor::new(
            crate::NativeOperationBoundaryReceipt {
                recording_id: reference.recording_id,
                source_epoch_id: reference.owner_epoch_id,
                source_generation: chronicle_canonical::SourceConnectionGeneration::Fixture,
                protocol_id: chronicle_common::ProtocolId::new("test"),
                direction: chronicle_common::Direction::ClientToServer,
                protocol_operation_boundary:
                    chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                        chronicle_common::ProtocolId::new("test"),
                        format!("test-boundary:{sequence}"),
                        chronicle_common::Direction::ClientToServer,
                    )
                    .unwrap(),
                source_range: native_range(sequence),
            },
            generation,
        )
        .unwrap()
    }

    #[test]
    fn native_observation_binds_and_resolves_through_existing_resolver() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(root), 10),
            native_session(session_for(member), 30),
        ];
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(70));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(71));
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(root, 10, parent_generation),
            native_anchor(member, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "test-native-http".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(root, 10), (member, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let context = CorrelationContext {
            role_resolutions: BTreeMap::from([(root, ingress_role()), (member, egress_role())]),
            evidence: BTreeMap::new(),
        };
        // Composition proof only: production E2E enters through observations.
        let binding = bind_native_execution_observations(
            &sessions,
            &boundaries,
            std::slice::from_ref(&observation),
        );
        let composed_from_bound_fact =
            compose_correlation_with_native_facts(&sessions, &context, &binding.facts).unwrap();
        let composed = compose_correlation_with_native(
            &sessions,
            &context,
            &boundaries,
            &[observation.clone(), observation],
        )
        .unwrap();
        assert_eq!(composed_from_bound_fact, composed.graph);
        assert_eq!(composed.facts.len(), 1);
        assert!(composed.diagnostics.is_empty());
        assert!(matches!(
            composed.graph.resolution(&member),
            Some(chronicle_canonical::CorrelationResolution::Resolved {
                scenario,
                confidence: chronicle_canonical::CorrelationConfidence::Strong,
                ..
            }) if *scenario == chronicle_canonical::scenario_id_v1(&root)
        ));
        assert_eq!(composed.graph.causal_edges.len(), 1);
        assert_eq!(composed.graph.causal_edges[0].parent, root);
        assert_eq!(composed.graph.causal_edges[0].child, member);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // Keep acceptance matrix and causal assertions together.
    fn overlapping_abc_handoffs_remain_context_isolated() {
        let a = ref_at(10);
        let a1 = ref_at(11);
        let a2 = ref_at(12);
        let a3 = ref_at(13);
        let b = ref_at(20);
        let b1 = ref_at(21);
        let b2 = ref_at(22);
        let c = ref_at(30);
        let c1 = ref_at(31);
        let entries = [
            (a, 10),
            (a1, 11),
            (a2, 12),
            (a3, 13),
            (b, 20),
            (b1, 21),
            (b2, 22),
            (c, 30),
            (c1, 31),
        ];
        let sessions = entries
            .iter()
            .map(|(reference, sequence)| native_session(session_for(*reference), *sequence))
            .collect::<Vec<_>>();
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in entries {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }

        let mut source = crate::NativeExecutionContextCarrier::new(
            crate::NativeSourceLimits::new(32, 32, 32).unwrap(),
        );
        let a_context = source.start_root().unwrap();
        let b_context = source.start_root().unwrap();
        let c_context = source.start_root().unwrap();
        let children = [
            (source.continue_from(a_context).unwrap(), a1, 11),
            (source.continue_from(a_context).unwrap(), a2, 12),
            (source.continue_from(a_context).unwrap(), a3, 13),
            (source.continue_from(b_context).unwrap(), b1, 21),
            (source.continue_from(b_context).unwrap(), b2, 22),
            (source.continue_from(c_context).unwrap(), c1, 31),
        ];
        let mut observations = Vec::new();
        for (context, reference, sequence) in children {
            observations.extend(
                source
                    .record_child_receipt(
                        context,
                        native_anchor(reference, sequence, context.generation()).boundary,
                    )
                    .unwrap(),
            );
        }
        for (context, reference, sequence) in
            [(a_context, a, 10), (b_context, b, 20), (c_context, c, 30)]
        {
            observations.extend(
                source
                    .record_parent_receipt(
                        context,
                        native_anchor(reference, sequence, context.generation()).boundary,
                    )
                    .unwrap(),
            );
        }
        assert_eq!(observations.len(), 6);

        let mut roles = BTreeMap::new();
        for root in [a, b, c] {
            roles.insert(root, ingress_role());
        }
        for child in [a1, a2, a3, b1, b2, c1] {
            roles.insert(child, egress_role());
        }
        let composed = compose_correlation_with_native(
            &sessions,
            &CorrelationContext {
                role_resolutions: roles,
                evidence: BTreeMap::new(),
            },
            &boundaries,
            &observations,
        )
        .unwrap();
        assert!(composed.diagnostics.is_empty());
        assert_eq!(composed.facts.len(), 6);
        for (root, children) in [
            (a, [a1, a2, a3].as_slice()),
            (b, [b1, b2].as_slice()),
            (c, [c1].as_slice()),
        ] {
            let scenario = chronicle_canonical::scenario_id_v1(&root);
            for child in children {
                assert!(matches!(
                    composed.graph.resolution(child),
                    Some(chronicle_canonical::CorrelationResolution::Resolved {
                        scenario: found,
                        ..
                    }) if *found == scenario
                ));
            }
        }
        assert_eq!(composed.graph.causal_edges.len(), 6);
        let reversed: Vec<_> = observations.iter().rev().cloned().collect();
        let reordered = compose_correlation_with_native(
            &sessions,
            &CorrelationContext {
                role_resolutions: {
                    let mut roles = BTreeMap::new();
                    for root in [a, b, c] {
                        roles.insert(root, ingress_role());
                    }
                    for child in [a1, a2, a3, b1, b2, c1] {
                        roles.insert(child, egress_role());
                    }
                    roles
                },
                evidence: BTreeMap::new(),
            },
            &boundaries,
            &reversed,
        )
        .unwrap();
        assert_eq!(composed.graph, reordered.graph);
        assert_eq!(composed.facts, reordered.facts);
    }

    #[test]
    fn exact_binder_zero_match_emits_unresolved_without_fact() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(root), 10),
            native_session(session_for(member), 30),
        ];
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(
                root,
                99,
                crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(80)),
            ),
            native_anchor(
                member,
                98,
                crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(81)),
            ),
            crate::NativeObservationProvenance {
                source: "zero-match".into(),
                parent_context_generation: crate::NativeExecutionContextGeneration::from_uuid(
                    uuid::Uuid::from_u128(80),
                ),
                child_context_generation: crate::NativeExecutionContextGeneration::from_uuid(
                    uuid::Uuid::from_u128(81),
                ),
            },
        )
        .unwrap();
        let output = crate::bind_native_execution_observations(
            &sessions,
            &crate::NativeBoundaryIndex::new(),
            &[observation],
        );
        assert!(output.facts.is_empty());
        assert!(output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        )));
    }

    #[test]
    fn cross_epoch_source_and_completion_ownership_bind_exactly() {
        let parent = ref_at(10);
        let child = ref_at(30);
        let source_epoch = chronicle_common::EpochId::from_uuid(uuid::Uuid::from_u128(9));
        let mut parent_session = native_session(session_for(parent), 10);
        parent_session.connections[0].operations[0]
            .provenance
            .epoch_ranges[0]
            .wal_sequence_range = Some((30, 30));
        parent_session.connections[0].operations[0]
            .provenance
            .epoch_ranges
            .push(chronicle_canonical::OperationEpochRange {
                parent_id: Some(parent.recording_id),
                epoch_id: source_epoch,
                epoch_ordinal: Some(0),
                wal_sequence_range: Some((10, 10)),
            });
        let child_session = native_session(session_for(child), 30);
        let sessions = vec![parent_session, child_session];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(parent, 10), (child, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(88));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(89));
        let mut parent_anchor = native_anchor(parent, 10, parent_generation);
        parent_anchor.boundary.source_epoch_id = source_epoch;
        let observation = crate::NativeExecutionHandoffObservation::new(
            parent_anchor,
            native_anchor(child, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "cross-epoch".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.diagnostics.is_empty());
        assert_eq!(output.facts.len(), 1);
        assert_eq!(
            output.facts[0].parent().owner_epoch_id,
            parent.owner_epoch_id
        );
    }

    #[test]
    fn protocol_adapter_disagreement_fails_closed() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(root), 10),
            native_session(session_for(member), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(root, 10), (member, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("canonical-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(86));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(87));
        let mut parent = native_anchor(root, 10, parent_generation);
        let mut child = native_anchor(member, 30, child_generation);
        parent.boundary.protocol_operation_boundary =
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "adapter-a-boundary",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap();
        child.boundary.protocol_operation_boundary =
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "adapter-b-boundary",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap();
        let observation = crate::NativeExecutionHandoffObservation::new(
            parent,
            child,
            crate::NativeObservationProvenance {
                source: "adapter-disagreement".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert!(output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        )));
    }

    #[test]
    fn reused_socket_generation_does_not_bind_stale_anchor() {
        let root = ref_at(10);
        let member = ref_at(30);
        let mut child_session = native_session(session_for(member), 30);
        child_session.connections[0].operations[0]
            .provenance
            .connection_generation =
            Some(chronicle_canonical::SourceConnectionGeneration::Socket {
                socket_cookie: 7,
                first_seen_boot_id: "new-boot".into(),
                first_seen_nanoseconds: 20,
                network_namespace: Some(1),
            });
        let sessions = vec![native_session(session_for(root), 10), child_session];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for reference in [root, member] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{}", if reference == root { 10 } else { 30 }),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(84));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(85));
        let mut stale_child = native_anchor(member, 30, child_generation);
        stale_child.boundary.source_generation =
            chronicle_canonical::SourceConnectionGeneration::Socket {
                socket_cookie: 7,
                first_seen_boot_id: "old-boot".into(),
                first_seen_nanoseconds: 10,
                network_namespace: Some(1),
            };
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(root, 10, parent_generation),
            stale_child,
            crate::NativeObservationProvenance {
                source: "generation-reuse".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert!(output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        )));
    }

    #[test]
    fn exact_binder_multi_match_emits_binding_ambiguity_without_fact() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(root), 10),
            native_session(session_for(member), 10),
        ];
        let identity = chronicle_protocol::ProtocolOperationBoundaryClaim::new(
            chronicle_common::ProtocolId::new("test"),
            "test-boundary:10",
            chronicle_common::Direction::ClientToServer,
        )
        .unwrap();
        let mut boundaries = crate::NativeBoundaryIndex::new();
        boundaries.insert_fixture(root, identity.clone());
        boundaries.insert_fixture(member, identity);
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(82));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(83));
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(root, 10, parent_generation),
            native_anchor(member, 10, child_generation),
            crate::NativeObservationProvenance {
                source: "multi-match".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert!(output.diagnostics.iter().any(|diagnostic| matches!(
            diagnostic.kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingAmbiguous
        )));
    }

    #[test]
    fn response_side_range_with_request_claim_does_not_bind() {
        let parent = ref_at(10);
        let child = ref_at(30);
        let sessions = vec![
            native_session(session_for(parent), 10),
            native_session(session_for(child), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        boundaries.insert_fixture(
            parent,
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "test-boundary:10",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap(),
        );
        boundaries.insert_fixture(
            child,
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "test-boundary:30",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap(),
        );
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(90));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(91));
        let mut response_parent = native_anchor(parent, 10, parent_generation);
        response_parent.boundary.direction = chronicle_common::Direction::ServerToClient;
        response_parent.boundary.source_range.direction =
            chronicle_common::Direction::ServerToClient;
        response_parent.boundary.protocol_operation_boundary =
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "test-boundary:10",
                chronicle_common::Direction::ServerToClient,
            )
            .unwrap();
        let observation = crate::NativeExecutionHandoffObservation::new(
            response_parent,
            native_anchor(child, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "wrong-direction".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert_eq!(output.diagnostics.len(), 1);
        assert_eq!(
            output.diagnostics[0].kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        );
    }

    #[test]
    fn epoch_and_range_must_belong_to_same_source_placement() {
        let parent = ref_at(10);
        let child = ref_at(30);
        let mut parent_session = native_session(session_for(parent), 10);
        parent_session.connections[0].operations[0]
            .provenance
            .epoch_ranges[0]
            .wal_sequence_range = Some((20, 20));
        let sessions = vec![parent_session, native_session(session_for(child), 30)];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(parent, 10), (child, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(92));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(93));
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(parent, 10, parent_generation),
            native_anchor(child, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "wrong-epoch-range-pair".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert_eq!(output.diagnostics.len(), 1);
        assert_eq!(
            output.diagnostics[0].kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        );
    }

    #[test]
    fn runtime_claim_cannot_fabricate_trusted_boundary_binding() {
        let parent = ref_at(10);
        let child = ref_at(30);
        let sessions = vec![
            native_session(session_for(parent), 10),
            native_session(session_for(child), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        boundaries.insert_fixture(
            parent,
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "test-boundary:10",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap(),
        );
        boundaries.insert_fixture(
            child,
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "test-boundary:30",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap(),
        );
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(94));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(95));
        let mut forged_parent = native_anchor(parent, 10, parent_generation);
        forged_parent.boundary.protocol_operation_boundary =
            chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                chronicle_common::ProtocolId::new("test"),
                "runtime-counter:7",
                chronicle_common::Direction::ClientToServer,
            )
            .unwrap();
        let observation = crate::NativeExecutionHandoffObservation::new(
            forged_parent,
            native_anchor(child, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "runtime-claim".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let output =
            crate::bind_native_execution_observations(&sessions, &boundaries, &[observation]);
        assert!(output.facts.is_empty());
        assert_eq!(output.diagnostics.len(), 1);
        assert_eq!(
            output.diagnostics[0].kind,
            crate::NativeBindingDiagnosticKind::AnchorBindingUnresolved
        );
    }

    #[test]
    fn binding_diagnostics_preserve_both_endpoints_and_sort_input_independently() {
        let first_parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(96));
        let first_child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(97));
        let second_parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(98));
        let second_child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(99));
        let first = crate::NativeExecutionHandoffObservation::new(
            native_anchor(ref_at(40), 40, first_parent_generation),
            native_anchor(ref_at(41), 41, first_child_generation),
            crate::NativeObservationProvenance {
                source: "diagnostic-shuffle".into(),
                parent_context_generation: first_parent_generation,
                child_context_generation: first_child_generation,
            },
        )
        .unwrap();
        let second = crate::NativeExecutionHandoffObservation::new(
            native_anchor(ref_at(50), 50, second_parent_generation),
            native_anchor(ref_at(51), 51, second_child_generation),
            crate::NativeObservationProvenance {
                source: "diagnostic-shuffle".into(),
                parent_context_generation: second_parent_generation,
                child_context_generation: second_child_generation,
            },
        )
        .unwrap();
        let forward = crate::bind_native_execution_observations(
            &[],
            &crate::NativeBoundaryIndex::new(),
            &[first.clone(), second.clone()],
        );
        let reverse = crate::bind_native_execution_observations(
            &[],
            &crate::NativeBoundaryIndex::new(),
            &[second, first],
        );
        assert!(forward.facts.is_empty());
        assert_eq!(forward.diagnostics.len(), 4);
        assert_eq!(
            serde_json::to_vec(&forward.diagnostics).unwrap(),
            serde_json::to_vec(&reverse.diagnostics).unwrap()
        );
    }

    #[test]
    fn native_and_trace_agree_without_source_priority() {
        let root = ref_at(10);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(root), 10),
            native_session(session_for(member), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(root, 10), (member, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(100));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(101));
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(root, 10, parent_generation),
            native_anchor(member, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "native-and-trace".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let composed = compose_correlation_with_native(
            &sessions,
            &CorrelationContext {
                role_resolutions: BTreeMap::from([(root, ingress_role()), (member, egress_role())]),
                evidence: BTreeMap::from([
                    (
                        root,
                        vec![trace_item("trace", "scenario", Some("root"), None)],
                    ),
                    (
                        member,
                        vec![trace_item(
                            "trace",
                            "scenario",
                            Some("member"),
                            Some("root"),
                        )],
                    ),
                ]),
            },
            &boundaries,
            &[observation],
        )
        .unwrap();
        assert!(matches!(
            composed.graph.resolution(&member),
            Some(chronicle_canonical::CorrelationResolution::Resolved {
                scenario,
                ..
            }) if *scenario == chronicle_canonical::scenario_id_v1(&root)
        ));
    }

    #[test]
    fn native_and_trace_disagreement_remains_causal_ambiguity() {
        let native_root = ref_at(10);
        let trace_root = ref_at(20);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(native_root), 10),
            native_session(session_for(trace_root), 20),
            native_session(session_for(member), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(native_root, 10), (trace_root, 20), (member, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(102));
        let child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(103));
        let observation = crate::NativeExecutionHandoffObservation::new(
            native_anchor(native_root, 10, parent_generation),
            native_anchor(member, 30, child_generation),
            crate::NativeObservationProvenance {
                source: "native-trace-disagreement".into(),
                parent_context_generation: parent_generation,
                child_context_generation: child_generation,
            },
        )
        .unwrap();
        let composed = compose_correlation_with_native(
            &sessions,
            &CorrelationContext {
                role_resolutions: BTreeMap::from([
                    (native_root, ingress_role()),
                    (trace_root, ingress_role()),
                    (member, egress_role()),
                ]),
                evidence: BTreeMap::from([
                    (
                        trace_root,
                        vec![trace_item("trace", "other", Some("root"), None)],
                    ),
                    (
                        member,
                        vec![trace_item("trace", "other", Some("member"), Some("root"))],
                    ),
                ]),
            },
            &boundaries,
            &[observation],
        )
        .unwrap();
        assert!(matches!(
            composed.graph.resolution(&member),
            Some(chronicle_canonical::CorrelationResolution::Ambiguous { .. })
        ));
    }

    #[test]
    fn two_exact_native_parents_remain_causally_ambiguous() {
        let first_root = ref_at(10);
        let second_root = ref_at(20);
        let member = ref_at(30);
        let sessions = vec![
            native_session(session_for(first_root), 10),
            native_session(session_for(second_root), 20),
            native_session(session_for(member), 30),
        ];
        let mut boundaries = crate::NativeBoundaryIndex::new();
        for (reference, sequence) in [(first_root, 10), (second_root, 20), (member, 30)] {
            boundaries.insert_fixture(
                reference,
                chronicle_protocol::ProtocolOperationBoundaryClaim::new(
                    chronicle_common::ProtocolId::new("test"),
                    format!("test-boundary:{sequence}"),
                    chronicle_common::Direction::ClientToServer,
                )
                .unwrap(),
            );
        }
        let first_parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(104));
        let first_child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(105));
        let second_parent_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(106));
        let second_child_generation =
            crate::NativeExecutionContextGeneration::from_uuid(uuid::Uuid::from_u128(107));
        let first = crate::NativeExecutionHandoffObservation::new(
            native_anchor(first_root, 10, first_parent_generation),
            native_anchor(member, 30, first_child_generation),
            crate::NativeObservationProvenance {
                source: "two-native-parents".into(),
                parent_context_generation: first_parent_generation,
                child_context_generation: first_child_generation,
            },
        )
        .unwrap();
        let second = crate::NativeExecutionHandoffObservation::new(
            native_anchor(second_root, 20, second_parent_generation),
            native_anchor(member, 30, second_child_generation),
            crate::NativeObservationProvenance {
                source: "two-native-parents".into(),
                parent_context_generation: second_parent_generation,
                child_context_generation: second_child_generation,
            },
        )
        .unwrap();
        let composed = compose_correlation_with_native(
            &sessions,
            &CorrelationContext {
                role_resolutions: BTreeMap::from([
                    (first_root, ingress_role()),
                    (second_root, ingress_role()),
                    (member, egress_role()),
                ]),
                evidence: BTreeMap::new(),
            },
            &boundaries,
            &[first, second],
        )
        .unwrap();
        assert!(matches!(
            composed.graph.resolution(&member),
            Some(chronicle_canonical::CorrelationResolution::Ambiguous { .. })
        ));
    }
}
