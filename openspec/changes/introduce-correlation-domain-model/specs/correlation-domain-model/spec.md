## Purpose

Define Chronicle's framework-neutral correlation domain for turning independently observed logical interactions into explainable, replayable application scenarios without making tracing SDKs or infrastructure carriers part of domain ownership.

## ADDED Requirements

### Requirement: Chronicle owns scenario and interaction semantics

Chronicle SHALL own the meaning of a scenario, logical interaction, causal relationship, and correlation outcome. For this capability, an interaction SHALL mean the existing `CanonicalOperation` concept and SHALL represent one logical request/response exchange or equivalent protocol operation. It SHALL NOT mean a TCP connection, socket, process, thread, packet, protocol stream, or WAL record. Existing protocol canonicalizers SHALL continue to own protocol-local request/response pairing; scenario correlation SHALL be a distinct Chronicle domain concern.

#### Scenario: Logical interaction is independent of transport carrier

- **WHEN** one HTTP/2 connection carries streams for three independent requests
- **THEN** each logical request/response exchange can be represented as a separate interaction
- **AND** no interaction identity is derived from or merged solely because streams share one connection

#### Scenario: Protocol correlation remains protocol-owned

- **WHEN** an HTTP/1.1 canonicalizer pairs a request and response on a persistent connection
- **THEN** it produces one logical operation/interaction for that exchange
- **AND** scenario correlation consumes that interaction and its evidence without rediscovering HTTP framing

### Requirement: Scenario boundary is conservative and root-based

A `Scenario` SHALL represent one root ingress interaction and the causally correlated synchronous interactions triggered while handling that ingress, ending at the ingress response boundary. A root ingress SHALL be a logical interaction, not a socket or listener. Background work that starts after the ingress response SHALL NOT automatically become a member of that scenario; a future specification MAY extend the boundary. Evidence for excluded or unresolved work SHALL remain representable rather than being silently discarded.

#### Scenario: Root ingress with synchronous children

- **WHEN** an ingress request triggers an outbound HTTP call, a database operation, and a cache operation before its response
- **THEN** one scenario can contain the ingress interaction as root and those child interactions as causally related members
- **AND** the ingress response closes the 0.2 scenario boundary

#### Scenario: Post-response background work

- **WHEN** background work begins after the ingress response has completed
- **THEN** that work is not automatically assigned to the completed scenario
- **AND** its interaction and correlation evidence remain available as unresolved, uncorrelated, or separately rooted domain data

### Requirement: Stable Chronicle-owned identities remain distinct

Event, interaction, and scenario identities SHALL be stable typed identities owned by Chronicle rather than by a tracing SDK or infrastructure carrier. The implementation SHALL reuse existing `OperationId` as the identity of a canonical interaction and SHALL NOT create a second independent operation/interaction identity. A stable `EventId` SHALL identify one observed event/evidence instance, and a stable `ScenarioId` SHALL identify one scenario aggregate. `RecordingId`, `EpochId`, `SessionId`, `ConnectionId`, process IDs, thread IDs, socket identities, stream identities, WAL sequence numbers, and timestamps SHALL retain their existing meanings and SHALL NOT be repurposed as scenario identity.

#### Scenario: Interaction identity survives connection reuse

- **WHEN** three database request/response exchanges reuse one physical connection
- **THEN** each exchange has its own stable interaction identity
- **AND** connection reuse does not replace, alias, or merge those identities

#### Scenario: Storage and scenario identities remain separate

- **WHEN** a canonical session or recording contains multiple scenarios
- **THEN** each scenario has its own scenario identity while session, epoch, and recording identities remain separately addressable
- **AND** identifier equality is not treated as lineage or ownership

#### Scenario: WAL order is not event identity

- **WHEN** an event is recovered from a different WAL segment or is replayed through ETL
- **THEN** its event identity remains the identity of the observed event
- **AND** WAL sequence and byte position remain ordering/provenance evidence only

### Requirement: Causal relationships are explicit

The domain SHALL represent parent/child or equivalent directed causal relationships between interactions. A selected causal edge SHALL identify a parent interaction, a child interaction, and the relationship provenance. Selected causal edges SHALL be acyclic within a scenario, SHALL not self-reference, and SHALL give an interaction at most one selected parent in the 0.2 tree; competing parent candidates SHALL remain in resolution data instead of becoming graph edges. A connection, process, thread, task worker, or time window SHALL NOT implicitly create a causal edge. An interaction with unresolved parentage SHALL be allowed to exist without a selected parent.

#### Scenario: Interleaved concurrent ingresses

- **WHEN** concurrent roots A `POST /checkout`, B `GET /profile`, and C `POST /login` produce interleaved PostgreSQL, HTTP, and Redis interactions in the order `A→postgres`, `B→postgres`, `C→redis`, `A→HTTP`, `C→postgres`, `A→postgres`
- **THEN** the domain can represent Scenario A with its two PostgreSQL interactions and HTTP child, Scenario B with its PostgreSQL interaction, and Scenario C with Redis and PostgreSQL children
- **AND** interleaving does not create cross-scenario parent edges

#### Scenario: Multiplexed protocol streams

- **WHEN** one HTTP/2 connection carries stream 1 for Scenario A, stream 3 for Scenario B, and stream 5 for Scenario C
- **THEN** each stream's logical interactions can have independent scenario membership and causal edges
- **AND** the shared connection is retained only as carrier evidence

#### Scenario: Connection-pool reuse

- **WHEN** PostgreSQL connection number 5 serves request A, then B, then C
- **THEN** each query interaction can be independently linked to its owning scenario
- **AND** connection number 5 does not become a permanent parent or owner

### Requirement: Correlation evidence is framework-neutral and explainable

The domain SHALL represent correlation evidence as Chronicle-owned, typed, provenance-bearing data. Evidence SHALL be able to express trace relationships, protocol request/response ownership, execution or task lineage, process identity, thread identity, socket or connection identity, protocol stream identity, temporal/lifetime relationships, and custom correlation metadata. Each retained evidence item SHALL identify its evidence kind and source/provenance sufficiently for a future user or diagnostic tool to explain how a resolution was reached. An opaque numeric score without evidence provenance SHALL NOT be the sole persisted explanation.

#### Scenario: Trace evidence is normalized

- **WHEN** a future W3C Trace Context, B3, Datadog, AWS X-Ray, or OpenTelemetry adapter provides a trace relationship
- **THEN** the adapter maps it into Chronicle-owned trace evidence with normalized/opaque identifiers and source provenance
- **AND** no OpenTelemetry or other tracing SDK type crosses into the domain model

#### Scenario: Protocol ownership evidence explains a database assignment

- **WHEN** a protocol adapter proves that a PostgreSQL response belongs to one logical request and that request is linked to Scenario A
- **THEN** the resolution can retain protocol ownership evidence and its provenance
- **AND** a diagnostic tool can state that protocol ownership contributed to the Scenario A assignment

#### Scenario: Custom evidence remains namespaced

- **WHEN** an application supplies custom correlation metadata
- **THEN** the domain preserves it under an explicit provider/namespace and bounded value representation
- **AND** custom metadata cannot silently replace Chronicle-owned identity or resolution semantics

### Requirement: Trace context is optional enrichment

Recording, interaction construction, and scenario representation SHALL work with complete trace context, partial trace context, another tracing system, W3C Trace Context without an SDK, or no trace context. Trace evidence MAY improve resolution confidence, but its absence SHALL NOT prevent capture, canonical interaction construction, scenario creation, or preservation of other evidence. The domain SHALL NOT synthesize missing trace parentage.

#### Scenario: Complete trace context

- **WHEN** an interaction carries a valid normalized trace relationship
- **THEN** the interaction preserves that trace evidence as optional enrichment
- **AND** the scenario model remains Chronicle-owned and usable if that evidence is later removed

#### Scenario: No trace context

- **WHEN** an application emits no trace context at all
- **THEN** Chronicle can still represent ingress, outbound, database, cache, and response interactions
- **AND** correlation uses available non-trace evidence or records an unresolved outcome

#### Scenario: Partial trace context

- **WHEN** only a trace ID or only a parent relationship is observed
- **THEN** the partial trace evidence is preserved with its completeness/provenance state
- **AND** Chronicle does not fill missing fields from PID, TID, socket, connection, or time overlap

### Requirement: Resolution preserves ambiguity and categorical confidence

Correlation resolution SHALL distinguish a resolved assignment from an unresolved candidate set. A resolved result SHALL carry exactly one of the categorical confidence levels `Exact`, `Strong`, or `Inferred` plus the evidence supporting that assignment. An `Ambiguous` result SHALL preserve one or more candidate scenarios and the evidence for each candidate without selecting an owner. An `Uncorrelated` result SHALL preserve the interaction and available evidence while stating that no scenario assignment is justified. Resolution SHALL NOT require every interaction to belong to a scenario, and confidence SHALL NOT be represented only as an unexplained numeric score.

#### Scenario: Exact or strong resolution

- **WHEN** Chronicle-owned evidence uniquely or strongly identifies an interaction's scenario
- **THEN** the resolution records the selected `ScenarioId`, the categorical confidence, and supporting evidence
- **AND** the selected causal relationship remains inspectable

#### Scenario: Inferred resolution

- **WHEN** evidence supports a likely scenario but does not establish a unique exact relationship
- **THEN** the resolution MAY record `Inferred` with all evidence used
- **AND** the model does not present the inference as an exact fact

#### Scenario: Ambiguous candidate ownership

- **WHEN** evidence is insufficient or conflicting between Scenario A and Scenario B
- **THEN** the resolution records `Ambiguous` with both candidate scenario identities and candidate-specific evidence
- **AND** it does not assign the interaction to either scenario merely to make the scenario complete

#### Scenario: Uncorrelated interaction

- **WHEN** no viable evidence links an interaction to a scenario
- **THEN** the interaction remains representable with an `Uncorrelated` resolution
- **AND** no synthetic root, owner, or scenario membership is created

### Requirement: Infrastructure and time are evidence, never ownership

PID, TID, task-worker identity, socket identity, connection identity, protocol stream identity, and timestamps SHALL be represented only as evidence/carriers. The model SHALL NOT assume one PID, one TID, one socket, one connection, one stream, or one time window equals one scenario. Temporal overlap alone SHALL NEVER produce a resolved scenario assignment; temporal evidence MAY be retained as weak or fallback evidence only when combined with a Chronicle-owned resolution policy.

#### Scenario: Same-process concurrency

- **WHEN** multiple simultaneous ingress requests execute in one process ID
- **THEN** the domain can distinguish their interaction and scenario identities
- **AND** process identity alone does not correlate them

#### Scenario: Thread-pool reuse

- **WHEN** request A runs on a thread and request B later reuses that thread
- **THEN** the thread identity remains evidence attached to each observation
- **AND** it does not permanently own either request's scenario

#### Scenario: Async task migration

- **WHEN** one request moves between Tokio worker threads, Go goroutines, Java platform/virtual threads, or Node.js event-loop turns
- **THEN** execution evidence can record lineage changes without changing the interaction or scenario identity
- **AND** no single OS thread is required for correlation

#### Scenario: Overlapping lifetimes are insufficient

- **WHEN** two ingress lifetimes overlap in the same process, runtime, thread pool, or connection pool
- **THEN** overlap may be retained as temporal evidence
- **AND** overlap alone cannot select either scenario as owner

### Requirement: Domain and pipeline boundaries remain dependency-safe

The canonical correlation domain SHALL live with Chronicle's protocol-independent canonical model, reusing `CanonicalOperation` and `OperationId`; neutral identity primitives SHALL remain in `chronicle-common`. Capture and eBPF layers SHALL provide only observation evidence, session reconstruction SHALL provide connection/stream reconstruction, protocol implementations SHALL provide protocol-local ownership evidence, and ETL SHALL compose future correlation with canonical publication. Core/domain crates SHALL NOT depend on OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, or any tracing SDK. Future adapters SHALL depend inward on Chronicle-owned evidence contracts rather than the reverse.

#### Scenario: Architecture validation rejects tracing leakage

- **WHEN** a future change adds a tracing SDK dependency to `chronicle-common` or `chronicle-canonical`
- **THEN** existing architecture/dependency validation fails
- **AND** the adapter must map external types at an outer boundary instead

#### Scenario: Existing capture pipeline remains layered

- **WHEN** committed WAL evidence flows through reconstruction, protocol decoding, canonicalization, and ETL
- **THEN** scenario correlation consumes Chronicle-owned interactions/evidence at the canonical/ETL seam
- **AND** WAL durability, socket reconstruction, protocol framing, and storage publication retain their existing owners

#### Scenario: No new adapter is required by the foundation

- **WHEN** this domain model is implemented without a tracing integration
- **THEN** core Chronicle crates compile and operate without any tracing SDK
- **AND** later adapters can be added without changing the domain vocabulary

### Requirement: Correlation foundation remains algorithm- and feature-neutral

This capability SHALL define domain values, identity ownership, evidence provenance, causal relationship semantics, resolution outcomes, validation, and architectural boundaries only. It SHALL NOT require a correlation algorithm, weighted candidate scoring, runtime-specific task instrumentation, tracing adapter implementation, scenario replay, deterministic dependency matching, scenario export, replay assertions, test generation, AI/LLM correlation, or a Web UI.

#### Scenario: Algorithm can evolve without schema ownership change

- **WHEN** a future correlation engine uses different deterministic heuristics or evidence providers
- **THEN** it can emit the same Chronicle-owned evidence and resolution vocabulary
- **AND** the scenario/interaction identity and ambiguity semantics remain unchanged

#### Scenario: Unsupported future feature remains out of scope

- **WHEN** implementation planning reaches replay, export, or test generation
- **THEN** those features are tracked as separate changes
- **AND** this capability is accepted only when its domain and validation contracts are complete
