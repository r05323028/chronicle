## Purpose

Define Chronicle's deterministic, provider-neutral, ambiguity-safe resolver that selects scenario ownership and selected causal-parent structure for canonical operations from Chronicle-owned role resolutions and candidate-relative `CorrelationEvidence`, producing valid `CorrelationGraph` output. The resolver computes what the `correlation-domain-model` capability validates; foundation validation semantics remain authoritative for resolver output.

## ADDED Requirements

### Requirement: Resolver is deterministic and provider-neutral

Chronicle SHALL provide a two-stage resolver that consumes recording-scoped canonical operation references, supplied `InteractionRoleResolution` values, and Chronicle-owned `CorrelationEvidence`, and SHALL produce a populated `CorrelationGraph` satisfying all `correlation-domain-model` invariants without modification of that validator. Given identical inputs, results SHALL be identical — including derived `ScenarioId`s and representation ordering — across process restarts, ETL retries, ETL replay over the same published artifacts, input iteration order, map/hash iteration order, worker scheduling, and epoch publication boundaries of already-identified operations. The resolver SHALL NOT depend on or expose OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, Jaeger, Zipkin, or other tracing SDK types.

#### Scenario: Restart and retry stability

- **WHEN** the resolver runs twice over identical canonical inputs in separate invocations simulating restart, ETL retry, and replay over the same published artifacts
- **THEN** both outputs are identical including every derived `ScenarioId`
- **AND** no output depends on wall-clock time, randomness, or processing counters

#### Scenario: Input order independence

- **WHEN** the same canonical inputs are presented in different iteration orders, including orders that would differ under hash-map traversal
- **THEN** semantic correlation outcomes, memberships, edges, and identifiers are unchanged
- **AND** any representation reordering is a deterministic canonical sort, not an outcome change

#### Scenario: No provider SDK enters resolution

- **WHEN** trace-shaped evidence exists only as Chronicle-owned opaque provider-labelled `CorrelationEvidence`
- **THEN** the resolver resolves it like any other relationship evidence
- **AND** no provider SDK type appears in the resolver API, and removing all provider adapters changes nothing except evidence availability

### Requirement: Scenario identity derives from recording and logical root operation only

The resolver SHALL derive each `ScenarioId` as `scenario-id-v1`: the first 16 bytes of `SHA-256(fixed_domain_separator || recording_uuid_be || root_operation_uuid_be)` rendered as a UUID with version field 8 (`octets[6] = (octets[6] & 0x0F) | 0x80`) and RFC 4122 variant (`octets[8] = (octets[8] & 0x3F) | 0x80`). The fixed domain separator SHALL be the exact ASCII bytes `chronicle-correlation/scenario-id/v1`; field order SHALL be separator, then `RecordingId` UUID bytes, then root `OperationId` UUID bytes, each as `Uuid::as_bytes()` big-endian arrays. Epoch identity and session identity SHALL NOT be derivation inputs. The stability guarantee SHALL be scoped exactly: scenario identity is stable within one recording lineage's persisted operation identities — across restarts, retries, resumed processing, replay over the same artifacts, input ordering, and regrouping of the same identified operations into different epoch/session publications. Stability SHALL NOT be claimed across independent re-canonicalization that regenerates random `OperationId`s from raw WAL; that stronger guarantee requires a dedicated future identity/versioning change.

#### Scenario: Deterministic derivation under repetition and permutation

- **WHEN** resolution repeats across simulated restarts and permuted input orders over the same identified operations
- **THEN** every scenario keeps exactly the same derived identifier
- **AND** the identifier equals the value produced by the specified algorithm for its root reference

#### Scenario: Publication regrouping preserves identity

- **WHEN** the same identified operations are grouped into different epoch/session publications and resolution reruns
- **THEN** scenario identifiers are unchanged because epoch/session scope is not a derivation input
- **AND** session or epoch substitution never occurs at any other decision point either

#### Scenario: Identity guarantee boundary is honest

- **WHEN** the same raw WAL is independently re-canonicalized without persisted operation identities, producing regenerated random `OperationId`s
- **THEN** scenario identifiers may legitimately differ and this is conforming behavior
- **AND** no requirement claims cross-republication stability, which stays deferred to a future identity/versioning change

### Requirement: Candidates come only from known ingress roots

The resolver SHALL construct one scenario candidate per admitted operation whose supplied role resolution is `Known(Ingress)`. Operations with `Known(Egress)`, `Unknown`, or `Ambiguous` role resolutions SHALL never own or root a scenario candidate. Scenario candidates SHALL exist as graph entities even when nothing resolves into them.

#### Scenario: One candidate per known ingress

- **WHEN** two admitted operations have role resolution `Known(Ingress)` and others do not
- **THEN** the resolver creates exactly two scenario candidates rooted at those references
- **AND** each candidate's identity derives from its root reference within the recording scope

#### Scenario: Egress never becomes a candidate owner

- **WHEN** an operation has role resolution `Known(Egress)` regardless of its evidence strength
- **THEN** it can resolve only as a member of an existing scenario candidate
- **AND** it never roots, owns, or creates a scenario

#### Scenario: Unknown or ambiguous role never becomes a root

- **WHEN** an operation's supplied role resolution is `Unknown` or `Ambiguous` while its evidence strongly suggests ingress causality
- **THEN** the resolver creates no scenario rooted at that operation
- **AND** it preserves the supplied role resolution verbatim instead of promoting it

### Requirement: Evidence acts only through named candidate-relative predicates

The resolver SHALL interpret evidence exclusively through defined relational predicates comparing child-side items against candidate-side items; it SHALL NOT infer semantics from field names alone. A predicate fires only when both sides carry the referenced items; a missing side yields no relation and never a default. For the current evidence kinds:

- `SharedTraceIdentity` — child `TraceRelationship` and candidate `TraceRelationship` share equal non-empty `provider` AND equal non-empty `trace_id`: positive scenario-level relationship support for that specific scenario; never direct-parent evidence; provider namespaces are part of identity so identical opaque values under different providers never match.
- `ExplicitParentSpan` — child `parent_span_id` is non-empty and equals a candidate's non-empty `span_id` under the same `provider`+`trace_id`: declared direct span parenthood; sufficient for direct-parent selection unless the target span is shared by multiple operations; also supports transitive membership inheritance through resolved members.
- `SameTaskLineage` — child `ExecutionTaskLineage.task` exactly equals a candidate's non-empty `task`: co-execution relationship supporting that specific scenario; never direct-parent evidence; differing task values never contradict because spawned work legitimately carries new task identities.
- Shared-span marking — multiple distinct operations carrying one `provider`+`trace_id`+`span_id` block direct-parent sufficiency for that span while leaving scenario-level relations intact.
- `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, and `Custom` are contextual only: they support no ownership, no parenthood, and contradict nothing; shared carriers and environments remain attached evidence for inspection.

Promoting any additional predicate, including namespaced `Custom` relations, SHALL require a specification change naming its comparison rule. Trace evidence remains opaque and provider-neutral while namespaced by provider.

#### Scenario: Same trace supports scenario membership but not a direct edge

- **WHEN** egresses X and Y each carry a trace relationship sharing Ingress A's provider and trace id with no parent-span linkage between them
- **THEN** X and Y resolve into A's scenario on `SharedTraceIdentity`
- **AND** no selected edge between X and Y or from A is fabricated from trace-id equality alone

#### Scenario: Explicit parent span selects the direct parent

- **WHEN** egress Z carries `parent_span_id` equal to HTTP X's unique `span_id` under the same provider and trace id
- **THEN** Stage B emits the X → Z selected edge within their common scenario

#### Scenario: Cross-provider identical IDs never match

- **WHEN** a child carries `provider: "otel", trace_id: "T"` and a candidate carries `provider: "xray", trace_id: "T"`
- **THEN** no predicate fires between them
- **AND** provider namespaces remain part of trace identity

#### Scenario: Missing sides degrade to no relation

- **WHEN** a predicate's required item is absent on either the child or every candidate, such as empty `span_id` values
- **THEN** the predicate does not fire and no default relation is synthesized

#### Scenario: Context-only agreement manufactures nothing

- **WHEN** an egress shares process/thread generation, connection environment, protocol family, and temporal overlap with several ingresses and holds no relational predicate items
- **THEN** no scenario becomes a supported candidate from those items
- **AND** they remain retained contextual evidence on whatever outcome is emitted

### Requirement: Ownership requires positive supported candidates

Stage A SHALL distinguish possible owners (eligible roots), supported candidates (roots bound to THIS operation by at least one positive ownership predicate: `SharedTraceIdentity`, `SameTaskLineage`, or inherited membership through explicit parent relations to resolved members), and sufficiently supported owners (supported candidates surviving contradiction checks). Absence of eliminating evidence SHALL NOT create support. Outcome selection over sufficiently supported owners SHALL be: zero → `Uncorrelated { evidence }`; exactly one → `Resolved { scenario, confidence }`; two or more → `Ambiguous { candidates }` where every candidate retains the specific evidence explaining why that scenario remains viable. Non-candidate-specific context SHALL never manufacture supported candidates.

#### Scenario: No evidence does not manufacture ambiguity

- **WHEN** Ingress A and Ingress B exist and egress X carries no candidate-specific relational evidence
- **THEN** X's outcome is `Uncorrelated`
- **AND** it does not become `Ambiguous(A, B)` merely because neither was eliminated

#### Scenario: Weak global context creates no candidates

- **WHEN** egress X shares process, connection environment, overlap, and protocol family with Ingresses A, B, and C but holds no predicate binding it to any one of them
- **THEN** the outcome is `Uncorrelated`
- **AND** none of A, B, or C appears as a candidate

#### Scenario: Candidate-specific ambiguity only

- **WHEN** egress X carries positive relational evidence specifically supporting Scenario A and specifically supporting Scenario B, while unrelated Ingress C holds no such relation
- **THEN** the outcome is `Ambiguous(A, B)`
- **AND** C never appears merely because it was not eliminated
- **AND** each emitted candidate retains its own explaining evidence

#### Scenario: One ingress owns multiple egress interactions

- **WHEN** Ingress A's scenario is positively linked by relational predicates to HTTP egress X and PostgreSQL egress Y, with no competing supported candidate for either
- **THEN** X and Y both resolve to Scenario A
- **AND** Scenario A contains Ingress A as root with both egress members

#### Scenario: Concurrent ingresses resolve independently

- **WHEN** Ingress A and Ingress B overlap in time while relational predicates uniquely connect egress X to A and egress Y to B
- **THEN** the resolver produces Scenario A {A, X} and Scenario B {B, Y}
- **AND** neither scenario absorbs the other's member despite lifetime overlap

#### Scenario: Three overlapping ingresses with sparse egresses

- **WHEN** Ingress A, B, and C lifetimes mutually overlap and egresses X and Y carry unique relational bindings for A and B respectively
- **THEN** X resolves into Scenario A and Y into Scenario B regardless of start order, duration, or overlap degree
- **AND** C owns no member merely by being open during X or Y

#### Scenario: Strong causal relation overrides timing intuition

- **WHEN** a decisive trace or task relation binds egress X to Ingress A while Ingress B is temporally closer
- **THEN** X resolves to Scenario A
- **AND** temporal proximity contributes nothing anywhere in the decision

#### Scenario: No valid parent remains uncorrelated

- **WHEN** no supported candidate survives for an egress
- **THEN** the result is `Uncorrelated` with retained evidence
- **AND** no synthetic owner, root, or membership entry is created

### Requirement: Temporal evidence is contextual only

Because current canonical offset semantics provide no documented recording-global timeline guarantee, the resolver SHALL perform no temporal elimination of any kind in this change. Lifetime overlap SHALL contribute no support; lifetime non-overlap SHALL eliminate no candidate — including a candidate whose lifetime ended before the child's began, since asynchronous downstream work may start after its cause completes. Safe contradiction evidence may reject an asserted causal relationship only when the contradiction is logically incompatible with that relationship under documented, trustworthy, comparable timeline semantics; ordinary lifetime non-overlap is not such a contradiction. Temporal relationships SHALL never override positive relational evidence. Any future temporal contradiction rule requires a dedicated change establishing timeline guarantees in the canonical model first.

#### Scenario: Async child after ingress completion

- **WHEN** Ingress A completed before egress X began, and task-lineage or trace relational evidence binds X to A's scenario
- **THEN** X resolves to Scenario A
- **AND** lifetime non-overlap eliminates nothing and downgrades nothing

#### Scenario: Decisive linkage despite disjoint lifetimes

- **WHEN** an `ExplicitParentSpan` or `SameTaskLineage` relation binds a child to a candidate whose lifetime is entirely earlier
- **THEN** the relation retains full force
- **AND** no rule compares lifetimes for elimination

#### Scenario: Temporal-only evidence stays unresolved

- **WHEN** an egress overlaps one or more ingress lifetimes and holds no relational predicate
- **THEN** the outcome is not `Resolved`
- **AND** the egress remains uncorrelated with its temporal evidence retained

#### Scenario: Incomparable timelines need no exception

- **WHEN** offsets originate from streams or sessions lacking proven comparability
- **THEN** outcomes are unaffected because temporal comparison never influences them

#### Scenario: Reverse causal ordering is not evaluated in this change

- **WHEN** a candidate's lifetime begins strictly after the child relationship could have begun on any observable offsets
- **THEN** this revision still performs no elimination because comparable-timeline guarantees do not yet exist
- **AND** introducing that check requires the dedicated timeline-guarantee change

### Requirement: Confidence reflects relationship semantics, not counts

The resolver SHALL map confidence from how ownership was established: `Exact` when ownership rests directly on at least one positive ownership predicate evaluated between the operation and the owning scenario's root or members; `Strong` when ownership rests solely on transitive inheritance through explicit parent relations to already-resolved members without any direct predicate of its own. The resolver SHALL never emit `Inferred`; externally supplied graphs retain that freedom. Numeric scores, dimension tallies, vote counts, and weighted combinations SHALL NOT exist in the resolver, and agreements among contextual-only kinds SHALL NOT produce resolution.

#### Scenario: Directly bound ownership is Exact

- **WHEN** an egress resolves because `SharedTraceIdentity` or `SameTaskLineage` binds it to the owning scenario directly
- **THEN** the resolution carries `Exact` confidence

#### Scenario: Inherited ownership is Strong

- **WHEN** a grandchild operation joins a scenario solely through explicit parent-span chains via already-resolved members
- **THEN** the resolution carries `Strong` confidence
- **AND** scenario ownership semantics are unchanged from any direct resolution

#### Scenario: Weak agreements stay weak

- **WHEN** two contextual-only dimensions agree about a candidate
- **THEN** no resolution and no confidence upgrade follows from their count

### Requirement: Direct causal-parent selection is a separate stage

For operations already `Resolved` into a scenario, Stage B SHALL evaluate sufficient direct-parent predicates among members of that same scenario only. Exactly one sufficient parent — currently a fired, non-shared `ExplicitParentSpan` — SHALL emit one `SelectedCausalEdge`. Multiple equally sufficient parents, insufficient relationships, or contextual-only relations SHALL leave the operation resolved with NO selected edge; unresolved parenthood is represented by the absence of the edge plus retained evidence, not by a second ambiguity channel or duplicate scenario candidates. Proposed parents outside the owning scenario SHALL be ignored for edges without rewriting scenario ownership. Chained structure attaches descendants along evidence-indicated chains; every emitted edge satisfies foundation invariants unchanged — full scoped endpoints, same scenario, acyclic, at most one parent per child, root never a child — and ambiguous operations never participate in edges.

#### Scenario: Same scenario, unresolved direct parent

- **WHEN** Database Z positively belongs to Scenario A while both HTTP X and HTTP Y carry equally sufficient direct-parent relations toward Z
- **THEN** Z remains `Resolved(Scenario A)`
- **AND** no selected edge references Z as child
- **AND** no duplicated or self-referential ambiguity candidate is invented

#### Scenario: Unique direct parent emits the edge

- **WHEN** evidence uniquely establishes X → Z inside Scenario A
- **THEN** one selected edge connects X to Z with full scoped references

#### Scenario: Cross-scenario direct-parent evidence is ignored for edges

- **WHEN** a proposed direct parent belongs to another scenario
- **THEN** the edge is not emitted and neither operation's scenario ownership changes

#### Scenario: Chained egress forms a grandchild edge without changing ownership

- **WHEN** an ingress causes an HTTP egress and that egress causes a database egress through explicit relations
- **THEN** the graph contains ingress → HTTP and HTTP → database edges within one scenario
- **AND** all three belong to the same single scenario

#### Scenario: Tree invariant violations are impossible by construction

- **WHEN** the resolver completes any input set
- **THEN** the emitted graph contains no self-edge, cycle, multi-parent child, or child root
- **AND** existing graph validation accepts the edge set unchanged

#### Scenario: Ambiguous candidates never become edges

- **WHEN** an operation ends ambiguous between Scenarios A and B
- **THEN** no selected edge references that operation in either scenario
- **AND** its candidate relationships remain only in resolution evidence

### Requirement: Role resolution is preserved verbatim

The resolver SHALL copy supplied `InteractionRoleResolution` values into the output graph unchanged. Correlation outcomes, selected edges, and scenario membership SHALL NOT rewrite an `Unknown` or `Ambiguous` role into a known role or let membership alter role evidence. Operations with unknown or ambiguous roles MAY become scenario members through ordinary relational resolution but SHALL never become roots.

#### Scenario: Unknown-role member keeps unknown role

- **WHEN** an operation with role resolution `Unknown` carries relational evidence binding it to Scenario A
- **THEN** it may appear as a Scenario A member with a `Resolved` correlation outcome
- **AND** its role resolution remains `Unknown` with original evidence intact

#### Scenario: Ambiguous-role member is never promoted

- **WHEN** an operation with `Ambiguous { Ingress, Egress }` role resolution joins Scenario A via strong relations
- **THEN** its role candidates and their evidence remain inspectable and unchanged
- **AND** it never qualifies as Scenario A's root

#### Scenario: Correlation never fabricates ingress

- **WHEN** a scenario needs a root and only non-root-eligible operations relate to it
- **THEN** the resolver leaves them without a scenario rather than inventing a root

### Requirement: Connection reuse preserves operation independence

Multiple logical operations sharing one physical connection SHALL remain independent canonical operations and independent correlation subjects throughout resolution. Connection, socket, stream, process, and thread identities SHALL remain evidence only and SHALL never merge operations, transfer resolutions between operations, or equal any scenario identity.

#### Scenario: Shared connection does not merge scenarios

- **WHEN** two egress exchanges reuse one database connection while belonging to relation-distinct scenarios
- **THEN** both remain separate members of their respective scenarios
- **AND** shared carrier identity neither merges nor reassigns either

#### Scenario: Carrier identity never equals scenario identity

- **WHEN** any resolution completes over connection-heavy traffic
- **THEN** every scenario identity derives from its root operation reference
- **AND** no connection, socket, stream, PID, TID, or port equals a scenario identity

### Requirement: Cross-epoch scenarios resolve through scoped references

The resolver SHALL support scenarios whose root and children are published in different finalized epoch sessions, resolving every operation through its full `CanonicalOperationRef` with lineage verification. Epoch rollover SHALL NOT split, rename, or duplicate a scenario whose operations keep their identities, and terminal operations SHALL participate under their completion-owner scope. Identical `OperationId` values under different session scopes SHALL remain distinct resolution subjects.

#### Scenario: Root and child span epochs

- **WHEN** an ingress is owned by epoch N's session and a related egress by epoch N+1's session, with relational evidence linking them
- **THEN** both resolve into one scenario whose root reference targets epoch N's session
- **AND** the selected edge crosses sessions with full scoped endpoints

#### Scenario: Terminal scope participates

- **WHEN** an operation begins in epoch N but its canonical completion is owned by epoch N+1
- **THEN** the resolver admits it under the completion-owner reference
- **AND** predecessor continuation ranges remain provenance, never a second subject

#### Scenario: Same operation ID in two sessions stays distinct

- **WHEN** equal `OperationId` values exist under different session scopes in one recording
- **THEN** the resolver treats them as distinct subjects with independent outcomes
- **AND** resolving one never affects the other

### Requirement: Resolution works identically with and without tracing

Trace context SHALL act as high-quality relational evidence when present, and the resolver SHALL produce complete, valid results from Chronicle-native relational evidence when such evidence is actually supplied — with no tracing SDK installed. This change does not make ordinary captured traffic auto-correlate: production of native correlation evidence from real recordings is the deferred `derive-native-correlation-evidence` follow-up. The absence of trace providers SHALL NOT degrade capture, WAL, ETL, canonicalization, storage, replay, or CLI behavior.

#### Scenario: Trace-assisted resolution

- **WHEN** provider-neutral trace relational evidence uniquely establishes ownership for concurrent traffic
- **THEN** the resolver uses it like any relational evidence
- **AND** no provider SDK type or adapter participates in resolution

#### Scenario: Supplied native relations suffice

- **WHEN** no trace context exists and Chronicle-native relational evidence — task lineage relations or equivalent supplied predicates — uniquely determines ownership
- **THEN** scenarios resolve with the same outcome shapes and determinism guarantees as trace-assisted runs
- **AND** missing trace parents are never synthesized from infrastructure identifiers or timestamps

#### Scenario: Unsupplied evidence is not mined from sessions

- **WHEN** an ordinary canonical session arrives with no correlation context
- **THEN** the composition boundary reports missing context per its join rules instead of fabricating lineage evidence
- **AND** resulting operations correctly stay uncorrelated rather than pretending native evidence exists

### Requirement: ETL composes through an explicit CorrelationContext join

`chronicle-etl` MAY compose resolution by joining published `CanonicalSession` values with an explicitly supplied provider-neutral correlation context containing role resolutions and correlation evidence keyed by full operation reference. Canonical sessions alone SHALL NOT constitute complete resolver input because frozen v1 sessions contain neither roles nor correlation evidence, and the helper SHALL NOT synthesize or fabricate either. Join rules SHALL be explicit: an admitted session operation with no context role-resolution entry SHALL fail composition as an error; a missing context evidence entry SHALL be treated as empty evidence; a context entry referencing no verified session operation SHALL fail composition as an error. The helper SHALL verify full operation references against session lineage, enrich them with lifetime/provenance views, contain zero ownership-selection semantics, and remain explicitly invoked — the default publication/checkpoint path SHALL NOT call it.

#### Scenario: Composition joins sessions with supplied context

- **WHEN** the helper receives sessions plus a complete correlation context and invokes the resolver
- **THEN** every outcome equals direct resolver invocation over the joined inputs
- **AND** the helper contributes reference verification and ordering normalization only

#### Scenario: Missing role context fails closed

- **WHEN** an admitted session operation lacks a role-resolution context entry
- **THEN** composition returns a typed error identifying the reference
- **AND** it neither defaults the role to ingress/unknown implicitly nor drops the operation

#### Scenario: Missing evidence entries are valid emptiness

- **WHEN** a context supplies role resolutions but no evidence entry for some referenced operation
- **THEN** that operation joins with empty evidence
- **AND** it typically resolves `Uncorrelated`, which is a normal outcome rather than an error

#### Scenario: Orphan context entries fail closed

- **WHEN** a context entry references an operation absent from all supplied sessions after lineage verification
- **THEN** composition returns a typed error naming the mismatched reference
- **AND** no silent partial admission occurs

#### Scenario: Publication path stays unchanged

- **WHEN** a recording publishes canonical sessions without requesting correlation
- **THEN** publication, checkpoints, and stored artifacts are byte-identical to pre-change behavior

#### Scenario: Architecture policy untouched

- **WHEN** architecture validation runs after implementation
- **THEN** the crate membership, allowlist edges, semantic boundaries, and external-dependency guards pass without edits beyond declaring the workspace-standard hashing dependency
- **AND** no standalone correlation crate exists and no provider package enters any protected closure

### Requirement: Resolver resource use is bounded and indexed

The resolver SHALL use deterministic ordered indexes for candidate and relation lookup, SHALL avoid full scans proportional to operations-times-ingresses-times-evidence where index hits suffice, SHALL bound transitive chaining rounds by owning-scenario member count with explicit recursion depth limits, SHALL cap retained evidence per outcome at a documented constant with deterministic selection that never drops predicate-bearing justification items before contextual ones, and SHALL keep internal state proportional to the input set rather than arbitrary external identifier cardinalities. Correctness SHALL never be traded for these bounds; where adversarial key-sharing degrades lookup, behavior remains correct.

#### Scenario: Indexed lookups replace global scans

- **WHEN** resolution processes many operations and concurrent ingress requests
- **THEN** candidate evaluation consults ordered indexes keyed by relation fields and reference
- **AND** no step iterates every operation against every ingress against every evidence item

#### Scenario: Chaining terminates deterministically

- **WHEN** transitive inheritance runs on any input set
- **THEN** rounds are bounded by owning-scenario member count and recursion depth is explicitly capped
- **AND** results match the unbounded fixpoint semantics for all representable inputs

#### Scenario: Retained evidence stays bounded

- **WHEN** an operation carries more evidence items than the documented retention constant
- **THEN** the outcome retains justification-bearing items first and fills remaining slots deterministically
- **AND** outcomes never depend on unbounded accumulation

### Requirement: Invalid inputs fail closed while insufficient evidence resolves normally

Typed resolver errors — failing closed — SHALL cover: duplicate full operation references; reference lineage or recording-scope mismatch; invalid supplied `InteractionRoleResolution`; malformed `CorrelationEvidence`; correlation-context join violations; and impossible internal graph invariants after construction. Ordinary evidence situations SHALL always yield an outcome rather than an error: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without an edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant because temporal evidence is contextual only.

#### Scenario: Broken inputs are typed errors

- **WHEN** resolver inputs contain duplicate references, mismatched recording scope, invalid roles, malformed evidence, or context join violations
- **THEN** the resolver rejects the input set with a typed error naming the violation
- **AND** it neither silently drops the offending data nor guesses substitutes

#### Scenario: Insufficient evidence is a normal outcome

- **WHEN** inputs are valid but carry no relational evidence, several competing relations, or uncertain parenthood
- **THEN** the resolver returns `Uncorrelated`, `Ambiguous`, or `Resolved` without an edge respectively
- **AND** no error is raised for any of these situations

### Requirement: Resolver remains runtime-only and compatibility-safe

The resolver SHALL operate in memory over non-v1 domain values and SHALL NOT persist correlation results, add fields to Capture Event v1 / Canonical Session v1 / WAL v1, introduce `EventId`, or add reader defaults or compatibility fallbacks. Persisted correlation artifacts SHALL require a dedicated follow-up change selecting a separately versioned sidecar/new artifact or an explicit versioned contract with migration policy.

#### Scenario: Frozen contracts unchanged

- **WHEN** the resolver ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and CLI contracts contain no correlation additions
- **AND** no persisted artifact records resolver output

#### Scenario: Persistence deferred explicitly

- **WHEN** durable correlation artifacts are required later
- **THEN** a dedicated change such as `persist-correlation-scenario-artifacts` chooses the versioned sidecar/new-artifact or migration design
- **AND** this resolver change authorizes neither
