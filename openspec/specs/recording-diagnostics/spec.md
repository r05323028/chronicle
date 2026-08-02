## Purpose

Safe operational diagnostics for production recording, storage, protocol, and replay prerequisites.

## Requirements

### Requirement: Typed diagnostic status model

Doctor SHALL mark each probe required or optional and represent it as `supported`, `supported_with_warnings`, `unsupported`, or `not_checked`, with stable code, safe message, and remediation. Aggregate precedence SHALL be: any required unsupported -> unsupported; else any required not-checked -> not-checked; else any warning or optional unsupported/not-checked -> supported-with-warnings; else supported. Required unsupported/not-checked SHALL exit 4; supported/warnings SHALL exit 0.

#### Scenario: Fully supported environment

- **WHEN** every required P1 probe passes without warning
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

Doctor SHALL check supplied WAL/output path creation/writability, private permission support, advisory locking, available space warning, file/data sync, one-sync in-WAL-marker group-commit prerequisites, strict physical hard-cap accounting, and atomic no-replace publication using private temporary data. If path option is omitted, corresponding path probe SHALL be optional `not_checked` with remediation and SHALL NOT invent default path. It SHALL remove probe artifacts best-effort and SHALL NOT overwrite user files.

#### Scenario: Writable private filesystem

- **WHEN** directory supports required permissions, sync, lock, and atomic rename
- **THEN** storage probe reports supported

#### Scenario: Path omitted

- **WHEN** doctor runs without WAL or output path
- **THEN** corresponding path probe is optional `not_checked`, aggregate is at most supported-with-warnings, and no filesystem path is guessed

#### Scenario: Low disk space

- **WHEN** filesystem is writable but available bytes are below documented warning threshold
- **THEN** doctor reports supported-with-warnings and required free-space guidance

#### Scenario: Atomic publication unavailable

- **WHEN** filesystem cannot provide required no-replace atomic publication behavior
- **THEN** doctor reports storage unsupported

### Requirement: Protocol and replay policy probes

Doctor SHALL report registered protocol capability availability and validate configured replay safety shape without making network connections. It SHALL confirm P1 plaintext HTTP/1.1 capability, loopback-only explicit target policy, timeout, and no-redirect behavior; missing target SHALL not be an error because replay target must be supplied per command.

#### Scenario: HTTP capability available

- **WHEN** built-in registry has HTTP detect/decode/canonicalize/replay/verify implementations
- **THEN** doctor reports plaintext HTTP/1.1 supported and all other protocols at actual status

#### Scenario: Unsafe replay config

- **WHEN** configuration attempts to provide implicit execution or broaden target authorization
- **THEN** doctor reports warning/unsupported policy code and states CLI gates cannot be supplied by config

### Requirement: Safe human and JSON output

Doctor human and JSON output SHALL include aggregate status, probe statuses/codes, detected non-sensitive versions/capabilities, and remediation in deterministic order. It SHALL not print environment values, credentials, process command lines, captured data, or arbitrary file contents.

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

Doctor SHALL add P2 probes for recorder configuration version/semantics, exclusive filesystem-domain ownership/state-root lease availability, persisted metadata/manifest/checkpoint readability and version, filesystem private modes/ownership, file/data/directory sync, same-filesystem rename/trash behavior, segment-age timer support, global quota/minimum-free-space reservation, retention eligibility policy, bounded lifecycle-index limits/compaction, filesystem RecordingStore conformance, and recorder-outside-selected-subtree placement. Probes SHALL be non-destructive or use private temporary artifacts removed best-effort; doctor SHALL not repair state, acquire long-lived ownership, delete retained artifacts, attach persistent eBPF programs, run ETL, or start recorder.

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

Application SHALL expose versioned bounded local recorder status for `healthy`, `degraded`, or `failed`, distinct from process liveness, capture readiness, and processing readiness. Report SHALL contain recorder/process-attempt/epoch/segment IDs, lifecycle/capture-readiness/processing-readiness/overall-health, configuration digest, final recovery-authoritative commit boundary, `IncrementalEtlCheckpoint v1` boundary and lag records/bytes/age, segment lifecycle counts, retained/quota/free bytes per filesystem domain, capture/WAL loss counters, last recovery/cleanup/publication result, stable failure/remediation codes, and update time.

Report SHALL be read from atomically persisted state and direct safe probes. It SHALL not parse capture payload, print headers/bodies, expose command line/environment values, claim live state from stale timestamp without warning, or treat manifest/checkpoint claim as committed authority without validation status.

#### Scenario: Running healthy

- **WHEN** capture/WAL/ETL/store are current and quota headroom is safe
- **THEN** status is healthy with capture readiness and processing readiness true and exact watermarks/counters

#### Scenario: ETL lag degraded

- **WHEN** WAL is durable but `IncrementalEtlCheckpoint v1` lag exceeds configured warning threshold
- **THEN** processing readiness is false or degraded and overall status reports lag/retention impact while capture readiness remains true and committed capture remains reported accurately

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
