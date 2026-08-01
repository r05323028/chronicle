//! Crash-safe lifecycle manifest model. Commit markers remain WAL authority.
#![allow(
    clippy::format_collect,
    clippy::too_many_lines,
    clippy::unnecessary_wraps
)]

use crate::segment_file_name;
use chronicle_common::RecordingId;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const WAL_MANIFEST_SCHEMA_VERSION: u16 = 1;
pub const WAL_MANIFEST_FILE: &str = "manifest.json";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalManifestV1 {
    pub version: u16,
    pub revision: u64,
    pub predecessor_revision: Option<u64>,
    pub recording_id: RecordingId,
    pub epoch_ordinal: u64,
    pub policy: ManifestPolicy,
    pub active_segment: Option<u64>,
    pub authoritative_marker: Option<MarkerSummary>,
    pub segments: Vec<WalManifestSegment>,
}

impl WalManifestV1 {
    pub fn validate(&self) -> Result<(), ManifestError> {
        if self.version != WAL_MANIFEST_SCHEMA_VERSION {
            return Err(ManifestError::UnsupportedVersion);
        }
        if self
            .predecessor_revision
            .is_some_and(|revision| revision >= self.revision)
        {
            return Err(ManifestError::Contradiction("revision"));
        }
        self.policy.validate()?;
        let mut active = None;
        let mut previous_range = None;
        for segment in &self.segments {
            segment.validate()?;
            if let Some(previous) = previous_range
                && segment.first_sequence <= previous
            {
                return Err(ManifestError::OverlappingRange);
            }
            previous_range = segment.last_sequence.or(Some(segment.first_sequence));
            if segment.lifecycle == SegmentLifecycle::Active
                && active.replace(segment.ordinal).is_some()
            {
                return Err(ManifestError::Contradiction("active_segment"));
            }
        }
        if active != self.active_segment {
            return Err(ManifestError::Contradiction("active_segment"));
        }
        if let Some(marker) = &self.authoritative_marker {
            marker.validate()?;
            let segment = self
                .segments
                .iter()
                .find(|segment| segment.ordinal == marker.segment_ordinal)
                .ok_or(ManifestError::Contradiction("authoritative_marker"))?;
            if marker.sequence < segment.first_sequence
                || segment
                    .last_sequence
                    .is_some_and(|last| marker.sequence > last)
            {
                return Err(ManifestError::Contradiction("authoritative_marker"));
            }
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<Vec<u8>, ManifestError> {
        self.validate()?;
        serde_json::to_vec_pretty(self)
            .map_err(|_| ManifestError::Serialization)
            .map(|mut bytes| {
                bytes.push(b'\n');
                bytes
            })
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ManifestError> {
        let value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| ManifestError::Parse)?;
        let version = value
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or(ManifestError::UnsupportedVersion)?;
        if version != u64::from(WAL_MANIFEST_SCHEMA_VERSION) {
            return Err(ManifestError::UnsupportedVersion);
        }
        let manifest: Self = serde_json::from_value(value).map_err(|_| ManifestError::Parse)?;
        manifest.validate()?;
        Ok(manifest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestPolicy {
    pub max_segment_bytes: u64,
    pub max_segment_age_millis: u64,
    pub retention: ManifestRetention,
}

impl ManifestPolicy {
    fn validate(&self) -> Result<(), ManifestError> {
        if self.max_segment_bytes == 0 || self.max_segment_age_millis == 0 {
            return Err(ManifestError::Contradiction("policy"));
        }
        if let ManifestRetention::DeleteAfter { min_age_seconds } = self.retention
            && min_age_seconds == 0
        {
            return Err(ManifestError::Contradiction("retention"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestRetention {
    Retain,
    DeleteAfter { min_age_seconds: u64 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarkerSummary {
    pub sequence: u64,
    pub segment_ordinal: u64,
    pub byte_offset: u64,
    pub digest: String,
}

impl MarkerSummary {
    fn validate(&self) -> Result<(), ManifestError> {
        validate_digest(&self.digest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WalManifestSegment {
    pub ordinal: u64,
    pub file_name: String,
    pub first_sequence: u64,
    pub last_sequence: Option<u64>,
    pub first_marker: Option<MarkerSummary>,
    pub last_marker: Option<MarkerSummary>,
    pub created_unix_nanos: i64,
    pub sealed_unix_nanos: Option<i64>,
    pub physical_length: u64,
    pub sha256: Option<String>,
    pub lifecycle: SegmentLifecycle,
    pub checkpoint: Option<CheckpointReference>,
    pub retention_eligible_at_unix_nanos: Option<i64>,
    pub deletion: SegmentDeletion,
}

impl WalManifestSegment {
    fn validate(&self) -> Result<(), ManifestError> {
        if self.file_name != segment_file_name(self.first_sequence)
            || self
                .last_sequence
                .is_some_and(|last| last < self.first_sequence)
            || self.physical_length == 0
        {
            return Err(ManifestError::Contradiction("segment"));
        }
        if let Some(marker) = &self.first_marker {
            marker.validate()?;
        }
        if let Some(marker) = &self.last_marker {
            marker.validate()?;
        }
        if let (Some(first), Some(last)) = (&self.first_marker, &self.last_marker)
            && (first.sequence > last.sequence
                || first.segment_ordinal != self.ordinal
                || last.segment_ordinal != self.ordinal)
        {
            return Err(ManifestError::Contradiction("marker"));
        }
        if let Some(digest) = &self.sha256 {
            validate_digest(digest)?;
        }
        if let Some(checkpoint) = &self.checkpoint {
            checkpoint.validate()?;
        }
        match self.lifecycle {
            SegmentLifecycle::Active => {
                if self.sha256.is_some()
                    || self.sealed_unix_nanos.is_some()
                    || self.checkpoint.is_some()
                {
                    return Err(ManifestError::Contradiction("active_segment"));
                }
            }
            SegmentLifecycle::Sealed => {
                if self.sha256.is_none() || self.sealed_unix_nanos.is_none() {
                    return Err(ManifestError::Contradiction("sealed_segment"));
                }
            }
            SegmentLifecycle::Processed => {
                if self.sha256.is_none() || self.checkpoint.is_none() {
                    return Err(ManifestError::Contradiction("processed_segment"));
                }
            }
            SegmentLifecycle::RetentionEligible => {
                if self.sha256.is_none()
                    || self.checkpoint.is_none()
                    || self.retention_eligible_at_unix_nanos.is_none()
                {
                    return Err(ManifestError::Contradiction("retention_segment"));
                }
            }
            SegmentLifecycle::Deleting => {
                if !matches!(self.deletion, SegmentDeletion::Deleting { .. }) {
                    return Err(ManifestError::Contradiction("deleting_segment"));
                }
            }
            SegmentLifecycle::Deleted => {
                if !matches!(self.deletion, SegmentDeletion::Deleted { .. }) {
                    return Err(ManifestError::Contradiction("deleted_segment"));
                }
            }
            SegmentLifecycle::Quarantined => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentLifecycle {
    Active,
    Sealed,
    Processed,
    RetentionEligible,
    Deleting,
    Deleted,
    Quarantined,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReference {
    pub digest: String,
    pub marker_sequence: u64,
    pub output_count: u64,
}

impl CheckpointReference {
    fn validate(&self) -> Result<(), ManifestError> {
        validate_digest(&self.digest)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
#[derive(Default)]
pub enum SegmentDeletion {
    #[default]
    NotRequested,
    Deleting {
        transaction_id: String,
    },
    Deleted {
        transaction_id: String,
        deleted_unix_nanos: i64,
    },
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("WAL manifest version is unsupported")]
    UnsupportedVersion,
    #[error("WAL manifest contains a contradictory {0}")]
    Contradiction(&'static str),
    #[error("WAL manifest contains overlapping segment ranges")]
    OverlappingRange,
    #[error("WAL manifest contains an invalid digest")]
    InvalidDigest,
    #[error("WAL manifest could not be parsed")]
    Parse,
    #[error("WAL manifest serialization failed")]
    Serialization,
    #[error("WAL manifest I/O failed")]
    Io,
    #[error("WAL manifest is missing")]
    Missing,
    #[error("WAL manifest reconciliation failed")]
    Reconciliation,
    #[error("WAL manifest claims data beyond recovery authority")]
    AuthorityAhead,
}

fn validate_digest(value: &str) -> Result<(), ManifestError> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(ManifestError::InvalidDigest);
    }
    Ok(())
}

pub fn write_manifest_atomic(
    wal_directory: impl AsRef<Path>,
    manifest: &WalManifestV1,
) -> Result<PathBuf, ManifestError> {
    let directory = wal_directory.as_ref();
    let bytes = manifest.encode()?;
    let destination = directory.join(WAL_MANIFEST_FILE);
    let temporary = directory.join(format!(".{WAL_MANIFEST_FILE}.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary).map_err(|_| ManifestError::Io)?;
        file.write_all(&bytes).map_err(|_| ManifestError::Io)?;
        file.sync_all().map_err(|_| ManifestError::Io)?;
        fs::rename(&temporary, &destination).map_err(|_| ManifestError::Io)?;
        File::open(directory)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ManifestError::Io)?;
        Ok(destination.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn load_manifest(wal_directory: impl AsRef<Path>) -> Result<WalManifestV1, ManifestError> {
    let path = wal_directory.as_ref().join(WAL_MANIFEST_FILE);
    let bytes = fs::read(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ManifestError::Missing
        } else {
            ManifestError::Io
        }
    })?;
    WalManifestV1::decode(&bytes)
}

pub fn rebuild_manifest(
    wal_directory: impl AsRef<Path>,
    recording_id: RecordingId,
    epoch_ordinal: u64,
    policy: ManifestPolicy,
) -> Result<WalManifestV1, ManifestError> {
    let wal_directory = wal_directory.as_ref();
    let segments_directory = wal_directory.join("segments");
    let discovered = super::discover_segments(&segments_directory, recording_id)
        .map_err(|_| ManifestError::Reconciliation)?;
    let scan = super::scan_wal(wal_directory, recording_id, super::DEFAULT_MAX_RECORD_BYTES)
        .map_err(|_| ManifestError::Reconciliation)?;
    let existing = match load_manifest(wal_directory) {
        Ok(manifest) => {
            if manifest.recording_id != recording_id || manifest.epoch_ordinal != epoch_ordinal {
                return Err(ManifestError::AuthorityAhead);
            }
            Some(manifest)
        }
        Err(ManifestError::Missing) => None,
        Err(error) => return Err(error),
    };
    if let Some(existing) = existing.as_ref() {
        validate_existing_not_ahead(existing, &discovered, &scan)?;
    }
    let rebuilt = build_manifest_from_scan(
        &discovered,
        &scan,
        recording_id,
        epoch_ordinal,
        policy,
        existing.as_ref(),
    )?;
    if let Some(existing) = existing
        && existing.segments == rebuilt.segments
        && existing.active_segment == rebuilt.active_segment
        && existing.authoritative_marker == rebuilt.authoritative_marker
        && existing.policy == rebuilt.policy
    {
        return Ok(existing);
    }
    Ok(rebuilt)
}

fn validate_existing_not_ahead(
    existing: &WalManifestV1,
    discovered: &[super::DiscoveredSegment],
    scan: &super::RecoveryScan,
) -> Result<(), ManifestError> {
    for segment in &existing.segments {
        let discovered_segment = discovered
            .iter()
            .find(|candidate| candidate.header.segment_ordinal == segment.ordinal)
            .ok_or(ManifestError::AuthorityAhead)?;
        if discovered_segment.header.first_sequence != segment.first_sequence {
            return Err(ManifestError::AuthorityAhead);
        }
        let actual_last = scan
            .committed
            .iter()
            .filter_map(|record| {
                record
                    .provenance
                    .as_ref()
                    .filter(|provenance| provenance.segment_ordinal == segment.ordinal)
                    .map(|_| record.sequence)
            })
            .max();
        if segment
            .last_sequence
            .is_some_and(|last| actual_last.is_none_or(|actual| last > actual))
        {
            return Err(ManifestError::AuthorityAhead);
        }
        if let Some(expected) = &segment.sha256 {
            let bytes =
                fs::read(&discovered_segment.path).map_err(|_| ManifestError::Reconciliation)?;
            if digest_bytes(&bytes)? != *expected {
                return Err(ManifestError::AuthorityAhead);
            }
        }
    }
    Ok(())
}

fn build_manifest_from_scan(
    discovered: &[super::DiscoveredSegment],
    scan: &super::RecoveryScan,
    recording_id: RecordingId,
    epoch_ordinal: u64,
    policy: ManifestPolicy,
    existing: Option<&WalManifestV1>,
) -> Result<WalManifestV1, ManifestError> {
    let mut committed_by_segment: BTreeMap<u64, Vec<&super::WalRecordEnvelope>> = BTreeMap::new();
    for envelope in &scan.committed {
        if let Some(provenance) = &envelope.provenance {
            committed_by_segment
                .entry(provenance.segment_ordinal)
                .or_default()
                .push(envelope);
        }
    }
    let active_ordinal = discovered
        .last()
        .map(|segment| segment.header.segment_ordinal);
    let mut segments = Vec::with_capacity(discovered.len());
    for (index, discovered_segment) in discovered.iter().enumerate() {
        let ordinal = discovered_segment.header.segment_ordinal;
        let records = committed_by_segment
            .get(&ordinal)
            .cloned()
            .unwrap_or_default();
        let last_sequence = records.iter().map(|record| record.sequence).max();
        if last_sequence.is_none() && Some(ordinal) != active_ordinal {
            return Err(ManifestError::Reconciliation);
        }
        let metadata =
            fs::metadata(&discovered_segment.path).map_err(|_| ManifestError::Reconciliation)?;
        let is_active = Some(ordinal) == active_ordinal;
        let (sha256, sealed_unix_nanos, lifecycle) = if is_active {
            (None, None, SegmentLifecycle::Active)
        } else {
            let bytes =
                fs::read(&discovered_segment.path).map_err(|_| ManifestError::Reconciliation)?;
            let digest = Sha256::digest(bytes);
            (
                Some(digest.iter().map(|byte| format!("{byte:02x}")).collect()),
                Some(discovered_segment.header.created_unix_nanos),
                SegmentLifecycle::Sealed,
            )
        };
        let markers = records
            .iter()
            .filter(|record| record.kind == super::RecordKind::CommitMarker)
            .filter_map(|record| {
                record
                    .provenance
                    .as_ref()
                    .map(|provenance| (record, provenance))
            })
            .map(
                |(record, provenance)| -> Result<MarkerSummary, ManifestError> {
                    let encoded = super::encode_envelope(record)
                        .map_err(|_| ManifestError::Reconciliation)?;
                    Ok(MarkerSummary {
                        sequence: record.sequence,
                        segment_ordinal: provenance.segment_ordinal,
                        byte_offset: provenance.byte_offset,
                        digest: digest_bytes(&encoded)?,
                    })
                },
            )
            .collect::<Result<Vec<_>, ManifestError>>()?;
        segments.push(WalManifestSegment {
            ordinal,
            file_name: super::segment_file_name(discovered_segment.header.first_sequence),
            first_sequence: discovered_segment.header.first_sequence,
            last_sequence,
            first_marker: markers.first().cloned(),
            last_marker: markers.last().cloned(),
            created_unix_nanos: discovered_segment.header.created_unix_nanos,
            sealed_unix_nanos,
            physical_length: metadata.len(),
            sha256,
            lifecycle,
            checkpoint: existing
                .and_then(|manifest| manifest.segments.get(index))
                .and_then(|segment| segment.checkpoint.clone()),
            retention_eligible_at_unix_nanos: None,
            deletion: existing
                .and_then(|manifest| manifest.segments.get(index))
                .map_or(SegmentDeletion::NotRequested, |segment| {
                    segment.deletion.clone()
                }),
        });
    }
    let authoritative_marker = match scan.final_marker.as_ref() {
        Some(marker) => match marker.provenance.as_ref() {
            Some(provenance) => {
                let encoded =
                    super::encode_envelope(marker).map_err(|_| ManifestError::Reconciliation)?;
                Some(MarkerSummary {
                    sequence: marker.sequence,
                    segment_ordinal: provenance.segment_ordinal,
                    byte_offset: provenance.byte_offset,
                    digest: digest_bytes(&encoded)?,
                })
            }
            None => return Err(ManifestError::AuthorityAhead),
        },
        None => None,
    };
    let revision = existing.map_or(0, |manifest| manifest.revision.saturating_add(1));
    let predecessor_revision = existing.map(|manifest| manifest.revision);
    let manifest = WalManifestV1 {
        version: WAL_MANIFEST_SCHEMA_VERSION,
        revision,
        predecessor_revision,
        recording_id,
        epoch_ordinal,
        policy,
        active_segment: active_ordinal,
        authoritative_marker,
        segments,
    };
    manifest.validate()?;
    Ok(manifest)
}

fn digest_bytes(bytes: &[u8]) -> Result<String, ManifestError> {
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub fn capture_sealed_segment(
    path: impl AsRef<Path>,
    ordinal: u64,
    first_sequence: u64,
    last_sequence: u64,
    created_unix_nanos: i64,
    sealed_unix_nanos: i64,
) -> Result<WalManifestSegment, ManifestError> {
    let path = path.as_ref();
    let bytes = fs::read(path).map_err(|_| ManifestError::Io)?;
    if bytes.is_empty() {
        return Err(ManifestError::Contradiction("segment"));
    }
    let digest = Sha256::digest(&bytes);
    Ok(WalManifestSegment {
        ordinal,
        file_name: segment_file_name(first_sequence),
        first_sequence,
        last_sequence: Some(last_sequence),
        first_marker: None,
        last_marker: None,
        created_unix_nanos,
        sealed_unix_nanos: Some(sealed_unix_nanos),
        physical_length: u64::try_from(bytes.len()).map_err(|_| ManifestError::Io)?,
        sha256: Some(digest.iter().map(|byte| format!("{byte:02x}")).collect()),
        lifecycle: SegmentLifecycle::Sealed,
        checkpoint: None,
        retention_eligible_at_unix_nanos: None,
        deletion: SegmentDeletion::NotRequested,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(ordinal: u64, first: u64, lifecycle: SegmentLifecycle) -> WalManifestSegment {
        WalManifestSegment {
            ordinal,
            file_name: segment_file_name(first),
            first_sequence: first,
            last_sequence: Some(first + 1),
            first_marker: None,
            last_marker: None,
            created_unix_nanos: 1,
            sealed_unix_nanos: (lifecycle != SegmentLifecycle::Active).then_some(2),
            physical_length: 64,
            sha256: (lifecycle != SegmentLifecycle::Active).then_some("a".repeat(64)),
            lifecycle,
            checkpoint: None,
            retention_eligible_at_unix_nanos: None,
            deletion: SegmentDeletion::NotRequested,
        }
    }

    #[test]
    fn manifest_round_trip_and_order_validation() {
        let value = WalManifestV1 {
            version: 1,
            revision: 2,
            predecessor_revision: Some(1),
            recording_id: RecordingId::new(),
            epoch_ordinal: 4,
            policy: ManifestPolicy {
                max_segment_bytes: 1024,
                max_segment_age_millis: 300_000,
                retention: ManifestRetention::Retain,
            },
            active_segment: Some(1),
            authoritative_marker: None,
            segments: vec![
                segment(0, 1, SegmentLifecycle::Sealed),
                segment(1, 3, SegmentLifecycle::Active),
            ],
        };
        let decoded = WalManifestV1::decode(&value.encode().unwrap()).unwrap();
        assert_eq!(decoded, value);
        let mut invalid = value;
        invalid.segments[1].first_sequence = 2;
        invalid.segments[1].file_name = segment_file_name(2);
        assert_eq!(invalid.validate(), Err(ManifestError::OverlappingRange));
    }

    #[cfg(any(target_os = "linux", target_vendor = "apple"))]
    #[test]
    fn rebuild_manifest_is_idempotent_and_does_not_mutate_wal() {
        let root = std::env::temp_dir().join(format!("chronicle-rebuild-{}", uuid::Uuid::new_v4()));
        let recording_id = RecordingId::new();
        let mut writer = crate::GroupCommitWalWriter::create(
            &root,
            recording_id,
            crate::MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        writer
            .append(crate::RecordKind::CaptureEvent, 1, 0, b"data".to_vec(), 0)
            .unwrap();
        writer.shutdown(10).unwrap();
        drop(writer);
        let before = fs::read(root.join("segments").join(segment_file_name(1))).unwrap();
        let policy = ManifestPolicy {
            max_segment_bytes: crate::MIN_SEGMENT_BYTES,
            max_segment_age_millis: crate::DEFAULT_SEGMENT_AGE_MILLIS,
            retention: ManifestRetention::Retain,
        };
        let first = rebuild_manifest(&root, recording_id, 0, policy.clone()).unwrap();
        write_manifest_atomic(&root, &first).unwrap();
        let second = rebuild_manifest(&root, recording_id, 0, policy).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs::read(root.join("segments").join(segment_file_name(1))).unwrap(),
            before
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn atomic_manifest_and_sealed_digest_round_trip() {
        let root =
            std::env::temp_dir().join(format!("chronicle-manifest-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let segment_path = root.join(segment_file_name(1));
        std::fs::write(&segment_path, b"sealed").unwrap();
        let sealed = capture_sealed_segment(&segment_path, 0, 1, 2, 1, 2).unwrap();
        assert_eq!(sealed.physical_length, 6);
        let manifest = WalManifestV1 {
            version: 1,
            revision: 1,
            predecessor_revision: None,
            recording_id: RecordingId::new(),
            epoch_ordinal: 0,
            policy: ManifestPolicy {
                max_segment_bytes: 1024,
                max_segment_age_millis: 1,
                retention: ManifestRetention::Retain,
            },
            active_segment: None,
            authoritative_marker: None,
            segments: vec![sealed],
        };
        write_manifest_atomic(&root, &manifest).unwrap();
        assert_eq!(load_manifest(&root).unwrap(), manifest);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn non_v1_is_rejected_before_payload_deserialization() {
        let json = br#"{"version":2,"recording_id":"not-a-uuid","segments":[]}"#;
        assert_eq!(
            WalManifestV1::decode(json),
            Err(ManifestError::UnsupportedVersion)
        );
    }

    #[test]
    fn leading_manifest_claim_is_rejected() {
        let mut value = WalManifestV1 {
            version: 1,
            revision: 1,
            predecessor_revision: None,
            recording_id: RecordingId::new(),
            epoch_ordinal: 0,
            policy: ManifestPolicy {
                max_segment_bytes: 1024,
                max_segment_age_millis: 1,
                retention: ManifestRetention::Retain,
            },
            active_segment: Some(0),
            authoritative_marker: None,
            segments: vec![segment(0, 1, SegmentLifecycle::Active)],
        };
        value.authoritative_marker = Some(MarkerSummary {
            sequence: 3,
            segment_ordinal: 0,
            byte_offset: 64,
            digest: "b".repeat(64),
        });
        assert_eq!(
            value.validate(),
            Err(ManifestError::Contradiction("authoritative_marker"))
        );
    }
}
