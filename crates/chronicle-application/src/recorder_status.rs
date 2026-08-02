//! Safe bounded recorder status, distinct from process liveness.

use crate::{
    CommitWatermark, IncrementalCheckpointSummaryV1, MetadataCode, QuotaStatus, RecorderCounters,
    RecorderHealth, RecorderLifecycleState, RecorderMetadataV1, RecorderReadiness,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const RECORDER_STATUS_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecorderStatusState {
    Starting,
    Recovering,
    LoadingEbpf,
    Ready,
    Degraded,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecorderStatusV1 {
    pub version: u16,
    pub state: RecorderStatusState,
    pub liveness: bool,
    pub capture_readiness: RecorderReadiness,
    pub processing_readiness: RecorderReadiness,
    pub health: RecorderHealth,
    pub lifecycle: RecorderLifecycleState,
    pub recorder_id: uuid::Uuid,
    pub attempt_id: uuid::Uuid,
    pub config_digest: String,
    pub current_epoch: Option<u64>,
    pub active_segment: Option<u64>,
    pub commit: Option<CommitWatermark>,
    pub checkpoint: Option<IncrementalCheckpointSummaryV1>,
    pub lag_records: u64,
    pub lag_bytes: u64,
    pub lag_age_seconds: u64,
    pub segment_counts: SegmentCounts,
    pub quota: Vec<QuotaStatus>,
    pub counters: RecorderCounters,
    pub last_recovery: Option<MetadataCode>,
    pub last_cleanup: Option<MetadataCode>,
    pub last_publication: Option<MetadataCode>,
    pub stale_owner: bool,
    pub remediation: Option<StatusRemediation>,
}

impl RecorderStatusV1 {
    pub fn from_metadata(
        metadata: &RecorderMetadataV1,
        owner_live: bool,
    ) -> Result<Self, RecorderStatusError> {
        metadata
            .validate()
            .map_err(|_| RecorderStatusError::InvalidMetadata)?;
        let stale_owner = !owner_live
            && matches!(
                metadata.lifecycle,
                RecorderLifecycleState::Running
                    | RecorderLifecycleState::Recovering
                    | RecorderLifecycleState::Draining
            );
        let health = if stale_owner {
            RecorderHealth::Failed
        } else {
            metadata.health
        };
        let state = if stale_owner
            || metadata.lifecycle == RecorderLifecycleState::Failed
            || health == RecorderHealth::Failed
        {
            RecorderStatusState::Failed
        } else if metadata.capture_readiness == RecorderReadiness::Ready
            && metadata.lifecycle == RecorderLifecycleState::Running
        {
            // Capture admission remains safe while incremental ETL is catching up.
            RecorderStatusState::Ready
        } else {
            match metadata.lifecycle {
                RecorderLifecycleState::Starting => RecorderStatusState::Starting,
                RecorderLifecycleState::Recovering => RecorderStatusState::Recovering,
                RecorderLifecycleState::Running
                    if metadata.capture_readiness != RecorderReadiness::Ready =>
                {
                    RecorderStatusState::LoadingEbpf
                }
                RecorderLifecycleState::Running => RecorderStatusState::Degraded,
                RecorderLifecycleState::Draining | RecorderLifecycleState::Stopped => {
                    RecorderStatusState::Degraded
                }
                RecorderLifecycleState::Failed => RecorderStatusState::Failed,
            }
        };
        Ok(Self {
            version: RECORDER_STATUS_SCHEMA_VERSION,
            state,
            liveness: owner_live,
            capture_readiness: if stale_owner {
                RecorderReadiness::NotReady
            } else {
                metadata.capture_readiness
            },
            processing_readiness: if stale_owner {
                RecorderReadiness::Unknown
            } else {
                metadata.processing_readiness
            },
            health,
            lifecycle: metadata.lifecycle,
            recorder_id: metadata.recorder_id,
            attempt_id: metadata.attempt_id,
            config_digest: metadata.config_digest.clone(),
            current_epoch: metadata.current_epoch.as_ref().map(|epoch| epoch.ordinal),
            active_segment: metadata.active_segment,
            commit: metadata.commit.clone(),
            checkpoint: metadata.incremental_checkpoint.clone(),
            lag_records: metadata.lag.records,
            lag_bytes: metadata.lag.bytes,
            lag_age_seconds: metadata.lag.age_seconds,
            segment_counts: SegmentCounts::default(),
            quota: metadata.quota.clone(),
            counters: metadata.counters.clone(),
            last_recovery: metadata.recovery.as_ref().map(|summary| summary.code),
            last_cleanup: None,
            last_publication: None,
            stale_owner,
            remediation: stale_owner.then_some(StatusRemediation::CrashRecoveryRequired),
        })
    }

    pub fn render_json(&self) -> Result<String, RecorderStatusError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| RecorderStatusError::Serialization)
    }

    pub fn render_human(&self) -> Result<String, RecorderStatusError> {
        self.validate()?;
        Ok(format!(
            "state={:?} liveness={} capture_readiness={:?} processing_readiness={:?} health={:?} lifecycle={:?} lag_records={} lag_bytes={} stale_owner={}",
            self.state,
            self.liveness,
            self.capture_readiness,
            self.processing_readiness,
            self.health,
            self.lifecycle,
            self.lag_records,
            self.lag_bytes,
            self.stale_owner,
        ))
    }

    pub fn validate(&self) -> Result<(), RecorderStatusError> {
        if self.version != RECORDER_STATUS_SCHEMA_VERSION
            || self.config_digest.len() != 64
            || !self
                .config_digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.quota.len() > 16
        {
            return Err(RecorderStatusError::InvalidMetadata);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SegmentCounts {
    pub active: u64,
    pub sealed: u64,
    pub processed: u64,
    pub retention_eligible: u64,
    pub deleting: u64,
    pub deleted: u64,
    pub quarantined: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusRemediation {
    CrashRecoveryRequired,
    ResolveConfiguration,
    ResolveQuota,
    InspectCorruption,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RecorderStatusError {
    #[error("recorder status metadata is invalid")]
    InvalidMetadata,
    #[error("recorder status serialization failed")]
    Serialization,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NormalizedScopeConfig;
    use crate::recorder_metadata::{EpochMetadata, LagSummary, RecoverySummary};

    fn metadata() -> RecorderMetadataV1 {
        RecorderMetadataV1 {
            version: 1,
            recorder_id: uuid::Uuid::new_v4(),
            attempt_id: uuid::Uuid::new_v4(),
            config_digest: "a".repeat(64),
            scope: NormalizedScopeConfig {
                cgroup_path: "/scope".into(),
                cgroup_id: 1,
                shared_scope_acknowledged: true,
            },
            boot_clock_identity: "boot".into(),
            lifecycle: RecorderLifecycleState::Running,
            capture_readiness: RecorderReadiness::Ready,
            processing_readiness: RecorderReadiness::Ready,
            health: RecorderHealth::Healthy,
            current_epoch: Some(EpochMetadata {
                ordinal: 1,
                recording_id: uuid::Uuid::new_v4(),
            }),
            previous_epoch: None,
            active_segment: Some(0),
            commit: None,
            incremental_checkpoint: None,
            lag: LagSummary {
                records: 0,
                bytes: 0,
                age_seconds: 0,
            },
            quota: Vec::new(),
            counters: RecorderCounters::default(),
            recovery: Some(RecoverySummary {
                code: MetadataCode::CleanStart,
                repaired_tail: false,
            }),
            shutdown: None,
            failure: None,
            updated_at_unix_seconds: 0,
            checksum: "b".repeat(64),
        }
    }

    #[test]
    fn stale_owner_is_not_reported_healthy() {
        let status = RecorderStatusV1::from_metadata(&metadata(), false).unwrap();
        assert!(status.stale_owner);
        assert_eq!(status.state, RecorderStatusState::Failed);
        assert_eq!(status.health, RecorderHealth::Failed);
        assert!(
            status
                .render_json()
                .unwrap()
                .contains("crash_recovery_required")
        );
    }

    #[test]
    fn capture_ready_is_admission_ready_while_etl_is_degraded() {
        let mut value = metadata();
        value.processing_readiness = RecorderReadiness::NotReady;
        value.health = RecorderHealth::Degraded;
        let status = RecorderStatusV1::from_metadata(&value, true).unwrap();
        assert_eq!(status.state, RecorderStatusState::Ready);
        assert_eq!(status.processing_readiness, RecorderReadiness::NotReady);
    }

    #[test]
    fn human_output_is_bounded_and_payload_free() {
        let status = RecorderStatusV1::from_metadata(&metadata(), true).unwrap();
        let text = status.render_human().unwrap();
        assert_eq!(status.state, RecorderStatusState::Ready);
        assert!(text.contains("state=Ready"));
        assert!(text.contains("capture_readiness=Ready"));
        assert!(!text.contains("/scope"));
    }
}
