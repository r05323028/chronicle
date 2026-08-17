//! Commit-marker planning and durable lag projection for incremental ETL.

use crate::LagSummary;
use chronicle_common::{EpochId, RecordingId};
use chronicle_etl::IncrementalEtlCheckpoint;
use chronicle_wal::{
    CommittedRecordSnapshot, RecordKind, WalError, WalRecordEnvelope, decode_commit_marker,
    encode_envelope,
};
use sha2::{Digest, Sha256};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub(crate) enum IncrementalRuntimeError {
    #[error(transparent)]
    Wal(#[from] WalError),
    #[error("incremental checkpoint marker {0} is absent from committed WAL")]
    MissingCheckpointMarker(u64),
    #[error("incremental checkpoint marker {0} is not a valid commit marker")]
    InvalidCheckpointMarker(u64),
    #[error("incremental snapshot has no complete commit marker")]
    MissingCommitMarker,
    #[error("incremental checkpoint lineage mismatch: {0}")]
    CheckpointLineage(&'static str),
}

#[derive(Debug)]
pub(crate) struct PlannedIncrementalBatch {
    pub envelopes: Vec<WalRecordEnvelope>,
    pub marker_sequence: u64,
    pub marker_segment_ordinal: u64,
    pub marker_digest: [u8; 32],
    pub segment_lineage: Vec<chronicle_wal::CommittedSegment>,
    pub lag_before: LagSummary,
    pub lag_after: LagSummary,
}

pub(crate) fn validate_checkpoint(
    snapshot: &CommittedRecordSnapshot,
    checkpoint: &IncrementalEtlCheckpoint,
    wal_directory: &Path,
    recording_id: RecordingId,
) -> Result<(), IncrementalRuntimeError> {
    if checkpoint.epoch_id != EpochId::from_uuid(recording_id.0)
        || checkpoint.marker.sequence > snapshot.snapshot.marker_sequence
    {
        return Err(IncrementalRuntimeError::CheckpointLineage(
            "epoch or marker sequence",
        ));
    }
    let marker = snapshot
        .envelopes
        .iter()
        .find(|envelope| envelope.sequence == checkpoint.marker.sequence)
        .ok_or(IncrementalRuntimeError::MissingCheckpointMarker(
            checkpoint.marker.sequence,
        ))?;
    let marker_digest: [u8; 32] = Sha256::digest(encode_envelope(marker)?).into();
    if digest_hex(&marker_digest) != checkpoint.marker.marker_digest {
        return Err(IncrementalRuntimeError::CheckpointLineage("marker digest"));
    }
    let active_segment_digest = chronicle_wal::verified_segment_prefix_sha256(
        wal_directory,
        recording_id,
        &snapshot.envelopes,
        checkpoint.marker.segment_ordinal,
        checkpoint.marker.sequence,
    )?;
    if digest_hex(&active_segment_digest) != checkpoint.marker.active_segment_digest {
        return Err(IncrementalRuntimeError::CheckpointLineage(
            "active segment digest",
        ));
    }
    Ok(())
}

pub(crate) fn plan_next_batch(
    snapshot: &CommittedRecordSnapshot,
    checkpoint: Option<&IncrementalEtlCheckpoint>,
    batch_records: u64,
    now_millis: u64,
) -> Result<Option<PlannedIncrementalBatch>, IncrementalRuntimeError> {
    let lag_before = lag_summary(snapshot, checkpoint, now_millis)?;
    let start = checkpoint
        .map(|checkpoint| {
            snapshot
                .envelopes
                .iter()
                .position(|envelope| envelope.sequence == checkpoint.marker.sequence)
                .map(|index| index.saturating_add(1))
                .ok_or(IncrementalRuntimeError::MissingCheckpointMarker(
                    checkpoint.marker.sequence,
                ))
        })
        .transpose()?
        .unwrap_or(0);

    let mut new_records = 0_u64;
    let mut end = None;
    let mut last_complete_marker = None;
    for (index, envelope) in snapshot.envelopes.iter().enumerate().skip(start) {
        if envelope.kind != RecordKind::CommitMarker {
            new_records = new_records.saturating_add(1);
            continue;
        }
        decode_commit_marker(&envelope.payload)
            .map_err(|_| IncrementalRuntimeError::MissingCommitMarker)?;
        if new_records > 0 {
            last_complete_marker = Some(index);
        }
        if new_records >= batch_records {
            end = Some(index);
            break;
        }
    }
    let Some(end) = end.or(last_complete_marker) else {
        return Ok(None);
    };
    let marker = snapshot
        .envelopes
        .get(end)
        .ok_or(IncrementalRuntimeError::MissingCommitMarker)?;
    let marker_provenance = marker
        .provenance
        .as_ref()
        .ok_or(IncrementalRuntimeError::MissingCommitMarker)?;
    let marker_counts = decode_commit_marker(&marker.payload)
        .map_err(|_| IncrementalRuntimeError::MissingCommitMarker)?;
    let marker_digest: [u8; 32] = Sha256::digest(encode_envelope(marker)?).into();
    let segment_lineage = snapshot
        .snapshot
        .segment_lineage
        .iter()
        .filter(|segment| segment.last_sequence <= marker.sequence)
        .cloned()
        .collect();
    let lag_after = lag_summary_for_progress(
        snapshot,
        marker.sequence,
        marker_counts.durable_record_count,
        marker_counts.durable_payload_bytes,
        now_millis,
    );
    Ok(Some(PlannedIncrementalBatch {
        envelopes: snapshot.envelopes[..=end].to_vec(),
        marker_sequence: marker.sequence,
        marker_segment_ordinal: marker_provenance.segment_ordinal,
        marker_digest,
        segment_lineage,
        lag_before,
        lag_after,
    }))
}

pub(crate) fn lag_summary(
    snapshot: &CommittedRecordSnapshot,
    checkpoint: Option<&IncrementalEtlCheckpoint>,
    now_millis: u64,
) -> Result<LagSummary, IncrementalRuntimeError> {
    let Some(checkpoint) = checkpoint else {
        return Ok(lag_summary_for_progress(snapshot, 0, 0, 0, now_millis));
    };
    let marker = snapshot
        .envelopes
        .iter()
        .find(|envelope| envelope.sequence == checkpoint.marker.sequence)
        .ok_or(IncrementalRuntimeError::MissingCheckpointMarker(
            checkpoint.marker.sequence,
        ))?;
    let marker_counts = decode_commit_marker(&marker.payload).map_err(|_| {
        IncrementalRuntimeError::InvalidCheckpointMarker(checkpoint.marker.sequence)
    })?;
    Ok(lag_summary_for_progress(
        snapshot,
        checkpoint.marker.sequence,
        marker_counts.durable_record_count,
        marker_counts.durable_payload_bytes,
        now_millis,
    ))
}

fn lag_summary_for_progress(
    snapshot: &CommittedRecordSnapshot,
    processed_marker_sequence: u64,
    processed_records: u64,
    processed_bytes: u64,
    now_millis: u64,
) -> LagSummary {
    let records = snapshot
        .snapshot
        .committed_record_count
        .saturating_sub(processed_records);
    let bytes = snapshot
        .snapshot
        .committed_payload_bytes
        .saturating_sub(processed_bytes);
    if records == 0 && bytes == 0 {
        return LagSummary {
            records: 0,
            bytes: 0,
            age_seconds: 0,
        };
    }
    let age_seconds = snapshot
        .envelopes
        .iter()
        .find(|envelope| {
            envelope.sequence > processed_marker_sequence
                && envelope.kind != RecordKind::CommitMarker
        })
        .and_then(|envelope| envelope.provenance.as_ref())
        .and_then(|provenance| segment_created_nanos(snapshot, provenance.segment_ordinal))
        .map_or(0, |created| {
            let created_millis = u64::try_from(created.max(0)).unwrap_or(0) / 1_000_000;
            now_millis.saturating_sub(created_millis) / 1_000
        });
    LagSummary {
        records,
        bytes,
        age_seconds,
    }
}

fn digest_hex(value: &[u8; 32]) -> String {
    let mut output = String::with_capacity(64);
    for byte in value {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn segment_created_nanos(snapshot: &CommittedRecordSnapshot, ordinal: u64) -> Option<i64> {
    if ordinal == snapshot.snapshot.marker_segment_ordinal {
        return Some(snapshot.snapshot.marker_segment_created_unix_nanos);
    }
    snapshot
        .snapshot
        .segment_lineage
        .iter()
        .find(|segment| segment.ordinal == ordinal)
        .map(|segment| segment.created_unix_nanos)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_common::{EpochId, RecordingId};
    use chronicle_etl::{
        CheckpointLifecycle, CheckpointOwner, ContinuationDependency, DecoderReconstructionState,
        ETL_PIPELINE_VERSION, MarkerLineage, SourceStatus,
    };
    use chronicle_wal::{CommitMarker, WalRecordProvenance, encode_commit_marker};
    use uuid::Uuid;

    fn event(recording_id: RecordingId, sequence: u64) -> WalRecordEnvelope {
        WalRecordEnvelope::unplaced(
            recording_id,
            sequence,
            RecordKind::CaptureEvent,
            1,
            0,
            vec![1],
        )
        .with_provenance(WalRecordProvenance {
            segment_ordinal: 0,
            segment_first_sequence: 1,
            byte_offset: sequence * 100,
        })
    }

    fn marker(
        recording_id: RecordingId,
        sequence: u64,
        last_data: u64,
        count: u64,
    ) -> WalRecordEnvelope {
        WalRecordEnvelope::unplaced(
            recording_id,
            sequence,
            RecordKind::CommitMarker,
            chronicle_wal::COMMIT_MARKER_VERSION,
            0,
            encode_commit_marker(&CommitMarker {
                durable_through_sequence: last_data,
                durable_record_count: count,
                durable_payload_bytes: count,
                batch_first_sequence: last_data.saturating_sub(1),
                batch_last_sequence: last_data,
                batch_sha256: [0; 32],
            })
            .to_vec(),
        )
        .with_provenance(WalRecordProvenance {
            segment_ordinal: 0,
            segment_first_sequence: 1,
            byte_offset: sequence * 100,
        })
    }

    fn snapshot() -> CommittedRecordSnapshot {
        let recording_id = RecordingId::new();
        let envelopes = vec![
            event(recording_id, 1),
            event(recording_id, 2),
            marker(recording_id, 3, 2, 2),
            event(recording_id, 4),
            event(recording_id, 5),
            marker(recording_id, 6, 5, 4),
        ];
        CommittedRecordSnapshot {
            snapshot: chronicle_wal::CommittedSnapshot {
                recording_id,
                marker_sequence: 6,
                durable_through_sequence: 5,
                marker_segment_ordinal: 0,
                marker_byte_offset: 600,
                marker_segment_created_unix_nanos: 0,
                marker_digest: [6; 32],
                input_digest: [7; 32],
                segment_count: 1,
                segment_lineage: vec![chronicle_wal::CommittedSegment {
                    ordinal: 0,
                    digest: [8; 32],
                    first_sequence: 1,
                    last_sequence: 6,
                    created_unix_nanos: 0,
                }],
                committed_record_count: 4,
                committed_payload_bytes: 4,
            },
            envelopes,
        }
    }

    fn checkpoint(marker_sequence: u64) -> IncrementalEtlCheckpoint {
        let epoch_id = EpochId::from_uuid(Uuid::new_v4());
        IncrementalEtlCheckpoint {
            version: chronicle_etl::INCREMENTAL_CHECKPOINT_SCHEMA_VERSION,
            owner: CheckpointOwner::Etl,
            lifecycle: CheckpointLifecycle::Active,
            parent_id: RecordingId::new(),
            epoch_id,
            epoch_ordinal: 0,
            config_digest: "a".repeat(64),
            pipeline_version: ETL_PIPELINE_VERSION.to_string(),
            canonical_schema_version: chronicle_canonical::CANONICAL_SCHEMA_VERSION,
            marker: MarkerLineage {
                sequence: marker_sequence,
                segment_ordinal: 0,
                marker_digest: "b".repeat(64),
                active_segment_digest: "c".repeat(64),
            },
            segment_lineage: Vec::new(),
            continuation: ContinuationDependency::pending(
                EpochId::from_uuid(Uuid::new_v4()),
                epoch_id,
            ),
            decoder: DecoderReconstructionState {
                kind: chronicle_etl::DECODER_KIND.to_owned(),
                implementation_version: chronicle_etl::DECODER_IMPLEMENTATION_VERSION,
                snapshot_version: chronicle_etl::DECODER_SNAPSHOT_VERSION,
                session_id: Uuid::new_v4(),
                recording_id: epoch_id.as_uuid(),
                epoch_ordinal: 0,
                marker_sequence,
                marker_digest: "b".repeat(64),
                state: Vec::new(),
                pending_connection_count: 0,
                serialized_bytes: 0,
            },
            outputs: Vec::new(),
            published_operation_keys: Vec::new(),
            source_status: SourceStatus::Live,
            checksum: String::new(),
        }
    }

    #[test]
    fn planner_uses_complete_marker_batches_and_durable_lag() {
        let snapshot = snapshot();
        let first = plan_next_batch(&snapshot, None, 2, 2_000).unwrap().unwrap();
        assert_eq!(first.marker_sequence, 3);
        assert_eq!(first.envelopes.last().unwrap().sequence, 3);
        assert_eq!(
            first.lag_before,
            LagSummary {
                records: 4,
                bytes: 4,
                age_seconds: 2
            }
        );
        assert_eq!(
            first.lag_after,
            LagSummary {
                records: 2,
                bytes: 2,
                age_seconds: 2
            }
        );

        let checkpoint = checkpoint(3);
        let second = plan_next_batch(&snapshot, Some(&checkpoint), 2, 2_000)
            .unwrap()
            .unwrap();
        assert_eq!(second.marker_sequence, 6);
        assert_eq!(second.lag_before.records, 2);
        assert_eq!(
            second.lag_after,
            LagSummary {
                records: 0,
                bytes: 0,
                age_seconds: 0
            }
        );
    }

    #[test]
    fn planner_finishes_small_final_batch_without_splitting() {
        let snapshot = snapshot();
        let planned = plan_next_batch(&snapshot, None, 3, 2_000).unwrap().unwrap();
        assert_eq!(planned.marker_sequence, 6);
        assert_eq!(planned.lag_after.records, 0);
    }
}
