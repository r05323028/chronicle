## MODIFIED Requirements

### Requirement: Typed canonical provenance and completeness

Canonical Session v1 SHALL represent source status/reason, connection generation, WAL ranges/commit-marker provenance, loss windows, and typed completeness. Epoch output SHALL append optional parent/epoch/completion-owner, typed `epoch_wal_ranges`, and continuation-reference provenance fields only when they preserve the Canonical Session v1 contract and remain readable when absent; legacy single-epoch `wal_ranges` remain readable. A WAL epoch boundary SHALL not mark a connection or operation complete or incomplete by itself. A pending request/connection is represented by immutable continuation evidence; the epoch where decoding terminates owns the canonical operation. Parent aggregation SHALL not merge completeness maps into a false complete operation and SHALL expose unsupported/lost continuation explicitly. Changes to field meaning, provenance integrity, or replay-relevant completeness require the explicit public compatibility process.

#### Scenario: Complete epoch operation provenance

- **WHEN** an operation is decoded from several WAL records inside one epoch
- **THEN** canonical data identifies stable parent, epoch, connection generation, source ranges, and complete state under Canonical Session v1

#### Scenario: Capture loss affects completeness

- **WHEN** an epoch reports loss overlapping or ambiguously affecting a connection
- **THEN** the affected connection/operation is degraded while provably outside evidence remains eligible

#### Scenario: Connection crosses boundary

- **WHEN** one observed connection continues after epoch rollover
- **THEN** each epoch session preserves its own typed completeness and no parent aggregate claims a complete cross-epoch operation without explicit continuation evidence

#### Scenario: Complete operation provenance

- **WHEN** an operation is decoded from several WAL records
- **THEN** canonical data identifies recording, connection generation, and all source ranges with complete state
