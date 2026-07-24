//! Versioned protocol-independent replay model.

use chronicle_common::{ConnectionId, Endpoint, OperationId, ProtocolId, SessionId, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const CANONICAL_SCHEMA_VERSION: u16 = 1;
pub type Attributes = BTreeMap<String, String>;

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
    pub schema_version: u16,
    pub id: SessionId,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    pub source: SourceMetadata,
    pub connections: Vec<CanonicalConnection>,
    pub timeline: Vec<TimelineEntry>,
    pub replay: ReplayMetadata,
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
    }
}
