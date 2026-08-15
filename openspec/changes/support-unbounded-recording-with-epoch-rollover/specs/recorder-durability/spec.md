## MODIFIED Requirements

### Requirement: Startup recovery is lease-owned

Recorder startup SHALL acquire the exclusive exact domain lease and quota authorities before mutating parent run/epoch catalog state, rollover transition state, reopening or creating epoch WAL writers, reconciling per-epoch checkpoints, publishing active metadata, or attaching capture. Read-only catalog/manifest inspection may occur before ownership only when it cannot mutate or claim authority.

#### Scenario: Prepared tail recovery requires ownership

- **WHEN** startup finds a prepared successor epoch or interrupted parent transition
- **THEN** catalog/transition loading remains read-only until lease and quota authorities are owned, and recovery completes or rolls back the tail under those authorities

#### Scenario: Recovery failure preserves predecessor

- **WHEN** WAL, parent/epoch metadata, quota, or checkpoint validation fails during startup
- **THEN** capture attachment does not occur and predecessor/parent metadata and evidence remain immutable

### Requirement: Rollover is crash-recoverable

Epoch rollover SHALL persist enough checksummed transition state to recover or roll back successor creation, epoch-local WAL appendability, accepted-observation outcome ranges, predecessor continuation export, catalog activation, parent/epoch metadata publication, and quota reservations after any process interruption. The parent-aware transition SHALL bind stable parent `RecordingId`, old/new `EpochId`, old/new ordinals, paths, predecessor, reservation identity, continuation digest, and monotonic phases including `prepared`, `successor_created`, `boundary_committed`, `continuation_checkpointed`, and `metadata_published`.

Exactly one successor may become active for one predecessor. A prepared/evidence-free successor may be rolled back; a successor or continuation artifact with committed WAL, metadata, or outcome evidence is never deleted by guesswork. Predecessor identity and committed WAL remain immutable after boundary proof. Successful rollover means the old outcome journal, any required continuation checkpoint, and successor activation are durable; it does not mean the parent recording ended. The transition journal is in-flight proof only; `epochs.json` remains topology authority.

#### Scenario: Successor activation

- **WHEN** an active epoch reaches a configured age/size boundary with committed evidence and successor resources are reserved
- **THEN** exactly one successor becomes active under the recorder lease, the parent ID remains stable, and predecessor identity/lineage remain immutable

#### Scenario: Empty rollover

- **WHEN** a time boundary occurs without committed or admitted evidence
- **THEN** no durable empty epoch remains active or catalogued solely because a timer elapsed

#### Scenario: Quota denial

- **WHEN** successor reservation or creation is denied by quota
- **THEN** predecessor authority remains intact, the parent enters controlled pressure/failure state, and all temporary reservations/artifacts are released or retained as recoverable evidence

#### Scenario: Crash after successor allocation

- **WHEN** the process stops after successor directory/header creation but before predecessor boundary commitment
- **THEN** recovery rolls back only an evidence-free prepared successor or completes the exact journaled transition; it never allocates a competing successor

#### Scenario: Crash after predecessor finalization

- **WHEN** predecessor boundary and outcome evidence are durable but catalog activation or successor metadata is incomplete
- **THEN** recovery activates the matching prepared successor only after exact identity/digest validation, otherwise remains failed/not ready with both evidence sets preserved

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

## ADDED Requirements

### Requirement: Active epoch authority is unique within a recording run

At any durable point a parent run SHALL have zero or one prepared successor and exactly one active epoch after recovery. The active epoch is selected from validated catalog state plus transition/WAL evidence; ETL publication state never makes an inactive or unrelated epoch active.

#### Scenario: Multiple active claims

- **WHEN** catalog, metadata, or transition files claim two active successors for one parent
- **THEN** recovery fails closed before capture and preserves all conflicting evidence

#### Scenario: Recovery converges

- **WHEN** recovery is run twice after completing one valid rollover transition
- **THEN** both runs select the same active epoch, parent ID, predecessor, checkpoint lineage, and quota state without duplicate successor or publication

### Requirement: Continuation handoff is durable before successor activation

The predecessor continuation checkpoint SHALL be written atomically after the predecessor commit marker and before successor activation when pending protocol state exists. It SHALL bind the stable parent, predecessor and successor IDs/ordinals, decoder/schema/pipeline versions, exact marker/segment lineage, bounded state/counters, and checksum. Recovery SHALL restore it at most once from the unique predecessor; it SHALL never mutate a published predecessor session or select an unrelated checkpoint.

#### Scenario: Continuation checkpoint before activation

- **WHEN** predecessor marker is durable and pending decoder state crosses into the allocated successor
- **THEN** continuation checkpoint becomes durable and referenced by the transition before successor activation

#### Scenario: Crash after continuation checkpoint

- **WHEN** process crashes after continuation checkpoint publication but before successor catalog activation
- **THEN** recovery verifies the checkpoint and completes exactly one matching activation/seed or fails closed without starting a clean decoder

#### Scenario: Invalid continuation handoff

- **WHEN** checksum, predecessor/successor identity, decoder version, or marker lineage does not match
- **THEN** recovery preserves evidence, blocks readiness/retention, and emits explicit continuation-loss provenance

#### Scenario: Repeated handoff recovery

- **WHEN** recovery/retry observes the same predecessor boundary more than once
- **THEN** it adopts one matching continuation and successor seed without duplicate state or publication
