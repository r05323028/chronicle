## ADDED Requirements

### Requirement: Recorder daemon preflight diagnostics

Doctor SHALL add P2 probes for recorder configuration version/semantics, exclusive state-root lease availability, persisted metadata/manifest/checkpoint readability and version, filesystem private modes/ownership, file/data/directory sync, same-filesystem rename/trash behavior, segment-age timer support, global quota/minimum-free-space reservation, retention eligibility policy, filesystem RecordingStore conformance, and recorder-outside-selected-subtree placement. Probes SHALL be non-destructive or use private temporary artifacts removed best-effort; doctor SHALL not repair state, acquire long-lived ownership, delete retained artifacts, attach persistent eBPF programs, run ETL, or start recorder.

#### Scenario: Ready production state root

- **WHEN** supported host/config/state/store pass every required P2 probe
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

Application SHALL expose versioned bounded local recorder status for `healthy`, `degraded`, or `failed`, distinct from process liveness and readiness. Report SHALL contain recorder/process-attempt/epoch/segment IDs, lifecycle/readiness, configuration digest, final recovery-authoritative commit boundary, ETL checkpoint and lag records/bytes/age, segment lifecycle counts, retained/quota/free bytes per filesystem domain, capture/WAL loss counters, last recovery/cleanup/publication result, stable failure/remediation codes, and update time.

Report SHALL be read from atomically persisted state and direct safe probes. It SHALL not parse capture payload, print headers/bodies, expose command line/environment values, claim live state from stale timestamp without warning, or treat manifest/checkpoint claim as committed authority without validation status.

#### Scenario: Running healthy

- **WHEN** capture/WAL/ETL/store are current and quota headroom is safe
- **THEN** status is healthy and ready with exact watermarks/counters

#### Scenario: ETL lag degraded

- **WHEN** WAL is durable but checkpoint lag exceeds configured warning threshold
- **THEN** status is degraded with lag and retention impact while committed capture remains reported accurately

#### Scenario: Stale status file

- **WHEN** no live owner exists and last state says running
- **THEN** status reports stale/crash-recovery-required rather than healthy

### Requirement: P2 diagnostics retain existing status semantics

New P2 probes SHALL use existing required/optional `supported`, `supported_with_warnings`, `unsupported`, and `not_checked` model and aggregate precedence. Required uncertainty about state-root durability, ownership, quota safety, retention proof, checkpoint lineage, filesystem store semantics, or recorder placement SHALL be `unsupported` or `not_checked`, never supported by assumption. Existing P1 platform/eBPF/WAL/output/protocol/replay probes and no-side-effects guarantee SHALL remain.

#### Scenario: Optional remote backend absent

- **WHEN** no remote store is configured in P2
- **THEN** filesystem backend may be supported and S3-compatible backend is reported unsupported/not implemented without failing local required capability

#### Scenario: Required probe blocked by permission

- **WHEN** doctor cannot determine state-root ownership or durability semantics
- **THEN** required probe is not-checked and aggregate exits 4
