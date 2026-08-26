## Purpose

Define Chronicle-owned native execution-handoff evidence from pre-canonical runtime observations through exact canonical binding, while keeping passive recording conservative and preserving existing resolver and compatibility boundaries.

## ADDED Requirements

### Requirement: Native handoff observations are pre-canonical and selection-free

A Chronicle-native source SHALL emit a serializable `NativeExecutionHandoffObservation` with a parent `NativeOperationAnchor`, child `NativeOperationAnchor`, `ExecutionContinuation` relation, and Chronicle-owned source provenance before final canonical operation references necessarily exist. An observation SHALL contain no `CanonicalOperationRef`, `ScenarioId`, selected scenario, confidence, selected causal edge, resolver outcome, or trace-provider SDK value. The source SHALL report an explicit execution-context handoff and SHALL NOT infer a handoff from equality or proximity.

#### Scenario: Runtime observation precedes canonicalization

- **WHEN** an opted-in runtime transfers Chronicle execution context from parent operation A to child operation A1 before reconstruction and protocol canonicalization finish
- **THEN** Chronicle records an observation whose endpoints are pre-canonical anchors
- **AND** no final canonical reference is synthesized at observation time

#### Scenario: Observation carries explicit positive relation only

- **WHEN** the runtime calls the Chronicle-native continuation operation
- **THEN** the observation records only the explicit `ExecutionContinuation` relation and source provenance
- **AND** it does not choose scenario ownership, confidence, witnesses, or selected edges

#### Scenario: Unobserved execution is not inferred

- **WHEN** no explicit Chronicle-native handoff occurs between two captured operations
- **THEN** no native observation is created from shared task, process, socket, connection, stream, protocol, timestamp, direction, or proximity values

### Requirement: First native source is cooperative and provider-neutral

The first production source SHALL be an opt-in Chronicle cooperative execution-context carrier plus a Chronicle application/transport boundary adapter. The application/runtime integration SHALL create or receive a generation-safe Chronicle context, transfer it through an explicit continuation API, and deliver the resulting observations through a bounded non-frozen runtime/composition channel. The source SHALL work without OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, Jaeger, Zipkin, or any provider SDK.

#### Scenario: Cooperative source creates observation

- **WHEN** an application/runtime integration explicitly transfers Chronicle execution context into a child operation
- **THEN** the cooperative source creates a native observation before canonical session construction completes
- **AND** the observation is delivered through the Chronicle-owned composition contract

#### Scenario: Parent context reaches child explicitly

- **WHEN** child operation A1 is invoked under parent context A
- **THEN** the Chronicle-native carrier passes an opaque Chronicle context generation into A1
- **AND** PID/TID, task name, process ancestry, socket identity, connection identity, or trace-provider context is not used as the handoff mechanism

#### Scenario: Application integration is required

- **WHEN** an application runs without Chronicle cooperative integration
- **THEN** it remains recordable through existing passive capture
- **AND** it does not receive native execution-causality claims

#### Scenario: Native path has no provider dependency

- **WHEN** cooperative native mode runs with no tracing provider installed
- **THEN** observations, binding, evidence derivation, and resolver execution remain available
- **AND** the core/domain/default executable dependency closure contains no provider SDK

### Requirement: NativeOperationAnchor is complete, generation-safe, and pre-canonical

A `NativeOperationAnchor` SHALL contain a complete Chronicle-owned boundary receipt with recording scope, source epoch placement, `SourceConnectionGeneration`, protocol identity, direction, an authoritative protocol-owned exact operation-boundary identity, and exact source/reconstruction provenance sufficient to identify one later canonical operation or fail closed. The boundary identity MAY be opaque; it MUST be produced by, or derived from, the same protocol boundary/canonicalization authority that deterministically defines operation segmentation. The anchor MAY contain an opaque Chronicle execution-context generation as provenance, but that value SHALL NOT be an ownership predicate by itself. An anchor SHALL be bounded and SHALL NOT contain transitive ancestry.

The first supported receipt SHALL use current facts where available: `SourceConnectionGeneration`, the authoritative protocol-owned operation-boundary identity from the protocol segmentation path, recording/epoch/reconstruction lineage, and exact `WalByteRange` or equivalent source placement. An application/runtime integration MUST NOT synthesize a request number, message index, counter, or ordinal and use it as binding authority. A new non-frozen boundary receipt is required because current passive code exposes connection generation and provenance but no execution-handoff-to-operation binding fact. An implementation SHALL NOT substitute an unsupported field or bare identifier.

#### Scenario: Valid anchor has complete source scope

- **WHEN** a cooperative transport boundary supplies a parent or child anchor
- **THEN** the anchor contains recording scope, source epoch placement, generation-safe source generation, protocol identity, direction, exact local position, and exact source provenance
- **AND** the anchor contains no final operation ID or scenario identity

#### Scenario: SourceConnectionGeneration is not sufficient alone

- **WHEN** an anchor contains only `SourceConnectionGeneration` or only a socket cookie, connection ID, or stream ID
- **THEN** the anchor is rejected as incomplete
- **AND** no canonical operation binding is attempted

#### Scenario: Forbidden bare values do not form anchors

- **WHEN** a proposed anchor relies only on PID, TID, task ID, worker name, file descriptor, socket cookie, connection ID, protocol stream ID, WAL sequence, timestamp, or operation position without generation-safe owning scope
- **THEN** the source rejects the anchor
- **AND** no positive native relation is emitted

#### Scenario: Identifier reuse remains distinguishable

- **WHEN** a PID/TID/task, FD, socket, connection, or stream value is reused in a later generation
- **THEN** complete source generation and exact source/reconstruction provenance distinguish the anchors
- **AND** stale context cannot bind to the later operation

#### Scenario: Unsupported protocol boundary fails closed

- **WHEN** a protocol adapter cannot provide an authoritative exact operation-boundary identity and source provenance for an anchor
- **THEN** the observation is unresolved
- **AND** protocol kind, timing, processing order, or runtime-local position does not replace the missing boundary fact

#### Scenario: Runtime-local operation counters are rejected

- **WHEN** an application/runtime integration supplies its own request number, message index, counter, or ordinal instead of the protocol-owned segmentation identity
- **THEN** the anchor is rejected as lacking authoritative boundary provenance
- **AND** no canonical binding or positive native relation is emitted

#### Scenario: Protocol authority disagreement fails closed

- **WHEN** runtime/transport and protocol canonicalization disagree about which operation boundary a receipt describes
- **THEN** the receipt is unresolved
- **AND** neither source may choose a boundary or repair the disagreement with order, timing, or connection identity

### Requirement: Pending handoff observations wait for complete receipts

A `NativeExecutionHandoffObservation` SHALL be assembled only after both parent and child `NativeOperationAnchor` receipts are complete. The source SHALL retain incomplete parent/child receipt state in bounded pending storage and SHALL emit no partial anchor to ETL. Parent and child completion MAY occur in either order. Each pending child SHALL remain independent, duplicate completion SHALL be idempotent, and one incomplete observation SHALL NOT block unrelated observations. If an endpoint never completes, pending state is lost on bounded expiry, overflow, or restart, or side-channel delivery loses the pending entry, the source SHALL emit a typed unresolved or bounded-loss diagnostic and no positive native relation. Timestamp, proximity, processing order, active-operation, PID/TID/task, socket/connection, stream, or other contextual fallback is forbidden.

#### Scenario: Child continuation precedes parent receipt

- **WHEN** child continuation is observed before parent boundary receipt completion
- **THEN** the handoff remains pending with no partial parent anchor
- **AND** later completion of both receipts produces one complete observation that may bind exactly

#### Scenario: Parent receipt completes later

- **WHEN** child receipt completes first and parent receipt completes later within the bounded source lifecycle
- **THEN** the completed observation binds using both authoritative receipts
- **AND** no child-first ordering heuristic is used

#### Scenario: Parent receipt completes first

- **WHEN** parent receipt completes before child receipt
- **THEN** the parent is retained as pending context for the child
- **AND** child completion produces one complete observation without using an active-operation fallback

#### Scenario: Parent never completes

- **WHEN** child continuation is observed but parent receipt never becomes complete
- **THEN** bounded expiry/loss emits a typed unresolved or bounded-loss diagnostic
- **AND** no native relation is emitted

#### Scenario: Child never completes

- **WHEN** parent receipt completes but child receipt never becomes complete
- **THEN** bounded expiry/loss emits a typed unresolved or bounded-loss diagnostic
- **AND** no native relation is emitted

#### Scenario: Restart loses pending state

- **WHEN** source restart loses an incomplete parent, child, or handoff entry
- **THEN** recovery emits no guessed relation
- **AND** native evidence remains absent while existing WAL, completeness, and replay semantics remain unchanged

#### Scenario: Duplicate completion is idempotent

- **WHEN** receipt or handoff completion notifications are delivered repeatedly
- **THEN** one deterministic complete observation is emitted
- **AND** duplicate relations and duplicate positive evidence are not created

#### Scenario: Multiple children remain independent and bounded

- **WHEN** two children are pending against one incomplete parent
- **THEN** each child retains an independent bounded pending entry
- **AND** completion or loss of one child does not block, complete, or mutate the other

### Requirement: Anchor binding is exact and fail-closed

ETL SHALL bind each pre-canonical anchor against supplied canonical sessions using exact composite identity: recording scope, source epoch/lineage, source generation, owning protocol, direction, protocol-local operation boundary, and exact retained source provenance. Exactly one matching canonical operation SHALL produce one full `CanonicalOperationRef`. Zero matches SHALL produce an unresolved binding diagnostic. Multiple matches SHALL produce an ambiguous binding diagnostic. Binding SHALL never choose a nearest, first, last, active, ordered, role-based, or otherwise guessed candidate.

Binding diagnostics SHALL remain distinct from resolver causal ambiguity. An unresolved or binding-ambiguous anchor SHALL produce no bound handoff fact and no positive correlation evidence. Only two independently exact-bound parent candidates may create resolver-level `Ambiguous` support.

#### Scenario: Exact anchor maps to one operation

- **WHEN** a complete anchor matches exactly one canonical operation under recording, source epoch/lineage, generation, protocol boundary, and source provenance
- **THEN** binding returns that operation's full `CanonicalOperationRef`, including verified completion-owner epoch and session scope
- **AND** no field is inferred from operation order or timing

#### Scenario: Zero-match anchor

- **WHEN** no canonical operation satisfies every binding predicate for an anchor
- **THEN** binding returns an unresolved diagnostic
- **AND** no positive native relation or resolver support is emitted

#### Scenario: Multi-match anchor

- **WHEN** two or more canonical operation occurrences satisfy the supplied anchor
- **THEN** binding returns an ambiguous-binding diagnostic
- **AND** binding does not choose nearest, first, last, active ingress, WAL order, processing order, role, or protocol kind
- **AND** resolver causal ambiguity is not materialized

#### Scenario: Cross-epoch operation binds through source lineage

- **WHEN** a parent source operation is in epoch N and child source operation is in epoch N+1, while either operation completes under a later owner epoch
- **THEN** each anchor binds through its source epoch/reconstruction provenance
- **AND** resulting references use each operation's verified completion-owner epoch

#### Scenario: Binding validates full reference scope

- **WHEN** an anchor appears to match an operation ID but recording, epoch, session, or occurrence validation fails
- **THEN** binding returns an unresolved or ambiguous diagnostic
- **AND** bare `OperationId` equality cannot admit the operation

### Requirement: Bound handoff facts are the only bridge to canonical evidence

After both endpoints bind exactly, ETL SHALL create a bounded `BoundNativeExecutionHandoffFact` containing the parent and child `CanonicalOperationRef`, `ExecutionContinuation`, and Chronicle-owned provenance. ETL SHALL verify same-recording scope, endpoint existence, direction, and input validity before deriving evidence. Exact duplicate observations SHALL be idempotent under deterministic canonical ordering. The producer SHALL emit only child-side `NativeExecutionLineage` correlation evidence; it SHALL NOT create scenarios, confidence, witnesses, selected edges, or role resolutions.

#### Scenario: Bound fact derives native evidence

- **WHEN** parent A and child A1 each bind exactly and the handoff relation is valid
- **THEN** ETL creates one bound fact and emits `NativeExecutionLineage { parent: A, relation: ExecutionContinuation }` on A1's correlation channel
- **AND** the existing resolver remains the only authority for selection

#### Scenario: One endpoint fails binding

- **WHEN** either parent or child anchor is unresolved or binding-ambiguous
- **THEN** no bound fact is created
- **AND** no partial relation, guessed endpoint, or contextual fallback is emitted

#### Scenario: Duplicate delivery is idempotent

- **WHEN** the same observation is delivered more than once in different input orders
- **THEN** exact duplicates collapse deterministically
- **AND** output evidence, diagnostics, and resolver input remain byte-identical

#### Scenario: Conflicting facts remain positive evidence

- **WHEN** two independently bound facts name different valid parents for one child
- **THEN** both valid positive facts reach the existing resolver
- **AND** ETL does not vote, prioritize, or discard one source to force ownership

### Requirement: Passive-only mode remains conservative

Existing passive recording SHALL remain operational without the cooperative source. Passive capture, reconstruction, socket generations, protocol sequences, WAL placement, timestamps, roles, and contextual evidence SHALL remain available as evidence/context, but SHALL NOT create `ExecutionContinuation` ownership support without an explicit native observation and exact binding. A passive recording with no native observation and no trace relationship SHALL leave non-root operations uncorrelated unless existing non-native resolver evidence independently proves otherwise.

#### Scenario: Passive-only no-observation boundary

- **WHEN** `chronicle record -- app` runs without Chronicle runtime cooperation and without trace evidence
- **THEN** capture and canonical session artifacts remain valid under existing contracts
- **AND** non-root operations remain conservatively `Uncorrelated` when no other authorized positive relation exists

#### Scenario: Passive facts do not become causality

- **WHEN** passive recording exposes one active ingress, matching socket/connection identifiers, timestamps, protocol order, or shared process metadata
- **THEN** those facts remain contextual
- **AND** they do not create a native parent or scenario edge

### Requirement: Native state and loss behavior are bounded without normative count constants

Native source and ETL SHALL bound active context state, pending observations, per-child native facts, batch work, diagnostics, and retained one-hop relations. They SHALL NOT retain transitive ancestry histories or operation-by-scenario matrices. Overflow, missing receipt, side-channel loss, restart, or incompatible lineage SHALL fail closed or emit typed bounded-loss diagnostics. Positive native relations SHALL NOT be silently truncated while claiming complete evidence. Concrete capacity defaults remain implementation/configuration decisions requiring resource measurement, not normative values in this capability.

#### Scenario: Bounded producer overflow

- **WHEN** native source or ETL reaches its configured resource bound
- **THEN** it stops admitting positive work or emits typed bounded-loss diagnostics
- **AND** it does not silently drop positive relations while reporting complete native evidence

#### Scenario: Restart without durable side channel

- **WHEN** the process restarts before non-frozen observations are available for binding
- **THEN** missing observations produce no native relation
- **AND** WAL recovery, checkpoint ordering, completeness, and replayability remain unchanged

#### Scenario: No unbounded ancestry

- **WHEN** a long handoff chain is processed
- **THEN** the producer retains only bounded one-hop observation/fact state needed for current composition
- **AND** transitive ancestry is recomputed by the existing resolver rather than stored by the producer

### Requirement: Validation separates composition proof from native production proof

The change SHALL validate both an already-bound composition path and a real cooperative observation path. Composition tests MAY begin with `BoundNativeExecutionHandoffFact`. Native production E2E SHALL begin with a real cooperative runtime handoff and pre-canonical observations, then exercise exact binding, bound-fact creation, evidence derivation, resolver selection, and graph validation. Production E2E SHALL NOT begin with manually constructed final `CanonicalOperationRef` handoff facts.

#### Scenario: Composition/integration proof

- **WHEN** an integration test supplies canonical sessions plus an already-bound handoff fact
- **THEN** ETL composition derives native evidence and the existing resolver returns a validated graph
- **AND** the result is labeled composition/integration, not native production E2E

#### Scenario: Native production E2E

- **WHEN** an overlapping A/B/C scenario uses the cooperative source to emit real parent/child anchors
- **THEN** the test observes anchors, binds them exactly to canonical operations, derives native evidence, and invokes the existing resolver
- **AND** the starting input does not contain final bound handoff references

#### Scenario: Required native edge cases

- **WHEN** production validation exercises exact binding, zero-match, multi-match, identifier reuse, async continuation, missing observation, binding ambiguity, true causal ambiguity, native/trace agreement, native/trace disagreement, passive-only mode, and no-provider mode
- **THEN** each result follows its specified fail-closed or existing-resolver behavior

### Requirement: Native evidence preserves frozen compatibility and provider boundaries

Native observations, anchors, binding diagnostics, bound facts, and resolver results SHALL remain non-frozen runtime/composition values in this change. They SHALL NOT be added to Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public CLI JSON, replay-safety contracts, or persisted scenario artifacts. The implementation SHALL NOT add `EventId` or provider SDK types to Chronicle core/domain/default executable APIs.

#### Scenario: Frozen contracts remain unchanged

- **WHEN** cooperative native mode is enabled
- **THEN** all frozen 0.1.x contracts and default publication/checkpoint behavior remain unchanged
- **AND** native side-channel loss cannot weaken WAL durability or recovery authority

#### Scenario: Durable native evidence is deferred

- **WHEN** durable restart/replay of native observations is required
- **THEN** a later explicitly versioned sidecar/evidence-contract change is required
- **AND** this capability does not smuggle observations into an existing frozen artifact
