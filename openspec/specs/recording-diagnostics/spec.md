## Purpose

Safe operational diagnostics for production recording, storage, protocol, and replay prerequisites.

## Requirements

### Requirement: Typed diagnostic status model

Doctor SHALL mark each probe required or optional and represent it as `supported`, `supported_with_warnings`, `unsupported`, or `not_checked`, with stable code, safe message, and remediation. Aggregate precedence SHALL be: any required unsupported -> unsupported; else any required not-checked -> not-checked; else any warning or optional unsupported/not-checked -> supported-with-warnings; else supported. Required unsupported/not-checked SHALL exit 4; supported/warnings SHALL exit 0.

#### Scenario: Fully supported environment

- **WHEN** every required doctor probe passes without warning
- **THEN** doctor reports supported and exits success

#### Scenario: Warning-only environment

- **WHEN** required features pass but disk space or optional evidence triggers warning
- **THEN** doctor reports supported-with-warnings and exits success with warning summary

#### Scenario: Unsupported environment

- **WHEN** required recording feature is absent
- **THEN** doctor reports unsupported with non-success exit and remediation

#### Scenario: Required probe not checked

- **WHEN** permissions prevent required non-destructive probe from deciding support
- **THEN** probe and aggregate are not-checked and doctor exits 4 rather than claiming support

#### Scenario: Optional probe not checked

- **WHEN** optional probe cannot run but all required probes pass
- **THEN** aggregate is supported-with-warnings and doctor exits 0

### Requirement: Platform and eBPF probes

Doctor SHALL check operating system, architecture, kernel version, cgroup v2, BTF, required eBPF hooks/helpers, effective privileges/capabilities, capture object/backend availability, and attach feasibility where safely testable. Selector diagnostics SHALL use **direct cgroup TGID set** to mean distinct host-visible TGIDs represented by numeric PIDs listed directly in the selected node's `cgroup.procs`, after resolving each PID to its host-visible TGID; its cardinality SHALL be **direct TGID count**. It SHALL NOT mean POSIX PGID, session ID, thread count, container ID, or descendant membership. Multiple listed PIDs resolving to one TGID SHALL count once. Descendant cgroups SHALL be counted separately as **descendant cgroup count** because attachment covers the **selected subtree**.

Record preflight SHALL reuse these diagnostics to show canonical path, inode/ID, direct TGID count, descendant cgroup count, selected-subtree scope, and acknowledgement state; reject forbidden roots, Chronicle containment anywhere in the selected subtree, unreadable/unsafe enumeration, or shared PID scope; and require `--allow-shared-cgroup` only for explicit shared cgroup. PID safety SHALL compare the selected PID's host-visible TGID with the direct cgroup TGID set and reject any unrelated direct TGID. A listed PID that exits or cannot be safely resolved SHALL NOT be omitted: required record preflight SHALL reject, and doctor SHALL report `not_checked` or rejection according to the probe's required/optional policy. Chronicle's containment check SHALL compare its own host-visible cgroup identity against the full selected subtree, not only direct `cgroup.procs` membership. Doctor/record SHALL NOT expose command lines/environment values or leave programs attached after probe failure.

#### Scenario: Missing BTF

- **WHEN** `/sys/kernel/btf/vmlinux` is unavailable or unusable
- **THEN** doctor reports stable BTF unsupported code and expected remediation

#### Scenario: Insufficient capability

- **WHEN** kernel supports capture but caller lacks attach/load privilege
- **THEN** doctor reports privilege unsupported/not-checked separately from kernel support

#### Scenario: Forbidden broad cgroup selector

- **WHEN** selector resolves root/known host-wide root or Chronicle's host-visible cgroup identity is in any node of the selected subtree even with acknowledgement
- **THEN** diagnostic reports unsupported broad-scope code and no attachment occurs

#### Scenario: Shared PID cgroup

- **WHEN** PID-resolved cgroup's direct cgroup TGID set contains another host-visible TGID
- **THEN** diagnostic rejects without offering shared acknowledgement override

#### Scenario: Explicit shared cgroup

- **WHEN** explicit selector has direct TGID count greater than one or descendant cgroup count greater than zero
- **THEN** diagnostic reports supported-with-warning only when explicit acknowledgement is present; otherwise preflight rejects

#### Scenario: Multithreaded TGID deduplication

- **WHEN** several listed PIDs resolve to one host-visible TGID
- **THEN** diagnostic reports direct TGID count one and does not use thread count or POSIX PGID

#### Scenario: PID exits during enumeration

- **WHEN** a PID from `cgroup.procs` exits or cannot be resolved safely
- **THEN** diagnostic does not omit it and reports `not_checked` or rejection under the probe's required/optional policy

#### Scenario: PID identity race

- **WHEN** PID cgroup identity changes across initial, pre-attach, or post-attach resolution
- **THEN** diagnostic fails safely, removes links, and reports expected/observed non-sensitive IDs

#### Scenario: Non-Linux development host

- **WHEN** doctor runs on macOS
- **THEN** live capture is unsupported while portable ETL/inspect/replay checks still run

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

### Requirement: Protocol and replay policy probes

Doctor SHALL report registered protocol capability availability and validate configured replay safety shape without making network connections. It SHALL confirm plaintext HTTP/1.1 capability, loopback-only explicit target policy, timeout, and no-redirect behavior; missing target SHALL not be an error because replay target must be supplied per command.

#### Scenario: HTTP capability available

- **WHEN** built-in registry has HTTP detect/decode/canonicalize/replay/verify implementations
- **THEN** doctor reports plaintext HTTP/1.1 supported and all other protocols at actual status

#### Scenario: Unsafe replay config

- **WHEN** configuration attempts to provide implicit execution or broaden target authorization
- **THEN** doctor reports warning/unsupported policy code and states CLI gates cannot be supplied by config

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

### Requirement: Doctor is diagnostic only

Doctor SHALL not create recording metadata, start capture, mutate WAL/session data, send replay traffic, repair files, or change system configuration. Failed probe SHALL not prevent independent safe probes from running unless prerequisite makes them not-checkable.

#### Scenario: One probe fails

- **WHEN** eBPF privilege probe fails but filesystem checks are independent
- **THEN** doctor continues filesystem/protocol checks and reports both results

#### Scenario: No side effects

- **WHEN** doctor completes
- **THEN** no eBPF link, recording, replay connection, or persistent probe artifact remains

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

Application SHALL expose versioned bounded local recorder status for `healthy`, `degraded`, or `failed`, distinct from process liveness, capture readiness, processing readiness, and overall health. Report SHALL contain stable parent recording ID, recorder/process-attempt ID, active epoch ID/ordinal, finalized epoch summaries, lifecycle/capture-readiness/processing-readiness/overall-health, configuration digest, final recovery-authoritative marker per visible epoch, `IncrementalEtlCheckpoint v1` compatibility and `IncrementalEtlCheckpointV2` marker/lag per epoch where applicable, continuation-in/out state (`pending`, `ready`, `consumed`, `unavailable`, or `failed`) and digest status, segment lifecycle counts, retained/quota/free bytes per filesystem domain, capture/WAL loss counters, last rollover/recovery/cleanup/publication result, stable failure/remediation codes, and observational update time.

Report SHALL be read from atomically persisted state plus direct safe probes. It SHALL distinguish committed from merely written bytes, identify whether a successor epoch is active, show predecessor ETL/retention/continuation blockers, and report capture readiness independently from processing readiness. `capture_readiness=ready`, `processing_readiness=degraded`/`waiting_for_continuation`, and `overall_health=degraded` are valid while quota/safety remains sufficient. It SHALL never claim parent completion while required epoch state is unresolved. It SHALL not parse payload, expose secrets/commands, or treat an unvalidated manifest/checkpoint/continuation as committed authority.

#### Scenario: Running healthy with active successor

- **WHEN** predecessor epochs have verified output/retention state and active successor WAL has safe headroom
- **THEN** status is healthy with active epoch capture readiness true, processing readiness true, and exact per-epoch watermarks

#### Scenario: ETL lag on finalized epoch

- **WHEN** active capture is durable but finalized epoch checkpoint lag exceeds threshold
- **THEN** capture readiness may remain true while processing readiness is degraded/`waiting_for_continuation`, lag and protected-byte impact are reported, overall health is degraded, and WAL remains protected

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
- **THEN** processing readiness is false/degraded or `waiting_for_continuation`, overall status reports lag/retention impact, capture readiness remains true while safe, and committed capture remains reported accurately

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

### Requirement: Doctor data-directory probing and actionable remediation

Doctor SHALL resolve the public data directory per `recording-identity`, report its source, and non-destructively assess existing-directory access or prospective creation/private-mode support from the nearest existing ancestor. Results SHALL be `supported`, `unsupported`, or `not_checked`; doctor SHALL NOT claim conclusive writability/private-mode support when metadata alone cannot prove it. Every probe SHALL carry a stable code, status, safe message, and an actionable remediation string that states what the user can do — for example installing capabilities, mounting cgroup v2, enabling BTF, fixing directory permissions, or adjusting replay configuration. Doctor SHALL remain strictly non-destructive: it SHALL NOT start capture, create recording metadata, repair state, send replay traffic, mutate WAL/session data, or change system configuration; a probe failure SHALL NOT prevent independent probes from running.

#### Scenario: Data directory probed

- **WHEN** doctor runs with no explicit paths
- **THEN** it reports resolved path/source plus supported/unsupported/not-checked access status without creating it

#### Scenario: Actionable remediation

- **WHEN** a required probe fails, such as missing BTF or insufficient capability
- **THEN** the probe report includes a remediation string naming the concrete fix and doctor exits per the required-probe status

#### Scenario: No side effects

- **WHEN** doctor completes on any environment
- **THEN** no capture, repair, replay traffic, metadata creation, or persistent probe artifact remains

#### Scenario: Probe independence

- **WHEN** one required probe fails but other probes are independent
- **THEN** doctor continues and reports every independent probe result

#### Scenario: Prospective access is uncertain

- **WHEN** nonexistent path access/private-mode support cannot be proved from nearest existing ancestor
- **THEN** doctor reports `not_checked` with remediation and creates no probe artifact

### Requirement: Continuation health is visible and bounded

Recorder status and ETL summaries SHALL expose per-epoch continuation-in/out state, checkpoint version, digest/lineage verification, pending connection/request counts, serialized bytes, unsupported/loss counters, and lag without payload values. Invalid or unavailable continuation SHALL degrade processing/retention readiness and SHALL never be reported as a clean successor start.

#### Scenario: Pending continuation status

- **WHEN** finalized epoch N has a durable final marker, active epoch N+1 is capturing, and predecessor ETL has not yet produced continuation-out
- **THEN** status reports bounded `pending` state, expected digest/lineage, and per-epoch processing dependency while capture readiness remains independently evaluated

#### Scenario: Continuation failure status

- **WHEN** continuation restore fails checksum, version, lineage, or bound validation
- **THEN** status reports stable failure/incompleteness and protects dependent WAL/artifacts

#### Scenario: Continuous status summary

- **WHEN** a parent recording has one active successor, one ETL-pending epoch, and one published epoch
- **THEN** status lists all three deterministic states and identifies exact blockers/counters

#### Scenario: Rollover failure

- **WHEN** successor epoch cannot acquire required lock/create sealed metadata
- **THEN** diagnostics retain predecessor committed boundary, report typed rollover failure, and do not claim successor readiness
