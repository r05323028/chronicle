## Purpose

Recorder durability, recovery, ETL, quota, retention, and privileged acceptance requirements.

## Requirements

### Requirement: Startup recovery is lease-owned

Recorder startup SHALL acquire the exclusive exact domain lease and quota authorities before mutating parent run/epoch catalog state, rollover transition state, reopening or creating epoch WAL writers, reconciling per-epoch checkpoints, publishing active metadata, or attaching capture. Read-only catalog/manifest inspection may occur before ownership only when it cannot mutate or claim authority.

#### Scenario: Prepared tail recovery requires ownership

- **WHEN** startup finds a prepared successor epoch or interrupted parent transition
- **THEN** catalog/transition loading remains read-only until lease and quota authorities are owned, and recovery completes or rolls back the tail under those authorities

#### Scenario: Recovery failure preserves predecessor

- **WHEN** WAL, parent/epoch metadata, quota, or checkpoint validation fails during startup
- **THEN** capture attachment does not occur and predecessor/parent metadata and evidence remain immutable

### Requirement: Rollover is crash-recoverable

Epoch rollover SHALL persist enough checksummed transition state to recover or roll back successor creation, epoch-local WAL appendability, admitted-observation outcome ranges, catalog activation, parent/epoch metadata publication, and quota reservations after any process interruption. The parent-aware `RolloverTransition` SHALL bind stable parent `RecordingId`, old/new `EpochId`, old/new ordinals, paths, predecessor boundary/outcome digests, and reservation identity. Its monotonic capture/topology phases SHALL be `prepared`, `successor_created`, `boundary_committed`, `topology_activated`, and `complete`; it SHALL have no ETL-continuation completion phase or predicate.

Exactly one successor may become active for one predecessor. A prepared/evidence-free successor may be rolled back; a successor or WAL/outcome artifact with committed evidence is never deleted by guesswork. Predecessor identity and committed WAL remain immutable after boundary proof. Successful capture rollover means predecessor boundary/outcome, successor WAL handoff, and authoritative topology activation are durable; continuation may still be pending in ETL. The transition journal is in-flight proof only; `epochs.json` remains topology authority.

#### Scenario: Successor activation

- **WHEN** an active epoch reaches a configured age/size boundary with committed evidence and successor resources are reserved
- **THEN** exactly one successor becomes active under the recorder lease, the parent ID remains stable, predecessor identity/lineage remain immutable, and no ETL cursor or continuation artifact is required for activation

#### Scenario: Empty rollover

- **WHEN** a time boundary occurs without committed or admitted evidence
- **THEN** no durable empty epoch remains active or catalogued solely because a timer elapsed

#### Scenario: Quota denial

- **WHEN** successor reservation or creation is denied by quota
- **THEN** predecessor authority remains intact, parent enters controlled pressure/failure state, and temporary reservations/artifacts are released or retained as recoverable evidence

#### Scenario: Crash after successor allocation

- **WHEN** process stops after successor directory/header creation but before predecessor boundary commitment
- **THEN** recovery rolls back only an evidence-free prepared successor or completes the exact capture journaled transition; it never allocates a competing successor and never consults ETL continuation

#### Scenario: Crash after predecessor finalization

- **WHEN** predecessor boundary and outcome evidence are durable but topology activation or successor metadata is incomplete
- **THEN** recovery activates the matching prepared successor only after exact identity/digest validation, otherwise remains failed/not ready with both evidence sets preserved; predecessor ETL progress is not a prerequisite

### Requirement: Checkpoint recovery is authoritative before capture

A per-epoch incremental checkpoint SHALL validate against the committed WAL epoch identity, stable parent recording identity, epoch ID/ordinal, marker, segment lineage, pipeline, configuration, decoder state, and referenced output artifacts before capture source attachment. A parent run may become capture-ready while a finalized prior epoch is still ETL-degraded only when current epoch recovery/quota remains safe.

#### Scenario: Ahead or forked checkpoint

- **WHEN** an epoch checkpoint is ahead of or diverges from its committed WAL/parent/epoch evidence
- **THEN** startup fails closed before invoking capture source `start`, and checkpoint/WAL evidence remains available for diagnosis

#### Scenario: Output committed before checkpoint

- **WHEN** a finalized epoch delta or session output is durable but its checkpoint publication was interrupted
- **THEN** restart adopts or deterministically reconciles the verified matching output, advances that epoch checkpoint once, and does not duplicate output

#### Scenario: Previous epoch checkpoint lag

- **WHEN** a prior epoch checkpoint is temporarily unavailable but the active successor has safe WAL headroom
- **THEN** capture readiness may remain true, processing status reports bounded degradation, and prior raw evidence remains retention-protected

### Requirement: Epoch ETL preserves independence and incremental equivalence

ETL SHALL process each epoch as an independent WAL snapshot, immutable publication, checkpoint, and retention unit while preserving one protocol reconstruction lineage through bounded continuation checkpoints. A successor processor MAY restore only the unique verified predecessor continuation; it SHALL NOT infer state from epoch order, wall-clock time, filesystem mtime, or unrelated output. Each finalized epoch emits completed operations and, when needed, an immutable continuation-out artifact. Parent aggregation SHALL order verified epoch sessions without merging mutable decoder state or epoch-local WAL sequences. A finalized epoch SHALL be eligible for parent publication/retention only after verified incremental lineage is equivalent to clean one-shot ETL for the same parent/epoch input and continuation seed.

#### Scenario: Boundary-incomplete socket evidence

- **WHEN** successor payload lacks trusted endpoint evidence at an epoch boundary
- **THEN** successor output is typed incomplete/non-replayable and predecessor connection/decoder state is not injected

#### Scenario: Incremental and one-shot equivalence

- **WHEN** an epoch is finalized after incremental delta publication
- **THEN** verified incremental reconstruction equals clean one-shot ETL for that epoch, and the parent index records one corresponding immutable epoch session

#### Scenario: Capture continues during prior ETL

- **WHEN** ETL processes a finalized epoch while the successor captures
- **THEN** publication/checkpoint progress for the prior epoch proceeds independently and does not require stopping or detaching the successor capture source

#### Scenario: Parent aggregation order

- **WHEN** multiple epoch sessions are published in different worker completion order
- **THEN** the parent view orders them by validated epoch ordinal and never by publication time

### Requirement: Quota and retention protect durable evidence

Production quota accounting SHALL cover every durable transaction peak, including active and finalized epochs, successor rollover, parent/epoch indexes, ETL staging, immutable outputs, final session publication, and cleanup trash. It SHALL rebuild deterministically after restart and distinguish recoverable pressure from terminal pressure. Retention SHALL delete only proof-eligible finalized epoch evidence and SHALL preserve corrupt, uncertain, failed, or protected parent lineage.

#### Scenario: Recoverable quota pressure

- **WHEN** minimum-free-space protection blocks successor/ETL work but eligible cleanup can restore headroom
- **THEN** the recorder remains alive or degraded, rejects new admission until cleanup/recovery restores capacity, and reports pressure without deleting protected data

#### Scenario: Terminal quota pressure

- **WHEN** required headroom cannot be restored without deleting protected/uncertain evidence or violating parent lineage
- **THEN** the parent enters explicit terminal failure/not-ready, stops capture, and does not claim a clean rollover or complete recording

#### Scenario: Protected cleanup

- **WHEN** cleanup encounters protected, corrupt, unresolved, or lineage-conflicting epoch evidence
- **THEN** deletion is refused and parent/epoch evidence remains byte-identical after restart

### Requirement: Privileged acceptance proves recorder operation truthfully

Privileged acceptance SHALL execute or compatibly reuse evidence for real process interruption, restart, reboot, quota, corruption, cleanup, replay, and resource-cleanup scenarios through unified profile runner; retain artifacts under `acceptance/<profile>/<fingerprint>/<run-id>/`; verify validation fingerprint and validation-relevant content, compatible evidence schema, required scenario coverage, compatible environment, artifact manifest integrity, and every required check; and fail when any required check failed, is `not_checked`, timed out, or is incomplete. Every destructive scenario SHALL establish explicit preconditions and SHALL NOT pass until its asynchronous postconditions converge, including removal of scenario-owned pressure, pause/control markers, temporary mounts/loop devices, stale ownership, pending checkpoint/cleanup state, or unstable service state as applicable. Git commit SHA SHALL be retained as provenance metadata but SHALL NOT by itself invalidate otherwise compatible acceptance evidence or determine release eligibility.

#### Scenario: Development provenance report

- **WHEN** normal privileged acceptance completes, including with uncommitted source
- **THEN** report records commit SHA, tree SHA when known, acceptance fingerprint, dirty state, environment, run identity, and scenario execution status
- **AND THEN** status reflects actual scenario results without using commit equality as evidence identity

#### Scenario: Release-grade content identity

- **WHEN** `scripts/acceptance.sh --profile recorder --executor multipass --release` executes or reuses evidence from clean, identifiable current checkout
- **THEN** report is `release_eligible` only when current source remains unchanged and fingerprint, validation contract, compatible schema and environment, manifest, required scenario coverage, every required check, and every scenario postcondition are complete and passed
- **AND THEN** historical evidence commit and tree SHA may differ from current checkout

#### Scenario: Release request integrity

- **WHEN** any release request starts
- **THEN** current commit and tree SHALL be known, current working tree SHALL be clean, and start/end fingerprint, commit, tree, dirty state, and identity status SHALL remain unchanged through validation
- **AND THEN** release fails before reused evidence is accepted when current source is dirty or unidentifiable, and fails if current source changes during validation
- **AND THEN** current-checkout integrity does not impose commit or tree equality on historical evidence provenance

#### Scenario: Crash and cleanup evidence

- **WHEN** recorder or cleanup is interrupted at durable boundary
- **THEN** restart convergence, immutable evidence, protected lineage, process cleanup, cgroup cleanup, eBPF cleanup, absence of pause/control files, and stable service ownership are machine-validated before scenario passes

#### Scenario: Quota pressure convergence

- **WHEN** isolated quota or minimum-free pressure is introduced and then removed
- **THEN** acceptance waits for pressure entry and recovery conditions and does not pass until recorder state is stable, filler is absent, unit is stopped, mount is absent, loop device is detached, and scenario cgroup is removed

#### Scenario: Checkpoint recovery convergence

- **WHEN** recorder is killed at publication/checkpoint boundary
- **THEN** acceptance waits for fresh recorder readiness and consistent checkpoint/output lineage with no pending checkpoint artifact before scenario passes

#### Scenario: Compatible evidence reuse

- **WHEN** a prior recorder run has matching fingerprint, compatible schema and environment, complete valid manifest, unchanged required scenario set, and all required scenarios and checks passed without timeout, incomplete state, or unresolved postcondition
- **THEN** it may satisfy recorder release eligibility or a live-capture request when live-capture scenarios are a subset
- **AND THEN** live-capture evidence never satisfies recorder and commit equality is not required

#### Scenario: Reboot evidence

- **WHEN** supported Linux VM reboots during recorder operation
- **THEN** pre-reboot handoff proves clean stop, post-reboot fresh readiness is observed, and checkpoint lineage, catalog identity, counters, and WAL digests prove monotonic continuation

### Requirement: Active epoch authority is unique within a recording run

At any durable point a parent run SHALL have zero or one prepared successor and exactly one active epoch after recovery. The active epoch is selected from validated catalog state plus transition/WAL evidence; ETL publication state never makes an inactive or unrelated epoch active.

#### Scenario: Multiple active claims

- **WHEN** catalog, metadata, or transition files claim two active successors for one parent
- **THEN** recovery fails closed before capture and preserves all conflicting evidence

#### Scenario: Recovery converges

- **WHEN** recovery is run twice after completing one valid rollover transition
- **THEN** both runs select the same active epoch, parent ID, predecessor, checkpoint lineage, and quota state without duplicate successor or publication

### Requirement: Continuation handoff is ETL-owned and asynchronous

The predecessor-to-successor continuation dependency SHALL be maintained separately from WAL/topology authority. Once the predecessor final marker and successor identity are durable, ETL SHALL create a bounded checksummed dependency record in `pending` state without requiring the predecessor decoder to have processed to that marker. ETL SHALL transition it to `ready` only after `IncrementalEtlCheckpoint` catches up exactly to the predecessor final marker and emits one lineage-verified `EpochContinuationCheckpoint`; successor ETL may then consume it once and record `consumed`. Unsupported, corrupt, over-limit, or retry-exhausted state uses explicit `unavailable`/`failed` processing semantics.

Continuation state, checkpoint digests, and decoder progress SHALL never decide whether an epoch exists, whether `epochs.json` may activate a successor, or whether successor capture may write WAL. A pending continuation may make successor processing not-ready while capture remains ready. Invalid continuation preserves valid successor WAL/topology and produces explicit incomplete/loss provenance rather than a clean decoder reset.

#### Scenario: Continuation is pending after capture activation

- **WHEN** predecessor WAL is sealed at marker 1,000, successor capture is active, and predecessor ETL has processed only to marker 900
- **THEN** continuation state is `pending`, successor WAL may continue to commit, successor ETL reports blocked/degraded processing, and capture does not wait for ETL catch-up

#### Scenario: Crash after successor capture activation while continuation is pending

- **WHEN** process crashes after successor topology/WAL activation but before predecessor ETL produces continuation-out
- **THEN** recovery restores the same parent/epoch topology and active successor, preserves the pending ETL dependency, and resumes capture whenever lease/WAL/quota safety holds

#### Scenario: ETL crash while predecessor catches up

- **WHEN** predecessor ETL crashes between checkpoint progress and continuation publication
- **THEN** retry resumes from the last verified `IncrementalEtlCheckpoint` cursor and emits the same continuation artifact at the same predecessor final marker or records explicit processing failure

#### Scenario: Continuation becomes ready after successor WAL exists

- **WHEN** successor already contains committed WAL before predecessor ETL reaches its final marker
- **THEN** successor ETL verifies and consumes the continuation first, then processes accumulated successor WAL deterministically from its own checkpoint

#### Scenario: Invalid continuation preserves capture evidence

- **WHEN** continuation checksum, identity, marker lineage, version, or bound validation fails
- **THEN** successor capture evidence and topology remain intact; ETL fails closed or emits incomplete/loss provenance and never invalidates valid WAL

#### Scenario: Repeated continuation recovery

- **WHEN** ETL recovery observes the same predecessor final marker more than once
- **THEN** one deterministic continuation identity is adopted/consumed without duplicate decoder state, output, or publication
