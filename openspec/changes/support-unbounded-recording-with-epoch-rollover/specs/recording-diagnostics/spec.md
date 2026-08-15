## MODIFIED Requirements

### Requirement: WAL and filesystem probes

Doctor SHALL check supplied state/WAL/output filesystem support for private files, advisory locks, file/data/directory sync, atomic no-replace publication, bounded epoch segment rotation, minimum-free-space reservation, global quota accounting, and retention/tombstone prerequisites. It SHALL distinguish active epoch-local segment limits from parent recording lifetime and report effective non-sensitive bounds. If a path option is omitted, its path probe remains optional `not_checked`; doctor SHALL not invent a path. Probe artifacts SHALL be private and removed best-effort without overwriting user files.

#### Scenario: Epoch rollover filesystem support

- **WHEN** filesystem supports bounded segment rotation and same-domain atomic publication
- **THEN** storage probe reports supported with epoch/segment limits and reserved-space policy

#### Scenario: Global quota despite per-epoch bound

- **WHEN** each epoch is within its local limit but retained epochs exceed global quota
- **THEN** doctor reports quota/retention risk and does not claim unbounded recording support

#### Scenario: Atomic publication unavailable

- **WHEN** filesystem cannot provide required no-replace atomic publication for epoch output
- **THEN** doctor reports storage unsupported

#### Scenario: Writable private filesystem

- **WHEN** directory supports required permissions, sync, lock, and atomic rename
- **THEN** storage probe reports supported

#### Scenario: Path omitted

- **WHEN** doctor runs without WAL or output path
- **THEN** corresponding path probe is optional `not_checked`, aggregate is at most supported-with-warnings, and no filesystem path is guessed

#### Scenario: Low disk space

- **WHEN** filesystem is writable but available bytes are below documented warning threshold
- **THEN** doctor reports supported-with-warnings and required free-space guidance

### Requirement: Safe human and JSON output

Doctor, recorder status, record results, and ETL summaries SHALL include stable parent recording ID, epoch ID/ordinal, active/finalized epoch state, segment/marker watermarks, per-epoch and aggregate lag/retention state, bounded counters, stable codes, and remediation where applicable. Output SHALL be deterministic and versioned, omit body/header/capture values, credentials, environment values, command lines, arbitrary file contents, and raw WAL paths where path disclosure is not needed.

#### Scenario: Epoch JSON diagnostics

- **WHEN** user requests JSON while successor epoch is active and predecessor ETL is pending
- **THEN** output contains both epoch states, deterministic watermarks, lag/retention blockers, and no sensitive values

#### Scenario: Stable repeated status

- **WHEN** no state changes between two status requests
- **THEN** serialized diagnostic fields and ordering remain stable except explicitly observational update fields

#### Scenario: JSON diagnostics

- **WHEN** user requests doctor JSON output
- **THEN** valid versioned machine-readable structure contains every probe and aggregate status

#### Scenario: Sensitive environment present

- **WHEN** runtime credential environment variable exists
- **THEN** doctor reports only presence/check status and never value

### Requirement: Recorder daemon preflight diagnostics

Doctor SHALL add recorder-operation probes for configuration/version semantics, exclusive state-root ownership, parent/epoch lifecycle metadata and manifest readability, checkpoint/segment lineage, private modes/ownership, file/data/directory sync, same-filesystem epoch rollover and trash behavior, segment-age/size trigger support, global quota/minimum-free-space reservation, bounded lifecycle-index limits/compaction, filesystem RecordingStore conformance, and recorder-outside-selected-subtree placement. Probes SHALL validate that a successor epoch can be opened without weakening predecessor recovery/retention proof. Doctor SHALL remain non-destructive: no repair, long-lived lease, ETL, deletion, capture, or persistent eBPF attachment.

#### Scenario: Ready continuous-recording state root

- **WHEN** parent/epoch metadata, rollover, store, quota, and retention probes pass
- **THEN** doctor reports recorder foundation supported with effective bounds and no claim about live capture itself

#### Scenario: Inconsistent epoch metadata

- **WHEN** parent lifecycle index references duplicate/unknown/gapped epoch lineage
- **THEN** required probe reports unsupported or not-checked with remediation and does not repair it

#### Scenario: Unsafe retention proof

- **WHEN** retention could delete an epoch with pending ETL/checkpoint/output/parent proof
- **THEN** doctor reports required unsupported

#### Scenario: Ready production state root

- **WHEN** supported host/config/state/store pass every required recorder-operation probe
- **THEN** doctor reports recorder foundation supported with effective non-sensitive bounds

#### Scenario: Active owner

- **WHEN** live recorder owns state root
- **THEN** doctor reports ownership as expected active status without mutating lease or reading mutable payload bytes

#### Scenario: Unsafe cleanup policy

- **WHEN** retention could delete uncheckpointed/unverified WAL
- **THEN** doctor reports required unsupported with remediation

#### Scenario: Sidecar placement risk

- **WHEN** recorder identity resolves inside selected subtree
- **THEN** doctor reports unsupported placement before any attach probe

### Requirement: Safe runtime lifecycle health report

Application SHALL expose versioned bounded local recorder status for `healthy`, `degraded`, or `failed`, distinct from process liveness, capture readiness, processing readiness, and overall health. Report SHALL contain stable parent recording ID, recorder/process-attempt ID, active epoch ID/ordinal, finalized epoch summaries, lifecycle/capture-readiness/processing-readiness/overall-health, configuration digest, final recovery-authoritative marker per visible epoch, `IncrementalEtlCheckpoint v1` marker/lag per epoch, segment lifecycle counts, retained/quota/free bytes per filesystem domain, capture/WAL loss counters, last rollover/recovery/cleanup/publication result, stable failure/remediation codes, and observational update time.

Report SHALL be read from atomically persisted state plus direct safe probes. It SHALL distinguish committed from merely written bytes, identify whether a successor epoch is active, show predecessor ETL/retention blockers, and never claim parent completion while required epoch state is unresolved. It SHALL not parse payload, expose secrets/commands, or treat an unvalidated manifest/checkpoint as committed authority.

#### Scenario: Running healthy with active successor

- **WHEN** predecessor epochs have verified output/retention state and active successor WAL has safe headroom
- **THEN** status is healthy with active epoch capture readiness true, processing readiness true, and exact per-epoch watermarks

#### Scenario: ETL lag on finalized epoch

- **WHEN** active capture is durable but finalized epoch checkpoint lag exceeds threshold
- **THEN** capture readiness may remain true, processing/retention readiness is degraded or false, lag and quota impact are reported, and WAL remains protected

#### Scenario: Rollover blocked by quota

- **WHEN** no safe segment/epoch retention transition exists and global quota is exhausted
- **THEN** status is failed or degraded with stable quota code, capture stops visibly, and no accepted data is silently dropped

#### Scenario: Stale status after crash

- **WHEN** no live owner exists but state reports running in an active epoch
- **THEN** status reports stale/crash-recovery-required and identifies parent/epoch lineage without claiming healthy

#### Scenario: Running healthy

- **WHEN** capture/WAL/ETL/store are current and quota headroom is safe
- **THEN** status is healthy with capture readiness and processing readiness true and exact watermarks/counters

#### Scenario: ETL lag degraded

- **WHEN** WAL is durable but `IncrementalEtlCheckpoint v1` lag exceeds configured warning threshold
- **THEN** processing readiness is false or degraded and overall status reports lag/retention impact while capture readiness remains true and committed capture remains reported accurately

#### Scenario: Stale status file

- **WHEN** no live owner exists and last state says running
- **THEN** status reports stale/crash-recovery-required rather than healthy

### Requirement: Recorder diagnostics retain existing status semantics

New parent/epoch/rollover probes SHALL use existing required/optional `supported`, `supported_with_warnings`, `unsupported`, and `not_checked` model and aggregate precedence. Required uncertainty about parent/epoch identity, active ownership, marker lineage, quota safety, retention proof, checkpoint lineage, store semantics, or placement SHALL be unsupported/not-checked, never assumed supported. Existing platform/eBPF/WAL/output/protocol/replay probes and no-side-effects guarantees remain.

#### Scenario: ETL worker unavailable

- **WHEN** recorder can capture but per-epoch ETL worker cannot be started
- **THEN** diagnostics report processing warning/failure and retention protection without claiming ETL readiness

#### Scenario: Optional remote backend absent

- **WHEN** no remote store is configured
- **THEN** local filesystem operation may be supported while remote backend remains non-required unsupported/not implemented

#### Scenario: Required probe blocked by permission

- **WHEN** doctor cannot determine state-root ownership or durability semantics
- **THEN** required probe is not-checked and aggregate exits 4

## ADDED Requirements

#### Scenario: Continuous status summary

- **WHEN** a parent recording has one active successor, one ETL-pending epoch, and one published epoch
- **THEN** status lists all three deterministic states and identifies exact blockers/counters

#### Scenario: Rollover failure

- **WHEN** successor epoch cannot acquire required lock/create sealed metadata
- **THEN** diagnostics retain predecessor committed boundary, report typed rollover failure, and do not claim successor readiness
