## ADDED Requirements

### Requirement: Resolver admits only exact bound native execution continuation

The existing correlation resolver SHALL recognize `NativeExecutionLineage { parent: P, relation: ExecutionContinuation }` only when it appears on child C's explicit correlation-evidence channel, passed domain validation, and names an admitted full parent reference in the same recording. The predicate SHALL be directional `C → P` and SHALL NOT match task, process, socket, connection, stream, timestamp, protocol kind, role, WAL order, processing order, or any other contextual value.

#### Scenario: Bound native parent supplies support

- **WHEN** root A is pinned to Scenario A and child A1 has valid native lineage naming A
- **THEN** the resolver receives candidate-specific positive support from A to A1
- **AND** the producer does not assign Scenario A

#### Scenario: Invalid native evidence is ignored or rejected

- **WHEN** native-shaped evidence has an orphan, cross-recording, unverified, self, or binding-ambiguous parent
- **THEN** composition/domain validation prevents it from becoming a resolver relation
- **AND** no contextual fallback creates support

#### Scenario: Direction remains child to parent

- **WHEN** A1 names A as its native parent
- **THEN** support propagates from A to A1 through the existing parent predicate
- **AND** the resolver does not infer that A owns every operation that names A's generation, task, socket, or stream

### Requirement: Native support uses existing three-phase resolver semantics

Native relations SHALL feed the existing resolver without a second selection algorithm:

1. Phase A1 SHALL use sparse monotonic support closure over previous-round snapshots.
2. Phase A2 SHALL derive outcomes, existing confidence semantics, and bounded witnesses without fabricating direct trace evidence.
3. Phase B SHALL run the fixed parent pre-filter, unique-parent mapping, complete provisional graph, global SCC analysis, and internal cyclic-edge removal sequence.

Native-only transitive ownership SHALL use existing `Strong` semantics unless an existing resolver rule independently proves another outcome. Chronicle-native origin SHALL NOT manufacture `Exact` confidence.

#### Scenario: Native-only transitive ownership is Strong

- **WHEN** one operation is supported only through valid native execution-continuation propagation
- **THEN** the resolver materializes the existing transitive `Strong` outcome
- **AND** it does not invent an `Exact` witness or direct trace relationship

#### Scenario: Support closure is snapshot-based

- **WHEN** a native chain A3 → A2 → A1 → A is processed in any order
- **THEN** each phase uses the same previous-round support snapshots as existing closure
- **AND** output does not depend on discovery order, worker scheduling, or map iteration order

#### Scenario: Parent edges use existing SCC sequence

- **WHEN** valid native relations produce direct parent candidates
- **THEN** candidates pass through pre-filter, unique-parent, complete provisional graph, SCC, and internal-edge removal in that order
- **AND** native evidence does not add a separate parent selector

### Requirement: Native and trace evidence are additive positive sources

The resolver SHALL union valid native execution-continuation support with valid Chronicle-owned trace support. Source type, count, provenance label, and arrival order SHALL NOT create weights, priority, negative votes, or tie-breaking authority. Role evidence SHALL remain excluded from correlation indexes.

#### Scenario: Native and trace agree

- **WHEN** native and trace evidence both support child A1 under Scenario A
- **THEN** the resolver keeps Scenario A under existing outcome and witness rules
- **AND** neither source is promoted to a special priority class

#### Scenario: Native and trace disagree

- **WHEN** native evidence supports Scenario A and trace evidence supports Scenario B
- **THEN** the resolver materializes existing `Ambiguous` semantics for both candidates
- **AND** neither source overrides or votes down the other

#### Scenario: Role-channel evidence remains excluded

- **WHEN** native-shaped or trace-shaped evidence appears only in role-resolution evidence
- **THEN** it does not enter native or trace relation indexes
- **AND** supplied role evidence remains preserved

### Requirement: Binding ambiguity and causal ambiguity remain distinct

The resolver SHALL receive only exact-bound native relations. A zero-match or multi-match pre-canonical anchor SHALL be represented only by a binding diagnostic and SHALL emit no relation. Resolver-level `Ambiguous` SHALL be reserved for multiple independently exact-bound positive candidates or existing resolver ambiguity, not for an unresolved identity binding.

#### Scenario: Binding ambiguity emits no resolver candidate

- **WHEN** one observation anchor maps to multiple canonical operations
- **THEN** ETL emits an ambiguous-binding diagnostic and no native evidence
- **AND** the resolver does not return causal `Ambiguous` for that missing relation

#### Scenario: True causal ambiguity remains Ambiguous

- **WHEN** two independently observed and exactly bound parent handoffs support different scenarios for one child
- **THEN** the resolver retains both positive candidates and returns existing `Ambiguous` semantics
- **AND** no source-priority rule resolves the conflict

#### Scenario: Missing observation remains Uncorrelated

- **WHEN** one active ingress exists but no exact native or trace relation reaches an egress
- **THEN** the resolver returns existing `Uncorrelated` semantics
- **AND** active-ingress uniqueness does not supply a fallback parent

### Requirement: Native resolver integration preserves roots, roles, and compatibility

Native evidence SHALL not create or accept caller-supplied `ScenarioRoot`, change root pinning, classify interaction roles, change completeness/replayability, persist scenario graphs, add `EventId`, alter frozen contracts, or introduce provider SDK types. Existing resolver input reservation and graph validation remain authoritative.

#### Scenario: Root generation remains resolver-owned

- **WHEN** native evidence supports a known ingress root
- **THEN** the existing resolver creates any valid `ScenarioRoot` output under existing placement rules
- **AND** caller-supplied root evidence remains rejected

#### Scenario: Roles remain independent

- **WHEN** native support assigns scenario ownership to an operation with supplied role evidence
- **THEN** the operation's role resolution remains unchanged
- **AND** scenario membership is not substituted for role classification

#### Scenario: Compatibility boundary remains intact

- **WHEN** native resolver support is enabled
- **THEN** Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, checkpoints, public CLI JSON, replay-safety, and default dependency closure remain unchanged
- **AND** no provider SDK type crosses Chronicle core/domain APIs
