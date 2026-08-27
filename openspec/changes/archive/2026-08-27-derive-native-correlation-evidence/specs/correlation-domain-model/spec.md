## ADDED Requirements

### Requirement: Correlation evidence may include explicit native execution lineage

Chronicle correlation evidence MAY include `NativeExecutionLineage { parent: CanonicalOperationRef, relation: ExecutionContinuation }`. The item SHALL be Chronicle-owned, non-temporal, provider-neutral, and valid only as candidate-relative evidence on the child operation's explicit correlation-evidence channel. It SHALL mean that a separate native producer observed an explicit execution handoff and exact binding produced the parent reference. The item SHALL NOT select a scenario, assign confidence, create a witness, select an edge, or classify interaction role.

#### Scenario: Native item is tagged and non-temporal

- **WHEN** a valid child correlation channel contains `NativeExecutionLineage { parent: A, relation: ExecutionContinuation }`
- **THEN** the item serializes as a Chronicle-owned tagged evidence value
- **AND** `is_temporal()` returns false

#### Scenario: Native item is candidate-relative

- **WHEN** child A1 carries native lineage naming complete parent A
- **THEN** A is the only candidate named by that item
- **AND** the item does not mean that every operation sharing A's task, process, socket, connection, stream, timestamp, or protocol is related

#### Scenario: Native item does not classify role

- **WHEN** native lineage is present on an operation with any supplied interaction-role resolution
- **THEN** the role resolution and nested role evidence remain unchanged
- **AND** native lineage cannot create `Known(Ingress)`, `Known(Egress)`, or role ambiguity

### Requirement: Native lineage requires exact validated reference scope

A `NativeExecutionLineage` item SHALL be admitted only when child and parent references are complete, recording-scoped, and resolvable against supplied canonical sessions. The parent SHALL be distinct from the child, the relation SHALL be `ExecutionContinuation`, and the item SHALL be rejected when parent recording, owner epoch, session, operation occurrence, or canonical lineage is invalid. Domain validation SHALL never repair a reference from a bare `OperationId`, source identifier, timing value, processing order, or contextual equality.

#### Scenario: Valid full scope is accepted

- **WHEN** a native item names a unique parent reference whose recording, owner epoch, session, and operation occurrence resolve under supplied canonical sessions
- **THEN** domain validation accepts the item in the child correlation channel
- **AND** the item remains non-temporal positive provenance only

#### Scenario: Orphan or cross-recording parent fails

- **WHEN** a native item names an orphan parent, a parent in another recording, an invalid owner epoch/session, or a duplicate operation occurrence
- **THEN** domain/input validation rejects the item or prevents it from reaching resolver admission
- **AND** no replacement reference is inferred

#### Scenario: Self-parent fails

- **WHEN** parent and child references are equal
- **THEN** domain validation rejects the native item
- **AND** it is not converted into scenario membership or a selected edge

#### Scenario: Binding diagnostics are not domain causal ambiguity

- **WHEN** a pre-canonical anchor has zero or multiple canonical matches
- **THEN** no `NativeExecutionLineage` item exists for that anchor
- **AND** the domain does not represent binding ambiguity as `CorrelationResolution::Ambiguous`

### Requirement: Native evidence remains separate from role evidence and contextual variants

`NativeExecutionLineage` SHALL be consumed only from the explicit correlation-evidence channel. It SHALL NOT be interpreted from `InteractionRoleResolution::Known.evidence`, `Unknown.evidence`, or ambiguous role-candidate evidence. Existing task, process/thread, socket/connection, protocol stream/ownership, wire direction, socket role, temporal/lifetime, and custom evidence variants remain contextual unless separately authorized by a named future requirement.

#### Scenario: Role channel cannot create native support

- **WHEN** a native-shaped value appears only in role-resolution evidence
- **THEN** role validation may preserve or reject that role evidence under existing rules
- **AND** it cannot become native correlation support

#### Scenario: Contextual variants remain context

- **WHEN** a graph contains contextual identifiers equal across parent and child but no valid native lineage
- **THEN** those values remain inspectable evidence
- **AND** domain validation does not reinterpret them as ownership

#### Scenario: Supplied role evidence is preserved

- **WHEN** valid native correlation evidence is combined with supplied role evidence
- **THEN** role classification and role provenance remain exactly as supplied
- **AND** native evidence cannot mutate or replace role evidence

### Requirement: Native lineage does not alter frozen or replay contracts

The additive native evidence variant SHALL affect only the non-frozen runtime correlation-domain surface. It SHALL NOT add fields to Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public stable JSON, replay-safety contracts, or persisted scenario artifacts, and SHALL NOT introduce `EventId`.

#### Scenario: Frozen contracts remain unchanged

- **WHEN** a validated native lineage item is present in an in-memory correlation graph
- **THEN** frozen capture, WAL, canonical-session, manifest, checkpoint, CLI, and replay contracts remain unchanged
- **AND** native side-channel loss cannot change completeness or replayability

#### Scenario: Scenario selection remains outside domain evidence

- **WHEN** a graph contains native lineage items
- **THEN** scenario IDs, confidence, ownership outcomes, and selected causal edges are produced or validated by existing graph/resolver semantics
- **AND** the evidence value itself does not persist a selected scenario
