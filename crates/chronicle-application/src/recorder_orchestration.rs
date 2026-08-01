//! Continuous recorder boundary around the existing single bounded ingest queue.
#![allow(
    clippy::large_enum_variant,
    clippy::result_large_err,
    clippy::unnecessary_wraps
)]

use crate::{
    EpochOutcomeError, EpochOutcomeJournalV1, IngestAdmission, QueuedWalRecord, RecordingIngest,
    RecordingIngestFailure, RecordingIngestResult, RolloverAssignment,
};
use chronicle_capture::{
    CAPTURE_EVENT_SCHEMA_VERSION, CaptureError, CaptureEvent, CaptureEventKind, CaptureSource,
    MonotonicTimestamp, encode_event,
};
use chronicle_wal::{GroupCommitWalWriter, RecordKind};
use std::time::{Duration, Instant};
use thiserror::Error;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedObservation {
    pub ingest_ordinal: u64,
    pub payload_bytes: u64,
    pub capture_timestamp: Option<MonotonicTimestamp>,
}

#[derive(Debug, Error)]
pub enum RecorderOrchestrationError {
    #[error(transparent)]
    Capture(#[from] CaptureError),
    #[error("recorder ingest is backpressured")]
    Backpressure(CaptureEvent),
    #[error(transparent)]
    Wal(#[from] chronicle_wal::WalError),
    #[error(transparent)]
    Outcome(#[from] EpochOutcomeError),
    #[error("recorder graceful shutdown timed out")]
    ShutdownTimeout,
    #[error("recorder graceful shutdown failed during ingest")]
    IngestFailure,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecorderEpochBoundary {
    pub old_epoch_ordinal: u64,
    pub new_epoch_ordinal: u64,
    pub old_commit: Option<crate::RecordingCommitBoundary>,
}

#[derive(Debug)]
pub enum RecorderPoll {
    Idle,
    Accepted(AcceptedObservation),
    Backpressured(CaptureEvent),
}

/// Owns one capture source and the existing bounded ingest queue. Epoch handoff
/// callers keep this object alive, so capture is not detached for planned rollover.
pub struct RecorderOrchestrator<S> {
    source: S,
    ingest: RecordingIngest,
    next_ingest_ordinal: u64,
    outcome_journal: EpochOutcomeJournalV1,
    last_outcome_ordinal: Option<u64>,
    epoch_ordinal: u64,
    started: bool,
}

impl<S: CaptureSource> RecorderOrchestrator<S> {
    pub fn new(source: S, ingest: RecordingIngest) -> Self {
        Self {
            source,
            ingest,
            next_ingest_ordinal: 0,
            outcome_journal: EpochOutcomeJournalV1::new(0, None),
            last_outcome_ordinal: None,
            epoch_ordinal: 0,
            started: false,
        }
    }

    pub fn start(&mut self) -> Result<(), CaptureError> {
        if self.started {
            return Ok(());
        }
        self.source.start()?;
        self.started = true;
        Ok(())
    }

    pub fn poll(&mut self, now_millis: u64) -> Result<RecorderPoll, RecorderOrchestrationError> {
        let Some(event) = self.source.poll()? else {
            return Ok(RecorderPoll::Idle);
        };
        match self.admit_event(event.clone()) {
            Ok(observation) => {
                self.ingest.drain(now_millis)?;
                Ok(RecorderPoll::Accepted(observation))
            }
            Err(event) => Ok(RecorderPoll::Backpressured(event)),
        }
    }

    pub fn admit_event(
        &mut self,
        event: CaptureEvent,
    ) -> Result<AcceptedObservation, CaptureEvent> {
        let original = event.clone();
        let timestamp = event_timestamp(&event);
        let payload = encode_event(&event).map_err(|_| original.clone())?;
        let payload_bytes = u64::try_from(payload.len()).unwrap_or(u64::MAX);
        let record = QueuedWalRecord {
            kind: RecordKind::CaptureEvent,
            schema_version: CAPTURE_EVENT_SCHEMA_VERSION,
            flags: 0,
            payload,
            capture_timestamp: timestamp.clone(),
        };
        match self.ingest.admit(record) {
            Ok(IngestAdmission::Accepted) => {
                let observation = AcceptedObservation {
                    ingest_ordinal: self.next_ingest_ordinal,
                    payload_bytes,
                    capture_timestamp: timestamp.clone(),
                };
                self.outcome_journal
                    .admit(payload_bytes, timestamp.clone(), None)
                    .map_err(|_| event.clone())?;
                self.next_ingest_ordinal = self.next_ingest_ordinal.saturating_add(1);
                Ok(observation)
            }
            Ok(IngestAdmission::RejectedAfterStop) => Err(event),
            Err(_record) => Err(original),
        }
    }

    pub fn drain_ingest(&mut self, now_millis: u64) -> Result<(), chronicle_wal::WalError> {
        self.ingest.drain(now_millis)
    }

    pub fn request_shutdown(&mut self) -> Result<(), CaptureError> {
        self.source.request_shutdown()
    }

    pub fn capture_drain(&mut self) -> Result<Option<CaptureEvent>, CaptureError> {
        self.source.drain()
    }

    pub fn finalize_source(
        &mut self,
    ) -> Result<chronicle_capture::CaptureSourceSummary, CaptureError> {
        self.source.finalize()
    }

    pub fn finish(
        &mut self,
        reason: crate::ShutdownReason,
        now_millis: u64,
    ) -> Result<RecordingIngestResult, RecordingIngestFailure> {
        self.ingest.finish(reason, now_millis)
    }

    pub fn graceful_shutdown(
        &mut self,
        reason: crate::ShutdownReason,
        now_millis: u64,
        timeout: Duration,
    ) -> Result<RecordingIngestResult, RecorderOrchestrationError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(RecorderOrchestrationError::ShutdownTimeout)?;
        if Instant::now() >= deadline {
            return Err(RecorderOrchestrationError::ShutdownTimeout);
        }
        self.source.request_shutdown()?;
        loop {
            if Instant::now() >= deadline {
                return Err(RecorderOrchestrationError::ShutdownTimeout);
            }
            let Some(event) = self.source.drain()? else {
                break;
            };
            let mut event = event;
            loop {
                match self.admit_event(event) {
                    Ok(_) => break,
                    Err(retry) => {
                        event = retry;
                        self.ingest.drain(now_millis)?;
                    }
                }
                if Instant::now() >= deadline {
                    return Err(RecorderOrchestrationError::ShutdownTimeout);
                }
            }
        }
        if Instant::now() >= deadline {
            return Err(RecorderOrchestrationError::ShutdownTimeout);
        }
        let result = self
            .ingest
            .finish(reason, now_millis)
            .map_err(|_| RecorderOrchestrationError::IngestFailure)?;
        self.source.finalize()?;
        Ok(result)
    }

    pub fn ingest(&self) -> &RecordingIngest {
        &self.ingest
    }

    pub fn epoch_ordinal(&self) -> u64 {
        self.epoch_ordinal
    }

    pub fn ingest_mut(&mut self) -> &mut RecordingIngest {
        &mut self.ingest
    }

    pub fn record_rollover_outcome(
        &mut self,
        start_ordinal: u64,
        end_ordinal: u64,
        assignment: RolloverAssignment,
    ) -> Result<(), EpochOutcomeError> {
        self.outcome_journal
            .record_outcome(start_ordinal, end_ordinal, assignment)
    }

    pub fn rollover_to(
        &mut self,
        next_writer: GroupCommitWalWriter,
        now_millis: u64,
        outcome_path: impl AsRef<std::path::Path>,
    ) -> Result<RecorderEpochBoundary, RecorderOrchestrationError> {
        self.rollover_inner(now_millis, outcome_path, |ingest| {
            ingest.rollover_to(next_writer, now_millis)
        })
    }

    /// Roll over without acquiring a second lock on same WAL directory.
    pub fn rollover(
        &mut self,
        now_millis: u64,
        outcome_path: impl AsRef<std::path::Path>,
    ) -> Result<RecorderEpochBoundary, RecorderOrchestrationError> {
        self.rollover_inner(now_millis, outcome_path, |ingest| {
            ingest.rollover(now_millis)
        })
    }

    fn rollover_inner(
        &mut self,
        _now_millis: u64,
        outcome_path: impl AsRef<std::path::Path>,
        rollover: impl FnOnce(
            &mut RecordingIngest,
        )
            -> Result<Option<crate::RecordingCommitBoundary>, chronicle_wal::WalError>,
    ) -> Result<RecorderEpochBoundary, RecorderOrchestrationError> {
        let old_epoch_ordinal = self.epoch_ordinal;
        let old_commit = rollover(&mut self.ingest)?;
        if let Some(end_ordinal) = self.next_ingest_ordinal.checked_sub(1) {
            let start_ordinal = self
                .last_outcome_ordinal
                .map_or(0, |ordinal| ordinal.saturating_add(1));
            if start_ordinal <= end_ordinal {
                let marker_sequence = old_commit
                    .as_ref()
                    .map_or(0, |commit| commit.marker_sequence);
                self.outcome_journal.record_outcome(
                    start_ordinal,
                    end_ordinal,
                    RolloverAssignment::OldEpoch { marker_sequence },
                )?;
                self.last_outcome_ordinal = Some(end_ordinal);
            }
        }
        self.outcome_journal
            .ensure_complete()
            .map_err(RecorderOrchestrationError::Outcome)?;
        crate::write_epoch_outcome_atomic(outcome_path, &self.outcome_journal)?;
        self.epoch_ordinal = self.epoch_ordinal.saturating_add(1);
        Ok(RecorderEpochBoundary {
            old_epoch_ordinal,
            new_epoch_ordinal: self.epoch_ordinal,
            old_commit,
        })
    }

    pub fn persist_rollover_outcomes(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<std::path::PathBuf, EpochOutcomeError> {
        self.outcome_journal.ensure_complete()?;
        crate::write_epoch_outcome_atomic(path, &self.outcome_journal)
    }

    pub fn outcome_journal(&self) -> &EpochOutcomeJournalV1 {
        &self.outcome_journal
    }
}

pub type ContinuousRecorder<S> = RecorderOrchestrator<S>;

fn event_timestamp(event: &CaptureEvent) -> Option<MonotonicTimestamp> {
    match &event.kind {
        CaptureEventKind::SocketConnectObserved(value) => Some(value.timestamp.clone()),
        CaptureEventKind::SocketConnected(value)
        | CaptureEventKind::SocketClosedObserved(value)
        | CaptureEventKind::SocketResetObserved(value) => Some(value.timestamp.clone()),
        CaptureEventKind::SocketStateChangedObserved(value) => Some(value.socket.timestamp.clone()),
        CaptureEventKind::PayloadFragment(value) => Some(value.timestamp.clone()),
        CaptureEventKind::LossWindowObserved(value) => Some(value.end.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chronicle_capture::{FixtureCaptureSource, InMemoryCaptureSource};
    use chronicle_common::RecordingId;
    use chronicle_wal::GroupCommitWalWriter;
    use std::fs;

    #[test]
    fn start_is_idempotent_and_admission_assigns_stable_ordinals() {
        let root =
            std::env::temp_dir().join(format!("chronicle-orchestration-{}", uuid::Uuid::new_v4()));
        let source = FixtureCaptureSource::from_json(
            br#"{"schema_version":1,"connections":[],"events":[]}"#,
        )
        .unwrap();
        let writer = GroupCommitWalWriter::create(
            &root,
            RecordingId::new(),
            chronicle_wal::MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let mut recorder = RecorderOrchestrator::new(source, RecordingIngest::new(writer));
        recorder.start().unwrap();
        recorder.start().unwrap();
        assert!(matches!(recorder.poll(0).unwrap(), RecorderPoll::Idle));
        drop(recorder);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn graceful_shutdown_is_ordered_and_zero_timeout_is_rejected() {
        let root =
            std::env::temp_dir().join(format!("chronicle-shutdown-{}", uuid::Uuid::new_v4()));
        let writer = GroupCommitWalWriter::create(
            &root,
            RecordingId::new(),
            chronicle_wal::MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let mut recorder = RecorderOrchestrator::new(
            InMemoryCaptureSource::default(),
            RecordingIngest::new(writer),
        );
        assert!(matches!(
            recorder.graceful_shutdown(crate::ShutdownReason::TerminationSignal, 0, Duration::ZERO),
            Err(RecorderOrchestrationError::ShutdownTimeout)
        ));
        let result = recorder
            .graceful_shutdown(
                crate::ShutdownReason::TerminationSignal,
                1,
                Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(result.status, crate::RecordingStatus::Completed);
        drop(recorder);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_boundary_keeps_one_owned_source() {
        let root =
            std::env::temp_dir().join(format!("chronicle-orchestration-{}", uuid::Uuid::new_v4()));
        let writer = GroupCommitWalWriter::create(
            &root,
            RecordingId::new(),
            chronicle_wal::MIN_SEGMENT_BYTES,
            1,
            0,
        )
        .unwrap();
        let mut recorder = RecorderOrchestrator::new(
            InMemoryCaptureSource::default(),
            RecordingIngest::new(writer),
        );
        assert!(recorder.capture_drain().unwrap().is_none());
        drop(recorder);
        fs::remove_dir_all(root).unwrap();
    }
}
