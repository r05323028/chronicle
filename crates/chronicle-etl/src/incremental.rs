//! Recorder-owned incremental snapshot processing boundary.
#![allow(clippy::needless_pass_by_value)]

use crate::{EtlError, EtlOutput, EtlPipeline};
use chronicle_common::SessionId;
use chronicle_protocol::ProtocolRegistry;
use chronicle_session::SessionLimits;
use chronicle_wal::WalRecordEnvelope;

#[derive(Clone, Debug)]
pub struct CommittedWalSnapshot<'a> {
    pub marker_sequence: u64,
    pub envelopes: &'a [WalRecordEnvelope],
}

#[derive(Debug)]
pub struct IncrementalResult {
    pub output: EtlOutput,
    pub first_marker_sequence: u64,
    pub last_marker_sequence: u64,
}

pub struct IncrementalProcessor {
    pipeline: EtlPipeline,
    last_marker_sequence: Option<u64>,
}

impl IncrementalProcessor {
    pub fn new(limits: SessionLimits) -> Self {
        Self {
            pipeline: EtlPipeline::new(limits),
            last_marker_sequence: None,
        }
    }

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
        let first_marker_sequence = self
            .last_marker_sequence
            .map_or(0, |sequence| sequence.saturating_add(1));
        let output = self
            .pipeline
            .process_envelopes(snapshot.envelopes, registry, session_id)?;
        self.last_marker_sequence = Some(snapshot.marker_sequence);
        Ok(IncrementalResult {
            output,
            first_marker_sequence,
            last_marker_sequence: snapshot.marker_sequence,
        })
    }

    pub fn last_marker_sequence(&self) -> Option<u64> {
        self.last_marker_sequence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_committed_snapshots_advance_marker_without_protocol_input() {
        let mut processor = IncrementalProcessor::new(SessionLimits::default());
        let registry = ProtocolRegistry::default();
        let first = processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 3,
                    envelopes: &[],
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        assert_eq!(
            (first.first_marker_sequence, first.last_marker_sequence),
            (0, 3)
        );
        let second = processor
            .process_snapshot(
                CommittedWalSnapshot {
                    marker_sequence: 5,
                    envelopes: &[],
                },
                &registry,
                SessionId::new(),
            )
            .unwrap();
        assert_eq!(second.first_marker_sequence, 4);
        assert_eq!(processor.last_marker_sequence(), Some(5));
    }
}
