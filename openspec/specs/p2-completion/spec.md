## Purpose

P2 recorder durability, recovery, ETL, quota, retention, and privileged acceptance requirements.

## Requirements

### Requirement: Startup recovery is lease-owned

Recorder startup SHALL acquire exclusive domain lease and quota authorities before mutating epoch catalog state, reopening or creating WAL writers, reconciling checkpoints, publishing active metadata, or attaching capture sources.

#### Scenario: Prepared tail recovery requires ownership

- **WHEN** startup finds a prepared successor epoch
- **THEN** catalog loading remains read-only until lease and quota authorities are owned
- **AND THEN** recovery either completes or rolls back the prepared tail under those authorities

#### Scenario: Recovery failure preserves predecessor

- **WHEN** WAL, metadata, quota, or checkpoint validation fails during startup
- **THEN** capture attachment does not occur
- **AND THEN** predecessor metadata and evidence remain immutable

### Requirement: Rollover is crash-recoverable

Epoch rollover SHALL persist enough transition state to recover or roll back successor creation, catalog activation, metadata publication, WAL appendability, and quota reservations after any process interruption.

#### Scenario: Successor activation

- **WHEN** an active epoch reaches a configured size or age boundary with committed evidence
- **THEN** exactly one successor becomes active under the recorder lease
- **AND THEN** predecessor identity and lineage remain immutable

#### Scenario: Empty rollover

- **WHEN** a time boundary occurs without committed or admitted evidence
- **THEN** no durable empty epoch remains active or catalogued

#### Scenario: Quota denial

- **WHEN** successor reservation or creation is denied by quota
- **THEN** the predecessor remains active or enters explicit terminal state
- **AND THEN** all temporary reservations and successor artifacts are released or retained as recoverable evidence

### Requirement: Checkpoint recovery is authoritative before capture

A checkpoint SHALL validate against the committed WAL recording identity, epoch, marker, segment lineage, pipeline, configuration, and referenced output artifacts before capture source attachment.

#### Scenario: Ahead or forked checkpoint

- **WHEN** checkpoint lineage is ahead of or diverges from committed WAL evidence
- **THEN** startup fails closed before invoking capture source `start`
- **AND THEN** the checkpoint and WAL evidence remain available for diagnosis

#### Scenario: Output committed before checkpoint

- **WHEN** a delta output is durable but checkpoint publication was interrupted
- **THEN** restart adopts or deterministically reconciles the verified output
- **AND THEN** retry does not duplicate or rewrite immutable output

### Requirement: Epoch ETL preserves independence and incremental equivalence

ETL SHALL process each recording epoch independently, preserve incomplete boundary evidence without borrowing predecessor protocol identity, and publish stopped sessions from verified incremental lineage.

#### Scenario: Boundary-incomplete socket evidence

- **WHEN** successor payload lacks trusted endpoint evidence at the recording boundary
- **THEN** successor output is typed incomplete and non-replayable
- **AND THEN** predecessor connection, envelope, and decoder state are not injected

#### Scenario: Incremental and one-shot equivalence

- **WHEN** an epoch is finalized after incremental delta publication
- **THEN** verified incremental reconstruction equals clean one-shot ETL output
- **AND THEN** the reconstructed session is published as the authoritative final session

### Requirement: Quota and retention protect durable evidence

Production quota accounting SHALL cover every durable transaction peak, rebuild deterministically after restart, and distinguish recoverable pressure from terminal pressure. Retention SHALL delete only proof-eligible finalized evidence and SHALL preserve corrupt or protected lineage.

#### Scenario: Recoverable quota pressure

- **WHEN** minimum-free-space protection blocks new work but cleanup can restore headroom
- **THEN** recorder remains alive, rejects new admission, and reports degraded pressure

#### Scenario: Terminal quota pressure

- **WHEN** required headroom cannot be restored without deleting protected or corrupt evidence
- **THEN** recorder enters explicit terminal failure and does not claim readiness

#### Scenario: Protected cleanup

- **WHEN** cleanup encounters protected, corrupt, or unresolved lineage
- **THEN** deletion is refused and evidence remains byte-identical after restart

### Requirement: Privileged acceptance proves P2 truthfully

Privileged acceptance SHALL execute or compatibly reuse evidence for real process interruption, restart, reboot, quota, corruption, cleanup, replay, and resource-cleanup scenarios through unified profile runner; retain artifacts under `acceptance/<profile>/<fingerprint>/<run-id>/`; verify validation fingerprint and validation-relevant content, compatible evidence schema, required scenario coverage, compatible environment, artifact manifest integrity, and every required check; and fail when any required check failed, is `not_checked`, timed out, or is incomplete. Git commit SHA SHALL be retained as provenance metadata but SHALL NOT by itself invalidate otherwise compatible acceptance evidence or determine release eligibility.

#### Scenario: Development provenance report

- **WHEN** normal privileged acceptance completes, including with uncommitted source
- **THEN** report records commit SHA, tree SHA when known, acceptance fingerprint, dirty state, environment, and run identity
- **AND THEN** status reflects actual scenario results without using commit equality as evidence identity

#### Scenario: Release-grade content identity

- **WHEN** `scripts/acceptance.sh --profile p2 --executor multipass --release` executes or reuses evidence from a clean, identifiable current checkout
- **THEN** report is `release_eligible` only when current source remains unchanged and fingerprint, validation contract, compatible schema and environment, manifest, required scenario coverage, and every required check are complete and passed
- **AND THEN** historical evidence commit and tree SHA may differ from the current checkout

#### Scenario: Release request integrity

- **WHEN** any release request starts
- **THEN** current commit and tree SHALL be known, current working tree SHALL be clean, and start/end fingerprint, commit, tree, dirty state, and identity status SHALL remain unchanged through validation
- **AND THEN** release fails before reused evidence is accepted when current source is dirty or unidentifiable, and fails if current source changes during validation
- **AND THEN** current-checkout integrity does not impose commit or tree equality on historical evidence provenance

#### Scenario: Crash and cleanup evidence

- **WHEN** recorder or cleanup is interrupted at a durable boundary
- **THEN** restart convergence, immutable evidence, protected lineage, process cleanup, cgroup cleanup, and eBPF cleanup are machine-validated

#### Scenario: Compatible evidence reuse

- **WHEN** prior P2 run has matching fingerprint, compatible schema and environment, complete valid manifest, unchanged required scenario set, and all required scenarios and checks passed without timeout or incomplete state
- **THEN** it may satisfy P2 release eligibility or P1 request when P1 scenarios are subset
- **AND THEN** P1 evidence never satisfies P2 and commit equality is not required

#### Scenario: Reboot evidence

- **WHEN** supported Linux VM reboots during recorder operation
- **THEN** post-reboot markers, checkpoint lineage, catalog identity, counters, and WAL digests prove monotonic continuation
