## Purpose

Define Chronicle's deterministic, provider-neutral, ambiguity-safe resolver that selects scenario ownership and selected causal-parent structure for canonical operations from Chronicle-owned role resolutions and candidate-relative `CorrelationEvidence`, producing valid `CorrelationGraph` output. The resolver computes what the `correlation-domain-model` capability validates; foundation validation semantics remain authoritative for resolver output.

## ADDED Requirements

### Requirement: Resolver is deterministic and provider-neutral

Chronicle SHALL provide a two-stage resolver that consumes recording-scoped canonical operation references, supplied `InteractionRoleResolution` values, and Chronicle-owned `CorrelationEvidence`, and SHALL produce a populated `CorrelationGraph` satisfying all `correlation-domain-model` invariants without modification of that validator. Given identical inputs, results SHALL be identical — including derived `ScenarioId`s, representation ordering, and retained evidence — across process restarts, ETL retries, ETL replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash iteration order. The resolver SHALL NOT depend on or expose OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, Jaeger, Zipkin, or other tracing SDK types.

#### Scenario: Restart and retry stability

- **WHEN** the resolver runs twice over identical canonical inputs in separate invocations simulating restart, ETL retry, and replay over the same persisted artifacts
- **THEN** both outputs are identical including every derived `ScenarioId` and retained evidence selection
- **AND** no output depends on wall-clock time, randomness, or processing counters

#### Scenario: Input order independence

- **WHEN** the same canonical inputs are presented in different iteration orders, including orders that would differ under hash-map traversal
- **THEN** semantic correlation outcomes, memberships, edges, identifiers, and retained witnesses are unchanged
- **AND** any representation reordering is a deterministic canonical sort, not an outcome change

#### Scenario: No provider SDK enters resolution

- **WHEN** trace-shaped evidence exists only as Chronicle-owned opaque provider-labelled `CorrelationEvidence`
- **THEN** the resolver resolves it like any other relationship evidence
- **AND** no provider SDK type appears in the resolver API, and removing all provider adapters changes nothing except evidence availability

### Requirement: Scenario identity derives from the complete authoritative root reference

The resolver SHALL derive each `ScenarioId` as `scenario-id-v1`: the first 16 bytes of `SHA-256(fixed_domain_separator || recording_uuid_be || owner_epoch_uuid_be || session_uuid_be || operation_uuid_be)` rendered as a UUID with version field 8 (`octets[6] = (octets[6] & 0x0F) | 0x80`) and RFC 4122 variant (`octets[8] = (octets[8] & 0x3F) | 0x80`). The fixed domain separator SHALL be the exact ASCII bytes `chronicle-correlation/scenario-id/v1`; field order SHALL be separator, then the four `Uuid::as_bytes()` big-endian arrays of the root operation's authoritative `CanonicalOperationRef` in declaration order — recording, owner epoch, session, operation. Bare `OperationId` SHALL NOT be used outside its owning scope because the foundation forbids assuming cross-session uniqueness. Scenario identity SHALL be stable across resolver restarts, retries, replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash iteration order. Scenario identity SHALL NOT currently be guaranteed across canonical-session regrouping that changes the authoritative root reference, publication into a different owning session/epoch, independent re-canonicalization, or regenerated `OperationId`s; such durability requires a dedicated future logical-identity change.

#### Scenario: Deterministic derivation under repetition and permutation

- **WHEN** resolution repeats across simulated restarts and permuted input orders over the same persisted canonical artifacts
- **THEN** every scenario keeps exactly the same derived identifier
- **AND** the identifier equals the value produced by the specified algorithm for its full root reference

#### Scenario: Duplicate OperationId across sessions yields distinct scenarios

- **WHEN** Recording R owns Session A with a root whose `OperationId` equals X and Session B with a distinct root whose `OperationId` also equals X
- **THEN** the two roots remain distinct `CanonicalOperationRef`s
- **AND** their scenarios receive distinct derived identities because owner-epoch/session scope participates in the derivation
- **AND** neither scenario absorbs or collides with the other

#### Scenario: Identity guarantee boundary is honest

- **WHEN** canonical operations are regrouped into different owning sessions or independently re-canonicalized so authoritative references change or `OperationId`s regenerate
- **THEN** scenario identifiers may legitimately differ and this is conforming behavior
- **AND** no requirement claims stability across changed publication scope, which stays deferred to a dedicated durable logical-identity change

### Requirement: Scenario roots self-resolve through Chronicle-owned ScenarioRoot evidence

For every admitted operation whose role resolution is `Known(Ingress)`, the resolver SHALL derive the scenario identifier from that operation's complete authoritative reference, create `Scenario { id, root, members: [root] }`, and record the root's own correlation resolution as `Resolved { scenario: id, confidence: Exact, evidence: [ScenarioRoot { root }] }`. This change SHALL add one additive Chronicle-owned evidence kind, `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`, meaning the resolver deterministically establishes this known-ingress operation as the root of the scenario derived from its full reference; it SHALL be non-temporal so foundation validation accepts the root's resolution unchanged. Root-establishment SHALL be correlation-level evidence distinct from role classification: role evidence answers ingress-versus-egress while `ScenarioRoot` answers which scenario the root owns. The resolver SHALL generate this evidence itself; callers SHALL never place synthetic root evidence into correlation context. Role resolutions SHALL remain verbatim.

#### Scenario: Root resolves to its own scenario exactly once

- **WHEN** one `Known(Ingress)` operation A is admitted
- **THEN** Scenario A is created with A as root and sole initial member
- **AND** A carries exactly one resolution: `Resolved(Scenario A, Exact)` witnessed by its `ScenarioRoot` evidence

#### Scenario: Root-only scenario validates immediately

- **WHEN** a recording contains one `Known(Ingress)` operation and no other operations resolve into its scenario
- **THEN** the resulting graph contains a valid scenario whose membership is exactly its root
- **AND** existing foundation validation accepts the graph without weakening

#### Scenario: Root establishment does not alter role state

- **WHEN** the resolver establishes Scenario A around known-ingress A
- **THEN** A's `InteractionRoleResolution` remains byte-identical to its supplied value
- **AND** inspecting scenario membership never substitutes for or mutates role evidence

### Requirement: Candidates come only from known ingress roots

Stage A SHALL construct one scenario per admitted `Known(Ingress)` operation via the root-establishment semantics above. Operations with `Known(Egress)`, `Unknown`, or `Ambiguous` role resolutions SHALL never own or root a scenario. Scenarios SHALL exist even when nothing else resolves into them.

#### Scenario: One scenario per known ingress

- **WHEN** two admitted operations have role resolution `Known(Ingress)` and others do not
- **THEN** exactly two scenarios exist rooted at those references
- **AND** each identity derives from its root's full scoped reference

#### Scenario: Egress never becomes a candidate owner

- **WHEN** an operation has role resolution `Known(Egress)` regardless of evidence strength
- **THEN** it can resolve only as a member of an existing scenario
- **AND** it never roots, owns, or creates a scenario

#### Scenario: Unknown or ambiguous role never becomes a root

- **WHEN** an operation's role resolution is `Unknown` or `Ambiguous` while its evidence suggests ingress causality
- **THEN** no scenario is created rooted at that operation
- **AND** the supplied role resolution is preserved instead of promoted

### Requirement: Evidence acts only through named candidate-relative predicates

The resolver SHALL interpret evidence exclusively through defined relational predicates comparing child-side items against candidate-side items; it SHALL NOT infer semantics from field names alone. A predicate fires only when both sides carry the referenced items; missing sides yield no relation, never defaults. Positive ownership predicates are:

- `SharedTraceIdentity` — child `TraceRelationship` and candidate `TraceRelationship` share equal non-empty `provider` AND equal non-empty `trace_id`: positive scenario-level relationship support for that specific scenario; never direct-parent evidence; provider namespaces are part of identity, so identical opaque values under different providers never match.
- `ExplicitParentSpan` — child non-empty `parent_span_id` equals a candidate's non-empty `span_id` under the same `provider`+`trace_id`: declared direct span parenthood; supports transitive membership inheritance and is sufficient for Stage B direct parenthood unless the target span is shared by multiple operations.

Resolver-generated `ScenarioRoot` evidence resolves each root to its own scenario per the root-establishment requirement. All remaining kinds — `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom` — are contextual only: they support no ownership, no parenthood, contradict nothing, and are retained for inspection. Task and process lineage equality is contextual because no repository contract defines those strings as unique causal-execution identity; `derive-native-correlation-evidence` owns defining such contracts and may promote named predicates through specification changes. Shared span identity (`provider`+`trace_id`+`span_id` carried by multiple operations) blocks only direct-parent sufficiency, never scenario ownership. Promoting any additional predicate requires a specification change naming its comparison rule.

#### Scenario: Same trace supports several members of one scenario without duplication

- **WHEN** DB Z shares one provider+trace identity with Ingress A, HTTP X, and HTTP Y — all members of Scenario A
- **THEN** Stage A produces exactly one supported candidate keyed by Scenario A
- **AND** the three member matches aggregate into one candidate witness set rather than duplicate Scenario A candidates

#### Scenario: Explicit parent span selects the direct parent

- **WHEN** egress Z carries `parent_span_id` equal to HTTP X's unique `span_id` under the same provider and trace id
- **THEN** Stage B emits the X → Z selected edge within their common scenario

#### Scenario: Shared span ambiguity blocks only the edge

- **WHEN** two operations in the same scenario expose the same `provider`+`trace_id`+`span_id` and a child's `parent_span_id` targets that span
- **THEN** scenario ownership may still resolve on other predicates
- **AND** no selected direct-parent edge is produced toward the shared span

#### Scenario: Same task string alone resolves nothing

- **WHEN** two concurrent known-ingress roots and one egress all carry identical `ExecutionTaskLineage.task` labels with no trace relation between them
- **THEN** no supported candidate exists for the egress
- **AND** the outcome is `Uncorrelated` until some positive predicate fires

#### Scenario: Cross-provider identical IDs never match

- **WHEN** a child carries `provider: "otel", trace_id: "T"` and a candidate carries `provider: "xray", trace_id: "T"`
- **THEN** no predicate fires between them
- **AND** provider namespaces remain part of trace identity

#### Scenario: Missing sides degrade to no relation

- **WHEN** a predicate's required item is absent on either side, such as empty `span_id` values
- **THEN** the predicate does not fire and no default relation is synthesized

#### Scenario: Context-only agreement manufactures nothing

- **WHEN** an egress shares process/thread generation, connection environment, protocol family, temporal overlap, and task label with several ingresses but holds no trace relational items
- **THEN** no scenario becomes a supported candidate from those items
- **AND** they remain retained contextual evidence on whatever outcome is emitted

### Requirement: Ownership requires positive supported candidates aggregated by scenario

Stage A SHALL distinguish possible owners (eligible roots), supported candidates (possible owners bound to THIS operation by at least one fired positive ownership predicate), and sufficiently supported owners (supported candidates surviving contradiction checks). Supported candidates SHALL be keyed by `ScenarioId`: multiple members of one scenario matching the same operation aggregate into ONE candidate carrying the union of matching witnesses, never duplicates. Absence of eliminating evidence SHALL NOT create support. Outcomes over sufficiently supported owners: zero → `Uncorrelated { evidence }`; exactly one → `Resolved { scenario, Exact }`; two or more → `Ambiguous { candidates }` where every candidate retains deterministic witness evidence explaining why that specific scenario remains viable. Non-candidate-specific context SHALL never manufacture candidates.

#### Scenario: No evidence does not manufacture ambiguity

- **WHEN** Ingress A and Ingress B exist and egress X carries no candidate-specific relational evidence
- **THEN** X's outcome is `Uncorrelated`
- **AND** it does not become `Ambiguous(A, B)` merely because neither was eliminated

#### Scenario: Candidate-specific ambiguity only

- **WHEN** egress X carries positive relational evidence specifically supporting Scenario A and specifically supporting Scenario B, while unrelated Ingress C holds none
- **THEN** the outcome is `Ambiguous(A, B)`
- **AND** C never appears merely because it was not eliminated
- **AND** each emitted candidate retains its own explaining witnesses

#### Scenario: One ingress owns multiple egress interactions

- **WHEN** Ingress A's scenario is positively linked by shared-trace relations to HTTP egress X and PostgreSQL egress Y, with no competing supported candidate for either
- **THEN** X and Y both resolve to Scenario A
- **AND** Scenario A contains Ingress A as root with both egress members

#### Scenario: Concurrent ingresses resolve independently

- **WHEN** Ingress A and Ingress B overlap in time while relational predicates uniquely connect egress X to A and egress Y to B
- **THEN** the resolver produces Scenario A {A, X} and Scenario B {B, Y}
- **AND** neither scenario absorbs the other's member despite lifetime overlap

#### Scenario: Strong causal relation overrides timing intuition

- **WHEN** a decisive trace relation binds egress X to Ingress A while Ingress B is temporally closer
- **THEN** X resolves to Scenario A
- **AND** temporal proximity contributes nothing anywhere in the decision

#### Scenario: No valid parent remains uncorrelated

- **WHEN** no supported candidate survives for an egress
- **THEN** the result is `Uncorrelated` with retained evidence
- **AND** no synthetic owner, root, or membership entry is created

### Requirement: Temporal evidence is contextual only

Because current canonical offset semantics provide no documented recording-global timeline guarantee, the resolver SHALL perform no temporal elimination of any kind in this change. Lifetime overlap SHALL contribute no support; lifetime non-overlap SHALL eliminate no candidate — including a candidate whose lifetime ended before the child's began, since asynchronous downstream work may start after its cause completes. Safe contradiction evidence may reject an asserted relationship only when logically incompatible under documented, trustworthy, comparable timeline semantics; ordinary lifetime non-overlap is not such a contradiction. Temporal relationships SHALL never override positive relational evidence. Any future temporal contradiction rule requires a dedicated change establishing timeline guarantees first.

#### Scenario: Async child after ingress completion

- **WHEN** Ingress A completed before egress X began, and trace relational evidence binds X to A's scenario
- **THEN** X resolves to Scenario A
- **AND** lifetime non-overlap eliminates nothing and downgrades nothing

#### Scenario: Decisive linkage despite disjoint lifetimes

- **WHEN** an explicit parent-span relation binds a child to a candidate whose lifetime is entirely earlier
- **THEN** the relation retains full force
- **AND** no rule compares lifetimes for elimination

#### Scenario: Temporal-only evidence stays unresolved

- **WHEN** an egress overlaps one or more ingress lifetimes and holds no relational predicate
- **THEN** the outcome is not `Resolved`
- **AND** the egress remains uncorrelated with its temporal evidence retained

#### Scenario: Reverse causal ordering is not evaluated in this change

- **WHEN** a candidate's lifetime begins strictly after the child relationship could have begun on any observable offsets
- **THEN** this revision still performs no elimination because comparable-timeline guarantees do not yet exist
- **AND** introducing that check requires the dedicated timeline-guarantee change

### Requirement: Confidence reflects relationship semantics, not counts

The resolver SHALL map confidence from how ownership was established: `Exact` when ownership rests on a direct positive scenario-level relationship using a named exact predicate — `SharedTraceIdentity` against the owning scenario, or resolver-generated `ScenarioRoot` establishment for roots; `Strong` when ownership is inherited transitively through a uniquely established `ExplicitParentSpan` member chain without any direct scenario-level identity match of its own. The resolver SHALL never emit `Inferred`; externally supplied graphs retain that freedom. Contextual evidence SHALL never yield `Strong`; counts of matching items, evidence dimensions, temporal proximity, connection reuse, and process/thread equality SHALL contribute nothing to confidence.

#### Scenario: Directly bound ownership is Exact

- **WHEN** an egress resolves because `SharedTraceIdentity` binds it to the owning scenario directly, or a root resolves through `ScenarioRoot` establishment
- **THEN** the resolution carries `Exact` confidence

#### Scenario: Inherited ownership is Strong

- **WHEN** a grandchild joins a scenario solely through explicit parent-span chains via already-resolved members with no direct identity match of its own
- **THEN** the resolution carries `Strong` confidence
- **AND** scenario ownership semantics are unchanged from direct resolution

#### Scenario: Weak agreements stay weak

- **WHEN** multiple contextual-only dimensions agree about a candidate
- **THEN** no resolution and no confidence upgrade follows from their count

### Requirement: Transitive inheritance runs as an iterative monotonic fixpoint

Chaining SHALL use iterative rounds only — no recursive traversal. Each round SHALL evaluate operations against a snapshot of the previous round's resolved state, admitting newly resolved operations only in the following round, and SHALL repeat until a round admits nothing new. Resolved membership SHALL never be removed or reassigned during one run. Every productive round SHALL admit at least one previously unresolved operation, bounding productive rounds by the admitted-operation count. After a no-progress round, unsupported operations remain `Uncorrelated`, multi-candidate-supported operations remain `Ambiguous`, and no fallback fires.

#### Scenario: Long chain resolves through iterative rounds

- **WHEN** a causal chain longer than any arbitrary depth heuristic connects many operations through explicit parent-span relations
- **THEN** successive snapshot rounds resolve every chain member
- **AND** no recursion-depth cap truncates the chain

#### Scenario: Reversed input order yields identical output

- **WHEN** the same multi-level chain is presented in forward and reversed operation order
- **THEN** outcomes, memberships, edges, and identifiers are identical
- **AND** intra-round eligibility never depends on presentation position within a round

#### Scenario: Stalled chains stop cleanly

- **WHEN** a round admits nothing new
- **THEN** resolution terminates leaving unsupported operations uncorrelated and multi-supported operations ambiguous

### Requirement: Direct causal-parent selection is a separate stage

For operations already `Resolved` into a scenario, Stage B SHALL evaluate sufficient direct-parent predicates among members of that same scenario only. Exactly one sufficient parent — currently a fired, unshared `ExplicitParentSpan` — SHALL emit one `SelectedCausalEdge` retaining its predicate witness. Multiple equally sufficient parents, insufficient relationships, or contextual-only relations SHALL leave the operation resolved with NO selected edge; unresolved parenthood is represented by edge absence plus retained witnesses, never by a second ambiguity channel or duplicate candidates. Proposed parents outside the owning scenario SHALL be ignored without rewriting ownership. Foundation edge validation runs unchanged: full scoped endpoints, same scenario, acyclic, at most one parent per child, root never a child, ambiguous operations never in edges.

#### Scenario: Same scenario, unresolved direct parent

- **WHEN** Database Z positively belongs to Scenario A while HTTP X and HTTP Y carry equally sufficient direct-parent relations toward Z
- **THEN** Z remains `Resolved(Scenario A)`
- **AND** no selected edge references Z as child
- **AND** no duplicated or self-referential ambiguity candidate is invented

#### Scenario: Unique direct parent emits the edge

- **WHEN** evidence uniquely establishes X → Z inside Scenario A
- **THEN** one selected edge connects X to Z with full scoped references and its predicate witness

#### Scenario: Cross-scenario direct-parent evidence is ignored for edges

- **WHEN** a proposed direct parent belongs to another scenario
- **THEN** the edge is not emitted and neither operation's scenario ownership changes

#### Scenario: Chained egress forms a grandchild edge without changing ownership

- **WHEN** an ingress causes an HTTP egress and that egress causes a database egress through explicit parent-span relations
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

The resolver SHALL copy supplied `InteractionRoleResolution` values into the output graph unchanged, including during root establishment. Correlation outcomes, selected edges, and scenario membership SHALL NOT rewrite an `Unknown` or `Ambiguous` role into a known role. Operations with unknown or ambiguous roles MAY become members through ordinary relational resolution but SHALL never become roots.

#### Scenario: Unknown-role member keeps unknown role

- **WHEN** an operation with role resolution `Unknown` carries relational evidence binding it to Scenario A
- **THEN** it may appear as a Scenario A member with a `Resolved` outcome
- **AND** its role resolution remains `Unknown` with original evidence intact

#### Scenario: Ambiguous-role member is never promoted

- **WHEN** an operation with `Ambiguous { Ingress, Egress }` role resolution joins Scenario A via strong relations
- **THEN** its role candidates and evidence remain inspectable and unchanged
- **AND** it never qualifies as Scenario A's root

#### Scenario: Correlation never fabricates ingress

- **WHEN** a scenario needs a root and only non-root-eligible operations relate to it
- **THEN** the resolver leaves them without a scenario rather than inventing a root

### Requirement: Connection reuse preserves operation independence

Multiple logical operations sharing one physical connection SHALL remain independent canonical operations and independent correlation subjects throughout resolution. Connection, socket, stream, process, and thread identities SHALL remain evidence only and SHALL never merge operations, transfer resolutions, or equal any scenario identity.

#### Scenario: Shared connection does not merge scenarios

- **WHEN** two egress exchanges reuse one database connection while belonging to relation-distinct scenarios
- **THEN** both remain separate members of their respective scenarios
- **AND** shared carrier identity neither merges nor reassigns either

#### Scenario: Carrier identity never equals scenario identity

- **WHEN** any resolution completes over connection-heavy traffic
- **THEN** every scenario identity derives from its root's full scoped reference
- **AND** no connection, socket, stream, PID, TID, or port equals a scenario identity

### Requirement: Cross-epoch scenarios resolve through scoped references

The resolver SHALL support scenarios whose root and children are published in different finalized epoch sessions, resolving every operation through its full `CanonicalOperationRef` with lineage verification. Terminal operations SHALL participate under their completion-owner scope, and identical `OperationId` values under different session scopes SHALL remain distinct resolution subjects with independent scenario derivations.

#### Scenario: Root and child span epochs

- **WHEN** an ingress is owned by epoch N's session and a related egress by epoch N+1's session, with relational evidence linking them
- **THEN** both resolve into one scenario whose identity derives from the root's epoch-N scoped reference
- **AND** the selected edge crosses sessions with full scoped endpoints

#### Scenario: Terminal scope participates

- **WHEN** an operation begins in epoch N but its canonical completion is owned by epoch N+1
- **THEN** the resolver admits it under the completion-owner reference
- **AND** predecessor continuation ranges remain provenance, never a second subject

#### Scenario: Same operation ID in two sessions stays distinct end to end

- **WHEN** equal `OperationId` values exist as roots under Session A and Session B of one recording
- **THEN** the resolver treats them as distinct subjects producing distinct scenario identities
- **AND** resolving one never affects the other

### Requirement: Resolution works identically with and without tracing

Trace context SHALL act as high-quality relational evidence when present, and the resolver SHALL produce complete, valid results from Chronicle-native relational evidence when such evidence is actually supplied — with no tracing SDK installed. This change does not make ordinary captured traffic auto-correlate: production of native correlation evidence is the deferred `derive-native-correlation-evidence` follow-up, which will define stronger native relational semantics including logical task generation, causal execution lineage, process/task inheritance, socket/connection generation relationships, and protocol-stream lineage. Until then, infrastructure equality fields remain contextual unless their relationship semantics are already proven. The absence of trace providers SHALL NOT degrade capture, WAL, ETL, canonicalization, storage, replay, or CLI behavior.

#### Scenario: Trace-assisted resolution

- **WHEN** provider-neutral trace relational evidence uniquely establishes ownership for concurrent traffic
- **THEN** the resolver uses it like any relational evidence
- **AND** no provider SDK type or adapter participates in resolution

#### Scenario: Supplied native relations suffice

- **WHEN** no trace context exists and Chronicle-native relational evidence with proven semantics is actually supplied
- **THEN** scenarios resolve with the same outcome shapes and determinism guarantees as trace-assisted runs
- **AND** missing trace parents are never synthesized from infrastructure identifiers or timestamps

#### Scenario: Unsupplied evidence is not mined from sessions

- **WHEN** an ordinary canonical session arrives with no correlation context
- **THEN** the composition boundary reports missing context per its join rules instead of fabricating lineage evidence
- **AND** resulting operations correctly stay uncorrelated rather than pretending native evidence exists

### Requirement: ETL composes through an explicit CorrelationContext join

`chronicle-etl` MAY compose resolution by joining published `CanonicalSession` values with an explicitly supplied provider-neutral correlation context containing role resolutions and correlation evidence keyed by full operation reference. Canonical sessions alone SHALL NOT constitute complete resolver input, and the helper SHALL NOT synthesize or fabricate roles, evidence, or root establishment — the resolver owns scenario creation and generates `ScenarioRoot` evidence itself. Join rules SHALL be explicit: an admitted session operation with no context role-resolution entry SHALL fail composition as an error; a missing context evidence entry SHALL be treated as empty evidence; a context entry referencing no verified session operation SHALL fail composition as an error. The helper SHALL verify full operation references against session lineage, contain zero ownership-selection semantics, and remain explicitly invoked — the default publication/checkpoint path SHALL NOT call it.

#### Scenario: Composition joins sessions with supplied context

- **WHEN** the helper receives sessions plus a complete correlation context and invokes the resolver
- **THEN** every outcome equals direct resolver invocation over the joined inputs
- **AND** the helper contributes reference verification and ordering normalization only

#### Scenario: Missing role context fails closed

- **WHEN** an admitted session operation lacks a role-resolution context entry
- **THEN** composition returns a typed error identifying the reference
- **AND** it neither defaults the role implicitly nor drops the operation

#### Scenario: Missing evidence entries are valid emptiness

- **WHEN** a context supplies role resolutions but no evidence entry for some referenced operation
- **THEN** that operation joins with empty evidence
- **AND** it typically resolves `Uncorrelated`, which is a normal outcome rather than an error

#### Scenario: Orphan context entries fail closed

- **WHEN** a context entry references an operation absent from all supplied sessions after lineage verification
- **THEN** composition returns a typed error naming the mismatched reference
- **AND** no silent partial admission occurs

#### Scenario: Callers never supply root evidence

- **WHEN** composition builds resolver inputs from context entries containing `ScenarioRoot` items
- **THEN** such caller-supplied root claims are rejected as invalid context content because root establishment belongs exclusively to the resolver
- **AND** scenario creation remains deterministic regardless of caller input

#### Scenario: Publication path stays unchanged

- **WHEN** a recording publishes canonical sessions without requesting correlation
- **THEN** publication, checkpoints, and stored artifacts are byte-identical to pre-change behavior

#### Scenario: Architecture policy untouched

- **WHEN** architecture validation runs after implementation
- **THEN** the crate membership, allowlist edges, semantic boundaries, and external-dependency guards pass without edits beyond declaring the workspace-standard hashing dependency
- **AND** no standalone correlation crate exists and no provider package enters any protected closure

### Requirement: Retention keeps minimal deterministic witnesses within bounded capacity

The resolver SHALL retain evidence sufficient to explain each semantic outcome, not every matching item. For `Resolved` outcomes it SHALL retain a deterministic minimal witness set proving chosen-scenario ownership and confidence — the fired predicate item(s), or the resolver-generated `ScenarioRoot` item for roots — plus the direct-parent predicate witness for any emitted edge, filling remaining capacity with contextual/input evidence in canonical input order up to the documented constant. For `Ambiguous` outcomes it SHALL retain, for EVERY emitted candidate, at least one deterministic candidate-specific witness proving that scenario's support, then fill canonically; retention capacity SHALL apply per candidate so ordinary valid ambiguity can neither silently drop candidates nor turn into errors. For `Uncorrelated` outcomes it SHALL retain contextual/input evidence deterministically up to the cap, since no ownership witness exists. Selected edges SHALL retain their direct-parent predicate witness. Witness ordering SHALL be justification-first in canonical predicate order, then canonical contextual fill; recency SHALL NOT determine priority.

#### Scenario: Cap pressure preserves correctness and witnesses

- **WHEN** more raw evidence exists than the retention constant
- **THEN** the semantic outcome remains correct, deterministic, and explained by retained minimal witnesses
- **AND** overflow affects only optional contextual fill

#### Scenario: Wide ambiguity keeps every candidate explainable

- **WHEN** an operation ends ambiguous across many scenarios and combined raw evidence exceeds the global constant
- **THEN** each candidate still retains at least one candidate-specific witness
- **AND** no candidate is dropped and no boundedness error is raised for ordinary valid ambiguity

#### Scenario: Uncorrelated retention has no fake witness

- **WHEN** an operation resolves as uncorrelated
- **THEN** retained items are contextual/input evidence only
- **AND** no ownership witness is manufactured after the fact

### Requirement: Resolver resource use is bounded and indexed

The resolver SHALL use deterministic ordered indexes for candidate and relation lookup, SHALL avoid full scans proportional to operations-times-ingresses-times-evidence where index hits suffice, SHALL run transitive inheritance as the bounded iterative monotonic fixpoint defined above, SHALL keep internal state proportional to the input set rather than arbitrary external identifier cardinalities, and SHALL document its retention constant. Correctness SHALL never be traded for these bounds; where adversarial key-sharing degrades lookup, behavior remains correct.

#### Scenario: Indexed lookups replace global scans

- **WHEN** resolution processes many operations and concurrent ingress requests
- **THEN** candidate evaluation consults ordered indexes keyed by relation fields and reference
- **AND** no step iterates every operation against every ingress against every evidence item

#### Scenario: State scales with inputs only

- **WHEN** recordings contain arbitrarily many distinct external identifier values unrelated to admitted operations
- **THEN** resolver memory grows with admitted inputs and retained witnesses, not identifier cardinality

### Requirement: Invalid inputs fail closed while insufficient evidence resolves normally

Typed resolver errors — failing closed — SHALL cover: duplicate full operation references; reference lineage or recording-scope mismatch; invalid supplied `InteractionRoleResolution`; malformed `CorrelationEvidence`; correlation-context join violations including caller-supplied `ScenarioRoot` content; and impossible internal graph invariants after construction. Ordinary evidence situations SHALL always yield an outcome rather than an error: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without an edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant because temporal evidence is contextual only.

#### Scenario: Broken inputs are typed errors

- **WHEN** resolver inputs contain duplicate references, mismatched recording scope, invalid roles, malformed evidence, or join violations
- **THEN** the resolver rejects the input set with a typed error naming the violation
- **AND** it neither silently drops the offending data nor guesses substitutes

#### Scenario: Insufficient evidence is a normal outcome

- **WHEN** inputs are valid but carry no relational evidence, several competing relations, or uncertain parenthood
- **THEN** the resolver returns `Uncorrelated`, `Ambiguous`, or `Resolved` without an edge respectively
- **AND** no error is raised for any of these situations

### Requirement: Resolver remains runtime-only and compatibility-safe

The resolver SHALL operate in memory over non-v1 domain values and SHALL NOT persist correlation results, add fields to Capture Event v1 / Canonical Session v1 / WAL v1, introduce `EventId`, or add reader defaults or compatibility fallbacks. Persisted correlation artifacts SHALL require a dedicated follow-up change selecting a separately versioned sidecar/new artifact or an explicit versioned contract with migration policy. The additive `ScenarioRoot` evidence kind SHALL exist only in the non-frozen 0.2 domain enum without altering frozen contracts.

#### Scenario: Frozen contracts unchanged

- **WHEN** the resolver ships
- **THEN** Capture Event v1, Canonical Session v1, WAL v1, public stable JSON, and CLI contracts contain no correlation additions beyond the non-frozen domain enum extension
- **AND** no persisted artifact records resolver output

#### Scenario: Persistence deferred explicitly

- **WHEN** durable correlation artifacts are required later
- **THEN** a dedicated change such as `persist-correlation-scenario-artifacts` chooses the versioned sidecar/new-artifact or migration design
- **AND** this resolver change authorizes neither
