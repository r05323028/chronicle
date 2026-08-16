# continuous-recorder-lifecycle

## Purpose

Define continuous recorder lifecycle, recovery, readiness, bounded epochs, and evidence obligations for supported production operation.

## Requirements

### Requirement: One foreground recorder owns one filesystem domain

Recorder service SHALL run in foreground and own one configured cgroup subtree, private state root, and Chronicle data-domain identity. Data-domain identity SHALL include canonical filesystem identity plus one exact normalized `<domain-lock-root>/.chronicle-domain.lock` path; a physical device number alone does not make two different flock files equivalent. Recorder and every standalone mutator targeting the same configured data domain SHALL resolve and acquire that exact path before subordinate state locks, recovery, capture attachment, name reservation, WAL/catalog/sidecar mutation, ETL, publication, or retention. Read-only commands need no lock.

Public intent commands default domain-lock root to their resolved data directory. When daemon and public commands share a domain, configuration SHALL align both to one exact lock path. A command that is configured for the same data root/domain but resolves a different lock path SHALL fail preflight as incompatible rather than claim exclusion. Operating multiple Chronicle data domains on one physical filesystem with different lock paths remains unsupported and SHALL be documented; device equality SHALL NOT be reported as an acquired conflict. A second owner of the exact configured domain SHALL fail before capture, WAL append, or metadata/catalog mutation. OS process death SHALL release locks; persisted evidence, not PID existence, drives recovery.

#### Scenario: First owner starts

- **WHEN** no live owner holds the exact configured domain lock
- **THEN** recorder acquires it before subordinate state lock, recovery, or capture

#### Scenario: Concurrent owner rejected

- **WHEN** public record, second recorder, or another mutator targets same configured data domain with live owner
- **THEN** second process uses same exact lock path, exits with stable ownership error, and performs no mutation or attachment

#### Scenario: Public and daemon configuration align

- **WHEN** public record and daemon recorder resolve same data-domain identity
- **THEN** both use same exact domain-lock path

#### Scenario: Incompatible lock mapping rejected

- **WHEN** supplied public/daemon configuration names the same Chronicle data root/domain but resolves different lock paths
- **THEN** preflight rejects configuration and does not treat device identity or state lock as mutual exclusion

#### Scenario: Standalone mutator rejected while recorder owns domain

- **WHEN** standalone mutator targets configured data domain owned by live recorder
- **THEN** command exits with stable ownership error before mutation or attachment

#### Scenario: Read-only command

- **WHEN** list or inspect reads a domain owned by recorder
- **THEN** it may produce bounded reconciled output without acquiring or mutating domain lock

#### Scenario: Process dies

- **WHEN** owner process terminates without application cleanup
- **THEN** OS releases lock and next exact-domain owner recovers from persisted evidence rather than PID existence

### Requirement: Persistent recorder lifecycle state machine

Recorder lifecycle SHALL distinguish one user-visible recording/capture run from its bounded epochs and segments. Run states SHALL remain `starting`, `recovering`, `running`, `draining`, `stopped`, and `failed`. Fresh start SHALL persist `starting` before recovery; every start SHALL enter `recovering`; capture readiness SHALL remain false until domain lease, parent/epoch lineage recovery, quota reservation, an appendable epoch, and capture attachment complete. Processing readiness SHALL remain separate and may be degraded while safe durable headroom exists. Successful attachment SHALL transition the run to `running` without making the current epoch's age/byte threshold a run terminal state.

A run SHALL retain one stable `RecordingId` across all epochs and recorder process attempts. A process restart with matching scope, state root, domain identity, configuration compatibility, and no durable end intent SHALL recover that run rather than allocate a new public recording. An explicit end intent SHALL make the run terminal; a crash or host/pod loss before such intent SHALL leave the run recoverable/continuable. Transitions SHALL be monotonic within one process attempt, idempotent under repeated stop notification, atomically persisted, and include bounded safe failure/shutdown codes without payload/header values.

#### Scenario: Fresh successful run

- **WHEN** valid configuration and empty state start on a supported host
- **THEN** persisted states progress `starting -> recovering -> running`, capture readiness becomes true only after attachment, and the first epoch is linked to one stable parent `RecordingId`

#### Scenario: Epoch threshold does not stop run

- **WHEN** the active epoch reaches its configured age or byte threshold while the source remains usable and successor resources are available
- **THEN** the run remains `running`, performs one epoch rollover, and does not emit a terminal duration/WAL-limit result

#### Scenario: Restart after crash

- **WHEN** a process or host/pod disappears while the run is `running` or `recovering` and no durable end intent exists
- **THEN** the next owner enters `recovering`, preserves the same parent `RecordingId`, reconciles the active epoch, and resumes or creates exactly one linked successor

#### Scenario: Explicit end intent

- **WHEN** an operator requests administrative shutdown with continuation not expected and the end intent is durable
- **THEN** recovery does not resume capture for that run and finalization reports the explicit administrative shutdown reason

#### Scenario: Startup recovery fails

- **WHEN** committed WAL, epoch lineage, retention evidence, parent metadata, or ETL checkpoint is contradictory
- **THEN** the run persists `failed` where possible, remains not ready, does not attach capture, and preserves all authoritative evidence

#### Scenario: Repeated stop request

- **WHEN** multiple graceful-stop notifications arrive while draining
- **THEN** shutdown runs once and lifecycle state does not regress

#### Scenario: Fresh successful start

- **WHEN** valid configuration and empty state root start on supported host
- **THEN** persisted states progress `starting -> recovering -> running`, capture readiness becomes true only after attachment, and processing readiness is reported independently

#### Scenario: Restart after clean stop

- **WHEN** stopped recorder starts with valid prior state
- **THEN** it enters `recovering`, verifies prior epoch/checkpoint state, opens new or resumable epoch, then becomes running with capture readiness evaluated separately from processing readiness

### Requirement: Continuous capture uses bounded recording epochs

The recorder SHALL keep one capture source and bounded ingest queue attached while rolling successive epochs under one stable parent recording. Each epoch SHALL have a unique `EpochId`, monotonic ordinal, epoch-local WAL identity/sequence space, bounded age and physical bytes, segment lineage, per-epoch publication/checkpoint state, immutable continuation-in/out references, and explicit predecessor. The existing WAL v1 frame/header bytes and commit-marker authority SHALL remain unchanged; the UUID serialized in an epoch WAL's existing `recording_id` field is its epoch/WAL identity, not a new user-visible recording.

Epoch age and byte thresholds SHALL request rollover only. A WAL epoch boundary is a durability/recovery/quota/retention/publication boundary, not inherently a protocol reconstruction boundary. Capture/WAL rollover SHALL stop admission at a bounded handoff boundary, commit and sync the old epoch's writable prefix, durably publish/link one successor, and route queued/new observations to exactly one epoch without planned source detach. It SHALL NOT wait for incremental ETL, decoder progress, or continuation export. ETL records a separate pending dependency and may export the bounded versioned continuation later at the exact predecessor final marker. Segment size/age rotation remains inside the active epoch. A parent run has no recording-wide WAL-size or implicit one-hour limit.

An accepted observation is assigned to an epoch only after that epoch's authoritative marker covers it. Continuation state is serialized immutable evidence, not shared mutable memory. It may carry only bounded connection/generation, reassembly, partial-frame, correlation, negotiation/session, loss, and issue state supported by the protocol adapter. If it cannot be exported, verified, or restored, the affected evidence receives explicit continuation-loss/incomplete provenance; successor ETL SHALL NOT claim a clean protocol start.

An accepted observation is an observation that has successfully entered the recorder-owned bounded ingest queue and received its ingest identity. At admission, every observation MUST receive a stable ingest ordinal, payload length, capture timestamp, and source metadata needed for loss reporting. During handoff, the recorder SHALL persist bounded durable outcome ranges mapping every admitted ordinal exactly once to old-epoch assignment only after that observation is covered by the old epoch's authoritative marker, successor assignment only after the successor commits it, or metadata-only `epoch_rollover_failure`. Rollover SHALL not report success until this outcome journal is durable. If outcome persistence fails, the recorder SHALL stop and leave handoff unresolved for deterministic recovery; admitted ordinals without proven assignment SHALL be counted once as rollover failure, never guessed or silently dropped.

Rollover failure, successor creation failure, protected quota pressure, metadata-limit exhaustion, or capture/WAL failure SHALL stop/fail the run visibly. A continuation handoff failure SHALL degrade/block affected ETL and emit explicit incomplete/loss provenance while valid successor capture/WAL remains intact, unless quota/safety policy separately makes further capture unsafe. The recorder SHALL NOT discard accepted observations silently, create repeated empty epochs, merge WAL sequence spaces, silently reset protocol state, or continue without durable ownership.

#### Scenario: Age epoch rollover

- **WHEN** the active epoch reaches configured maximum age before its byte bound
- **THEN** the old epoch is finalized and a successor continues the same parent recording without planned eBPF detach

#### Scenario: Byte epoch rollover

- **WHEN** the active epoch reaches configured maximum WAL bytes and reserved successor resources are available
- **THEN** the recorder rolls before hard-cap admission loss and continues the same parent recording

#### Scenario: Repeated rollover

- **WHEN** a run crosses three or more age/byte thresholds
- **THEN** ordinals increase monotonically, each epoch has one predecessor, the parent ID remains unchanged, and no epoch is exposed as an unrelated public recording

#### Scenario: Observation at boundary

- **WHEN** an event is admitted while epoch rollover is in progress
- **THEN** its ingest ordinal is assigned exactly once to an authoritative old/successor WAL marker or to typed `epoch_rollover_failure` metadata

#### Scenario: Rollover cannot open next epoch

- **WHEN** the successor directory/header cannot be durably created while accepted observations remain queued
- **THEN** intake stops, the prior authoritative prefix is preserved, exact metadata-only rollover-loss evidence is recorded, and the run enters failed/not-ready rather than terminating as an ordinary size limit

#### Scenario: Rollover outcome persistence fails

- **WHEN** outcome ranges cannot be atomically persisted during handoff
- **THEN** successful rollover is not reported, the previous authority remains intact, and recovery counts every admitted-but-unproven ordinal exactly once

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

The first SIGINT or SIGTERM, source completion, explicit whole-run deadline, or explicit administrative end SHALL request run drain. The recorder SHALL stop new capture intake, retain capture maps for the required final loss sample, drain capacity-qualified queued events, append/flush/sync the active epoch's final commit marker, seal its manifest/lineage state, complete or safely bound in-flight ETL publication/checkpoint work, release capture resources, atomically persist final parent and epoch metadata, then release the domain lease. Epoch rollover SHALL never initiate this sequence by itself.

SIGINT, SIGTERM, source completion, deadline, administrative shutdown, and fatal capture/runtime failure SHALL remain distinct reasons. Successful graceful finalization SHALL report exit success for handled user/source/deadline stops according to the CLI contract. A required final sample, WAL, manifest, checkpoint, metadata, or downstream publication failure SHALL preserve the authoritative prefix, mark the run recoverable/failed, and exit nonzero. A second signal MAY force immediate exit; the next start SHALL treat persisted `running`/`draining` state as crash recovery unless a durable end intent proves terminal completion.

#### Scenario: User stop

- **WHEN** the user presses Ctrl-C before a child/source completes
- **THEN** the run drains once, records `user_interrupt`, finalizes the active epoch, and does not treat any prior rollover as recording termination

#### Scenario: Whole-run deadline

- **WHEN** an optional `--duration` deadline expires
- **THEN** the run drains with `duration_limit` after any completed epoch rollovers and does not terminate a selected PID/cgroup workload

#### Scenario: Source completion

- **WHEN** a supervised child or finite fixture source completes before the next epoch threshold
- **THEN** the run ends with `source_completed` and does not create a successor only to satisfy an epoch bound

#### Scenario: Final sample fails

- **WHEN** the required post-intake loss sample fails during drain
- **THEN** uncertainty remains visible, the committed prefix is preserved, and the run fails with capture failure rather than claiming clean completion

#### Scenario: Second signal

- **WHEN** a second signal arrives before drain completes
- **THEN** the process may exit immediately and the next startup recovers stale active state as forced/crash termination, not graceful completion

#### Scenario: Graceful SIGTERM

- **WHEN** SIGTERM arrives and all ordered shutdown steps succeed
- **THEN** recorder stops with termination-signal reason, final committed boundary/checkpoint are persisted, and no eBPF resources remain

### Requirement: Restart and crash recovery precede capture

On every unclean restart the recorder SHALL, under the exclusive domain lease, reconcile parent run metadata and epoch catalog; recover each non-finalized epoch with existing commit-aware WAL scanning; repair only a verified incomplete final frame; validate/rebuild manifests; reconcile per-epoch ETL publications/checkpoints; resolve any rollover transition; complete or roll back retention transactions; rebuild quota usage; and open one appendable epoch before attaching capture. Corruption at or before a committed boundary, ambiguous parent/epoch lineage, checkpoint ahead of WAL authority, conflicting publication, or unexplained missing evidence SHALL fail closed and preserve bytes.

Recovery SHALL use transition phase, parent ID, epoch IDs/ordinals, predecessor, path, metadata, WAL marker authority, and exact output/checkpoint digests to choose one active epoch. It SHALL never select by mtime, PID existence, or newest directory. It SHALL resume the same epoch only when it is safely reopenable; otherwise it SHALL finalize that epoch as recovered/aborted and create one successor linked to the same parent. It SHALL not relabel a crash as graceful completion or infer runtime acknowledgement delivery.

#### Scenario: Crash while writing current epoch

- **WHEN** a process crashes during an active epoch write
- **THEN** recovery preserves the last authoritative marker, applies only allowed final-tail repair, reconciles checkpoint/output idempotently, and resumes the same epoch or creates one linked successor without duplicate committed observations

#### Scenario: Crash immediately before rollover

- **WHEN** the threshold is reached but no durable rollover transition exists
- **THEN** the current epoch remains authoritative and recovery retries rollover without creating an empty or unrelated epoch

#### Scenario: Crash after successor allocation

- **WHEN** a successor header/directory is durable but predecessor finalization is not
- **THEN** recovery rolls back an evidence-free prepared successor with its reservation, or completes/fails closed from durable transition and WAL evidence; it never guesses

#### Scenario: Crash after predecessor finalization

- **WHEN** the predecessor boundary is durable but successor activation/catalog publication is incomplete
- **THEN** recovery completes the one prepared successor activation only after exact identity/metadata/outcome proof, otherwise preserves evidence and remains not ready

#### Scenario: ETL crash on previous epoch

- **WHEN** ETL stops after publishing or before checkpointing a finalized epoch while capture writes its successor
- **THEN** recovery adopts or retries the exact immutable output, advances that epoch checkpoint once, and keeps the raw epoch retention-protected until proof completes

#### Scenario: Host or pod restart

- **WHEN** persistent state survives a process/host/pod restart and no end intent was committed
- **THEN** recovery resumes the same parent recording lineage before capture readiness and does not allocate a new public recording solely for the restart

#### Scenario: Committed corruption

- **WHEN** restart finds checksum, reference, digest, or lineage corruption within a claimed committed prefix
- **THEN** the run remains failed/not ready and does not skip, repair, delete, or attach capture

#### Scenario: Crash after WAL sync

- **WHEN** process crashes after valid marker sync but before metadata/checkpoint update
- **THEN** restart recognizes committed WAL, repairs derived state idempotently, and resumes without duplicate output

#### Scenario: Partial final frame

- **WHEN** crash leaves verified incomplete final frame in final active segment
- **THEN** recovery applies existing final-tail-only repair before reopen

### Requirement: Configuration changes are explicit restart boundaries

Recorder configuration SHALL be loaded from one versioned file and normalized into persisted digest. Hot reload SHALL NOT be supported in the recorder. Restart with changed capture scope, state root identity, durability/retention semantics, or storage backend SHALL require explicit operator acknowledgement or a new state root according to documented compatibility rules; incompatible change SHALL fail before mutation. Changes to safe operational thresholds MAY apply only after recovery and SHALL be recorded with new configuration digest and effective epoch.

#### Scenario: Unchanged restart

- **WHEN** normalized configuration matches persisted digest
- **THEN** recovery proceeds without operator migration action

#### Scenario: Incompatible scope change

- **WHEN** configured cgroup scope differs from persisted active lineage
- **THEN** recorder rejects reuse of state root and does not mix recordings

### Requirement: Bounded lifecycle index compaction

Recorder SHALL enforce configured byte and entry limits for the parent epoch catalog, epoch lineage history, deletion tombstones, and retention metadata. Startup recovery, capture readiness, list, inspect, and status SHALL use bounded current entries and lineage summaries and MUST NOT scan unbounded historical epochs. An epoch MAY enter compaction only after it is finalized, all required downstream publication/checkpoint state is durable, retention policy permits it, cleanup is complete, and deletion tombstones are durable.

Compaction SHALL preserve stable parent ID, first retained epoch ID/ordinal, last compacted epoch ID/ordinal, predecessor/digest-chain proof, cleanup summary, aggregate counts, active tail, and enough range data to detect missing committed evidence, digest mismatch, lineage fork, incomplete cleanup, or conflicting compaction. Candidate installation SHALL use private temp-write, file sync, atomic rename, and directory sync; source revision/digest and candidate digest SHALL match exactly. Protected history that cannot fit after legal compaction SHALL stop new admission/epoch creation with bounded metadata-limit failure evidence; it SHALL not delete protected lineage or exceed configured limits.

#### Scenario: Multi-year run compacts

- **WHEN** a long-running recording reaches catalog limits after finalized cleanup-complete epochs exist
- **THEN** the catalog compacts to bounded size while preserving parent identity, epoch range, predecessor proof, active tail, and contradiction detection

#### Scenario: Crash during compaction

- **WHEN** the process crashes before or after catalog compaction rename or directory sync
- **THEN** recovery validates old and candidate indexes and selects only the complete digest-matching authority

#### Scenario: Protected history exhausts bound

- **WHEN** failed, unprocessed, uncertain, or retained history cannot fit after legal compaction
- **THEN** the recorder stops new admission/epoch creation with stable metadata-limit failure and does not delete protected lineage

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

Rootless automated tests SHALL prove state transitions, exclusive ownership, rollover, graceful timeout, crash-point recovery, metadata reconciliation, bounded lifecycle compaction, and no duplicate epoch assignment with deterministic fake capture/WAL/storage adapters. Privileged acceptance SHALL separately prove supported Ubuntu 24.04 real eBPF continuous operation, signal handling, process restart, cgroup/eBPF cleanup, and retained machine-readable evidence tied to acceptance fingerprint, compatible environment, and source provenance. Exact commit/tree identity is required only for release-eligible evidence. OpenSpec validation SHALL NOT count as runtime acceptance.

#### Scenario: Continuous operation proof

- **WHEN** privileged acceptance runs configured short epochs over sustained real HTTP workload
- **THEN** at least three epochs and multiple segments are retained with uninterrupted recorder ownership and valid committed lineage

#### Scenario: Host reboot recovery proof

- **WHEN** supported acceptance reboots host/VM while recorder state persists
- **THEN** systemd restarts Chronicle, recorder completes WAL/manifest/checkpoint/cleanup reconciliation before readiness, and committed lineage resumes without duplicate output

#### Scenario: Cleanup proof

- **WHEN** privileged recorder stops or crashes and restarts
- **THEN** acceptance verifies no leaked Chronicle process, cgroup, eBPF link/map, or temporary state artifact

### Requirement: Recording, epoch, and segment responsibilities

Chronicle SHALL document and expose the hierarchy in which a recording/capture run is the user-visible potentially unbounded lifecycle, an epoch is the bounded durability/rollover/recovery/ETL unit, and a segment is the bounded append/commit/recovery unit inside an epoch. A WAL epoch boundary SHALL NOT be treated as an automatic protocol reconstruction boundary. Supported protocol state may continue through bounded verified continuation artifacts; a failure to carry it is explicit incomplete/loss evidence.

#### Scenario: Conceptual boundary

- **WHEN** a user or operator views a recording with multiple epochs and segments
- **THEN** the output identifies one stable recording, ordered epochs, and epoch-local segments rather than unrelated recordings or one unbounded WAL

#### Scenario: Segment rotation

- **WHEN** a segment reaches its age or byte bound inside an active epoch
- **THEN** only the segment rotates under existing WAL commit/recovery rules and the recording/epoch continues

#### Scenario: Epoch rotation

- **WHEN** an epoch reaches age or bytes
- **THEN** only the epoch rolls and the parent recording remains active

### Requirement: Protocol continuation is explicit at rollover

After capture rollover establishes the predecessor final marker and successor identity, ETL lineage SHALL create at most one pending continuation dependency for that unique pair. ETL SHALL export no more than one bounded continuation checkpoint after catching up to the predecessor final marker and SHALL seed no successor from any other source. Continuation identity, pending/ready/consumed/unavailable/failed state, checksum, marker lineage, decoder version, and limits SHALL be visible in lifecycle/status state. This dependency SHALL not control successor WAL activation.

#### Scenario: Rollover keeps protocol state eligible

- **WHEN** a connection or partial operation crosses an epoch boundary and the predecessor exports a valid bounded continuation
- **THEN** successor ETL may restore that continuation without treating the connection as a new protocol session

#### Scenario: Unsupported continuation is visible

- **WHEN** protocol state exceeds bounds or the adapter cannot serialize/restore it
- **THEN** lifecycle records typed continuation loss/incompleteness and does not claim a clean successor start
