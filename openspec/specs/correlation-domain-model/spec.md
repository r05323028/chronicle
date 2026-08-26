# correlation-domain-model Specification

## Purpose

Define Chronicle's framework-neutral correlation domain for application-relative interaction roles, durable references across canonical sessions and epochs, and explicit ownership of resolved, ambiguous, and uncorrelated outcomes. The capability validates supplied resolutions; it does not implement correlation selection.

## Requirements

### Requirement: Chronicle owns application-relative interaction role

Chronicle SHALL define two separate protocol-neutral concepts. `InteractionRole` SHALL contain only the known application-relative roles `Ingress` and `Egress`. `InteractionRoleResolution` SHALL classify each correlation input as `Known { role: InteractionRole, evidence: [...] }`, `Unknown { evidence: [...] }`, or `Ambiguous { candidates: [{ role: InteractionRole, evidence: [...] }] }`. In prose, `Known(Ingress)` and `Known(Egress)` are shorthand for evidence-bearing known records. Uncertainty SHALL be classification state, not an `InteractionRole` variant. Every ambiguous candidate SHALL retain its own supporting evidence; evidence SHALL NOT exist only as one undifferentiated shared list. An ambiguous candidate set SHALL contain at least two distinct viable roles, with no duplicate roles. With the current two-value role domain, valid ambiguity means both `Ingress` and `Egress`; validation SHALL express the at-least-two/distinct invariant rather than hard-coding exactly two, so future role values remain possible. Role semantics SHALL describe the logical interaction's relationship to the recorded application and SHALL NOT be inferred from packet direction or byte-flow direction alone. Role validation SHALL require a known role's evidence to include application-ownership evidence supporting that role and no contradictory ownership evidence; neutral enrichment such as trace relationships, timestamps, PID/TID, task lineage, connection/socket identity, protocol-stream identity, wire direction, or socket role alone MAY accompany a known role but SHALL NOT independently establish it. Each ambiguous candidate SHALL carry candidate-specific evidence that supports that candidate's own role.

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
- **THEN** role resolution is `Ambiguous` with at least two distinct candidate records
- **AND** evidence supporting `Ingress` is attached to the `Ingress` candidate while evidence supporting `Egress` is attached to the `Egress` candidate
- **AND** the operation remains represented but cannot qualify as a scenario root
- **AND** timing, PID, connection ownership, or processing order does not select an arbitrary role

#### Scenario: Candidate-specific role evidence is preserved

- **WHEN** supplied ambiguous role classification contains an `Ingress` candidate supported by passive application-server ownership and an `Egress` candidate supported by active outbound ownership
- **THEN** each candidate retains its own supporting evidence collection
- **AND** inspection can answer why the interaction could be ingress and independently why it could be egress
- **AND** neither candidate's evidence is represented only through one shared list

#### Scenario: Candidate evidence is not swapped

- **WHEN** an ambiguous role contains distinct ingress and egress candidate evidence
- **THEN** ingress evidence remains associated only with the ingress candidate and egress evidence remains associated only with the egress candidate
- **AND** the existence of both candidates does not make evidence supporting one role support the other role

#### Scenario: Duplicate role candidates are rejected

- **WHEN** an ambiguous role candidate list contains `Ingress` more than once
- **THEN** domain validation rejects the role resolution
- **AND** duplicate candidate records are not deduplicated silently

#### Scenario: A single candidate is known, not ambiguous

- **WHEN** supplied evidence supports only one viable role, such as one `Ingress` candidate with sufficient evidence
- **THEN** domain validation rejects an `Ambiguous` wrapper containing only that candidate
- **AND** the supplied resolution uses `Known(Ingress)` with its supporting evidence instead

#### Scenario: Empty ambiguous candidate list is rejected

- **WHEN** an ambiguous role resolution contains no candidate records
- **THEN** domain validation rejects the resolution
- **AND** it does not create an unknown or synthetic role implicitly

#### Scenario: Ambiguous candidate cardinality remains extensible

- **WHEN** the current role domain validates an ambiguous resolution
- **THEN** both distinct `Ingress` and `Egress` candidates are viable
- **AND** future role values, if introduced, may participate in an ambiguous set of at least two distinct viable candidates without changing candidate-specific evidence semantics
- **AND** validation does not assume exactly two candidates once the role domain grows

#### Scenario: Ambiguity is not resolved by elimination

- **WHEN** one candidate in an ambiguous role is `Ingress`
- **THEN** no candidate is selected merely to eliminate ambiguity
- **AND** the role remains ambiguous until supplied domain evidence resolves it to `Known(Ingress)` or `Known(Egress)`

### Requirement: Role classification and correlation resolution remain independent

The aggregate SHALL store role classification state separately from `CorrelationResolution`. Role uncertainty SHALL NOT be collapsed into scenario-correlation uncertainty, and a correlation outcome or its evidence SHALL NOT rewrite an unknown or ambiguous role into a known role. Scenario evidence SHALL NOT silently resolve role ambiguity, and role evidence SHALL NOT silently select a scenario. The model SHALL preserve both dimensions for every admitted operation.

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

- **WHEN** an operation has an ambiguous role with candidate-specific ingress/egress evidence and ambiguous scenario candidates with candidate-specific correlation evidence
- **THEN** both candidate sets and their evidence remain separate and inspectable
- **AND** neither uncertainty domain silently selects an owner or role

#### Scenario: Role ambiguity survives resolved correlation

- **WHEN** an operation remains `Ambiguous` between ingress and egress with candidate-specific role evidence while supplied correlation is `Resolved(Scenario A)`
- **THEN** the role resolution remains ambiguous and its candidate evidence remains inspectable
- **AND** the operation MAY be represented as a Scenario A member if approved correlation-domain invariants permit it
- **AND** it cannot qualify as Scenario A's root until role resolution is actually `Known(Ingress)`

#### Scenario: Scenario evidence cannot resolve role ambiguity

- **WHEN** supplied correlation evidence assigns an operation to a scenario but does not supply a selected application-relative role
- **THEN** domain validation preserves the existing ambiguous role candidates and their evidence
- **AND** it does not rewrite the role to `Known(Ingress)` or `Known(Egress)` merely to satisfy scenario structure

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

The aggregate SHALL represent selected causal edges with full parent and child operation references and selected relationship evidence. An edge SHALL be accepted only when both endpoints resolve to the same scenario. Selected edges SHALL be acyclic, SHALL reject self-edges, and SHALL give each child at most one selected parent in the 0.2 scenario tree. Ambiguous candidates SHALL never become selected edges, and a selected causal edge SHALL NOT make its scenario's root a child. Unresolved operations MAY exist without a selected parent.

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

### Requirement: Correlation evidence may include resolver-generated ScenarioRoot records valid only in root-establishment placement

Chronicle correlation evidence MAY include `ScenarioRoot { root: CanonicalOperationRef }` items. `ScenarioRoot` records Chronicle's explicit establishment of a `Known(Ingress)` operation as the root of its derived scenario; it is correlation-level membership evidence, non-temporal, and acceptable within a validated `Resolved` outcome. It SHALL NOT classify the operation's interaction role and SHALL NOT replace or mutate `InteractionRoleResolution`; role classification remains an independent dimension owned by role resolutions.

Responsibilities stay separate: the resolver owns creation (input reservation forbidding caller-supplied items in both channels is owned by the `correlation-resolver` capability), while the correlation domain owns consistency validation once the value exists inside a `CorrelationGraph`. Because `ScenarioRoot` has placement-dependent semantics, generic evidence validity alone is insufficient: a graph containing `ScenarioRoot { root: R }` is consistent ONLY when all of the following hold:

1. the item appears in the `Resolved` correlation outcome of operation R;
2. that outcome selects Scenario S;
3. Scenario S.root equals R;
4. R's preserved role resolution is `Known(Ingress)`.

Domain/graph validation SHALL reject semantically invalid placements, including: a `ScenarioRoot { root: B }` inside the resolution of a different operation A; a `ScenarioRoot` on an ordinary non-root member; a `ScenarioRoot { root: A }` inside a resolution selecting a scenario whose root is not A; a `ScenarioRoot` inside `Uncorrelated` evidence; a `ScenarioRoot` as ambiguous-candidate evidence; and a `ScenarioRoot` inside `SelectedCausalEdge.evidence`. This is a consistency rule for the new variant only: existing evidence validation is not redesigned, and externally supplied graphs are not rejected beyond semantically invalid `ScenarioRoot` placement.

The addition affects only the non-frozen runtime correlation-domain surface and SHALL NOT modify Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, or any persisted contract.

#### Scenario: ScenarioRoot witnesses scenario-root membership

- **WHEN** a resolver establishes Scenario A around known-ingress operation A
- **THEN** A's `Resolved` resolution carries a `ScenarioRoot { root: A }` item as its non-temporal correlation witness
- **AND** existing foundation validation accepts the resolution without weakening

#### Scenario: Valid root placement passes full validation

- **WHEN** Scenario S.root is A, role[A] is `Known(Ingress)`, and resolution[A] is `Resolved { scenario: S, confidence: Exact, evidence: [ScenarioRoot { root: A }] }`
- **THEN** graph validation succeeds under `validate_against_sessions(supplied_sessions)`
- **AND** the placement satisfies every condition of this requirement

#### Scenario: ScenarioRoot does not classify role

- **WHEN** any graph contains `ScenarioRoot` items for established roots
- **THEN** each root's `InteractionRoleResolution` remains exactly as supplied, with its own evidence intact
- **AND** inspecting scenario membership never substitutes for role classification

#### Scenario: Wrong-operation placement fails validation

- **WHEN** resolution[A] is Resolved but contains `ScenarioRoot { root: B }` where A and B differ
- **THEN** graph validation rejects the graph

#### Scenario: Non-root member placement fails validation

- **WHEN** Scenario S.root is A while ordinary member B's resolution contains `ScenarioRoot { root: B }`
- **THEN** graph validation rejects the graph because the item does not sit in the root's own resolution

#### Scenario: Wrong-scenario placement fails validation

- **WHEN** `ScenarioRoot { root: A }` appears in a resolution selecting Scenario T whose root is not A
- **THEN** graph validation rejects the graph

#### Scenario: Uncorrelated, ambiguous-candidate, and edge placements fail validation

- **WHEN** a `ScenarioRoot` item appears inside `Uncorrelated` evidence, inside any `Ambiguous.candidates[*].evidence`, or inside `SelectedCausalEdge.evidence`
- **THEN** graph validation rejects the graph in every case
- **AND** `ScenarioRoot` can never serve as generic relationship or parent evidence

#### Scenario: Caller-supplied ScenarioRoot is not resolver input

- **WHEN** caller-controlled input contains `ScenarioRoot` items in either evidence channel
- **THEN** the resolver rejects them per the `correlation-resolver` capability's input-reservation rules
- **AND** caller input cannot supply this variant to the resolver
- **AND** within `CorrelationGraph` values, this variant is valid only in the root-establishment placement defined above

#### Scenario: Frozen contracts remain untouched

- **WHEN** the variant ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and persisted contracts contain no additions from it

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

### Requirement: Trace providers are optional separately distributed adapters

Trace-provider integrations SHALL be optional, separately distributed adapter/plugin packages. The default Chronicle distribution SHALL NOT depend on, bundle, or link any trace-provider implementation package or provider SDK anywhere in its transitive dependency closure, and no provider SDK type SHALL cross the canonical/domain API boundary. Provider data enters Chronicle only through provider-neutral Chronicle-owned evidence contracts such as `CorrelationEvidence`. Trace context remains optional enrichment: correlation SHALL NOT require any trace provider, and the absence of provider adapters SHALL NOT prevent capture, WAL, ETL, canonicalization, storage, replay, or CLI behavior. Provider installation UX and runtime plugin discovery belong to future changes; this capability introduces neither. A small Chronicle-owned parser for a trace-context wire format is provider-neutral code, not a provider plugin, and is not forbidden by this invariant.

#### Scenario: Default distribution rejects a direct provider SDK

- **WHEN** a crate reachable from the default `chronicle` executable root, such as `chronicle-cli -> chronicle-application`, declares `opentelemetry_sdk`
- **THEN** architecture validation rejects the dependency
- **AND** the diagnostic names the dependency path from the default distribution root

#### Scenario: Deep transitive provider dependency is rejected

- **WHEN** a forbidden provider package sits below intermediate workspace crates, such as `chronicle-cli -> chronicle-application -> chronicle-etl -> <provider package>`
- **THEN** validation walks the transitive dependency closure and still rejects it
- **AND** checking only direct dependencies of the root would be insufficient

#### Scenario: Optional, renamed, and target-specific declarations cannot bypass policy

- **WHEN** a provider package inside the default-distribution closure is declared `optional = true`, listed under `[target.'cfg(target_os = "linux")'.dependencies]`, or bound to a renamed local key whose `package` value is the provider identity
- **THEN** validation evaluates Cargo package identity regardless of host platform or feature selection
- **AND** the declaration fails exactly as a plain dependency would

#### Scenario: Separately distributed adapter outside the closure is allowed

- **WHEN** an adapter crate such as `chronicle-trace-otel` depends on `opentelemetry_sdk` but is unreachable from the default distribution root
- **AND** it communicates with Chronicle only through Chronicle-owned contracts such as `CorrelationEvidence`
- **THEN** architecture validation accepts it
- **AND** no provider SDK type enters `chronicle-common` or `chronicle-canonical`

#### Scenario: Generic tracing facades are not provider SDKs

- **WHEN** `tracing` or `tracing-subscriber` appear where the workspace-edge policy already permits them
- **THEN** validation does not reject them merely for being tracing-related
- **AND** lightweight trace-context wire-format parsing is not treated as a heavyweight provider integration by this invariant alone

#### Scenario: No provider plugin is required for normal behavior

- **WHEN** no trace-provider adapter is installed
- **THEN** capture, WAL, ETL, canonicalization, storage, replay, and CLI behavior continue unchanged
- **AND** role resolution still represents known, unknown, ambiguous, and uncorrelated outcomes without trace evidence

#### Scenario: Provider installation UX stays outside this foundation

- **WHEN** a user later wants OpenTelemetry, Datadog, AWS X-Ray, or B3 integration
- **THEN** installation, discovery, registries, and loading runtimes are specified by a dedicated future change
- **AND** this capability adds no plugin command, registry, discovery mechanism, or dynamic-loading runtime

### Requirement: Canonical ownership and dependency direction remain intact

Correlation semantics SHALL remain in `chronicle-canonical` with neutral identity primitives in `chronicle-common`. Capture, session, protocol, WAL, storage, ETL, application, and CLI responsibilities SHALL remain as currently assigned. No standalone correlation crate SHALL be added. Core/domain crates SHALL remain free of tracing SDK/provider dependencies, and tracing evidence SHALL enter only through Chronicle-owned contracts. The existing architecture validation mechanism checks Chronicle workspace dependency direction, critical forbids, semantic/API boundaries, core/domain declarations against an external tracing-SDK/package denylist, and the transitive normal/build dependency closure of the declared default distribution roots against the same denylist — every check by actual Cargo package identity, so renamed, optional, and target-specific declarations cannot evade it, and all within the existing validator rather than a parallel one.

#### Scenario: Existing pipeline owners remain

- **WHEN** canonical correlation consumes protocol-produced operations
- **THEN** capture remains observation owner, session remains reconstruction owner, protocol remains protocol-pairing owner, ETL remains publication/checkpoint owner, and storage remains persistence/verification owner
- **AND** no lower layer assigns scenario ownership

#### Scenario: Extended architecture mechanism rejects provider SDK leakage

- **WHEN** an implementation adds an OpenTelemetry or provider-specific tracing dependency to a core/domain crate, or declares one anywhere inside the default `chronicle` executable dependency closure — directly, transitively, optionally, under a rename, or target-specifically
- **THEN** the existing architecture validation mechanism rejects the declaration through its external denylist and names the dependency path from the protected crate or default distribution root
- **AND** the policy continues to retain current workspace-edge and semantic-boundary checks
- **AND** no parallel validator, correlation crate, or SDK-specific domain API is introduced

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
