//! Protocol-independent canonical replay model.
//!
//! Canonical v1 is an evolving pre-release MVP schema until its contract is frozen.
//! Serde validates wire shape and schema version; [`CanonicalSession::validate`] checks
//! cross-field structure before publication.

use chronicle_common::{
    ConnectionId, Direction, Endpoint, OperationId, ProtocolId, RecordingId, SessionId, Timestamp,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const CANONICAL_SCHEMA_VERSION: u16 = 1;
pub const PROTOCOL_DATA_SCHEMA_VERSION: u16 = 1;
pub type Attributes = BTreeMap<String, String>;

/// Typed capture completeness. Only `Complete` is replayable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    Complete,
    #[default]
    Incomplete,
    Truncated,
    Malformed,
    Unmatched,
    Unsupported,
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitMarkerProvenance {
    pub segment_ordinal: u64,
    pub byte_offset: u64,
    pub sequence: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProvenance {
    pub recording_id: Option<RecordingId>,
    #[serde(default)]
    pub epoch_id: Option<chronicle_common::EpochId>,
    #[serde(default)]
    pub epoch_ordinal: Option<u64>,
    #[serde(default)]
    pub wal_sequence_range: Option<(u64, u64)>,
    #[serde(default)]
    pub continuation_in: Option<String>,
    #[serde(default)]
    pub continuation_out: Option<String>,
    pub status: SourceStatus,
    pub reason: Option<String>,
    pub commit_marker: Option<CommitMarkerProvenance>,
    pub wal_snapshot_sha256: Option<String>,
    pub pipeline_version: Option<String>,
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
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum SourceConnectionGeneration {
    Fixture,
    Socket {
        socket_cookie: u64,
        first_seen_boot_id: String,
        first_seen_nanoseconds: u64,
        network_namespace: Option<u64>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalByteRange {
    pub segment_ordinal: u64,
    pub segment_first_sequence: u64,
    pub frame_byte_offset: u64,
    pub wal_sequence: u64,
    pub direction: Direction,
    pub payload_byte_offset: u64,
    pub payload_byte_length: u64,
    pub gap_before: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationEpochRange {
    #[serde(default)]
    pub parent_id: Option<RecordingId>,
    pub epoch_id: chronicle_common::EpochId,
    #[serde(default)]
    pub epoch_ordinal: Option<u64>,
    #[serde(default)]
    pub wal_sequence_range: Option<(u64, u64)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationCompletion {
    #[default]
    Complete,
    Incomplete,
    Unsupported,
    Invalid,
    BoundExhausted,
}

impl OperationCompletion {
    pub const fn is_replayable(self) -> bool {
        matches!(self, Self::Complete)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationProvenance {
    pub connection_generation: Option<SourceConnectionGeneration>,
    pub wal_ranges: Vec<WalByteRange>,
    pub missing_payload_provenance: bool,
    /// Epoch where decoder reached terminal completion, if any.
    #[serde(default)]
    pub completion_owner_epoch: Option<chronicle_common::EpochId>,
    /// Ordered parent/epoch/WAL ranges contributing to this operation.
    #[serde(default)]
    pub epoch_ranges: Vec<OperationEpochRange>,
    /// Explicit fallback prevents incomplete handoffs from becoming replay traffic.
    #[serde(default)]
    pub completion: OperationCompletion,
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
    pub provenance: OperationProvenance,
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

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum CanonicalValidationError {
    #[error("unsupported canonical schema version {found}")]
    UnsupportedSchemaVersion { found: u16 },
    #[error("duplicate canonical connection {id}")]
    DuplicateConnectionId { id: ConnectionId },
    #[error("duplicate canonical operation {id}")]
    DuplicateOperationId { id: OperationId },
    #[error("canonical operation {id} has unsupported protocol-data schema version {found}")]
    UnsupportedProtocolDataSchemaVersion { id: OperationId, found: u16 },
    #[error("canonical connection {id} is missing completeness")]
    MissingConnectionCompleteness { id: ConnectionId },
    #[error("canonical operation {id} is missing completeness")]
    MissingOperationCompleteness { id: OperationId },
    #[error("canonical completeness references unknown connection {id}")]
    UnknownConnectionCompleteness { id: ConnectionId },
    #[error("canonical completeness references unknown operation {id}")]
    UnknownOperationCompleteness { id: OperationId },
    #[error("timeline references unknown connection {id}")]
    TimelineUnknownConnection { id: ConnectionId },
    #[error("timeline references unknown operation {id}")]
    TimelineUnknownOperation { id: OperationId },
    #[error(
        "timeline operation {operation_id} belongs to {expected_connection_id}, not {actual_connection_id}"
    )]
    TimelineOperationConnectionMismatch {
        operation_id: OperationId,
        expected_connection_id: ConnectionId,
        actual_connection_id: ConnectionId,
    },
    #[error("timeline contains duplicate operation {id}")]
    DuplicateTimelineOperation { id: OperationId },
    #[error("canonical operation {id} completion owner is absent from epoch provenance")]
    CompletionOwnerMissingRange { id: OperationId },
    #[error("canonical operation {id} epoch range names a different parent recording")]
    OperationRangeParentMismatch { id: OperationId },
    #[error("timeline is missing operation {id}")]
    MissingTimelineOperation { id: OperationId },
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
            .unwrap_or(Completeness::Incomplete)
    }

    pub fn operation_state(&self, id: &OperationId) -> Completeness {
        self.operation_completeness
            .get(id)
            .copied()
            .unwrap_or(Completeness::Incomplete)
    }

    /// Checks cross-field canonical structure. Serde intentionally does not call this,
    /// so inspect/hydration can choose typed corruption handling; publication must call it.
    pub fn validate(&self) -> Result<(), CanonicalValidationError> {
        if self.schema_version != CANONICAL_SCHEMA_VERSION {
            return Err(CanonicalValidationError::UnsupportedSchemaVersion {
                found: self.schema_version,
            });
        }

        let mut connections = BTreeSet::new();
        let mut operations = BTreeMap::new();
        for connection in &self.connections {
            if !connections.insert(connection.id) {
                return Err(CanonicalValidationError::DuplicateConnectionId { id: connection.id });
            }
            if !self.connection_completeness.contains_key(&connection.id) {
                return Err(CanonicalValidationError::MissingConnectionCompleteness {
                    id: connection.id,
                });
            }
            for operation in &connection.operations {
                if operation.protocol_data.schema_version != PROTOCOL_DATA_SCHEMA_VERSION {
                    return Err(
                        CanonicalValidationError::UnsupportedProtocolDataSchemaVersion {
                            id: operation.id,
                            found: operation.protocol_data.schema_version,
                        },
                    );
                }
                if operations.insert(operation.id, connection.id).is_some() {
                    return Err(CanonicalValidationError::DuplicateOperationId {
                        id: operation.id,
                    });
                }
                if !self.operation_completeness.contains_key(&operation.id) {
                    return Err(CanonicalValidationError::MissingOperationCompleteness {
                        id: operation.id,
                    });
                }
                if let Some(owner) = operation.provenance.completion_owner_epoch
                    && !operation
                        .provenance
                        .epoch_ranges
                        .iter()
                        .any(|range| range.epoch_id == owner)
                {
                    return Err(CanonicalValidationError::CompletionOwnerMissingRange {
                        id: operation.id,
                    });
                }
                if operation.provenance.epoch_ranges.iter().any(|range| {
                    range.parent_id.is_some()
                        && range.parent_id != self.source_provenance.recording_id
                }) {
                    return Err(CanonicalValidationError::OperationRangeParentMismatch {
                        id: operation.id,
                    });
                }
            }
        }

        for id in self.connection_completeness.keys() {
            if !connections.contains(id) {
                return Err(CanonicalValidationError::UnknownConnectionCompleteness { id: *id });
            }
        }
        for id in self.operation_completeness.keys() {
            if !operations.contains_key(id) {
                return Err(CanonicalValidationError::UnknownOperationCompleteness { id: *id });
            }
        }

        let mut timeline_operations = BTreeSet::new();
        for entry in &self.timeline {
            if !connections.contains(&entry.connection_id) {
                return Err(CanonicalValidationError::TimelineUnknownConnection {
                    id: entry.connection_id,
                });
            }
            let Some(expected_connection_id) = operations.get(&entry.operation_id) else {
                return Err(CanonicalValidationError::TimelineUnknownOperation {
                    id: entry.operation_id,
                });
            };
            if *expected_connection_id != entry.connection_id {
                return Err(
                    CanonicalValidationError::TimelineOperationConnectionMismatch {
                        operation_id: entry.operation_id,
                        expected_connection_id: *expected_connection_id,
                        actual_connection_id: entry.connection_id,
                    },
                );
            }
            if !timeline_operations.insert(entry.operation_id) {
                return Err(CanonicalValidationError::DuplicateTimelineOperation {
                    id: entry.operation_id,
                });
            }
        }

        for id in operations.keys() {
            if !timeline_operations.contains(id) {
                return Err(CanonicalValidationError::MissingTimelineOperation { id: *id });
            }
        }
        Ok(())
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

    fn operation() -> CanonicalOperation {
        CanonicalOperation {
            id: OperationId::new(),
            sequence: 1,
            started_at_offset: RelativeTimeNanos(0),
            completed_at_offset: None,
            kind: OperationKind::Request,
            effect: OperationEffect::Read,
            request: PayloadRef::Missing {
                reason: "test".into(),
            },
            recorded_response: None,
            attributes: Attributes::new(),
            protocol_data: ProtocolData {
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: None,
                bytes: Vec::new(),
            },
            provenance: OperationProvenance::default(),
            redactions: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn valid_session() -> CanonicalSession {
        let connection_id = ConnectionId::new();
        let first = operation();
        let mut second = operation();
        second.sequence = 2;
        second.started_at_offset = RelativeTimeNanos(1);
        let mut session = session();
        session.connections.push(CanonicalConnection {
            id: connection_id,
            protocol: ProtocolId::new("test/1"),
            client: Endpoint::new("127.0.0.1", 1),
            server: Endpoint::new("127.0.0.1", 2),
            attributes: Attributes::new(),
            operations: vec![first.clone(), second.clone()],
        });
        session
            .connection_completeness
            .insert(connection_id, Completeness::Complete);
        for operation in [&first, &second] {
            session
                .operation_completeness
                .insert(operation.id, Completeness::Complete);
            session.timeline.push(TimelineEntry {
                connection_id,
                operation_id: operation.id,
                offset: operation.started_at_offset,
            });
        }
        session
    }

    #[test]
    fn completion_owner_requires_contributing_epoch_range() {
        let mut session = valid_session();
        let owner = chronicle_common::EpochId::new();
        session.connections[0].operations[0]
            .provenance
            .completion_owner_epoch = Some(owner);
        assert!(matches!(
            session.validate(),
            Err(CanonicalValidationError::CompletionOwnerMissingRange { .. })
        ));
        session.connections[0].operations[0]
            .provenance
            .epoch_ranges
            .push(OperationEpochRange {
                parent_id: None,
                epoch_id: owner,
                epoch_ordinal: Some(1),
                wal_sequence_range: Some((10, 20)),
            });
        assert!(session.validate().is_ok());
        session.source_provenance.recording_id = Some(RecordingId::new());
        session.connections[0].operations[0].provenance.epoch_ranges[0].parent_id =
            Some(RecordingId::new());
        assert!(matches!(
            session.validate(),
            Err(CanonicalValidationError::OperationRangeParentMismatch { .. })
        ));
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
        assert_eq!(value["schema_version"], CANONICAL_SCHEMA_VERSION);
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
        let object = PayloadRef::Object {
            bucket: "bucket".into(),
            key: "key".into(),
            checksum: "sha256:deadbeef".into(),
            size: 4,
            content_type: None,
        };
        assert_eq!(
            serde_json::from_slice::<PayloadRef>(&serde_json::to_vec(&object).unwrap()).unwrap(),
            object
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
                schema_version: PROTOCOL_DATA_SCHEMA_VERSION,
                media_type: None,
                bytes: Vec::new(),
            },
            provenance: OperationProvenance::default(),
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
    fn rejects_non_v1_protocol_data() {
        let mut invalid = valid_session();
        let operation = &mut invalid.connections[0].operations[0];
        let id = operation.id;
        operation.protocol_data.schema_version = PROTOCOL_DATA_SCHEMA_VERSION + 1;
        assert_eq!(
            invalid.validate(),
            Err(
                CanonicalValidationError::UnsupportedProtocolDataSchemaVersion {
                    id,
                    found: PROTOCOL_DATA_SCHEMA_VERSION + 1,
                }
            )
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
            assert_eq!(
                invalid.validate(),
                Err(CanonicalValidationError::UnsupportedSchemaVersion { found: version })
            );
            let error = serde_json::to_value(invalid).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("unsupported canonical schema version")
            );
        }
    }

    #[test]
    fn validates_complete_session() {
        assert_eq!(valid_session().validate(), Ok(()));
    }

    #[test]
    fn rejects_duplicate_connection_and_operation_ids() {
        let mut duplicate_connection = valid_session();
        let id = duplicate_connection.connections[0].id;
        duplicate_connection
            .connections
            .push(duplicate_connection.connections[0].clone());
        assert_eq!(
            duplicate_connection.validate(),
            Err(CanonicalValidationError::DuplicateConnectionId { id })
        );

        let mut duplicate_operation = valid_session();
        let duplicate = duplicate_operation.connections[0].operations[0].clone();
        let id = duplicate.id;
        duplicate_operation.connections[0]
            .operations
            .push(duplicate);
        assert_eq!(
            duplicate_operation.validate(),
            Err(CanonicalValidationError::DuplicateOperationId { id })
        );

        let mut cross_connection_duplicate = valid_session();
        let mut connection = cross_connection_duplicate.connections[0].clone();
        connection.id = ConnectionId::new();
        let id = connection.operations[0].id;
        cross_connection_duplicate
            .connection_completeness
            .insert(connection.id, Completeness::Complete);
        cross_connection_duplicate.connections.push(connection);
        assert_eq!(
            cross_connection_duplicate.validate(),
            Err(CanonicalValidationError::DuplicateOperationId { id })
        );
    }

    #[test]
    fn completeness_states_have_stable_typed_names() {
        for (state, encoded) in [
            (Completeness::Complete, "\"complete\""),
            (Completeness::Incomplete, "\"incomplete\""),
            (Completeness::Truncated, "\"truncated\""),
            (Completeness::Malformed, "\"malformed\""),
            (Completeness::Unmatched, "\"unmatched\""),
            (Completeness::Unsupported, "\"unsupported\""),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), encoded);
            assert_eq!(
                serde_json::from_str::<Completeness>(encoded).unwrap(),
                state
            );
        }
    }

    #[test]
    fn rejects_incomplete_or_unknown_completeness() {
        let mut missing_connection = valid_session();
        let id = missing_connection.connections[0].id;
        missing_connection.connection_completeness.remove(&id);
        assert_eq!(
            missing_connection.validate(),
            Err(CanonicalValidationError::MissingConnectionCompleteness { id })
        );

        let mut missing_operation = valid_session();
        let id = missing_operation.connections[0].operations[0].id;
        missing_operation.operation_completeness.remove(&id);
        assert_eq!(
            missing_operation.validate(),
            Err(CanonicalValidationError::MissingOperationCompleteness { id })
        );

        let mut unknown_connection = valid_session();
        let id = ConnectionId::new();
        unknown_connection
            .connection_completeness
            .insert(id, Completeness::Incomplete);
        assert_eq!(
            unknown_connection.validate(),
            Err(CanonicalValidationError::UnknownConnectionCompleteness { id })
        );

        let mut unknown_operation = valid_session();
        let id = OperationId::new();
        unknown_operation
            .operation_completeness
            .insert(id, Completeness::Incomplete);
        assert_eq!(
            unknown_operation.validate(),
            Err(CanonicalValidationError::UnknownOperationCompleteness { id })
        );
    }

    #[test]
    fn rejects_invalid_timeline_references_and_cardinality() {
        let mut unknown_connection = valid_session();
        let id = ConnectionId::new();
        unknown_connection.timeline[0].connection_id = id;
        assert_eq!(
            unknown_connection.validate(),
            Err(CanonicalValidationError::TimelineUnknownConnection { id })
        );

        let mut unknown_operation = valid_session();
        let id = OperationId::new();
        unknown_operation.timeline[0].operation_id = id;
        assert_eq!(
            unknown_operation.validate(),
            Err(CanonicalValidationError::TimelineUnknownOperation { id })
        );

        let mut mismatch = valid_session();
        let actual_connection_id = ConnectionId::new();
        mismatch.connections.push(CanonicalConnection {
            id: actual_connection_id,
            protocol: ProtocolId::new("test/1"),
            client: Endpoint::new("127.0.0.1", 3),
            server: Endpoint::new("127.0.0.1", 4),
            attributes: Attributes::new(),
            operations: Vec::new(),
        });
        mismatch
            .connection_completeness
            .insert(actual_connection_id, Completeness::Complete);
        let operation_id = mismatch.timeline[0].operation_id;
        let expected_connection_id = mismatch.connections[0].id;
        mismatch.timeline[0].connection_id = actual_connection_id;
        assert_eq!(
            mismatch.validate(),
            Err(
                CanonicalValidationError::TimelineOperationConnectionMismatch {
                    operation_id,
                    expected_connection_id,
                    actual_connection_id,
                }
            )
        );

        let mut duplicate = valid_session();
        let id = duplicate.timeline[0].operation_id;
        duplicate.timeline.push(duplicate.timeline[0].clone());
        assert_eq!(
            duplicate.validate(),
            Err(CanonicalValidationError::DuplicateTimelineOperation { id })
        );

        let mut missing = valid_session();
        let id = missing.timeline.pop().unwrap().operation_id;
        assert_eq!(
            missing.validate(),
            Err(CanonicalValidationError::MissingTimelineOperation { id })
        );
    }
}
