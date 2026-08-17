//! Bounded recorder-owned incremental worker control state.

use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IncrementalWorkerPolicy {
    pub max_lag_records: u64,
    pub retry_attempts: u32,
    pub backoff_millis: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncrementalWorkerState {
    Stopped,
    Running,
    Backoff,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncrementalWorkerCounters {
    pub successful_batches: u64,
    pub failed_batches: u64,
    pub retries: u64,
    pub backoff_skips: u64,
}

pub struct IncrementalWorker {
    policy: IncrementalWorkerPolicy,
    state: IncrementalWorkerState,
    lag_records: u64,
    retry_attempts_used: u32,
    retry_not_before: Option<u64>,
    counters: IncrementalWorkerCounters,
}

impl IncrementalWorker {
    pub fn new(policy: IncrementalWorkerPolicy) -> Result<Self, IncrementalWorkerError> {
        if policy.max_lag_records == 0 || policy.retry_attempts > 100 {
            return Err(IncrementalWorkerError::InvalidPolicy);
        }
        Ok(Self {
            policy,
            state: IncrementalWorkerState::Stopped,
            lag_records: 0,
            retry_attempts_used: 0,
            retry_not_before: None,
            counters: IncrementalWorkerCounters::default(),
        })
    }

    pub fn start(&mut self) {
        if self.state == IncrementalWorkerState::Stopped {
            self.retry_attempts_used = 0;
            self.retry_not_before = None;
            self.state = IncrementalWorkerState::Running;
        }
    }

    /// Admit one non-blocking ETL attempt at the caller's current time.
    ///
    /// `false` means the worker is in backoff or terminal failure; no ETL work
    /// was invoked. The capture loop remains free to continue polling.
    pub fn begin_attempt(&mut self, now_millis: u64) -> Result<bool, IncrementalWorkerError> {
        if self.state == IncrementalWorkerState::Stopped {
            return Err(IncrementalWorkerError::Stopped);
        }
        if self.state == IncrementalWorkerState::Failed {
            return Ok(false);
        }
        if let Some(retry_not_before) = self.retry_not_before {
            if now_millis < retry_not_before {
                self.counters.backoff_skips = self.counters.backoff_skips.saturating_add(1);
                return Ok(false);
            }
            self.retry_not_before = None;
            self.state = IncrementalWorkerState::Running;
        }
        Ok(true)
    }

    /// Record durable success after ETL publication and checkpoint advancement.
    pub fn record_success(&mut self, lag_records: u64) {
        self.lag_records = lag_records;
        self.retry_attempts_used = 0;
        self.retry_not_before = None;
        self.counters.successful_batches = self.counters.successful_batches.saturating_add(1);
        self.state = IncrementalWorkerState::Running;
    }

    /// Record one failed ETL attempt. Returns whether another retry was queued.
    pub fn record_failure(&mut self, now_millis: u64) -> bool {
        if self.retry_attempts_used < self.policy.retry_attempts {
            self.retry_attempts_used += 1;
            self.counters.retries = self.counters.retries.saturating_add(1);
            self.retry_not_before = Some(now_millis.saturating_add(self.policy.backoff_millis));
            self.state = IncrementalWorkerState::Backoff;
            true
        } else {
            self.counters.failed_batches = self.counters.failed_batches.saturating_add(1);
            self.retry_not_before = None;
            self.state = IncrementalWorkerState::Failed;
            false
        }
    }

    /// Retained for callers that provide a complete single attempt.
    /// Runtime recorder polling uses `begin_attempt`, `record_success`, and
    /// `record_failure` so publication failures share the same retry state.
    pub fn process_once<F>(&mut self, mut batch: F) -> Result<(), IncrementalWorkerError>
    where
        F: FnMut() -> Result<u64, IncrementalWorkerError>,
    {
        if !self.begin_attempt(u64::MAX)? {
            return Err(IncrementalWorkerError::BatchFailed);
        }
        match batch() {
            Ok(lag_records) => {
                self.record_success(lag_records);
                Ok(())
            }
            Err(error) => {
                self.record_failure(u64::MAX);
                Err(error)
            }
        }
    }

    pub fn safe_point(&mut self) {
        self.retry_attempts_used = 0;
        self.retry_not_before = None;
        self.state = IncrementalWorkerState::Stopped;
    }

    pub fn update_lag(&mut self, lag_records: u64) {
        self.lag_records = lag_records;
    }

    pub fn lag_records(&self) -> u64 {
        self.lag_records
    }

    pub fn processing_ready(&self) -> bool {
        self.state == IncrementalWorkerState::Running
            && self.lag_records <= self.policy.max_lag_records
    }

    pub fn retry_not_before(&self) -> Option<u64> {
        self.retry_not_before
    }

    pub fn retry_attempts_used(&self) -> u32 {
        self.retry_attempts_used
    }

    pub fn state(&self) -> IncrementalWorkerState {
        self.state
    }

    pub fn counters(&self) -> &IncrementalWorkerCounters {
        &self.counters
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum IncrementalWorkerError {
    #[error("incremental worker policy is invalid")]
    InvalidPolicy,
    #[error("incremental worker is stopped")]
    Stopped,
    #[error("incremental worker batch failed")]
    BatchFailed,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker() -> IncrementalWorker {
        IncrementalWorker::new(IncrementalWorkerPolicy {
            max_lag_records: 4,
            retry_attempts: 2,
            backoff_millis: 10,
        })
        .unwrap()
    }

    #[test]
    fn backoff_deadline_skips_without_invoking_etl() {
        let mut worker = worker();
        worker.start();
        assert!(worker.begin_attempt(100).unwrap());
        assert!(worker.record_failure(100));
        assert_eq!(worker.retry_not_before(), Some(110));
        assert!(!worker.begin_attempt(109).unwrap());
        assert_eq!(worker.counters().backoff_skips, 1);
        assert!(worker.begin_attempt(110).unwrap());
    }

    #[test]
    fn retries_are_bounded_and_terminal_failure_is_not_capture_failure() {
        let mut worker = worker();
        worker.start();
        assert!(worker.begin_attempt(0).unwrap());
        assert!(worker.record_failure(0));
        assert!(worker.begin_attempt(10).unwrap());
        assert!(worker.record_failure(10));
        assert!(worker.begin_attempt(20).unwrap());
        assert!(!worker.record_failure(20));
        assert_eq!(worker.state(), IncrementalWorkerState::Failed);
        assert!(!worker.processing_ready());
        assert_eq!(worker.counters().retries, 2);
        assert_eq!(worker.counters().failed_batches, 1);
    }

    #[test]
    fn successful_checkpoint_resets_transient_retry_state_and_sets_lag() {
        let mut worker = worker();
        worker.start();
        assert!(worker.begin_attempt(0).unwrap());
        assert!(worker.record_failure(0));
        assert!(worker.begin_attempt(10).unwrap());
        worker.record_success(5);
        assert_eq!(worker.state(), IncrementalWorkerState::Running);
        assert_eq!(worker.retry_attempts_used(), 0);
        assert_eq!(worker.retry_not_before(), None);
        assert!(!worker.processing_ready());
        worker.record_success(0);
        assert!(worker.processing_ready());
    }

    #[test]
    fn restart_start_resets_transient_backoff() {
        let mut worker = worker();
        worker.start();
        assert!(worker.begin_attempt(0).unwrap());
        assert!(worker.record_failure(0));
        worker.safe_point();
        worker.start();
        assert_eq!(worker.retry_attempts_used(), 0);
        assert_eq!(worker.retry_not_before(), None);
        assert!(worker.begin_attempt(1).unwrap());
    }
}
