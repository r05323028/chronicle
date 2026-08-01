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
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IncrementalWorkerCounters {
    pub successful_batches: u64,
    pub failed_batches: u64,
    pub retries: u64,
}

pub struct IncrementalWorker {
    policy: IncrementalWorkerPolicy,
    state: IncrementalWorkerState,
    lag_records: u64,
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
            counters: IncrementalWorkerCounters::default(),
        })
    }

    pub fn start(&mut self) {
        if self.state == IncrementalWorkerState::Stopped {
            self.state = IncrementalWorkerState::Running;
        }
    }

    pub fn process_once<F>(&mut self, mut batch: F) -> Result<(), IncrementalWorkerError>
    where
        F: FnMut() -> Result<u64, IncrementalWorkerError>,
    {
        if self.state == IncrementalWorkerState::Stopped {
            return Err(IncrementalWorkerError::Stopped);
        }
        let mut attempts = 0;
        loop {
            match batch() {
                Ok(lag_records) => {
                    self.lag_records = lag_records;
                    self.counters.successful_batches =
                        self.counters.successful_batches.saturating_add(1);
                    self.state = IncrementalWorkerState::Running;
                    return Ok(());
                }
                Err(_error) if attempts < self.policy.retry_attempts => {
                    attempts += 1;
                    self.counters.retries = self.counters.retries.saturating_add(1);
                    self.state = IncrementalWorkerState::Backoff;
                    let _ = self.policy.backoff_millis;
                }
                Err(error) => {
                    self.counters.failed_batches = self.counters.failed_batches.saturating_add(1);
                    self.state = IncrementalWorkerState::Backoff;
                    return Err(error);
                }
            }
        }
    }

    pub fn safe_point(&mut self) {
        self.state = IncrementalWorkerState::Stopped;
    }

    pub fn lag_records(&self) -> u64 {
        self.lag_records
    }
    pub fn processing_ready(&self) -> bool {
        self.state == IncrementalWorkerState::Running
            && self.lag_records <= self.policy.max_lag_records
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

    #[test]
    fn retries_are_bounded_and_lag_does_not_stop_capture_control() {
        let mut worker = IncrementalWorker::new(IncrementalWorkerPolicy {
            max_lag_records: 4,
            retry_attempts: 2,
            backoff_millis: 1,
        })
        .unwrap();
        worker.start();
        let mut calls = 0;
        worker
            .process_once(|| {
                calls += 1;
                if calls < 3 {
                    Err(IncrementalWorkerError::BatchFailed)
                } else {
                    Ok(8)
                }
            })
            .unwrap();
        assert_eq!(calls, 3);
        assert!(!worker.processing_ready());
        assert_eq!(worker.counters().retries, 2);
        worker.safe_point();
        assert_eq!(worker.state(), IncrementalWorkerState::Stopped);
    }
}
