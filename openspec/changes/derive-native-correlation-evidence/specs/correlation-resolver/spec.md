## ADDED Requirements

### Requirement: Resolver promotes explicit native execution lineage as a named causal predicate

The existing correlation resolver SHALL recognize `NativeExecutionLineage { parent: P, relation: ExecutionContinuation }` only when it is attached to child C's explicit correlation-evidence channel, P is an admitted full reference in the same recording, and the native evidence passed domain validation. The predicate is directional `C -> P`: C inherits the complete previous-round support set of P as transitive candidate-specific support. It SHALL NOT match by task, process, socket, connection, stream, timestamp, protocol name, or any other contextual field.

For final parent construction, the same validated native relation MAY be a direct parent candidate for C and SHALL pass through the existing fixed pre-filter, unique-parent mapping, whole-graph SCC analysis, and internal-cycle-edge removal. Native relation support does not create a scenario root, change role classification, or add a second selection algorithm.

#### Scenario: Native parent propagates support

- **WHEN** root A is pinned to Scenario A and child A1 has valid native execution lineage naming A
- **THEN** A1 receives Scenario A support through the resolver's transitive closure
- **AND** the producer does not assign Scenario A itself

#### Scenario: Native-only ownership keeps existing confidence semantics

- **WHEN** a non-root operation is supported only through one or more native execution-continuation relations
- **THEN** it materializes with the resolver's existing transitive confidence semantics (`Strong`)
- **AND** the resolver does not invent a direct trace or `Exact` witness

#### Scenario: Native relation can produce one selected edge

- **WHEN** one final child has exactly one valid native parent in its resolved scenario
- **THEN** Phase B may emit one selected edge with the native relation as parent evidence
- **AND** edge construction remains resolver-owned

#### Scenario: Multiple native parents remain ambiguous

- **WHEN** valid native relations name parents in two different supported scenarios
- **THEN** support closure retains both scenarios and materializes `Ambiguous`
- **AND** no selected edge uses the ambiguous child

#### Scenario: Native cycle handling is unchanged

- **WHEN** native relations form a direct-parent cycle
- **THEN** the existing global SCC sequence removes only internal cycle edges
- **AND** ownership remains unchanged

### Requirement: Resolver combines native and trace support without source priority

The resolver SHALL union valid positive support from native execution lineage and Chronicle-owned trace relationships. Same-scenario evidence MAY corroborate one outcome, but source counts and source type SHALL NOT affect confidence or selection. Different-scenario support SHALL remain `Ambiguous`; competing direct-parent candidates SHALL follow existing unique-parent and cycle rules. Role evidence remains excluded from both relation indexes.

#### Scenario: Same scenario has two evidence sources

- **WHEN** native lineage and a trace relationship both support child A1 under Scenario A
- **THEN** the result remains Scenario A with existing confidence/witness rules
- **AND** both source provenances remain inspectable where retention permits

#### Scenario: Sources support different scenarios

- **WHEN** native lineage supports Scenario A and trace evidence supports Scenario B
- **THEN** the result is `Ambiguous(A, B)`
- **AND** neither source overrides the other

#### Scenario: Role-channel native item is ignored

- **WHEN** a native-shaped item appears only inside role-resolution evidence
- **THEN** it cannot add resolver support
- **AND** caller input reservation and role-channel preservation remain unchanged

### Requirement: Contextual native facts remain outside resolver predicates

The resolver SHALL continue to treat `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, and `Custom` as contextual-only for native correlation. Adding `NativeExecutionLineage` SHALL NOT authorize a fallback predicate or an absence-of-contradiction rule for any of those variants.

#### Scenario: Context-only evidence stays uncorrelated

- **WHEN** a child has only equal task, PID/TID, socket, connection, stream, protocol, or timing evidence with a root
- **THEN** the child remains `Uncorrelated`
- **AND** no native parent or scenario edge is emitted

#### Scenario: Explicit relation is required

- **WHEN** contextual facts coexist with no valid `NativeExecutionLineage` or trace relationship
- **THEN** contextual facts remain retained evidence only
- **AND** the resolver does not use them as a fallback
