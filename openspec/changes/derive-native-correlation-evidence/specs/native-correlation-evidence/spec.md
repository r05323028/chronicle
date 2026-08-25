## Purpose

Define how Chronicle derives bounded, deterministic, provider-neutral relational evidence from explicit Chronicle-owned execution facts while keeping infrastructure identifiers, timing, and role evidence contextual.

## ADDED Requirements

### Requirement: Native evidence is Chronicle-owned and provider-neutral

Native correlation evidence SHALL use Chronicle-owned values only. A native producer SHALL NOT expose OpenTelemetry, Datadog, AWS X-Ray, B3, W3C SDK, or any other provider SDK type to `chronicle-canonical`, `chronicle-etl`, or the default `chronicle` executable dependency closure. Provider adapters, when present, MAY translate external data into the same Chronicle-owned evidence boundary, but they are not native evidence producers.

#### Scenario: Native path works without tracing packages

- **WHEN** a recording supplies explicit Chronicle execution-handoff facts and no trace-provider package is installed
- **THEN** native evidence derivation and correlation resolution operate normally
- **AND** no provider dependency is required by the default distribution

#### Scenario: Provider data stays outside the native contract

- **WHEN** an outer adapter has external trace data
- **THEN** it may translate that data into existing Chronicle-owned correlation evidence
- **AND** native derivation consumes no provider SDK object or provider-specific API

#### Scenario: Provider dependency leakage fails architecture validation

- **WHEN** implementation adds a provider SDK to a core/domain crate or any crate in the default executable closure
- **THEN** existing architecture validation rejects the change
- **AND** this capability does not add a second provider-specific validation mechanism

### Requirement: Current recording facts remain contextual without an explicit causal relation

Native derivation MUST NOT promote equality, proximity, overlap, or absence of contradiction into candidate-specific support. The following remain contextual-only unless a later specification names a stronger proof: `ProcessMetadata` PID/TID/executable values; task or worker labels; process/thread generation values; file descriptors, socket cookies, connection IDs, and five-tuples; protocol stream IDs and message/WAL sequence values; wire direction and socket role; relative timestamps and lifetimes; and generic custom values. `ProtocolOwnership` remains role-classification evidence, not correlation evidence.

#### Scenario: Only one active ingress is not enough

- **WHEN** an egress has no explicit native relation and only one ingress happens to be active
- **THEN** derivation emits no native ownership evidence
- **AND** the resolver leaves the egress `Uncorrelated`

#### Scenario: Equal task or process values do not support ownership

- **WHEN** concurrent operations share a task label, PID, TID, process generation, or worker string
- **THEN** those equal values add no candidate-specific support
- **AND** no scenario is selected from the equality

#### Scenario: Same socket or connection generation is not causal ownership

- **WHEN** operations share a reuse-safe socket or connection generation, or share a protocol stream
- **THEN** the generation/stream fact may be retained as context
- **AND** it does not by itself create native correlation evidence

#### Scenario: Timing remains contextual

- **WHEN** an egress overlaps an ingress, starts nearest to an ingress, or begins after the ingress response completes
- **THEN** timing changes no native support result
- **AND** non-overlap does not eliminate an otherwise explicit causal relation

### Requirement: Explicit execution handoff is the first native relationship family

The first native producer SHALL support only an explicit Chronicle-owned execution handoff. A `NativeExecutionHandoffFact` SHALL identify one child operation and one parent operation by complete `CanonicalOperationRef` values and SHALL mean that a Chronicle-owned runtime handoff source observed execution context transferred from the parent operation to the child operation. The fact is a relation assertion, not a scenario assignment: it SHALL contain no `ScenarioId`, confidence, selected edge, or candidate choice. A handoff token or equivalent capability MAY be used by the source, but token equality alone is not the predicate; the source must record the explicit parent-to-child handoff event.

#### Scenario: Synchronous continuation produces one candidate-relative relation

- **WHEN** Chronicle's handoff source explicitly records parent operation A handing execution to child operation A1
- **THEN** derivation emits one `NativeExecutionLineage { parent: A }` item in A1's correlation-evidence channel
- **AND** it emits no scenario-selection result

#### Scenario: Async continuation survives response completion

- **WHEN** the explicit handoff from ingress A to child A3 is recorded and A3 begins after A's response completes
- **THEN** derivation emits the same parent-relative native relation
- **AND** timing and response completion do not suppress it

#### Scenario: No handoff means no native relation

- **WHEN** a child operation has no explicit Chronicle handoff fact
- **THEN** derivation emits no `NativeExecutionLineage` item for that child
- **AND** the existing resolver receives no manufactured support

#### Scenario: First slice does not infer broad lineage

- **WHEN** a recording contains process creation, task inheritance, socket reuse, or protocol-stream observations without an explicit handoff
- **THEN** those observations remain contextual
- **AND** process/task, socket/connection, and protocol-stream relationship families remain deferred

### Requirement: Native evidence uses exact operation scope and generation safety

Native facts SHALL be accepted only when both parent and child references resolve to exactly one admitted operation in the same recording scope. The complete reference — recording, owner epoch, session, and operation — is authoritative. Operation-ID equality, session naming, WAL order, or processing order SHALL NOT fill missing scope. If a pre-canonical producer uses an operation anchor, the anchor SHALL include a reuse-safe source generation and an explicit protocol-local operation position; a bare PID, TID, task ID, file descriptor, socket cookie, connection ID, stream ID, or sequence number is not an anchor. Ambiguous or stale anchor mapping SHALL produce no relation and a bounded diagnostic, not a guessed reference.

#### Scenario: Cross-epoch handoff is valid

- **WHEN** parent A is owned by epoch N and child A1 is owned by epoch N+1, and both full references are verified
- **THEN** native evidence may link them without treating the epoch boundary as a protocol or scenario boundary

#### Scenario: Reused operation IDs remain distinct

- **WHEN** two sessions contain equal `OperationId` values but a fact names only one complete reference
- **THEN** the fact applies only to that exact reference
- **AND** the other operation receives no inherited support

#### Scenario: Reused transport identifiers do not bind stale facts

- **WHEN** a socket, task, process, file descriptor, or connection identifier is reused across generations
- **THEN** a fact with a mismatched generation is rejected or omitted
- **AND** no relation crosses the generation boundary

#### Scenario: Orphan or mismatched reference fails closed

- **WHEN** a fact names a missing operation, a different recording, an unverified epoch/session, or multiple operations
- **THEN** derivation reports a typed bounded input issue or omits the relation according to its source contract
- **AND** it never infers a replacement from identifiers or order

### Requirement: Derivation and resolver selection remain separate

The native producer SHALL derive only Chronicle-owned `CorrelationEvidence` items and bounded derivation diagnostics. It SHALL NOT create scenarios, select owners, assign confidence, construct selected causal edges, resolve ambiguity, or rewrite role classifications. The existing correlation resolver remains the sole authority for support closure, outcome materialization, witness derivation, confidence, causal-parent selection, and cycle handling.

#### Scenario: Role evidence remains isolated

- **WHEN** a native fact is derived for an operation whose role evidence is `Known`, `Unknown`, or `Ambiguous`
- **THEN** the native item is added only to the explicit correlation-evidence channel
- **AND** nested role evidence remains byte-for-byte unchanged and cannot support correlation

#### Scenario: Native relation does not choose a scenario

- **WHEN** a child has native relations to parents in multiple scenarios
- **THEN** the producer emits both candidate-relative relations when both facts are valid
- **AND** the resolver materializes `Ambiguous` rather than the producer choosing one scenario

#### Scenario: Native relation does not choose a parent edge

- **WHEN** multiple valid native and/or trace parent relations target one child
- **THEN** the producer emits relations without filtering to one parent
- **AND** the resolver's existing unique-parent and SCC sequence decides whether any edge survives

### Requirement: Ambiguous native derivation never guesses

A native source ambiguity SHALL remain distinguishable from a proven relation. If one raw handoff cannot be mapped to exactly one parent/child reference, derivation SHALL emit no positive relation for that unresolved mapping and SHALL retain only bounded diagnostic state. Independently proven relations to multiple distinct parents SHALL be emitted as separate candidate-relative evidence; the resolver then preserves `Ambiguous` when support reaches multiple scenarios. No ambiguity may be collapsed by nearest time, first observation, latest observation, or one remaining candidate.

#### Scenario: Ambiguous anchor mapping stays unresolved

- **WHEN** one handoff anchor can map to parent A or parent B and no generation/protocol fact disambiguates it
- **THEN** no native relation is emitted from that handoff
- **AND** the child remains `Uncorrelated` unless another positive source supports it

#### Scenario: Independently proven competing parents stay ambiguous

- **WHEN** valid handoff facts explicitly prove child Z has parent A and parent B, and A/B belong to different scenarios
- **THEN** both native relations reach the resolver
- **AND** Z materializes `Ambiguous` with both supported candidates and no selected parent edge

#### Scenario: Contradictory source order does not choose

- **WHEN** equivalent native facts arrive in different order or a retry repeats one fact
- **THEN** deduplication and ordering are deterministic
- **AND** no first/last fact wins semantic ownership

### Requirement: Native and trace evidence share one positive-support boundary

Native and trace evidence SHALL enter the existing resolver through the same correlation-evidence channel and relation-index boundary. Each source may contribute candidate-specific positive support; neither source is a weighted vote, priority override, negative contradiction, or fallback selector. If valid native and trace relations support different scenarios, the resolver SHALL preserve `Ambiguous` or omit a parent edge according to its existing rules. If they support the same scenario, the resolver SHALL retain provenance for both without changing confidence semantics.

#### Scenario: Native and trace agree

- **WHEN** a native continuation and trace relationship both support child A1 under Scenario A
- **THEN** the resolver produces the same Scenario A ownership it would produce from the valid positive evidence
- **AND** both provenance sources remain inspectable within deterministic retention limits

#### Scenario: Native and trace disagree

- **WHEN** native evidence supports Scenario A while trace evidence independently supports Scenario B
- **THEN** both candidates remain supported
- **AND** the outcome is `Ambiguous` rather than provider-priority selection

#### Scenario: No provider coupling

- **WHEN** trace evidence is absent or supplied by an outer adapter
- **THEN** native derivation and resolver behavior remain unchanged
- **AND** no provider SDK type crosses the domain boundary

### Requirement: Native derivation is bounded and one-hop

The producer SHALL retain only bounded active handoff state and one-hop parent/child facts required for the current derivation batch. It SHALL NOT retain transitive ancestry paths, all possible explanations, an operation-by-scenario matrix, or an unbounded task/socket-generation history. Per-operation and active-handoff limits SHALL be explicit. On limit exhaustion, the producer SHALL fail closed or report bounded loss and leave affected operations without manufactured native support; it SHALL NOT silently truncate positive relations or change resolver semantics.

#### Scenario: Transitive ancestry is not stored

- **WHEN** A hands to B and B hands to C
- **THEN** native derivation retains A→B and B→C as bounded one-hop facts only
- **AND** it does not materialize or store an A→C explanation; resolver closure handles transitivity

#### Scenario: Active handoff state is bounded

- **WHEN** more concurrent execution contexts exist than the configured active-handoff bound
- **THEN** new observations receive typed bounded-loss handling
- **AND** no unbounded queue or history is allocated

#### Scenario: Per-operation evidence is bounded

- **WHEN** one child receives more native facts than its configured evidence bound
- **THEN** derivation reports bounded loss or fails closed for that child
- **AND** it does not discard an arbitrary relation and claim complete semantics

#### Scenario: Recording lifetime does not create ancestry growth

- **WHEN** recording spans many epochs and segments
- **THEN** each operation retains bounded direct native relations with explicit epoch/session scope
- **AND** epoch rollover does not create a transitive ancestry collection

### Requirement: Native derivation is deterministic and retry-safe

Given the same verified canonical sessions and the same native fact set, derivation SHALL produce byte-identical correlation evidence and diagnostics independent of input iteration order, hash-map ordering, worker scheduling, retries, or recorder restart. Facts SHALL be normalized by a canonical total order over child reference, parent reference, relation kind, and complete provenance; exact duplicate facts SHALL be idempotent. No random or processing-order identifier may be introduced. If the runtime fact set is unavailable on replay, canonical-only derivation SHALL deterministically emit no native support rather than reconstructing it from timing or identifiers.

#### Scenario: Permutation produces identical evidence

- **WHEN** the same handoff fact multiset is presented in forward, reverse, and shuffled order
- **THEN** derived evidence and diagnostics are byte-identical

#### Scenario: Retry is idempotent

- **WHEN** ETL retries derivation with the same canonical sessions and facts
- **THEN** it produces the same evidence once, without duplicate relations or changed provenance

#### Scenario: Restart with same inputs is stable

- **WHEN** derivation restarts and reloads the same canonical sessions and serializable native fact set
- **THEN** output is identical to the pre-restart output

#### Scenario: Canonical-only replay fails closed

- **WHEN** identical canonical artifacts are replayed without a native fact set
- **THEN** native derivation emits an empty relation set deterministically
- **AND** operations remain uncorrelated unless other supplied positive evidence exists

### Requirement: Frozen contracts and durable scenario artifacts remain unchanged

This capability SHALL NOT modify Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted ETL checkpoint formats, public CLI JSON, replay-safety contracts, or add `EventId`. Runtime/composed native facts MAY be serializable Chronicle-owned inputs, but durable capture of that side channel requires a separate compatibility and evidence-durability change. This change SHALL persist no scenario graph or scenario artifact.

#### Scenario: Existing v1 readers and writers remain valid

- **WHEN** native evidence derivation is enabled or absent
- **THEN** existing capture, WAL, canonical-session, checkpoint, manifest, CLI, and replay contracts remain unchanged

#### Scenario: Native input is optional

- **WHEN** no Chronicle-owned handoff source supplies facts
- **THEN** capture, WAL, ETL, canonicalization, storage, replay, and CLI continue unchanged
- **AND** the resolver retains its current conservative mostly-uncorrelated behavior

#### Scenario: Durable handoff capture is deferred explicitly

- **WHEN** future deployments require native correlation to survive process restart without re-supplying runtime facts
- **THEN** a separate change defines a versioned durable evidence stream, lineage, checksums, retention, and migration policy
- **AND** this capability does not smuggle those fields into a frozen contract

### Requirement: First vertical slice proves concurrent execution handoff correlation

The first implementation SHALL provide a production-shaped Chronicle-owned handoff source and an end-to-end composition proof for overlapping ingress scenarios. The proof SHALL use no external tracing package, no timing-based selection, and no provider-specific data. It SHALL cover only explicit execution-handoff relations; unsupported relationship families remain uncorrelated.

#### Scenario: Concurrent ingress vertical slice

- **WHEN** overlapping ingress roots A, B, and C have explicit native handoff facts A→A1, A→A2, A→A3, B→B1, B→B2, and C→C1
- **THEN** each child resolves only to its explicitly supported scenario
- **AND** no child crosses to another active ingress because of overlap, process equality, connection equality, or order

#### Scenario: Mixed database and HTTP children

- **WHEN** A1/C1 are database operations and A2/B2 are outbound HTTP operations with explicit handoffs
- **THEN** protocol kind does not alter native relation semantics
- **AND** each operation keeps its own canonical role and completeness independently

#### Scenario: Async child after parent response

- **WHEN** A3 is handed off explicitly and begins after A's response completes
- **THEN** A3 resolves to Scenario A without temporal heuristics

#### Scenario: No tracing installed

- **WHEN** the vertical slice runs with no OpenTelemetry, Datadog, X-Ray, B3, or other provider package installed
- **THEN** native evidence still reaches the resolver and the scenario graph is correct
