# continuous-recorder-lifecycle

## Purpose

Define continuous recorder lifecycle, recovery, readiness, bounded epochs, and evidence obligations for supported production operation.

## Requirements

### Requirement: One foreground recorder owns one filesystem domain

Recorder service SHALL run in foreground and own exactly one configured cgroup subtree and one private state root. Preflight SHALL resolve configured WAL/store roots to a canonical filesystem-domain identity and deterministic configured domain-lock root. Recorder SHALL acquire an OS-released exclusive lock at `<domain-lock-root>/.chronicle-domain.lock` before the subordinate state-root lock, recovery, or mutation. All Chronicle mutating commands SHALL use the same domain-lock resolution and reject a live owner; read-only commands need neither lock. A filesystem domain SHALL have only one active Chronicle mutating owner; multiple recorder instances sharing the same WAL/store filesystem are unsupported. A second process targeting that domain SHALL fail without attaching capture, opening WAL for append, or changing metadata. Different cgroup scopes requiring independent recorder instances SHALL use isolated filesystem domains. Recorder SHALL NOT implement self-daemonization, PID-file liveness as authority, multi-node ownership, or a custom supervisor.

#### Scenario: First owner starts

- **WHEN** no live owner holds configured state-root lease
- **THEN** recorder acquires lease before recovery or capture attachment

#### Scenario: Concurrent owner rejected

- **WHEN** second recorder targets state root or another WAL/store path in the same filesystem domain with live owner
- **THEN** second recorder exits with stable ownership error and performs no mutation or attachment

#### Scenario: Standalone mutator rejected while recorder owns domain

- **WHEN** a standalone mutating command targets a WAL/store filesystem domain owned by a live recorder
- **THEN** command exits with stable ownership error; read-only commands may continue without acquiring mutation ownership

#### Scenario: Process dies

- **WHEN** recorder process terminates without releasing application state
- **THEN** operating system releases lease and next process determines recovery from persisted evidence rather than PID existence

### Requirement: Persistent recorder lifecycle state machine

Recorder lifecycle SHALL use states `starting`, `recovering`, `running`, `draining`, `stopped`, and `failed`. Fresh start SHALL persist `starting` before recovery. Every start SHALL enter `recovering`. Capture readiness SHALL remain false until filesystem-domain lease, state/manifest/WAL recovery, quota reservation, appendable epoch creation, and capture preflight/attachment complete. Processing readiness SHALL be reported separately and SHALL require checkpoint consistency, output-store availability, and incremental processing health. Successful capture startup SHALL transition to `running`; processing may remain not ready or degraded without forcing capture failure while safe WAL headroom remains. Graceful stop SHALL transition `running -> draining -> stopped`. Any unrecoverable startup/runtime/drain failure SHALL transition to `failed` where persistence remains possible while preserving prior recovery-authoritative WAL.

Transitions SHALL be monotonic within one process attempt, idempotent under repeated stop notification, atomically persisted, and include bounded stable failure/shutdown codes without payload/header values.

#### Scenario: Fresh successful start

- **WHEN** valid configuration and empty state root start on supported host
- **THEN** persisted states progress `starting -> recovering -> running`, capture readiness becomes true only after attachment, and processing readiness is reported independently

#### Scenario: Restart after clean stop

- **WHEN** stopped recorder starts with valid prior state
- **THEN** it enters `recovering`, verifies prior epoch/checkpoint state, opens new or resumable epoch, then becomes running with capture readiness evaluated separately from processing readiness

#### Scenario: Startup recovery fails

- **WHEN** committed WAL, retention lineage, or ETL checkpoint is contradictory
- **THEN** recorder persists failed state where possible, remains not ready, does not attach capture, and preserves evidence

#### Scenario: Repeated stop request

- **WHEN** multiple graceful-stop notifications arrive while draining
- **THEN** shutdown sequence runs once and state does not regress

### Requirement: Continuous capture uses bounded recording epochs

Recorder SHALL keep one capture source attached while rolling successive recording epochs. Each epoch SHALL retain one unique recording ID and P1-compatible WAL identity, sequence, commit-marker, loss, metadata, and recovery semantics. Epoch SHALL roll before existing one-hour duration or 4 GiB per-recording hard bound, using configured lower limits when supplied. Rollover SHALL stop admission to old epoch, commit and sync its writable prefix, open and durably publish next epoch, then direct queued/new observations to exactly one epoch without planned source detach.

An **accepted observation** is an observation that has successfully entered the recorder-owned bounded ingest queue and received its ingest identity. At admission, every observation MUST receive a stable ingest ordinal, payload length, capture timestamp, and source metadata needed for loss reporting. During handoff, recorder SHALL persist bounded durable outcome ranges mapping every admitted ordinal exactly once to old-epoch assignment only after that observation is covered by the old epoch's authoritative marker, new-epoch assignment only after the successor commits it, or metadata-only `epoch_rollover_failure`. Rollover SHALL not report success until this outcome journal is durable. If outcome persistence fails, recorder SHALL stop and leave handoff unresolved for deterministic recovery; admitted ordinals without proven assignment SHALL be counted as rollover failure, never guessed or silently dropped. Epoch rollover SHALL preserve a recorder-level monotonically increasing epoch ordinal and explicit predecessor identity. It SHALL NOT merge WAL sequence spaces or canonical sessions across recording IDs. Rollover failure SHALL stop capture and fail visibly; accepted observations that cannot reach either epoch SHALL be represented by exact metadata-only recorder-loss counts/bytes and capture-time interval/ambiguity with stable `epoch_rollover_failure` reason. Recorder SHALL NOT discard accepted observations silently or continue without durable WAL ownership.

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

- **WHEN** next epoch directory/header cannot be durably created while accepted observations remain queued
- **THEN** recorder stops intake, finalizes prior authoritative prefix where possible, records exact metadata-only rollover-loss evidence for unpersisted accepted observations, enters failed state, and reports typed storage failure

#### Scenario: Rollover outcome persistence fails

- **WHEN** outcome ranges cannot be atomically persisted during epoch handoff
- **THEN** recorder does not report successful rollover, preserves prior authoritative WAL, and recovery counts every admitted-but-unproven ordinal exactly once as `epoch_rollover_failure`

### Requirement: Split liveness, capture readiness, processing readiness, and overall health

Recorder status SHALL expose four distinct concepts:

- **Liveness:** recorder process is alive/executing.
- **Capture readiness:** lease owns filesystem domain, WAL recovery is complete, an epoch is appendable, quota reservation is valid, and capture is attached.
- **Processing readiness:** `IncrementalEtlCheckpoint v1` is consistent, output store is available, and incremental ETL can make progress.
- **Overall health:** policy-derived `healthy`, `degraded`, or `failed` result.

ETL lag, retry, or retention backlog MAY make processing/overall health degraded while capture remains ready and durable WAL headroom is safe. ETL lag MUST NOT force capture failure by default. WAL unappendability, capture detachment, corruption, unsafe quota, or checkpoint contradiction SHALL set the corresponding readiness false and expose stable remediation.

#### Scenario: Capture ready while ETL lags

- **WHEN** lease, recovered WAL, appendable epoch, quota, and capture attachment are healthy but incremental ETL is behind
- **THEN** liveness and capture readiness are true, processing readiness is degraded/not ready, and overall health follows policy without claiming capture failure

#### Scenario: Capture not ready during recovery

- **WHEN** recorder process is alive but WAL recovery or appendable-epoch preparation is incomplete
- **THEN** liveness is true, capture readiness is false, processing readiness is false or unknown, and overall health reports recovery state

#### Scenario: Recoverable quota pressure

- **WHEN** managed usage or minimum-free headroom crosses configured admission limits while recorder is running
- **THEN** recorder remains alive, preserves committed WAL, stops new admission, reports capture readiness `not_ready`, health `degraded`, and bounded `quota_pressure` metadata, continues capacity polling, and resumes admission only after headroom is restored

### Requirement: Versioned active-recorder metadata

Recorder SHALL atomically maintain private active metadata containing schema version, recorder identity, process-attempt identity, configuration digest, canonical cgroup path/stable ID/scope acknowledgement, lifecycle state, readiness, start/update times, boot/clock identity, current and previous epoch IDs/ordinals, active segment identity, final recovery-authoritative commit boundary, `IncrementalEtlCheckpoint v1` boundary and lag, retained physical bytes, quota/minimum-free-space state, categorized capture/WAL loss counters, last recovery result, and shutdown/failure code.

Metadata SHALL be non-authoritative for committed WAL bytes and SHALL be reconciled from validated WAL, manifest, and checkpoint evidence after restart. Unknown versions, immutable identity/configuration contradictions, or metadata claims ahead of authoritative evidence SHALL fail closed. Metadata SHALL record capture readiness, processing readiness, and overall health separately. Metadata and default output SHALL omit command lines, environment values, captured payload, and arbitrary header values.

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

### Requirement: Bounded lifecycle index compaction

Recorder SHALL enforce configured maximum byte and entry limits for `epochs.json`, epoch lineage history, deletion tombstones, and retention metadata. Startup recovery, capture readiness, and status queries SHALL use bounded current entries and lineage summaries and MUST NOT scan unbounded historical epochs. An epoch MAY enter compaction only after it is finalized, retained according to policy, cleanup is complete, and all deletion tombstones are durably persisted.

Compaction SHALL replace eligible history with a deterministic bounded lineage anchor preserving first retained epoch identity, last compacted epoch identity, predecessor relationship, digest-chain root/tip, cleanup summary, and enough range/count data to detect missing committed evidence, digest mismatch, lineage fork, incomplete cleanup, or conflicting/incomplete compaction. Each candidate SHALL carry monotonic compaction generation, unique transaction ID, source-index revision/digest, candidate digest, and anchor data; candidate digest SHALL cover anchor, active epochs, tombstone summary, and source metadata. Private temp-write, file sync, atomic rename, and directory sync SHALL make crash recovery validate old and candidate indexes independently. Candidate installation is allowed only when source revision/digest matches the old authoritative index and candidate digest/anchor recompute exactly; stale, incomplete, or conflicting candidates SHALL preserve old index and report stable contradiction. Protected history that cannot fit after legal compaction SHALL fail closed with bounded metadata-limit evidence; recorder SHALL stop new admission/epoch creation rather than delete protected lineage or exceed configured limits.

#### Scenario: Multi-year lifecycle index compacts

- **WHEN** finalized epochs are policy-retained, cleanup-complete, tombstones-persisted, and `epochs.json` reaches configured size or entry limits
- **THEN** recorder installs a deterministic bounded anchor with first/last epoch identities, predecessor/digest-chain proof, cleanup summary, active epochs, and contradiction-detection data without scanning all history for readiness or status

#### Scenario: Crash during lifecycle compaction

- **WHEN** process crashes before or after compaction rename or directory sync
- **THEN** recovery validates old and candidate indexes, selects the candidate only on exact source revision/digest and candidate digest match, otherwise preserves old index, and preserves lineage/tombstone proof

#### Scenario: Conflicting lifecycle compaction candidate

- **WHEN** candidate source revision/digest is stale or candidate/anchor digest conflicts with its source index
- **THEN** recovery rejects candidate, records stable contradiction, and does not guess, install partial state, or scan unrelated historical epochs

#### Scenario: Protected history exhausts metadata bound

- **WHEN** failed, unprocessed, uncertain, or `retain` history cannot fit after legal compaction
- **THEN** recorder stops new admission/epoch creation, reports stable metadata-limit failure, and does not delete protected lineage or exceed limits

### Requirement: Lifecycle acceptance is evidence-based

Rootless automated tests SHALL prove state transitions, exclusive ownership, rollover, graceful timeout, crash-point recovery, metadata reconciliation, bounded lifecycle compaction, and no duplicate epoch assignment with deterministic fake capture/WAL/storage adapters. Privileged acceptance SHALL separately prove supported Ubuntu 24.04 real eBPF continuous operation, signal handling, process restart, cgroup/eBPF cleanup, and retained machine-readable evidence tied to exact commit/environment. OpenSpec validation SHALL NOT count as runtime acceptance.

#### Scenario: Continuous operation proof

- **WHEN** privileged acceptance runs configured short epochs over sustained real HTTP workload
- **THEN** at least three epochs and multiple segments are retained with uninterrupted recorder ownership and valid committed lineage

#### Scenario: Host reboot recovery proof

- **WHEN** supported acceptance reboots host/VM while recorder state persists
- **THEN** systemd restarts Chronicle, recorder completes WAL/manifest/checkpoint/cleanup reconciliation before readiness, and committed lineage resumes without duplicate output

#### Scenario: Cleanup proof

- **WHEN** privileged recorder stops or crashes and restarts
- **THEN** acceptance verifies no leaked Chronicle process, cgroup, eBPF link/map, or temporary state artifact
