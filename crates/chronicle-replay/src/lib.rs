//! Replay planning, target mapping, execution, and verification orchestration.

use chronicle_canonical::{
    CanonicalOperation, CanonicalSession, OperationEffect, RelativeTimeNanos,
};
use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId};
use chronicle_protocol::{ProtocolError, ProtocolRegistry, ReplayContext, VerificationResult};
use std::collections::BTreeMap;
use thiserror::Error;

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

#[derive(Clone, Debug)]
pub struct PlannedOperation {
    connection_id: ConnectionId,
    protocol: ProtocolId,
    target: Endpoint,
    recorded_target: Endpoint,
    scheduled_offset: RelativeTimeNanos,
    operation: CanonicalOperation,
}

impl PlannedOperation {
    pub fn connection_id(&self) -> ConnectionId {
        self.connection_id
    }

    pub fn protocol(&self) -> &ProtocolId {
        &self.protocol
    }

    pub fn target(&self) -> &Endpoint {
        &self.target
    }

    pub fn recorded_target(&self) -> &Endpoint {
        &self.recorded_target
    }

    pub fn scheduled_offset(&self) -> RelativeTimeNanos {
        self.scheduled_offset
    }

    pub fn operation(&self) -> &CanonicalOperation {
        &self.operation
    }
}

#[derive(Clone, Debug)]
pub struct ReplayPlan {
    timing: TimingMode,
    dry_run: bool,
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
        let mut operations = Vec::new();
        for connection in &session.connections {
            let target = targets
                .resolve(&connection.protocol, &connection.server, connection.id)
                .ok_or(ReplayError::MissingTarget {
                    connection_id: connection.id,
                })?;
            if policy.block_recorded_destination && target == &connection.server {
                return Err(ReplayError::RecordedDestination {
                    connection_id: connection.id,
                });
            }
            for operation in &connection.operations {
                if !policy.allows(operation.effect) {
                    return Err(ReplayError::PolicyDenied {
                        operation_id: operation.id,
                        effect: operation.effect,
                    });
                }
                operations.push(PlannedOperation {
                    connection_id: connection.id,
                    protocol: connection.protocol.clone(),
                    target: target.clone(),
                    recorded_target: connection.server.clone(),
                    scheduled_offset: match timing {
                        TimingMode::Preserve => operation.started_at_offset,
                        TimingMode::Asap => RelativeTimeNanos(0),
                    },
                    operation: operation.clone(),
                });
            }
        }
        operations
            .sort_by_key(|operation| (operation.scheduled_offset, operation.operation.sequence));
        Ok(ReplayPlan {
            timing,
            dry_run: policy.dry_run,
            operations,
        })
    }
}

#[derive(Clone, Debug)]
pub struct OperationReplayResult {
    pub operation_id: OperationId,
    pub verification: VerificationResult,
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
    ) -> Result<Vec<OperationReplayResult>, ReplayError> {
        if plan.dry_run {
            return Err(ReplayError::DryRun);
        }
        let mut results = Vec::with_capacity(plan.operations.len());
        for planned in &plan.operations {
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
            let mut connection = adapter.connect(&planned.target, &context).await?;
            let observed = connection.execute(&planned.operation).await?;
            results.push(OperationReplayResult {
                operation_id: planned.operation.id,
                verification: verifier.verify(&planned.operation, &observed),
            });
        }
        Ok(results)
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
    use chronicle_protocol::VerificationStatus;
    use time::OffsetDateTime;

    fn session(effect: OperationEffect) -> CanonicalSession {
        CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            source: SourceMetadata::default(),
            connections: vec![CanonicalConnection {
                id: ConnectionId::new(),
                protocol: ProtocolId::new("fake"),
                client: Endpoint::new("client", 1),
                server: Endpoint::new("production", 2),
                attributes: Attributes::new(),
                operations: vec![CanonicalOperation {
                    id: OperationId::new(),
                    sequence: 1,
                    started_at_offset: RelativeTimeNanos(0),
                    completed_at_offset: None,
                    kind: OperationKind::Request,
                    effect,
                    request: PayloadRef::Missing {
                        reason: "test".into(),
                    },
                    recorded_response: None,
                    attributes: Attributes::new(),
                    protocol_data: ProtocolData {
                        schema_version: 1,
                        media_type: None,
                        bytes: Vec::new(),
                    },
                    incomplete: true,
                    truncated: false,
                    redactions: Vec::new(),
                    warnings: Vec::new(),
                }],
                incomplete: false,
                truncated: false,
            }],
            timeline: Vec::new(),
            replay: ReplayMetadata::default(),
        }
    }

    #[test]
    fn target_mapping_is_mandatory() {
        let error = ReplayPlanner::plan(
            &session(OperationEffect::Read),
            &TargetMap::default(),
            &ReplayPolicy::default(),
            TimingMode::Asap,
        )
        .unwrap_err();
        assert!(matches!(error, ReplayError::MissingTarget { .. }));
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
        let error = ReplayPlanner::plan(
            &session,
            &targets,
            &ReplayPolicy::default(),
            TimingMode::Asap,
        )
        .unwrap_err();
        assert!(matches!(error, ReplayError::PolicyDenied { .. }));
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
        assert_eq!(plan.operations().len(), 1);
        assert_eq!(plan.operations()[0].target(), &Endpoint::new("test", 2));
        let error = ReplayExecutor::new(&ProtocolRegistry::new())
            .execute(&plan, &BTreeMap::new())
            .await
            .unwrap_err();
        assert!(matches!(error, ReplayError::DryRun));
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
