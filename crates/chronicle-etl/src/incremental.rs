//! Recorder-owned incremental snapshot processing boundary.
#![allow(clippy::format_collect, clippy::needless_pass_by_value)]

use crate::{
    DECODER_IMPLEMENTATION_VERSION, DECODER_KIND, DECODER_SNAPSHOT_VERSION,
    DecoderReconstructionState, EtlError, EtlEvidence, EtlIssue, EtlOutput, EtlPipeline,
    SegmentLineage,
};
use chronicle_canonical::CanonicalOperation;
use chronicle_common::{OperationId, SessionId};
use chronicle_protocol::ProtocolRegistry;
use chronicle_session::{ReconstructionAssembler, ReconstructionLimits, SessionLimits};
use chronicle_wal::WalRecordEnvelope;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct CommittedWalSnapshot<'a> {
    pub marker_sequence: u64,
    pub marker_digest: [u8; 32],
    pub envelopes: &'a [WalRecordEnvelope],
    pub segment_lineage: &'a [chronicle_wal::CommittedSegment],
}

#[derive(Debug)]
pub struct IncrementalResult {
    pub output: EtlOutput,
    pub first_marker_sequence: u64,
    pub last_marker_sequence: u64,
    pub segment_lineage: Vec<SegmentLineage>,
}

pub struct IncrementalProcessorState {
    assembler: ReconstructionAssembler,
    evidence: EtlEvidence,
    issues: Vec<EtlIssue>,
    segment_lineage: Vec<SegmentLineage>,
    emitted_operation_keys: BTreeSet<String>,
    last_marker_sequence: Option<u64>,
    last_marker_digest: Option<String>,
}

pub struct IncrementalProcessor {
    pipeline: EtlPipeline,
    limits: ReconstructionLimits,
    assembler: ReconstructionAssembler,
    evidence: EtlEvidence,
    issues: Vec<EtlIssue>,
    segment_lineage: Vec<SegmentLineage>,
    emitted_operation_keys: BTreeSet<String>,
    last_marker_sequence: Option<u64>,
    last_marker_digest: Option<String>,
}

impl IncrementalProcessor {
    pub fn new(limits: SessionLimits) -> Self {
        let reconstruction_limits = ReconstructionLimits {
            max_active_connections: limits.max_connections,
            max_bytes_per_connection: limits.max_bytes_per_connection,
            max_chunks_per_connection: limits.max_chunks_per_connection,
            ..ReconstructionLimits::default()
        };
        Self {
            pipeline: EtlPipeline::new(limits),
            limits: reconstruction_limits,
            assembler: ReconstructionAssembler::new(reconstruction_limits),
            evidence: EtlEvidence::default(),
            issues: Vec::new(),
            segment_lineage: Vec::new(),
            emitted_operation_keys: BTreeSet::new(),
            last_marker_sequence: None,
            last_marker_digest: None,
        }
    }

    #[allow(clippy::too_many_lines)]
    pub fn process_snapshot(
        &mut self,
        snapshot: CommittedWalSnapshot<'_>,
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<IncrementalResult, EtlError> {
        if self
            .last_marker_sequence
            .is_some_and(|previous| snapshot.marker_sequence < previous)
        {
            return Err(EtlError::SnapshotRegression);
        }
        let snapshot_marker_digest = digest_hex(&snapshot.marker_digest);
        if self.last_marker_sequence == Some(snapshot.marker_sequence)
            && self.last_marker_digest.as_deref() != Some(snapshot_marker_digest.as_str())
        {
            return Err(EtlError::SnapshotLineageMismatch);
        }
        let first_marker_sequence = self
            .last_marker_sequence
            .map_or(0, |sequence| sequence.saturating_add(1));
        let start = self.last_marker_sequence.map_or(0, |previous| {
            snapshot
                .envelopes
                .iter()
                .position(|envelope| envelope.sequence > previous)
                .unwrap_or(snapshot.envelopes.len())
        });
        let prior_cursor = self.last_marker_sequence.unwrap_or(0);
        if start == snapshot.envelopes.len() && snapshot.marker_sequence != prior_cursor {
            return Err(EtlError::SnapshotGap);
        }
        if let Some(previous) = self.last_marker_sequence {
            let expected = previous.saturating_add(1);
            if snapshot.envelopes[start..]
                .first()
                .is_some_and(|envelope| envelope.sequence != expected)
            {
                return Err(EtlError::SnapshotGap);
            }
        } else if snapshot.envelopes[start..].first().is_some_and(|envelope| {
            envelope
                .provenance
                .as_ref()
                .is_some_and(|provenance| envelope.sequence != provenance.segment_first_sequence)
        }) {
            return Err(EtlError::SnapshotGap);
        }
        if !snapshot.envelopes[start..].is_empty()
            && snapshot.envelopes.last().map(|envelope| envelope.sequence)
                != Some(snapshot.marker_sequence)
        {
            return Err(EtlError::SnapshotGap);
        }
        if snapshot.envelopes[start..]
            .windows(2)
            .any(|pair| pair[1].sequence != pair[0].sequence.saturating_add(1))
        {
            return Err(EtlError::SnapshotGap);
        }
        if self.last_marker_sequence.is_some()
            && (self.segment_lineage.len() > snapshot.segment_lineage.len()
                || self
                    .segment_lineage
                    .iter()
                    .zip(snapshot.segment_lineage)
                    .any(|(previous, current)| {
                        previous.ordinal != current.ordinal
                            || previous.first_sequence != current.first_sequence
                            || previous.last_sequence > current.last_sequence
                            || (previous.last_sequence == current.last_sequence
                                && previous.digest != hex_bytes(&current.digest))
                    }))
        {
            return Err(EtlError::SnapshotLineageMismatch);
        }

        // Work on clones. Publication failure can restore the exact pre-batch
        // reconstruction state instead of rolling back only the WAL marker.
        let mut assembler = self.assembler.clone();
        let mut evidence = self.evidence.clone();
        let mut issues = self.issues.clone();
        for envelope in &snapshot.envelopes[start..] {
            crate::ingest_reconstructed_envelope(
                envelope,
                &mut assembler,
                &mut evidence,
                &mut issues,
                self.pipeline.max_issues,
            )?;
        }
        let mut output = self.pipeline.finish_reconstructed(
            assembler.clone().finish(),
            issues.clone(),
            evidence.clone(),
            registry,
            session_id,
        )?;
        let mut emitted_operation_keys = self.emitted_operation_keys.clone();
        for connection in &mut output.session.connections {
            connection.operations.retain_mut(|operation| {
                let key = operation_key(operation);
                if emitted_operation_keys.insert(key.clone()) {
                    operation.id = operation_id_from_key(&key);
                    true
                } else {
                    false
                }
            });
        }
        self.assembler = assembler;
        self.evidence = evidence;
        self.issues = issues;
        self.emitted_operation_keys = emitted_operation_keys;
        self.segment_lineage = snapshot
            .segment_lineage
            .iter()
            .map(|segment| SegmentLineage {
                ordinal: segment.ordinal,
                digest: hex_bytes(&segment.digest),
                first_sequence: segment.first_sequence,
                last_sequence: segment.last_sequence,
            })
            .collect();
        self.last_marker_sequence = Some(snapshot.marker_sequence);
        self.last_marker_digest = Some(snapshot_marker_digest);
        Ok(IncrementalResult {
            output,
            first_marker_sequence,
            last_marker_sequence: snapshot.marker_sequence,
            segment_lineage: self.segment_lineage.clone(),
        })
    }

    pub fn finalize(
        &self,
        registry: &ProtocolRegistry,
        session_id: SessionId,
    ) -> Result<EtlOutput, EtlError> {
        self.pipeline.finish_reconstructed(
            self.assembler.clone().finish(),
            self.issues.clone(),
            self.evidence.clone(),
            registry,
            session_id,
        )
    }

    pub fn last_marker_sequence(&self) -> Option<u64> {
        self.last_marker_sequence
    }

    pub fn marker_digest_matches(&self, digest: &[u8; 32]) -> bool {
        self.last_marker_digest.as_deref() == Some(digest_hex(digest).as_str())
    }

    pub fn state_snapshot(&self) -> IncrementalProcessorState {
        IncrementalProcessorState {
            assembler: self.assembler.clone(),
            evidence: self.evidence.clone(),
            issues: self.issues.clone(),
            segment_lineage: self.segment_lineage.clone(),
            emitted_operation_keys: self.emitted_operation_keys.clone(),
            last_marker_sequence: self.last_marker_sequence,
            last_marker_digest: self.last_marker_digest.clone(),
        }
    }

    pub fn restore_state(&mut self, state: IncrementalProcessorState) {
        self.assembler = state.assembler;
        self.evidence = state.evidence;
        self.issues = state.issues;
        self.segment_lineage = state.segment_lineage;
        self.emitted_operation_keys = state.emitted_operation_keys;
        self.last_marker_sequence = state.last_marker_sequence;
        self.last_marker_digest = state.last_marker_digest;
    }

    /// Return exact mutable reconstruction bytes for checkpoint publication.
    pub fn decoder_snapshot(&self) -> Result<Vec<u8>, EtlError> {
        Ok(self.assembler.snapshot()?)
    }

    pub fn published_operation_keys(&self) -> Vec<String> {
        self.emitted_operation_keys.iter().cloned().collect()
    }

    pub fn restore_publication_keys(&mut self, keys: Vec<String>) -> Result<(), EtlError> {
        if keys
            .iter()
            .any(|key| key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(EtlError::DecoderCheckpointMismatch);
        }
        self.emitted_operation_keys = keys.into_iter().collect();
        Ok(())
    }

    pub fn restore_segment_lineage(&mut self, lineage: Vec<SegmentLineage>) {
        self.segment_lineage = lineage;
    }

    pub fn segment_lineage(&self) -> &[SegmentLineage] {
        &self.segment_lineage
    }

    pub fn restore_decoder_snapshot(
        &mut self,
        state: &[u8],
        marker_sequence: Option<u64>,
        segment_lineage: Vec<SegmentLineage>,
    ) -> Result<(), EtlError> {
        self.assembler = ReconstructionAssembler::restore(self.limits, state)?;
        self.evidence = EtlEvidence::default();
        self.issues.clear();
        self.segment_lineage = segment_lineage;
        self.last_marker_sequence = marker_sequence;
        self.last_marker_digest = None;
        Ok(())
    }

    pub fn decoder_state(
        &self,
        session_id: SessionId,
        recording_id: Uuid,
        epoch_ordinal: u64,
        marker_sequence: u64,
        marker_digest: String,
    ) -> Result<DecoderReconstructionState, EtlError> {
        let state = self.decoder_snapshot()?;
        Ok(DecoderReconstructionState {
            kind: DECODER_KIND.into(),
            implementation_version: DECODER_IMPLEMENTATION_VERSION,
            snapshot_version: DECODER_SNAPSHOT_VERSION,
            session_id: session_id.0,
            recording_id,
            epoch_ordinal,
            marker_sequence,
            marker_digest,
            state: state.clone(),
            pending_connection_count: self.assembler.pending_connection_count(),
            serialized_bytes: state.len(),
        })
    }

    pub fn restore_decoder_state(
        &mut self,
        decoder: &DecoderReconstructionState,
        session_id: SessionId,
        recording_id: Uuid,
        epoch_ordinal: u64,
        marker_sequence: u64,
        marker_digest: &str,
    ) -> Result<(), EtlError> {
        if decoder.kind != DECODER_KIND
            || decoder.implementation_version != DECODER_IMPLEMENTATION_VERSION
            || decoder.snapshot_version != DECODER_SNAPSHOT_VERSION
            || decoder.session_id != session_id.0
            || decoder.recording_id != recording_id
            || decoder.epoch_ordinal != epoch_ordinal
            || decoder.marker_sequence != marker_sequence
            || decoder.marker_digest != marker_digest
            || decoder.serialized_bytes != decoder.state.len()
        {
            return Err(EtlError::DecoderCheckpointMismatch);
        }
        self.assembler = ReconstructionAssembler::restore(self.limits, &decoder.state)?;
        self.last_marker_sequence = Some(marker_sequence);
        self.last_marker_digest = Some(marker_digest.to_owned());
        self.evidence = EtlEvidence::default();
        self.issues.clear();
        Ok(())
    }

    pub fn restore_marker_sequence(&mut self, marker_sequence: u64) {
        self.last_marker_sequence = Some(marker_sequence);
        self.last_marker_digest = None;
    }

    pub fn clear_marker_sequence(&mut self) {
        self.last_marker_sequence = None;
        self.last_marker_digest = None;
        self.segment_lineage.clear();
        self.emitted_operation_keys.clear();
        self.assembler = ReconstructionAssembler::new(self.limits);
        self.evidence = EtlEvidence::default();
        self.issues.clear();
    }
}

fn operation_id_from_key(key: &str) -> OperationId {
    let digest = Sha256::digest(key.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    OperationId(Uuid::from_bytes(bytes))
}

fn operation_key(operation: &CanonicalOperation) -> String {
    let mut normalized = operation.clone();
    normalized.id = OperationId(uuid::Uuid::nil());
    let bytes = serde_json::to_vec(&normalized).unwrap_or_default();
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn hex_bytes(value: &[u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digest_hex(value: &[u8; 32]) -> String {
    hex_bytes(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_committed_snapshots_do_not_advance_without_records() {
        let mut processor = IncrementalProcessor::new(SessionLimits::default());
        let registry = ProtocolRegistry::default();
        let first = processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 0,
                    marker_digest: [0; 32],
                    envelopes: &[],
                    segment_lineage: &[],
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        assert_eq!(
            (first.first_marker_sequence, first.last_marker_sequence),
            (0, 0)
        );
        let second = processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 0,
                    marker_digest: [0; 32],
                    envelopes: &[],
                    segment_lineage: &[],
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        assert_eq!(second.first_marker_sequence, 1);
        assert_eq!(processor.last_marker_sequence(), Some(0));
        assert!(!processor.decoder_snapshot().unwrap().is_empty());
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn restart_rejects_wal_sequence_gap_and_lineage_fork() {
        let mut processor = IncrementalProcessor::new(SessionLimits::default());
        let registry = ProtocolRegistry::default();
        processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 0,
                    marker_digest: [0; 32],
                    envelopes: &[],
                    segment_lineage: &[],
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        let state = processor.state_snapshot();
        let mut restored = IncrementalProcessor::new(SessionLimits::default());
        restored.restore_state(state);
        assert!(restored.marker_digest_matches(&[0; 32]));
        let session_id = SessionId::new();
        let recording_id = uuid::Uuid::new_v4();
        let decoder = processor
            .decoder_state(session_id, recording_id, 0, 0, hex_bytes(&[0; 32]))
            .unwrap();
        let mut decoder_restored = IncrementalProcessor::new(SessionLimits::default());
        decoder_restored
            .restore_decoder_state(
                &decoder,
                session_id,
                recording_id,
                0,
                0,
                &hex_bytes(&[0; 32]),
            )
            .unwrap();
        assert!(decoder_restored.marker_digest_matches(&[0; 32]));
        let envelope = WalRecordEnvelope {
            recording_id: chronicle_common::RecordingId::new(),
            sequence: 5,
            provenance: None,
            kind: chronicle_wal::RecordKind::CaptureEvent,
            schema_version: 1,
            flags: 0,
            payload: Vec::new(),
        };
        assert!(matches!(
            processor.process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 5,
                    marker_digest: [0; 32],
                    envelopes: std::slice::from_ref(&envelope),
                    segment_lineage: &[],
                },
                &registry,
                SessionId::new(),
            ),
            Err(EtlError::SnapshotGap)
        ));

        let mut processor = IncrementalProcessor::new(SessionLimits::default());
        let first_lineage = [chronicle_wal::CommittedSegment {
            ordinal: 0,
            digest: [1; 32],
            first_sequence: 0,
            last_sequence: 0,
        }];
        processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 0,
                    marker_digest: [0; 32],
                    envelopes: &[],
                    segment_lineage: &first_lineage,
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        let forked_lineage = [chronicle_wal::CommittedSegment {
            digest: [2; 32],
            ..first_lineage[0].clone()
        }];
        assert!(matches!(
            processor.process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 0,
                    marker_digest: [0; 32],
                    envelopes: &[],
                    segment_lineage: &forked_lineage,
                },
                &registry,
                SessionId::new(),
            ),
            Err(EtlError::SnapshotLineageMismatch)
        ));

        let envelope = WalRecordEnvelope {
            sequence: 1,
            ..envelope
        };
        let extended_lineage = [chronicle_wal::CommittedSegment {
            digest: [3; 32],
            last_sequence: 1,
            ..first_lineage[0].clone()
        }];
        processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 1,
                    marker_digest: [1; 32],
                    envelopes: std::slice::from_ref(&envelope),
                    segment_lineage: &extended_lineage,
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
    }
}
