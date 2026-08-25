## ADDED Requirements

### Requirement: Correlation evidence may include explicit native execution lineage

Chronicle correlation evidence MAY include `NativeExecutionLineage { parent: CanonicalOperationRef, relation: ExecutionContinuation }` items. The item is valid only as candidate-relative evidence attached to the child operation's explicit correlation-evidence channel. It means a Chronicle-owned native producer observed an explicit execution handoff from the named parent operation to that child; it is non-temporal and provider-neutral. It SHALL NOT classify role, select a scenario, assign confidence, or replace `InteractionRoleResolution`.

The parent reference SHALL be complete and recording-scoped. A native item with an orphan parent, a different recording, an invalid owner epoch/session, or a parent/child scope that cannot be verified SHALL fail domain/input validation or be omitted before resolver admission; no identifier equality or processing order may fill scope. Existing `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, and `Custom` variants remain context-only until separately named predicates are specified.

#### Scenario: Native item is tagged and non-temporal

- **WHEN** a valid child correlation channel contains `NativeExecutionLineage { parent: A, relation: ExecutionContinuation }`
- **THEN** the item serializes as Chronicle-owned tagged evidence
- **AND** `is_temporal()` is false

#### Scenario: Native item is candidate-relative

- **WHEN** child A1 carries a native item naming complete parent A
- **THEN** only A is the candidate referenced by that item
- **AND** the item does not mean that every operation sharing A's task, process, socket, stream, or timestamp is a candidate

#### Scenario: Native item does not classify role

- **WHEN** a native item is present on an operation with any role resolution
- **THEN** the supplied role resolution and nested role evidence remain unchanged
- **AND** the item cannot create `Known(Ingress)` or `Known(Egress)`

#### Scenario: Invalid native scope fails closed

- **WHEN** a native item names an orphan, cross-recording, unverified, or ambiguous parent reference
- **THEN** validation rejects or omits the item before resolution
- **AND** no replacement reference is inferred

#### Scenario: Contextual variants remain non-predicates

- **WHEN** native derivation also has equal task, PID/TID, socket-generation, connection, stream, or temporal values
- **THEN** those existing evidence variants remain inspectable context only
- **AND** none independently creates native support

#### Scenario: Frozen domain contracts remain unchanged

- **WHEN** the new evidence variant is used
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public JSON, and persisted scenario artifacts remain unchanged
