//! Explicit correlation composition boundary (`chronicle-etl`).
//!
//! Joins published canonical sessions with a caller-supplied provider-neutral
//! `CorrelationContext` and invokes the resolver. This module owns reference
//! verification and join ordering ONLY — zero scenario-selection semantics.
//! It is explicitly invoked on demand; the default publication/checkpoint path
//! never calls it.

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
                    parent_id: None,
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
}
