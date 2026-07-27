//! Versioned protocol-independent replay model.

use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CANONICAL_SCHEMA_VERSION: u16 = 1;
pub type Attributes = BTreeMap<String, String>;

/// Typed capture completeness. `Partial` never implies replay safety.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Completeness {
    Complete,
    Partial,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceStatus {
    Completed,
    Failed,
    Aborted,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProvenanceKind {
    Connection,
    RingLoss,
    WalLimitLoss,
    WalEnvelope,
    CommitMarker,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub kind: ProvenanceKind,
    pub sequence_range: Option<(u64, u64)>,
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProvenance {
    pub status: SourceStatus,
    pub reason: Option<String>,
    /// Deterministic source evidence order, never a stringly-typed attribute.
    pub evidence: Vec<ProvenanceEntry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RelativeTimeNanos(pub u64);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadRef {
    Inline {
        content_type: Option<String>,
        bytes: Vec<u8>,
    },
    Object {
        bucket: String,
        key: String,
        checksum: String,
        size: u64,
        content_type: Option<String>,
    },
    Artifact {
        key: String,
        checksum: String,
        size: u64,
        content_type: Option<String>,
    },
    Redacted {
        reason: String,
        original_size: Option<u64>,
    },
    Missing {
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationEffect {
    Read,
    Write,
    Publish,
    Authentication,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OperationKind {
    Request,
    Command,
    Publish,
    Subscription,
    Opaque,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolData {
    pub schema_version: u16,
    pub media_type: Option<String>,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactionRecord {
    pub location: String,
    pub method: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalWarning {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedactionStage {
    RemoteArchive,
    CanonicalStorage,
    Inspect,
    Log,
    Replay,
}

pub struct RedactionContext<'a> {
    pub stage: RedactionStage,
    pub location: &'a str,
    pub protocol: &'a ProtocolId,
}

pub trait Redactor: Send + Sync {
    fn redact(
        &self,
        context: RedactionContext<'_>,
        payload: PayloadRef,
    ) -> (PayloadRef, Vec<RedactionRecord>);
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalOperation {
    pub id: OperationId,
    /// Source/WAL provenance sequence; not session presentation order.
    pub sequence: u64,
    pub started_at_offset: RelativeTimeNanos,
    pub completed_at_offset: Option<RelativeTimeNanos>,
    pub kind: OperationKind,
    pub effect: OperationEffect,
    pub request: PayloadRef,
    pub recorded_response: Option<PayloadRef>,
    pub attributes: Attributes,
    pub protocol_data: ProtocolData,
    pub redactions: Vec<RedactionRecord>,
    pub warnings: Vec<CanonicalWarning>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalConnection {
    pub id: ConnectionId,
    pub protocol: ProtocolId,
    pub client: Endpoint,
    pub server: Endpoint,
    pub attributes: Attributes,
    pub operations: Vec<CanonicalOperation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub connection_id: ConnectionId,
    pub operation_id: OperationId,
    pub offset: RelativeTimeNanos,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub host: Option<String>,
    pub service: Option<String>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayMetadata {
    pub requires_explicit_target_mapping: bool,
    pub recorded_environment: Option<String>,
}

impl Default for ReplayMetadata {
    fn default() -> Self {
        Self {
            requires_explicit_target_mapping: true,
            recorded_environment: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalSession {
    #[serde(
        serialize_with = "validate_schema_version_on_serialize",
        deserialize_with = "validate_schema_version"
    )]
    pub schema_version: u16,
    pub id: SessionId,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub source: SourceMetadata,
    pub source_provenance: SourceProvenance,
    pub connections: Vec<CanonicalConnection>,
    /// Authoritative connection completeness. Connection flags are not duplicated.
    pub connection_completeness: BTreeMap<ConnectionId, Completeness>,
    /// Authoritative operation completeness. Operation flags are not duplicated.
    pub operation_completeness: BTreeMap<OperationId, Completeness>,
    /// Array order is sole authoritative session operation ordering.
    pub timeline: Vec<TimelineEntry>,
    pub replay: ReplayMetadata,
    pub replay_attributes: Attributes,
}

impl CanonicalSession {
    pub fn connection_state(&self, id: &ConnectionId) -> Completeness {
        self.connection_completeness
            .get(id)
            .copied()
            .unwrap_or(Completeness::Unknown)
    }

    pub fn operation_state(&self, id: &OperationId) -> Completeness {
        self.operation_completeness
            .get(id)
            .copied()
            .unwrap_or(Completeness::Unknown)
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // Serde `serialize_with` requires a reference.
fn validate_schema_version_on_serialize<S>(version: &u16, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    if *version == CANONICAL_SCHEMA_VERSION {
        serializer.serialize_u16(*version)
    } else {
        Err(serde::ser::Error::custom(format!(
            "unsupported canonical schema version {version}"
        )))
    }
}

fn validate_schema_version<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u16::deserialize(deserializer)?;
    if version == CANONICAL_SCHEMA_VERSION {
        Ok(version)
    } else {
        Err(serde::de::Error::custom(format!(
            "unsupported canonical schema version {version}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    fn session() -> CanonicalSession {
        CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            source: SourceMetadata::default(),
            source_provenance: SourceProvenance::default(),
            connections: Vec::new(),
            connection_completeness: BTreeMap::new(),
            operation_completeness: BTreeMap::new(),
            timeline: Vec::new(),
            replay: ReplayMetadata::default(),
            replay_attributes: Attributes::new(),
        }
    }

    #[test]
    fn schema_serialization_round_trip_flattens_mvp_metadata() {
        let mut session = session();
        session.timeline.push(TimelineEntry {
            connection_id: ConnectionId::new(),
            operation_id: OperationId::new(),
            offset: RelativeTimeNanos(0),
        });
        let encoded = serde_json::to_vec(&session).unwrap();
        let decoded: CanonicalSession = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, session);

        let value: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        assert!(value.get("v3").is_none());
        assert!(value.get("source_provenance").is_some());
        assert!(value.get("connection_completeness").is_some());
        assert!(value.get("operation_completeness").is_some());
        assert!(value.get("replay_attributes").is_some());
        assert!(
            value["timeline"]
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry.get("sequence").is_none())
        );
    }

    #[test]
    fn artifact_and_operation_round_trip() {
        let artifact = PayloadRef::Artifact {
            key: "sessions/example/payloads/deadbeef".into(),
            checksum: "sha256:deadbeef".into(),
            size: 4,
            content_type: Some("application/octet-stream".into()),
        };
        assert_eq!(
            serde_json::from_slice::<PayloadRef>(&serde_json::to_vec(&artifact).unwrap()).unwrap(),
            artifact
        );

        let operation = CanonicalOperation {
            id: OperationId::new(),
            sequence: 1,
            started_at_offset: RelativeTimeNanos(0),
            completed_at_offset: None,
            kind: OperationKind::Request,
            effect: OperationEffect::Read,
            request: artifact,
            recorded_response: None,
            attributes: Attributes::new(),
            protocol_data: ProtocolData {
                schema_version: 1,
                media_type: None,
                bytes: Vec::new(),
            },
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        assert_eq!(
            serde_json::from_slice::<CanonicalOperation>(&serde_json::to_vec(&operation).unwrap())
                .unwrap(),
            operation
        );
    }

    #[test]
    fn rejects_every_non_mvp_schema_version() {
        for version in [0, 2, u16::MAX] {
            let mut value = serde_json::to_value(session()).unwrap();
            value["schema_version"] = version.into();
            let error = serde_json::from_value::<CanonicalSession>(value).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported canonical schema version")
            );

            let mut invalid = session();
            invalid.schema_version = version;
            let error = serde_json::to_value(invalid).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported canonical schema version")
            );
        }
    }
}
