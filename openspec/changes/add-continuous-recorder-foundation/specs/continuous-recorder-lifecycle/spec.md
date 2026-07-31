## ADDED Requirements

### Requirement: One foreground recorder owns one scope and state root

Recorder service SHALL run in foreground and own exactly one configured cgroup subtree and one private state root. It SHALL acquire an OS-released exclusive lease before reading or mutating recorder state, and a second process targeting same state root SHALL fail without attaching capture, opening WAL for append, or changing metadata. Recorder SHALL NOT implement self-daemonization, PID-file liveness as authority, multi-node ownership, or a custom supervisor.

#### Scenario: First owner starts

- **WHEN** no live owner holds configured state-root lease
- **THEN** recorder acquires lease before recovery or capture attachment

#### Scenario: Concurrent owner rejected

- **WHEN** second recorder targets state root with live owner
- **THEN** second recorder exits with stable ownership error and performs no mutation or attachment

#### Scenario: Process dies

- **WHEN** recorder process terminates without releasing application state
- **THEN** operating system releases lease and next process determines recovery from persisted evidence rather than PID existence

### Requirement: Persistent recorder lifecycle state machine

Recorder lifecycle SHALL use states `starting`, `recovering`, `running`, `draining`, `stopped`, and `failed`. Fresh start SHALL persist `starting` before recovery. Every start SHALL enter `recovering`; readiness SHALL remain false until state/manifest/checkpoint reconciliation, WAL append recovery, quota reservation, appendable epoch creation, capture preflight/attachment, and incremental ETL resume complete. Successful start SHALL transition to `running`. Graceful stop SHALL transition `running -> draining -> stopped`. Any unrecoverable startup/runtime/drain failure SHALL transition to `failed` where persistence remains possible while preserving prior recovery-authoritative WAL.

Transitions SHALL be monotonic within one process attempt, idempotent under repeated stop notification, atomically persisted, and include bounded stable failure/shutdown codes without payload/header values.

#### Scenario: Fresh successful start

- **WHEN** valid configuration and empty state root start on supported host
- **THEN** persisted states progress `starting -> recovering -> running` and readiness becomes true only at `running`

#### Scenario: Restart after clean stop

- **WHEN** stopped recorder starts with valid prior state
- **THEN** it enters `recovering`, verifies prior epoch/checkpoint state, opens new or resumable epoch, then becomes running

#### Scenario: Startup recovery fails

- **WHEN** committed WAL, retention lineage, or ETL checkpoint is contradictory
- **THEN** recorder persists failed state where possible, remains not ready, does not attach capture, and preserves evidence

#### Scenario: Repeated stop request

- **WHEN** multiple graceful-stop notifications arrive while draining
- **THEN** shutdown sequence runs once and state does not regress

### Requirement: Continuous capture uses bounded recording epochs

Recorder SHALL keep one capture source attached while rolling successive recording epochs. Each epoch SHALL retain one unique recording ID and P1-compatible WAL identity, sequence, commit-marker, loss, metadata, and recovery semantics. Epoch SHALL roll before existing one-hour duration or 4 GiB per-recording hard bound, using configured lower limits when supplied. Rollover SHALL stop admission to old epoch, commit and sync its writable prefix, open and durably publish next epoch, then direct queued/new observations to exactly one epoch without planned source detach.

Epoch rollover SHALL preserve a recorder-level monotonically increasing epoch ordinal and explicit predecessor identity. It SHALL NOT merge WAL sequence spaces or canonical sessions across recording IDs. Rollover failure SHALL stop capture and fail visibly; accepted entries that cannot reach either epoch SHALL be represented by exact metadata-only recorder-loss counts/bytes and capture-time interval/ambiguity with stable `epoch_rollover_failure` reason. Recorder SHALL NOT discard accepted queue entries silently or continue without durable WAL ownership.

#### Scenario: Age epoch rollover

- **WHEN** active epoch reaches configured maximum age before byte bound
- **THEN** recorder finalizes old epoch and continues capture into next recording ID without planned eBPF detach

#### Scenario: Byte epoch rollover

- **WHEN** active epoch reaches configured maximum WAL bytes
- **THEN** recorder rolls before hard-cap admission loss whenever reserved next-epoch resources are available

#### Scenario: Observation at boundary

- **WHEN** event arrives while epoch rollover is in progress
- **THEN** bounded ingest assigns it to old or new epoch exactly once and preserves capture timestamp/loss evidence

#### Scenario: Rollover cannot open next epoch

- **WHEN** next epoch directory/header cannot be durably created while accepted entries remain queued
- **THEN** recorder stops intake, finalizes prior authoritative prefix where possible, records exact metadata-only rollover-loss evidence for unpersisted accepted entries, enters failed state, and reports typed storage failure

### Requirement: Versioned active-recorder metadata

Recorder SHALL atomically maintain private active metadata containing schema version, recorder identity, process-attempt identity, configuration digest, canonical cgroup path/stable ID/scope acknowledgement, lifecycle state, readiness, start/update times, boot/clock identity, current and previous epoch IDs/ordinals, active segment identity, final recovery-authoritative commit boundary, ETL checkpoint boundary and lag, retained physical bytes, quota/minimum-free-space state, categorized capture/WAL loss counters, last recovery result, and shutdown/failure code.

Metadata SHALL be non-authoritative for committed WAL bytes and SHALL be reconciled from validated WAL, manifest, and checkpoint evidence after restart. Unknown versions, immutable identity/configuration contradictions, or metadata claims ahead of authoritative evidence SHALL fail closed. Metadata and default output SHALL omit command lines, environment values, captured payload, and arbitrary header values.

#### Scenario: Metadata lags WAL

- **WHEN** committed marker is newer than active metadata after crash
- **THEN** recovery advances metadata from validated marker without inventing acknowledgement delivery

#### Scenario: Metadata leads WAL

- **WHEN** metadata claims segment/sequence not supported by recovery-authoritative WAL
- **THEN** recorder reports contradiction and remains not ready

#### Scenario: Unknown metadata version

- **WHEN** active metadata declares unsupported version
- **THEN** startup fails before capture attachment without compatibility guessing

### Requirement: Graceful shutdown ordering

First SIGINT or SIGTERM SHALL request graceful drain. Recorder SHALL stop new capture intake, retain capture maps for required final loss sample, drain capacity-qualified queued events, append/flush/sync final commit marker, seal current epoch/manifest state, stop incremental ETL only after its in-flight publication/checkpoint transaction reaches safe boundary, release capture resources, atomically persist final metadata with lifecycle `stopped`, then release lease. SIGINT and SIGTERM SHALL remain distinct shutdown reasons. Successful graceful shutdown SHALL exit 0.

Shutdown SHALL use documented configurable timeout. Timeout or required final-sample/WAL/manifest/checkpoint/metadata failure SHALL preserve prior committed prefix, enter failed where possible, and exit nonzero. A second signal MAY force immediate process exit after best-effort forced-termination metadata; next start SHALL treat persisted running/draining state as crash recovery, never graceful completion.

#### Scenario: Graceful SIGTERM

- **WHEN** SIGTERM arrives and all ordered shutdown steps succeed
- **THEN** recorder stops with termination-signal reason, final committed boundary/checkpoint are persisted, and no eBPF resources remain

#### Scenario: Final sample fails

- **WHEN** required post-intake loss sample fails during drain
- **THEN** recorder preserves uncertainty, fails with capture failure, and does not claim clean shutdown

#### Scenario: Second signal

- **WHEN** second signal arrives before drain completes
- **THEN** process may exit immediately and next startup recovers stale active state as forced/crash termination

### Requirement: Restart and crash recovery precede capture

On every unclean restart recorder SHALL, under exclusive lease, reconcile active metadata; recover each non-finalized epoch using existing commit-aware scan; repair only verified incomplete final frame; report/remove only allowed uncommitted suffix for append; validate or rebuild WAL manifest; reconcile incremental ETL publication/checkpoint state; complete or roll back interrupted retention transaction; verify quota headroom; and open a durably appendable epoch before attaching capture. Corruption at or before committed boundary, ambiguous retention lineage, checkpoint ahead of committed input, or conflicting publication SHALL fail closed and preserve bytes.

Restart SHALL resume append only to epoch that remains safely reopenable; otherwise it SHALL finalize recovered epoch as aborted and open a successor linked by epoch lineage. It SHALL not relabel process crash as graceful completion or infer runtime acknowledgement delivery.

#### Scenario: Crash after WAL sync

- **WHEN** process crashes after valid marker sync but before metadata/checkpoint update
- **THEN** restart recognizes committed WAL, repairs derived state idempotently, and resumes without duplicate output

#### Scenario: Partial final frame

- **WHEN** crash leaves verified incomplete final frame in final active segment
- **THEN** recovery applies existing final-tail-only repair before reopen

#### Scenario: Committed corruption

- **WHEN** restart finds checksum/reference/digest corruption within claimed committed prefix
- **THEN** recorder remains failed/not ready and does not skip, repair, delete, or attach capture

### Requirement: Configuration changes are explicit restart boundaries

Recorder configuration SHALL be loaded from one versioned file and normalized into persisted digest. Hot reload SHALL NOT be supported in P2. Restart with changed capture scope, state root identity, durability/retention semantics, or storage backend SHALL require explicit operator acknowledgement or a new state root according to documented compatibility rules; incompatible change SHALL fail before mutation. Changes to safe operational thresholds MAY apply only after recovery and SHALL be recorded with new configuration digest and effective epoch.

#### Scenario: Unchanged restart

- **WHEN** normalized configuration matches persisted digest
- **THEN** recovery proceeds without operator migration action

#### Scenario: Incompatible scope change

- **WHEN** configured cgroup scope differs from persisted active lineage
- **THEN** recorder rejects reuse of state root and does not mix recordings

### Requirement: Lifecycle acceptance is evidence-based

Rootless automated tests SHALL prove state transitions, exclusive ownership, rollover, graceful timeout, crash-point recovery, metadata reconciliation, and no duplicate epoch assignment with deterministic fake capture/WAL/storage adapters. Privileged acceptance SHALL separately prove supported Ubuntu 24.04 real eBPF continuous operation, signal handling, process restart, cgroup/eBPF cleanup, and retained machine-readable evidence tied to exact commit/environment. OpenSpec validation SHALL NOT count as runtime acceptance.

#### Scenario: Continuous operation proof

- **WHEN** privileged acceptance runs configured short epochs over sustained real HTTP workload
- **THEN** at least three epochs and multiple segments are retained with uninterrupted recorder ownership and valid committed lineage

#### Scenario: Host reboot recovery proof

- **WHEN** supported acceptance reboots host/VM while recorder state persists
- **THEN** systemd restarts Chronicle, recorder completes WAL/manifest/checkpoint/cleanup reconciliation before readiness, and committed lineage resumes without duplicate output

#### Scenario: Cleanup proof

- **WHEN** privileged recorder stops or crashes and restarts
- **THEN** acceptance verifies no leaked Chronicle process, cgroup, eBPF link/map, or temporary state artifact
