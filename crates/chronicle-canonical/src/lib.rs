//! Versioned protocol-independent replay model.

use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CANONICAL_SCHEMA_V1: u16 = 1;
pub const CANONICAL_SCHEMA_V2: u16 = 2;
pub const CANONICAL_SCHEMA_V3: u16 = 3;
pub const CANONICAL_SCHEMA_VERSION: u16 = CANONICAL_SCHEMA_V2;
pub type Attributes = BTreeMap<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum CanonicalSchemaVersion {
    V1 = CANONICAL_SCHEMA_V1,
    V2 = CANONICAL_SCHEMA_V2,
    V3 = CANONICAL_SCHEMA_V3,
}

impl TryFrom<u16> for CanonicalSchemaVersion {
    type Error = u16;

    fn try_from(value: u16) -> Result<Self, Self::Error> {
        match value {
            CANONICAL_SCHEMA_V1 => Ok(Self::V1),
            CANONICAL_SCHEMA_V2 => Ok(Self::V2),
            CANONICAL_SCHEMA_V3 => Ok(Self::V3),
            other => Err(other),
        }
    }
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
    pub sequence: u64,
    pub started_at_offset: RelativeTimeNanos,
    pub completed_at_offset: Option<RelativeTimeNanos>,
    pub kind: OperationKind,
    pub effect: OperationEffect,
    pub request: PayloadRef,
    pub recorded_response: Option<PayloadRef>,
    pub attributes: Attributes,
    pub protocol_data: ProtocolData,
    pub incomplete: bool,
    pub truncated: bool,
    pub redactions: Vec<RedactionRecord>,
    #[serde(default)]
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
    pub incomplete: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub sequence: u64,
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
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u16,
    pub id: SessionId,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub source: SourceMetadata,
    pub connections: Vec<CanonicalConnection>,
    pub timeline: Vec<TimelineEntry>,
    pub replay: ReplayMetadata,
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> Result<u16, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let version = u16::deserialize(deserializer)?;
    match CanonicalSchemaVersion::try_from(version) {
        Ok(CanonicalSchemaVersion::V1 | CanonicalSchemaVersion::V2) => Ok(version),
        Ok(CanonicalSchemaVersion::V3) | Err(_) => Err(serde::de::Error::custom(format!(
            "unsupported canonical schema version {version}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::OffsetDateTime;

    #[test]
    fn schema_serialization_round_trip() {
        let session = CanonicalSession {
            schema_version: CANONICAL_SCHEMA_VERSION,
            id: SessionId::new(),
            started_at: OffsetDateTime::UNIX_EPOCH,
            ended_at: None,
            source: SourceMetadata::default(),
            connections: Vec::new(),
            timeline: Vec::new(),
            replay: ReplayMetadata::default(),
        };
        let encoded = serde_json::to_vec(&session).unwrap();
        let decoded: CanonicalSession = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, session);

        let mut v1: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        v1["schema_version"] = 1.into();
        let decoded: CanonicalSession = serde_json::from_value(v1).unwrap();
        assert_eq!(decoded.schema_version, 1);
    }

    #[test]
    fn artifact_and_v1_operation_warning_defaults_round_trip() {
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
            incomplete: false,
            truncated: false,
            redactions: Vec::new(),
            warnings: Vec::new(),
        };
        let mut v1 = serde_json::to_value(operation).unwrap();
        v1.as_object_mut().unwrap().remove("warnings");
        let decoded: CanonicalOperation = serde_json::from_value(v1).unwrap();
        assert!(decoded.warnings.is_empty());
    }

    #[test]
    fn dispatches_v1_v2_and_rejects_unimplemented_or_unknown_versions() {
        assert_eq!(
            CanonicalSchemaVersion::try_from(CANONICAL_SCHEMA_V1).unwrap(),
            CanonicalSchemaVersion::V1
        );
        assert_eq!(
            CanonicalSchemaVersion::try_from(CANONICAL_SCHEMA_V2).unwrap(),
            CanonicalSchemaVersion::V2
        );
        assert_eq!(
            CanonicalSchemaVersion::try_from(CANONICAL_SCHEMA_V3).unwrap(),
            CanonicalSchemaVersion::V3
        );

        for version in [0, CANONICAL_SCHEMA_V3, CANONICAL_SCHEMA_V3 + 1] {
            let error = serde_json::from_value::<CanonicalSession>(serde_json::json!({
                "schema_version": version,
                "id": SessionId::new(),
                "started_at": OffsetDateTime::UNIX_EPOCH,
                "ended_at": null,
                "source": SourceMetadata::default(),
                "connections": [],
                "timeline": [],
                "replay": ReplayMetadata::default(),
            }))
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported canonical schema version")
            );
        }
    }
}
