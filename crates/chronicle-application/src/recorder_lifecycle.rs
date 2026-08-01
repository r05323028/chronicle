//! Recorder lifecycle and readiness semantics, separate from P1 one-shot status.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderLifecycleState {
    Starting,
    Recovering,
    Running,
    Draining,
    Stopped,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderReadiness {
    Ready,
    NotReady,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderHealth {
    Healthy,
    Degraded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderReadinessState {
    pub liveness: bool,
    pub capture: RecorderReadiness,
    pub processing: RecorderReadiness,
    pub health: RecorderHealth,
}

impl Default for RecorderReadinessState {
    fn default() -> Self {
        Self {
            liveness: true,
            capture: RecorderReadiness::NotReady,
            processing: RecorderReadiness::Unknown,
            health: RecorderHealth::Degraded,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderLifecycle {
    pub state: RecorderLifecycleState,
    pub readiness: RecorderReadinessState,
}

impl Default for RecorderLifecycle {
    fn default() -> Self {
        Self {
            state: RecorderLifecycleState::Starting,
            readiness: RecorderReadinessState::default(),
        }
    }
}

impl RecorderLifecycle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn begin_recovery(&mut self) -> Result<(), RecorderLifecycleError> {
        self.transition(RecorderLifecycleState::Recovering)
    }

    pub fn running(&mut self, processing: RecorderReadiness) -> Result<(), RecorderLifecycleError> {
        if processing == RecorderReadiness::Ready {
            self.readiness.processing = RecorderReadiness::Ready;
        } else {
            self.readiness.processing = processing;
        }
        self.readiness.capture = RecorderReadiness::Ready;
        self.transition(RecorderLifecycleState::Running)
    }

    pub fn set_processing(&mut self, readiness: RecorderReadiness) {
        self.readiness.processing = readiness;
        self.recompute_health();
    }

    pub fn set_capture(&mut self, readiness: RecorderReadiness) {
        self.readiness.capture = readiness;
        self.recompute_health();
    }

    pub fn set_liveness(&mut self, liveness: bool) {
        self.readiness.liveness = liveness;
        self.recompute_health();
    }

    pub fn begin_draining(&mut self) -> Result<(), RecorderLifecycleError> {
        match self.state {
            RecorderLifecycleState::Running | RecorderLifecycleState::Draining => {
                self.state = RecorderLifecycleState::Draining;
                self.readiness.capture = RecorderReadiness::NotReady;
                self.recompute_health();
                Ok(())
            }
            _ => Err(RecorderLifecycleError::InvalidTransition {
                from: self.state,
                to: RecorderLifecycleState::Draining,
            }),
        }
    }

    pub fn stop(&mut self) -> Result<(), RecorderLifecycleError> {
        match self.state {
            RecorderLifecycleState::Draining | RecorderLifecycleState::Stopped => {
                self.state = RecorderLifecycleState::Stopped;
                self.readiness.capture = RecorderReadiness::NotReady;
                self.readiness.processing = RecorderReadiness::NotReady;
                self.readiness.health = RecorderHealth::Degraded;
                Ok(())
            }
            _ => Err(RecorderLifecycleError::InvalidTransition {
                from: self.state,
                to: RecorderLifecycleState::Stopped,
            }),
        }
    }

    pub fn fail(&mut self) {
        self.state = RecorderLifecycleState::Failed;
        self.readiness.capture = RecorderReadiness::NotReady;
        self.readiness.processing = RecorderReadiness::NotReady;
        self.readiness.health = RecorderHealth::Failed;
    }

    fn transition(&mut self, to: RecorderLifecycleState) -> Result<(), RecorderLifecycleError> {
        let valid = matches!(
            (self.state, to),
            (
                RecorderLifecycleState::Starting,
                RecorderLifecycleState::Recovering
            ) | (
                RecorderLifecycleState::Recovering,
                RecorderLifecycleState::Running
            )
        );
        if !valid {
            return Err(RecorderLifecycleError::InvalidTransition {
                from: self.state,
                to,
            });
        }
        self.state = to;
        self.recompute_health();
        Ok(())
    }

    fn recompute_health(&mut self) {
        self.readiness.health = if !self.readiness.liveness
            || self.readiness.capture == RecorderReadiness::NotReady
                && matches!(self.state, RecorderLifecycleState::Running)
        {
            RecorderHealth::Failed
        } else if self.readiness.capture != RecorderReadiness::Ready
            || self.readiness.processing != RecorderReadiness::Ready
        {
            RecorderHealth::Degraded
        } else {
            RecorderHealth::Healthy
        };
    }
}

#[derive(Clone, Copy, Debug, Error, PartialEq, Eq)]
pub enum RecorderLifecycleError {
    #[error("invalid recorder lifecycle transition")]
    InvalidTransition {
        from: RecorderLifecycleState,
        to: RecorderLifecycleState,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_start_and_stop_sequence_is_monotonic() {
        let mut lifecycle = RecorderLifecycle::new();
        assert_eq!(lifecycle.begin_recovery(), Ok(()));
        assert_eq!(lifecycle.running(RecorderReadiness::Unknown), Ok(()));
        assert_eq!(lifecycle.state, RecorderLifecycleState::Running);
        assert_eq!(lifecycle.begin_draining(), Ok(()));
        assert_eq!(lifecycle.stop(), Ok(()));
        assert_eq!(lifecycle.stop(), Ok(()));
    }

    #[test]
    fn illegal_transition_does_not_change_state() {
        let mut lifecycle = RecorderLifecycle::new();
        assert!(lifecycle.stop().is_err());
        assert_eq!(lifecycle.state, RecorderLifecycleState::Starting);
        lifecycle.fail();
        assert!(lifecycle.begin_recovery().is_err());
    }

    #[test]
    fn capture_ready_can_degrade_on_etl_lag() {
        let mut lifecycle = RecorderLifecycle::new();
        lifecycle.begin_recovery().unwrap();
        lifecycle.running(RecorderReadiness::Ready).unwrap();
        lifecycle.set_processing(RecorderReadiness::NotReady);
        assert_eq!(lifecycle.readiness.capture, RecorderReadiness::Ready);
        assert_eq!(lifecycle.readiness.health, RecorderHealth::Degraded);
    }
}
