## Purpose

Define Chronicle's framework-neutral correlation domain for application-relative interaction roles, durable references across canonical sessions and epochs, and explicit ownership of resolved, ambiguous, and uncorrelated outcomes. The capability validates supplied resolutions; it does not implement correlation selection.

## ADDED Requirements

### Requirement: Chronicle owns application-relative interaction role

Chronicle SHALL define two separate protocol-neutral concepts. `InteractionRole` SHALL contain only the known application-relative roles `Ingress` and `Egress`. `InteractionRoleResolution` SHALL classify each correlation input as `Known(Ingress)`, `Known(Egress)`, `Unknown`, or `Ambiguous` with competing role candidates and evidence. Uncertainty SHALL be classification state, not an `InteractionRole` variant. Role semantics SHALL describe the logical interaction's relationship to the recorded application and SHALL NOT be inferred from packet direction or byte-flow direction alone.

#### Scenario: Known passive HTTP server interaction is ingress

- **WHEN** validated passive application-server evidence shows a remote client HTTP exchange accepted by the recorded application
- **THEN** the operation's role resolution is `Known(Ingress)`
- **AND** the classification describes remote client to recorded application ownership, not individual HTTP message direction

#### Scenario: Known active HTTP client interaction is egress

- **WHEN** validated outbound HTTP client evidence shows the recorded application actively connecting to a remote HTTP service
- **THEN** the operation's role resolution is `Known(Egress)`
- **AND** the classification remains egress even though response bytes travel from server to client on the wire

#### Scenario: Known dependency interaction is egress

- **WHEN** validated active evidence shows the recorded application connecting to PostgreSQL, MySQL, Redis, or another database/cache dependency
- **THEN** the operation's role resolution is `Known(Egress)`
- **AND** the dependency protocol name does not change the application-relative role

#### Scenario: Bidirectional payload does not change a known role

- **WHEN** one logical HTTP, database, or cache interaction contains payload bytes in both wire directions
- **THEN** a known role remains the same `Known(Ingress)` or `Known(Egress)` value
- **AND** individual request/response byte directions do not switch it between roles

#### Scenario: Direction does not solve an unknown role

- **WHEN** an operation has no sufficient application-relative ownership evidence and only `ClientToServer`/`ServerToClient` byte-flow labels
- **THEN** its role resolution remains `Unknown`
- **AND** wire direction does not promote it to `Known(Ingress)` or `Known(Egress)`

#### Scenario: Socket role contributes without becoming identity

- **WHEN** validated `SocketRole::Active` or `SocketRole::Passive` evidence is available
- **THEN** it MAY contribute to role normalization only when recorded-application ownership is established
- **AND** socket role, connection identity, stream identity, PID, TID, and packet direction SHALL NOT become interaction or scenario identity

#### Scenario: Missing ownership evidence is unknown

- **WHEN** application-relative ownership is insufficient to distinguish ingress from egress
- **THEN** role resolution is `Unknown` and the operation/evidence remains represented
- **AND** the operation cannot qualify as a scenario root without a later known classification
- **AND** correlation resolution remains an independent state

#### Scenario: Conflicting ownership evidence is ambiguous

- **WHEN** valid evidence supports both ingress and egress interpretations
- **THEN** role resolution is `Ambiguous` with the competing role candidates and their evidence
- **AND** the operation remains represented but cannot qualify as a scenario root
- **AND** timing, PID, connection ownership, or processing order does not select an arbitrary role

### Requirement: Role classification and correlation resolution remain independent

The aggregate SHALL store role classification state separately from `CorrelationResolution`. Role uncertainty SHALL NOT be collapsed into scenario-correlation uncertainty, and a correlation outcome SHALL NOT rewrite an unknown or ambiguous role into a known role. The model SHALL preserve both dimensions for every admitted operation.

#### Scenario: Known ingress with resolved correlation

- **WHEN** an operation has role `Known(Ingress)` and supplied correlation `Resolved(Scenario A)`
- **THEN** both states remain inspectable
- **AND** the operation may qualify as Scenario A's root if other invariants hold

#### Scenario: Known egress with ambiguous correlation

- **WHEN** an operation has role `Known(Egress)` and supplied correlation `Ambiguous(Scenario A, Scenario B)`
- **THEN** both states remain inspectable
- **AND** egress root validation fails while candidate correlation evidence remains preserved

#### Scenario: Unknown role with uncorrelated outcome

- **WHEN** an operation has role `Unknown` and supplied correlation `Uncorrelated`
- **THEN** the graph preserves both the unknown role evidence and uncorrelated outcome
- **AND** it creates no synthetic root or owner

#### Scenario: Ambiguous role with ambiguous correlation

- **WHEN** an operation has an ambiguous ingress/egress role and ambiguous scenario candidates
- **THEN** both candidate sets and their evidence remain separate and inspectable
- **AND** neither uncertainty domain silently selects an owner or role

### Requirement: Existing canonical operation remains interaction identity

The domain SHALL reuse `CanonicalOperation` as the logical interaction and `OperationId` as its interaction identity. It SHALL NOT introduce `InteractionId`, a parallel operation object, or a transport-derived replacement. `OperationId` SHALL retain its existing semantics and SHALL NOT be assumed globally unique outside its owning canonical session unless a separate durable invariant establishes that fact.

#### Scenario: Connection reuse preserves interaction identity

- **WHEN** multiple logical database exchanges reuse one physical connection
- **THEN** each exchange is represented by its own `CanonicalOperation` and `OperationId`
- **AND** connection reuse does not merge or replace those operation identities

#### Scenario: No duplicate interaction identity

- **WHEN** the correlation domain references a logical interaction
- **THEN** it uses `OperationId` through a scoped canonical reference
- **AND** no `InteractionId` is required or introduced

### Requirement: Canonical operation references are explicitly scoped

A durable reference to an operation outside its owning `CanonicalSession` SHALL include enough scope to identify its recording lineage, owning epoch/session artifact, and `OperationId`. The foundation SHALL use a shape equivalent to `CanonicalOperationRef { recording_id, owner_epoch_id, session_id, operation_id }`. The owning epoch is the epoch/session publication containing the authoritative canonical operation occurrence; contributing epoch ranges remain operation provenance. A bare `OperationId` SHALL NOT resolve an external reference.

#### Scenario: Same operation ID in different sessions is unambiguous

- **WHEN** Session A and Session B contain equal `OperationId` values under different session/epoch scopes
- **THEN** their `CanonicalOperationRef` values remain distinct
- **AND** lookup by one full reference cannot resolve the operation in the other session

#### Scenario: Reference lineage is verified

- **WHEN** a graph resolves a `CanonicalOperationRef`
- **THEN** it verifies recording ownership, owner epoch/session lineage, and exactly one operation occurrence within that session
- **AND** lineage is not inferred from identifier equality, WAL position, or session naming

#### Scenario: Fixture without persisted lineage

- **WHEN** a test or non-persisted fixture lacks recording or epoch provenance
- **THEN** the supplied domain context provides explicit scope for its reference
- **AND** the model does not silently promote an unscoped fixture identifier into durable recording lineage

### Requirement: Scenario references can span canonical sessions and epochs

A recording-scoped correlation graph SHALL be able to reference operations from multiple deterministic canonical sessions published for different finalized epochs. Epoch rollover SHALL be a publication boundary, not a scenario identity boundary. A causal relationship SHALL carry full scoped references for both parent and child, including when they belong to different sessions.

#### Scenario: Ingress crosses an epoch boundary

- **WHEN** an ingress operation begins in epoch N and its canonical completion is owned by epoch N+1
- **THEN** the domain preserves one logical `OperationId` and one authoritative full reference to the terminal canonical operation
- **AND** operation provenance retains the bounded contributing ranges from both epochs without creating a second interaction identity

#### Scenario: Successor epoch contains child egress

- **WHEN** an ingress scenario continues across rollover and a child egress operation is published in the successor epoch's canonical session
- **THEN** the graph can reference the ingress and child with their respective full operation references
- **AND** the selected edge may cross the two canonical sessions without becoming ambiguous

#### Scenario: Scenario identity survives rollover

- **WHEN** a recording publishes a successor canonical session after epoch rollover
- **THEN** an existing scenario retains its `ScenarioId`
- **AND** neither `SessionId` nor `EpochId` is substituted for scenario identity

#### Scenario: Restarted ETL resolves the same references

- **WHEN** ETL restarts and reloads a supplied correlation graph
- **THEN** it resolves each operation by recording, owner epoch, session, and operation ID with lineage verification
- **AND** it does not generate a new reference, use a bare operation lookup, or infer a relationship from processing order

### Requirement: CorrelationGraph owns every correlation outcome

Chronicle SHALL define a top-level `CorrelationGraph` or semantically equivalent recording-scoped aggregate that owns `Scenario` children, role-resolution state keyed by scoped operation reference, correlation resolutions keyed by scoped operation reference, and selected causal edges. `Scenario` SHALL be a child entity owned by the aggregate; scenario membership SHALL agree with the aggregate's resolved entries. Every operation admitted to the graph SHALL retain exactly one role-resolution state and one correlation-resolution outcome.

#### Scenario: Resolved interaction belongs to one scenario

- **WHEN** a supplied resolution selects Scenario A for an operation
- **THEN** the graph records one `Resolved` outcome with Scenario A and the operation appears in Scenario A membership
- **AND** the operation does not simultaneously belong to Scenario B

#### Scenario: Ambiguous interaction remains outside membership

- **WHEN** an operation has viable candidates Scenario A and Scenario B but no selected owner
- **THEN** the graph records `Ambiguous` with both candidate scenario identities and candidate-specific evidence
- **AND** the operation is not inserted into either scenario membership or selected causal edges

#### Scenario: Uncorrelated interaction remains represented

- **WHEN** no supplied evidence justifies assigning an operation to a scenario
- **THEN** the graph records `Uncorrelated` with the operation reference and available evidence
- **AND** it has no synthetic owner, root, or scenario membership

#### Scenario: Unknown role remains represented

- **WHEN** an admitted operation has role resolution `Unknown` or `Ambiguous`
- **THEN** its role state and evidence remain discoverable in the graph even if its correlation outcome is ambiguous or uncorrelated
- **AND** no operation is deleted or hidden because application-relative role classification failed

#### Scenario: Unresolved operation cannot disappear

- **WHEN** an admitted operation is not a selected scenario member
- **THEN** it remains discoverable through the graph's role-resolution and correlation-resolution indexes
- **AND** omission from `Scenario.members` is not treated as deletion

### Requirement: Only known ingress interactions can be scenario roots

A `Scenario.root` SHALL reference an interaction whose role resolution is `Known(Ingress)`. `Known(Egress)`, `Unknown`, and `Ambiguous` role resolutions SHALL NOT be accepted as scenario roots. The root rule SHALL be checked from the Chronicle-owned role-resolution state associated with the scoped operation, not from protocol message direction, socket identity, timing, or an external trace identifier.

#### Scenario: Valid known ingress root

- **WHEN** a supplied Scenario A root references an interaction with role resolution `Known(Ingress)`
- **THEN** graph validation accepts the root if all other identity and membership invariants hold

#### Scenario: Known egress cannot be root

- **WHEN** a supplied scenario uses an active outbound HTTP, database, or cache interaction with role resolution `Known(Egress)` as its root
- **THEN** graph validation rejects the scenario root
- **AND** the interaction may remain represented as egress, ambiguous, uncorrelated, or a selected child of an ingress scenario

#### Scenario: Unknown or ambiguous role cannot be root

- **WHEN** a supplied scenario uses an operation with role resolution `Unknown` or `Ambiguous` as its root
- **THEN** graph validation rejects the scenario root
- **AND** the operation and its role/correlation evidence remain represented without a guessed role

### Requirement: Selected causal edges are safe and explicit

The aggregate SHALL represent selected causal edges with full parent and child operation references and selected relationship evidence. An edge SHALL be accepted only when both endpoints resolve to the same scenario. Selected edges SHALL be acyclic, SHALL reject self-edges, and SHALL give each child at most one selected parent in the 0.2 scenario tree. Ambiguous candidates SHALL never become selected edges. Unresolved operations MAY exist without a selected parent.

#### Scenario: Cross-session causal edge

- **WHEN** a resolved parent in Session A and resolved child in Session B are both assigned to Scenario X
- **THEN** the graph can accept one selected parent-to-child edge using their full references
- **AND** session or epoch difference alone does not invalidate the edge

#### Scenario: Cross-scenario edge is rejected

- **WHEN** a proposed edge connects an operation resolved to Scenario A with an operation resolved to Scenario B
- **THEN** graph validation rejects the edge
- **AND** it does not reinterpret either resolution to make the edge valid

#### Scenario: Invalid edge shapes are rejected

- **WHEN** a supplied graph contains a self-edge, a directed cycle, or two selected parents for one child
- **THEN** graph validation rejects the graph
- **AND** no invalid edge is silently downgraded into scenario membership

#### Scenario: Ambiguous candidate is not an edge

- **WHEN** an ambiguous operation lists Scenario A and Scenario B as candidates
- **THEN** candidate evidence remains in the resolution
- **AND** no candidate-specific relationship is emitted as a selected causal edge

### Requirement: Evidence remains framework-neutral and optional

Correlation evidence SHALL be Chronicle-owned, tagged, provenance-bearing, and capable of representing trace relationships, protocol ownership, execution/task lineage, process/thread generation, socket/connection generation, protocol stream identity, temporal/lifetime relationships, and bounded namespaced custom values. Trace context SHALL be optional enrichment. Core/domain APIs SHALL NOT expose OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, or other tracing SDK types.

#### Scenario: Trace provider maps to Chronicle evidence

- **WHEN** an outer adapter receives W3C Trace Context, B3, Datadog, AWS X-Ray, or OpenTelemetry data
- **THEN** it maps that data into Chronicle-owned opaque/provider-labelled evidence
- **AND** no provider SDK type crosses the canonical/domain API boundary

#### Scenario: No trace context remains representable

- **WHEN** an application supplies no trace context
- **THEN** ingress, egress, database, cache, and other canonical interactions remain representable
- **AND** missing trace parents are not synthesized from infrastructure identifiers or timestamps

#### Scenario: Temporal-only evidence cannot resolve ownership

- **WHEN** a supplied `Resolved` outcome contains only temporal overlap or timestamp evidence
- **THEN** model validation rejects that outcome as lacking non-temporal resolution provenance
- **AND** the interaction may remain ambiguous or uncorrelated with temporal evidence retained

#### Scenario: Infrastructure identity remains evidence

- **WHEN** PID, TID, task worker, socket, connection, stream, or timestamp values are present
- **THEN** they remain evidence attached to observations or resolutions
- **AND** none becomes a scenario or interaction identity by equality or reuse

### Requirement: Correlation does not change replayability

Scenario membership and correlation resolution SHALL remain separate from operation completeness and replayability. Assigning an incomplete, lost, malformed, unsupported, or otherwise non-replayable operation to a scenario SHALL NOT make it replayable or authorize replay.

#### Scenario: Resolved incomplete operation

- **WHEN** an incomplete operation is supplied as resolved into Scenario A
- **THEN** the graph preserves its Scenario A membership and its incomplete status independently
- **AND** replay validation continues to reject it as non-replayable

### Requirement: Foundation validation is algorithm-neutral

This capability SHALL validate Chronicle-owned roles, scoped references, supplied resolutions, evidence provenance, membership consistency, and selected-edge invariants. It SHALL NOT require a correlation algorithm to derive scenario ownership from raw capture events or interleaved traffic. Scoring heuristics, weighted evidence, temporal inference, runtime lineage algorithms, and trace-based automatic selection SHALL be future capabilities.

#### Scenario: Supplied resolution is faithfully represented

- **WHEN** a domain fixture supplies canonical operations `op1`, `op2`, and `op3` with pre-resolved ownership `op1 -> Scenario A`, `op2 -> Scenario B`, and `op3 -> Scenario C`
- **THEN** validation preserves exactly those memberships and resolutions
- **AND** the fixture does not require the foundation to infer A/B/C from raw event order or interleaving

#### Scenario: Supplied ambiguity remains unresolved

- **WHEN** a fixture supplies an ambiguous candidate set and an uncorrelated operation
- **THEN** validation preserves both outcomes and their evidence
- **AND** it does not select an owner to satisfy scenario completeness

#### Scenario: Resolver is a future boundary

- **WHEN** implementation planning reaches automatic correlation of concurrent ingress and egress traffic
- **THEN** that behavior is specified in a separate resolver change such as `correlate-ingress-and-egress-interactions`
- **AND** this foundation remains usable with resolver outputs from different future algorithms

### Requirement: Frozen 0.1 contracts remain unchanged and EventId is deferred

This planning change SHALL NOT add fields to Capture Event v1 or Canonical Session v1, alter WAL v1, public stable JSON, or introduce a required `EventId`. Existing `OperationId`, scoped canonical references, and existing provenance are sufficient for this correlation foundation. Persisted correlation data SHALL require either a separately versioned sidecar/new artifact or an explicit 0.2 compatibility/migration change; no implicit v1 field addition, reader default, or compatibility fallback is authorized.

#### Scenario: Capture Event v1 remains unchanged

- **WHEN** this foundation is implemented
- **THEN** Capture Event v1 still contains its existing evidence contract without an EventId field
- **AND** no random or processing-order event identity is persisted by this change

#### Scenario: Canonical Session v1 remains unchanged

- **WHEN** roles, scoped references, or correlation outcomes are implemented
- **THEN** they use a non-v1 domain/sidecar integration surface until an explicit compatibility decision exists
- **AND** Canonical Session v1 readers and writers are not silently changed

#### Scenario: Future event identity requires its own change

- **WHEN** a later feature proves raw event identity necessary
- **THEN** a dedicated change chooses deterministic derivation from stable immutable provenance or a new versioned contract/sidecar with reader/writer and migration policy
- **AND** the choice does not rely on timestamps, WAL byte position alone, processing order, or random regeneration across restart
- **AND** a derived identity remains unchanged when WAL segmentation or ETL processing order changes

### Requirement: Canonical ownership and dependency direction remain intact

Correlation semantics SHALL remain in `chronicle-canonical` with neutral identity primitives in `chronicle-common`. Capture, session, protocol, WAL, storage, ETL, application, and CLI responsibilities SHALL remain as currently assigned. No standalone correlation crate SHALL be added. Core/domain crates SHALL remain free of tracing SDK/provider dependencies, and tracing evidence SHALL enter only through Chronicle-owned contracts. The existing architecture validation mechanism currently checks Chronicle workspace dependency direction, critical forbids, and semantic/API boundaries; it does not yet enforce an external tracing-SDK/package denylist. Future implementation SHALL extend `validation/architecture.toml`, `scripts/validation.py architecture`, and its existing standard-library fixture tests with the smallest maintainable direct-dependency guard based on actual Cargo package identity, without creating a parallel validator.

#### Scenario: Existing pipeline owners remain

- **WHEN** canonical correlation consumes protocol-produced operations
- **THEN** capture remains observation owner, session remains reconstruction owner, protocol remains protocol-pairing owner, ETL remains publication/checkpoint owner, and storage remains persistence/verification owner
- **AND** no lower layer assigns scenario ownership

#### Scenario: Extended architecture mechanism rejects provider SDK leakage

- **WHEN** a future implementation adds an OpenTelemetry or provider-specific tracing dependency to a core/domain crate
- **THEN** the extended existing architecture validation mechanism rejects the direct package dependency through its external denylist
- **AND** the policy continues to retain current workspace-edge and semantic-boundary checks
- **AND** no parallel validator, correlation crate, or SDK-specific domain API is introduced
