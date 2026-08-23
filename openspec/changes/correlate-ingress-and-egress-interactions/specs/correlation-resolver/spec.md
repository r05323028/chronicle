## Purpose

Define Chronicle's deterministic, provider-neutral, ambiguity-safe resolver that selects scenario ownership and selected causal-parent structure for canonical operations from Chronicle-owned role resolutions and candidate-relative `CorrelationEvidence`, producing valid `CorrelationGraph` output. The resolver computes what the `correlation-domain-model` capability validates; foundation validation semantics remain authoritative and unchanged.

## ADDED Requirements

### Requirement: Resolver is deterministic and provider-neutral

Chronicle SHALL provide a three-phase resolver that consumes recording-scoped canonical operation references, supplied `InteractionRoleResolution` values, and Chronicle-owned `CorrelationEvidence`, and SHALL produce a populated `CorrelationGraph` satisfying all correlation graph structural/domain invariants. Given identical inputs, results SHALL be identical — including derived `ScenarioId`s, representation ordering, retained witnesses, and selected edges — across process restarts, ETL retries, ETL replay over the same persisted canonical artifacts, worker scheduling, input iteration order, map/hash iteration order, causal-chain depth, support-discovery order, and evidence presentation order. The resolver SHALL NOT depend on or expose OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, Jaeger, Zipkin, or other tracing SDK types.

#### Scenario: Restart and retry stability

- **WHEN** the resolver runs twice over identical canonical inputs in separate invocations simulating restart, ETL retry, and replay over the same persisted artifacts
- **THEN** both outputs are identical including every derived `ScenarioId` and retained witness selection
- **AND** no output depends on wall-clock time, randomness, or processing counters

#### Scenario: Input order independence

- **WHEN** the same canonical inputs are presented in different operation orders, session orders, chain orders, or evidence orders, including orders that would differ under hash-map traversal
- **THEN** final support sets, correlation outcomes, memberships, edges, identifiers, and retained witnesses are identical

#### Scenario: No provider SDK enters resolution

- **WHEN** trace-shaped evidence exists only as Chronicle-owned opaque provider-labelled `CorrelationEvidence`
- **THEN** the resolver resolves it like any other relationship evidence
- **AND** no provider SDK type appears in the resolver API, and removing all provider adapters changes nothing except evidence availability

### Requirement: Role evidence and correlation evidence are distinct input channels

Evidence nested inside `InteractionRoleResolution::Known.evidence`, `Unknown.evidence`, and `Ambiguous.candidates[*].evidence` SHALL be used only for validating the supplied role classification, preserving role provenance, and inspecting role uncertainty. Role-channel evidence SHALL NOT participate in `SharedTraceIdentity`, `ExplicitParentSpan`, relation indexes, Phase A1 support propagation, Phase A2 scenario ownership, or Phase B direct-parent selection. Only the explicit per-operation correlation-evidence collection (`CorrelationInput.evidence`, joined from `CorrelationContext.evidence[CanonicalOperationRef]`) SHALL be indexed and consumed by correlation predicates. The one exception is the `ScenarioRoot` input-reservation scan, which SHALL inspect BOTH channels because caller-supplied synthetic root claims are invalid anywhere in caller-controlled input; scanning for that forbidden value does not convert role evidence into correlation evidence. Supplied role evidence SHALL reach the output byte-for-byte.

#### Scenario: Trace relationship only in role evidence does not correlate

- **WHEN** Ingress A holds trace T in its correlation evidence while egress X carries trace T ONLY inside its role resolution's evidence, with X's correlation-evidence collection empty
- **THEN** X gains no support from trace T
- **AND** no predicate fires across the channel boundary

#### Scenario: The same relationship in correlation evidence does correlate

- **WHEN** X's trace-T relationship instead appears in X's explicit correlation-evidence collection
- **THEN** normal predicate semantics apply and Scenario A may enter X's support set

#### Scenario: Role evidence survives resolution unchanged

- **WHEN** the resolver completes any run
- **THEN** every supplied role resolution, including all nested evidence items and their order, appears byte-for-byte in the output graph
- **AND** correlation processing never mutates, reorders, or consumes role-channel content

### Requirement: Scenario identity derives from the complete authoritative root reference

The resolver SHALL derive each `ScenarioId` as `scenario-id-v1`: the first 16 bytes of `SHA-256(fixed_domain_separator || recording_uuid_be || owner_epoch_uuid_be || session_uuid_be || operation_uuid_be)` rendered as a UUID with version field 8 (`octets[6] = (octets[6] & 0x0F) | 0x80`) and RFC 4122 variant (`octets[8] = (octets[8] & 0x3F) | 0x80`). The fixed domain separator SHALL be the exact ASCII byte sequence of `chronicle-correlation/scenario-id/v1` (36 bytes); field order SHALL be separator, then the four `Uuid::as_bytes()` big-endian arrays of the root operation's authoritative `CanonicalOperationRef` in declaration order — recording, owner epoch, session, operation. Bare `OperationId` SHALL NOT be used outside its owning scope. Known-answer tests SHALL hash the actual separator bytes rather than any hard-coded length. Scenario identity SHALL be stable across resolver restarts, retries, replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash iteration order. Scenario identity SHALL NOT currently be guaranteed across canonical-session regrouping that changes the authoritative root reference, publication into a different owning session/epoch, independent re-canonicalization, or regenerated `OperationId`s; such durability requires a dedicated future logical-identity change.

#### Scenario: Deterministic derivation under repetition and permutation

- **WHEN** resolution repeats across simulated restarts and permuted input orders over the same persisted canonical artifacts
- **THEN** every scenario keeps exactly the same derived identifier
- **AND** the identifier equals the value produced by the specified algorithm for its full root reference

#### Scenario: Duplicate OperationId across sessions yields distinct scenarios

- **WHEN** Recording R owns Session A with a root whose `OperationId` equals X and Session B with a distinct root whose `OperationId` also equals X
- **THEN** the two roots remain distinct `CanonicalOperationRef`s with distinct derived scenario identities because owner-epoch/session scope participates in the derivation
- **AND** neither scenario absorbs or collides with the other

#### Scenario: Identity guarantee boundary is honest

- **WHEN** canonical operations are regrouped into different owning sessions or independently re-canonicalized so authoritative references change or `OperationId`s regenerate
- **THEN** scenario identifiers may legitimately differ and this is conforming behavior
- **AND** no requirement claims stability across changed publication scope, which stays deferred to a dedicated durable logical-identity change

### Requirement: Scenario roots self-resolve through Chronicle-owned ScenarioRoot evidence and stay pinned

For every admitted operation whose role resolution is `Known(Ingress)`, the resolver SHALL derive the scenario identifier from that operation's complete authoritative reference, create `Scenario { id, root, members: [root] }`, and record the root's own correlation resolution as `Resolved { scenario: id, confidence: Exact, evidence: [ScenarioRoot { root }] }`. This change SHALL add one additive Chronicle-owned evidence kind, `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`, meaning the resolver deterministically establishes this known-ingress operation as the root of the scenario derived from its full reference; it SHALL be non-temporal so existing foundation validation accepts the root's resolution unchanged. Root establishment is correlation-level evidence distinct from role classification. During Phase A1 support propagation, every established root SHALL have its support set pinned to exactly its own `ScenarioId`: generic ownership predicates SHALL NOT add further scenarios to a root, roots SHALL never become ambiguous or migrate into another scenario, and trace/task/context evidence SHALL NOT override root establishment. Pinning applies only to scenario roots; non-root operations propagate ordinarily. Role resolutions — including nested evidence — SHALL remain byte-for-byte verbatim throughout.

#### Scenario: Root resolves to its own scenario exactly once

- **WHEN** one `Known(Ingress)` operation A is admitted
- **THEN** Scenario A is created with A as root and sole initial member
- **AND** A carries exactly one resolution: `Resolved(Scenario A, Exact)` witnessed by its `ScenarioRoot` evidence

#### Scenario: Root-only scenario validates immediately

- **WHEN** a recording contains one `Known(Ingress)` operation and no other operations resolve into its scenario
- **THEN** the resulting graph contains a valid scenario whose membership is exactly its root
- **AND** `validate_against_sessions(supplied_sessions)` accepts the graph without weakening any lineage check

#### Scenario: Roots sharing one trace remain independently owned

- **WHEN** Ingress A and Ingress B both carry trace relationships with equal provider and trace id in their correlation evidence
- **THEN** A stays pinned to Scenario A and B stays pinned to Scenario B
- **AND** neither root joins, supports, or becomes ambiguous across the other's scenario

#### Scenario: Shared span identity between roots does not merge them

- **WHEN** two known-ingress roots expose span items under one shared provider+trace+span identity
- **THEN** each root remains resolved to its own scenario
- **AND** shared-span effects apply only to direct-parent selection, never to root pinning

#### Scenario: Contextual overlap cannot pull a root across scenarios

- **WHEN** a root overlaps another scenario temporally or shares process, connection, or task context with it
- **THEN** the root's support set remains pinned to its own scenario

### Requirement: Caller-controlled input rejects ScenarioRoot everywhere in both channels

Before resolution begins, the resolver SHALL validate that no caller-supplied `ScenarioRoot` item appears anywhere in caller-controlled input: correlation-context evidence maps, `InteractionRoleResolution::Known.evidence`, `Unknown.evidence`, and `Ambiguous.candidates[*].evidence`. Each occurrence SHALL fail as a typed input error. `ScenarioRoot` remains a valid graph-domain value exclusively in resolver output; only the resolver may create it, after input validation identifies actual `Known(Ingress)` roots.

#### Scenario: Direct context evidence injection fails closed

- **WHEN** a context evidence entry contains a `ScenarioRoot` item
- **THEN** resolution fails with a typed input error naming the offending reference

#### Scenario: Role-evidence smuggling fails closed

- **WHEN** `ScenarioRoot` appears inside `Known.evidence`, `Unknown.evidence`, or any `Ambiguous.candidates[*].evidence`
- **THEN** resolution fails with a typed input error in every case
- **AND** no path exists to pre-establish scenarios through caller-supplied data in either channel

### Requirement: Candidates come only from known ingress roots

Phase A1 SHALL initialize support exactly once per admitted `Known(Ingress)` operation via pinned root establishment. Operations with `Known(Egress)`, `Unknown`, or `Ambiguous` role resolutions SHALL never own or root a scenario. Scenarios SHALL exist even when nothing else resolves into them.

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

### Requirement: Correlation predicates act only on named candidate-relative relationships in the correlation channel

The resolver SHALL interpret evidence exclusively through defined relational predicates comparing child-side items against candidate-side items within the CORRELATION evidence channel; it SHALL NOT infer semantics from field names alone. A predicate fires only when both sides carry the referenced items; missing sides yield no relation, never defaults. Ownership-relevant predicates are:

- `SharedTraceIdentity` — child `TraceRelationship` and candidate `TraceRelationship` share equal non-empty `provider` AND equal non-empty `trace_id`: adds that specific scenario to the child's support set as DIRECT scenario-level support; never direct-parent evidence; provider namespaces are part of identity, so identical opaque values under different providers never match.
- `ExplicitParentSpan` — child non-empty `parent_span_id` equals a candidate's non-empty `span_id` under the same `provider`+`trace_id`: declared direct span parenthood; the child inherits the candidate's previous-round support entries transitively; sufficient for Phase B direct parenthood unless the target span is shared by multiple operations.

Resolver-generated `ScenarioRoot` evidence initializes and pins each root per the root-establishment requirement. All remaining kinds — `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom` — are contextual only: they add nothing to any support set, contradict nothing, and are retained for inspection. Task and process lineage equality is contextual because no repository contract defines those strings as unique causal-execution identity; `derive-native-correlation-evidence` owns defining such contracts and may promote named predicates through specification changes. Shared span identity blocks only direct-parent sufficiency, never support propagation. Promoting any additional predicate requires a specification change naming its comparison rule.

All predicate evaluation, relation indexes, support propagation, ambiguity detection, and parent selection SHALL operate on the complete validated CORRELATION-channel evidence set; retention truncation SHALL occur only after semantic resolution is complete as output representation, so evidence volume beyond the retention cap can never alter outcomes.

#### Scenario: Same trace supports several members of one scenario without duplication

- **WHEN** DB Z shares one provider+trace identity with Ingress A, HTTP X, and HTTP Y — all of Scenario A
- **THEN** Z's support set contains Scenario A exactly once
- **AND** the three matches aggregate into one scenario-keyed support entry whose internal witnesses record all matching paths

#### Scenario: Explicit parent span selects the direct parent after closure

- **WHEN** egress Z carries `parent_span_id` equal to HTTP X's unique `span_id` under the same provider and trace id, and Z's final ownership is Resolved(X's scenario)
- **THEN** Phase B emits the X → Z selected edge within that scenario

#### Scenario: Shared span ambiguity blocks only the edge

- **WHEN** two operations expose the same `provider`+`trace_id`+`span_id` and a child's `parent_span_id` targets that span
- **THEN** support propagation through those members still works normally
- **AND** no selected direct-parent edge is produced toward the shared span

#### Scenario: Same task string alone resolves nothing

- **WHEN** two concurrent known-ingress roots and one egress all carry identical `ExecutionTaskLineage.task` labels in their correlation evidence with no trace relation between them
- **THEN** no support entry exists for the egress
- **AND** the outcome is `Uncorrelated` until some ownership predicate fires

#### Scenario: Cross-provider identical IDs never match

- **WHEN** a child carries `provider: "otel", trace_id: "T"` and a candidate carries `provider: "xray", trace_id: "T"`
- **THEN** no predicate fires between them

#### Scenario: Missing sides degrade to no relation

- **WHEN** a predicate's required item is absent on either side, such as empty `span_id` values
- **THEN** the predicate does not fire and no default relation is synthesized

#### Scenario: Context-only agreement manufactures nothing

- **WHEN** an egress shares process/thread generation, connection environment, protocol family, temporal overlap, and task label with several ingresses but holds no trace relational items in its correlation channel
- **THEN** no scenario enters the egress's support set from those items
- **AND** they remain retained contextual evidence on whatever outcome is emitted

### Requirement: Scenario support propagates monotonically to closure before outcomes materialize

The resolver SHALL split ownership determination into Phase A1 and Phase A2. Phase A1 SHALL maintain `support[operation] : Set<ScenarioId>`, initialized with each root pinned to its own scenario and each non-root operation holding scenarios discoverable through direct positive relationships. Each round SHALL evaluate against a snapshot of the previous round's support state and union newly discoverable scenarios from direct `SharedTraceIdentity`, transitive `ExplicitParentSpan` inheritance through candidates whose support was present in the snapshot, and any future spec-authorized predicates. Support sets SHALL grow monotonically within a run — scenarios may enter, never leave — and SHALL be stored sparsely, creating entries only for discovered positive relationships. A productive round strictly grows at least one `(operation, ScenarioId)` membership; rounds terminate when none grows. Phase A2 SHALL run only after termination: empty support → `Uncorrelated`; exactly one scenario → `Resolved`; multiple → `Ambiguous { candidates }` with one candidate per supported scenario, keyed by `ScenarioId`, each retaining deterministic support witnesses. Final `Resolved`/`Ambiguous` values SHALL NOT be assigned during propagation.

#### Scenario: Short path and long path converge to ambiguity

- **WHEN** Scenario A's support reaches operation Z through a shorter chain in an earlier round while Scenario B's support reaches Z through a longer chain discovered in a later round
- **THEN** at closure Z's support set contains both Scenario A and Scenario B
- **AND** the materialized outcome is `Ambiguous(A, B)`, never an early `Resolved(A)` frozen before B became discoverable

#### Scenario: Reversed chain depths do not change results

- **WHEN** the same inputs are presented with reversed operation order, reversed session order, reversed evidence order, or altered chain presentation
- **THEN** final support sets, resolutions, identifiers, witnesses, and edges are identical
- **AND** intra-round eligibility never depends on presentation position

#### Scenario: No evidence does not manufacture ambiguity

- **WHEN** Ingress A and Ingress B exist and egress X closes with empty support
- **THEN** X materializes as `Uncorrelated`
- **AND** it does not become ambiguous merely because neither was eliminated

#### Scenario: Candidate-specific ambiguity only

- **WHEN** egress X's closed support set is exactly {A, B} from positive relational paths, while unrelated Ingress C contributed nothing
- **THEN** the outcome is `Ambiguous(A, B)`
- **AND** C never appears and each emitted candidate retains its own explaining witnesses

#### Scenario: One ingress owns multiple egress interactions

- **WHEN** HTTP egress X and PostgreSQL egress Y close with support {A} through shared-trace relations and nothing else supports them
- **THEN** both materialize `Resolved(Scenario A)` and Scenario A contains Ingress A as root with both members

#### Scenario: Concurrent ingresses resolve independently

- **WHEN** Ingress A and Ingress B overlap in time while relational paths give egress X support {A} and egress Y support {B}
- **THEN** the materialized graph holds Scenario A {A, X} and Scenario B {B, Y}

#### Scenario: Strong causal relation overrides timing intuition

- **WHEN** a decisive trace path gives X support {A} while Ingress B is temporally closer
- **THEN** X materializes Resolved(Scenario A)
- **AND** temporal proximity contributes nowhere

#### Scenario: Stalled chains stop cleanly

- **WHEN** a round adds no support membership
- **THEN** propagation terminates leaving empty-support operations uncorrelated and multi-supported operations ambiguous

### Requirement: Temporal evidence is contextual only

Because current canonical offset semantics provide no documented recording-global timeline guarantee, the resolver SHALL perform no temporal elimination of any kind in this change. Lifetime overlap SHALL add no support; lifetime non-overlap SHALL remove nothing and block nothing — including a candidate whose lifetime ended before the child's began, since asynchronous downstream work may start after its cause completes. Safe contradiction evidence may reject an asserted relationship only when logically incompatible under documented, trustworthy, comparable timeline semantics; ordinary lifetime non-overlap is not such a contradiction. Temporal relationships SHALL never override positive relational evidence. Any future temporal contradiction rule requires a dedicated change establishing timeline guarantees first.

#### Scenario: Async child after ingress completion

- **WHEN** Ingress A completed before egress X began, and trace relational evidence puts A's scenario in X's support set
- **THEN** X may materialize Resolved(Scenario A)
- **AND** lifetime non-overlap eliminates nothing and downgrades nothing

#### Scenario: Decisive linkage despite disjoint lifetimes

- **WHEN** an explicit parent-span path binds a child to a candidate whose lifetime is entirely earlier
- **THEN** the path retains full force
- **AND** no rule compares lifetimes for elimination

#### Scenario: Temporal-only evidence stays unresolved

- **WHEN** an egress overlaps one or more ingress lifetimes and holds no relational predicate
- **THEN** its support set stays empty on that basis
- **AND** the outcome is uncorrelated with temporal evidence retained

#### Scenario: Reverse causal ordering is not evaluated in this change

- **WHEN** a candidate's lifetime begins strictly after the child relationship could have begun on any observable offsets
- **THEN** this revision still performs no elimination because comparable-timeline guarantees do not yet exist
- **AND** introducing that check requires the dedicated timeline-guarantee change

### Requirement: Confidence reflects final support proofs, not counts or discovery order

After Phase A2, confidence SHALL derive from how each single supported scenario entered the closed support set: `Exact` when the final support entry contains a direct scenario-level relationship for the operation itself (`SharedTraceIdentity`) or the operation is a pinned `ScenarioRoot`; `Strong` when the operation has no direct scenario-level relationship and its unique final scenario support exists solely through one or more transitive `ExplicitParentSpan` support paths. When several proof paths reach the same scenario, direct support yields `Exact` over transitive `Strong`. When support reaches multiple different scenarios the result is `Ambiguous` and confidence is not materialized. The resolver SHALL never emit `Inferred`; externally supplied graphs retain that freedom. Counts of matching items, evidence dimensions, temporal proximity, connection reuse, process/thread equality, and chain-discovery order SHALL contribute nothing to confidence.

#### Scenario: Directly bound ownership is Exact

- **WHEN** an egress closes with support {A} via `SharedTraceIdentity`, or a root closes through `ScenarioRoot` pinning
- **THEN** the materialized resolution carries `Exact` confidence

#### Scenario: Purely inherited ownership is Strong

- **WHEN** a grandchild's sole support path into its unique scenario runs through transitive explicit parent-span relations with no direct identity match of its own
- **THEN** the materialized resolution carries `Strong` confidence

#### Scenario: Discovery order never picks confidence

- **WHEN** the same operation has one direct path and one transitive path into the same scenario, presented in either discovery order
- **THEN** the resolution is `Exact` in both runs because direct beats transitive by canonical rule

#### Scenario: Ambiguity skips confidence

- **WHEN** closed support contains multiple scenarios
- **THEN** the outcome is `Ambiguous` without any confidence value

### Requirement: Phase B constructs parent edges globally with deterministic cycle safety

Phase B SHALL run exclusively on the final Phase A2 outcome, per resolved scenario, and SHALL be deterministic and GLOBAL: first compute ALL sufficient direct-parent relations among that scenario's final members (fired unshared `ExplicitParentSpan`, endpoints inside the scenario); then derive the provisional unique-parent mapping (children with zero sufficient relations get none; children with more than one equally sufficient relation get none); then evaluate the COMPLETE provisional directed graph; then globally identify directed cycles (strongly connected components of more than one node); then remove every provisional edge inside each cyclic component; finally emit the remaining edge set, which SHALL satisfy foundation invariants by construction rather than by post-hoc validator-rejection triage. There SHALL be no incremental add-if-acyclic traversal whose result depends on processing order. When otherwise-sufficient unique relations form cycles, the resolver SHALL preserve all scenario ownership, guess nothing, use no input-order/timestamp/lexical/span/operation tie-breaks, emit NO selected edge for cycle participants whose parent choice cannot stand without violating graph invariants, keep acyclic edges outside cyclic components whenever the remaining graph stays valid, and retain the relevant direct-parent evidence for inspection. Operations materializing `Ambiguous` or `Uncorrelated` SHALL receive no edges, and proposed parents outside the owning scenario SHALL be ignored without rewriting ownership. Foundation validation requirements — full scoped endpoints, same scenario, acyclic, at most one parent per child, root never a child — hold unchanged.

#### Scenario: Two-node direct-parent cycle loses only its unsafe edges

- **WHEN** X and Y in one scenario carry sufficient unique relations X→Y and Y→X
- **THEN** X and Y remain Resolved members of their scenario
- **AND** neither cyclic selected edge is emitted
- **AND** the direct-parent evidence remains retained for inspection

#### Scenario: Three-node direct-parent cycle behaves identically

- **WHEN** sufficient unique relations form X→Y, Y→Z, Z→X
- **THEN** ownership of all three remains Resolved
- **AND** no cyclic selected edge is emitted among them

#### Scenario: Acyclic surrounding structure survives a cycle

- **WHEN** a scenario contains D→A, A→B, and the cyclic pair B⇄C
- **THEN** the cyclic component's internal edges are removed
- **AND** the acyclic edges outside the cycle (D→A, A→B) remain because the resulting graph stays a valid forest
- **AND** B and C end with no selected parent while keeping their membership

#### Scenario: Cycle handling is order-independent

- **WHEN** the operations and evidence of a cyclic case are presented in permuted orders
- **THEN** the final selected-edge set is identical
- **AND** which edges were dropped never depends on traversal sequence

#### Scenario: Stage B waits for final ownership

- **WHEN** a child temporarily held support {A} during propagation and later gained B, closing as `Ambiguous(A, B)`
- **THEN** no selected edge referencing that child exists from the intermediate A-only state

#### Scenario: Same scenario, unresolved direct parent

- **WHEN** Database Z materializes Resolved(Scenario A) while HTTP X and HTTP Y carry equally sufficient direct-parent relations toward Z
- **THEN** Z remains Resolved(Scenario A) with no selected edge referencing Z as child

#### Scenario: Unique direct parent emits the edge

- **WHEN** evidence uniquely establishes X → Z inside final Scenario A membership
- **THEN** one selected edge connects X to Z with full scoped references and its predicate witness

#### Scenario: Cross-scenario direct-parent evidence is ignored for edges

- **WHEN** a proposed direct parent belongs to another scenario
- **THEN** the edge is not emitted and neither operation's scenario ownership changes

#### Scenario: Tree invariant violations are impossible by construction

- **WHEN** the resolver completes any input set
- **THEN** the emitted graph contains no self-edge, cycle, multi-parent child, or child root
- **AND** structural graph validation accepts the edge set unchanged

### Requirement: Retention happens after semantics, keeps minimal deterministic witnesses consistent with materialized confidence, and separates ownership from parent witnesses

Retention SHALL occur only after Phase A2 and Phase B complete, as output representation over the closed support structure. Retained ownership witnesses SHALL be CONSISTENT with the materialized confidence, chosen semantically rather than by one global priority list:

- Root `Exact` → the `ScenarioRoot` item;
- Non-root `Exact` → a canonical DIRECT `SharedTraceIdentity` witness proving Exact ownership — a transitive span proof SHALL never substitute merely because it sorts earlier;
- `Strong` → a canonical transitive `ExplicitParentSpan` support proof demonstrating the inherited path;
- `Ambiguous` candidates → each candidate prefers a direct candidate-specific ownership witness when one exists, otherwise a canonical transitive proof; no candidate may retain evidence inconsistent with why it is supported.

When multiple equivalent proofs exist within the required class, selection SHALL use the canonical ordering key — provider, then trace id, then span id where applicable, then candidate full-reference tuple — making discovery order and input iteration order irrelevant and byte-stable output well-defined. Ownership witnesses live in `CorrelationResolution.evidence`; direct-parent witnesses live on `SelectedCausalEdge.evidence`; any extra parent detail inside a resolution is optional contextual enrichment, never the primary ownership proof. Remaining capacity fills with contextual/input evidence in canonical input order up to the documented constant (64 items), applied per ambiguity candidate so ordinary valid ambiguity neither drops candidates nor turns into errors. `Uncorrelated` outcomes retain contextual/input evidence canonically with no manufactured ownership witness. Recency SHALL NEVER determine priority.

#### Scenario: Exact resolution keeps an Exact witness

- **WHEN** an operation's closed support includes BOTH a direct `SharedTraceIdentity` relation and a transitive `ExplicitParentSpan` path into the same scenario, materializing `Exact`
- **THEN** the retained ownership witness demonstrates the direct trace-identity relation
- **AND** the transitive path is never substituted as the primary ownership proof merely by ordering

#### Scenario: Strong resolution keeps a transitive witness

- **WHEN** an operation's sole support into its unique scenario runs through transitive parent-span paths, materializing `Strong`
- **THEN** the retained ownership witness demonstrates the transitive support path
- **AND** no direct-style witness is fabricated

#### Scenario: Cap pressure preserves correctness and witnesses

- **WHEN** more raw evidence exists than the retention constant while relational support stays fixed
- **THEN** support sets, materialized outcomes, and selected edges remain identical to an uncapped run
- **AND** only optional retained contextual fill differs according to the deterministic cap policy

#### Scenario: Wide ambiguity keeps every candidate explainable

- **WHEN** an operation ends ambiguous across many scenarios and combined raw evidence exceeds the global constant
- **THEN** each candidate retains at least one confidence-consistent candidate-specific witness
- **AND** no candidate drops and no boundedness error arises for ordinary valid ambiguity

#### Scenario: Uncorrelated retention has no fake witness

- **WHEN** an operation materializes uncorrelated
- **THEN** retained items are contextual/input evidence only
- **AND** no ownership witness is manufactured after the fact

### Requirement: Role resolution is preserved verbatim

The resolver SHALL copy supplied `InteractionRoleResolution` values — including all nested evidence — into the output graph byte-for-byte, including during root establishment and support propagation. Correlation outcomes, selected edges, and scenario membership SHALL NOT rewrite an `Unknown` or `Ambiguous` role into a known role. Operations with unknown or ambiguous roles MAY become members through ordinary relational resolution but SHALL never become roots.

#### Scenario: Unknown-role member keeps unknown role

- **WHEN** an operation with role resolution `Unknown` carries correlation-channel relational evidence putting Scenario A in its support set
- **THEN** it may materialize as a Scenario A member with a `Resolved` outcome
- **AND** its role resolution remains `Unknown` with original evidence intact

#### Scenario: Ambiguous-role member is never promoted

- **WHEN** an operation with `Ambiguous { Ingress, Egress }` role resolution joins Scenario A via strong relations
- **THEN** its role candidates and evidence remain inspectable and unchanged
- **AND** it never qualifies as Scenario A's root

#### Scenario: Correlation never fabricates ingress

- **WHEN** a scenario needs a root and only non-root-eligible operations relate to it
- **THEN** the resolver leaves them without a scenario rather than inventing a root

### Requirement: Connection reuse preserves operation independence

Multiple logical operations sharing one physical connection SHALL remain independent canonical operations and independent correlation subjects throughout resolution. Connection, socket, stream, process, and thread identities SHALL remain evidence only and SHALL never merge operations, transfer support, or equal any scenario identity.

#### Scenario: Shared connection does not merge scenarios

- **WHEN** two egress exchanges reuse one database connection while belonging to relation-distinct scenarios
- **THEN** both remain separate members of their respective scenarios

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

### Requirement: Provider-neutral architecture with an honest native-evidence capability boundary

The resolver architecture SHALL remain provider-neutral: it consumes Chronicle-owned evidence, trace providers remain outer adapters translating into `CorrelationEvidence`, future native producers may add named predicates through separate specifications, and no SDK dependency exists anywhere in the resolver or default distribution. THIS revision defines no Chronicle-native positive ownership predicate beyond resolver-generated `ScenarioRoot`; until `derive-native-correlation-evidence` defines and supplies such predicates — logical task generation identity, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage, and promotion of safe native predicates — non-root operations without supported trace relationships normally materialize `Uncorrelated`. This boundary is a capability statement about the current revision, not a limitation of provider neutrality. Trace context acts as high-quality relational evidence when present; the absence of trace providers SHALL NOT degrade capture, WAL, ETL, canonicalization, storage, replay, or CLI behavior.

#### Scenario: Trace-assisted resolution

- **WHEN** provider-neutral trace relational evidence uniquely populates support sets for concurrent traffic
- **THEN** the resolver uses it like any relational evidence
- **AND** no provider SDK type or adapter participates in resolution

#### Scenario: Contextual-only native evidence yields uncorrelated outcomes

- **WHEN** a non-root operation carries only task, process, socket, connection, stream, timing, or custom items — all contextual in this revision — with no trace-shaped relational evidence
- **THEN** its support set stays empty and the outcome is `Uncorrelated`
- **AND** no synthetic native resolution is invented from equality of infrastructure fields

#### Scenario: Future native predicates arrive through specification changes

- **WHEN** a later change defines a proven Chronicle-native positive predicate
- **THEN** it enters through a named spec-defined comparison rule and this resolver's architecture accepts it without SDK dependencies or structural change
- **AND** no equality-based native correlation is pre-authorized by this revision

#### Scenario: Unsupplied evidence is not mined from sessions

- **WHEN** an ordinary canonical session arrives with no correlation context
- **THEN** the composition boundary reports missing context per its join rules instead of fabricating lineage evidence
- **AND** resulting operations correctly stay uncorrelated rather than pretending native evidence exists

### Requirement: ETL composes through an explicit CorrelationContext join

`chronicle-etl` MAY compose resolution by joining published `CanonicalSession` values with an explicitly supplied provider-neutral correlation context containing role resolutions and correlation evidence keyed by full operation reference. Canonical sessions alone SHALL NOT constitute complete resolver input, and the helper SHALL NOT synthesize roles, evidence, or root establishment — including rejecting caller-supplied `ScenarioRoot` items wherever they appear in supplied context or nested role evidence. Join rules SHALL be explicit: an admitted session operation with no context role-resolution entry SHALL fail composition as an error; a missing context evidence entry SHALL be treated as empty evidence; a context entry referencing no verified session operation SHALL fail composition as an error. The helper SHALL verify full operation references against session lineage, contain zero ownership-selection semantics, and remain explicitly invoked — the default publication/checkpoint path SHALL NOT call it.

#### Scenario: Composition joins sessions with supplied context

- **WHEN** the helper receives sessions plus a complete correlation context and invokes the resolver
- **THEN** every outcome equals direct resolver invocation over the joined inputs
- **AND** the helper contributes reference verification and ordering normalization only

#### Scenario: Missing role context fails closed

- **WHEN** an admitted session operation lacks a role-resolution context entry
- **THEN** composition returns a typed error identifying the reference

#### Scenario: Missing evidence entries are valid emptiness

- **WHEN** a context supplies role resolutions but no evidence entry for some referenced operation
- **THEN** that operation joins with empty evidence
- **AND** it typically resolves `Uncorrelated`, which is a normal outcome rather than an error

#### Scenario: Orphan context entries fail closed

- **WHEN** a context entry references an operation absent from all supplied sessions after lineage verification
- **THEN** composition returns a typed error naming the mismatched reference

#### Scenario: Publication path stays unchanged

- **WHEN** a recording publishes canonical sessions without requesting correlation
- **THEN** publication, checkpoints, and stored artifacts are byte-identical to pre-change behavior

#### Scenario: Architecture policy untouched

- **WHEN** architecture validation runs after implementation
- **THEN** the crate membership, allowlist edges, semantic boundaries, and external-dependency guards pass without edits beyond declaring the workspace-standard hashing dependency
- **AND** no standalone correlation crate exists and no provider package enters any protected closure

### Requirement: Resolver resource use is bounded by sparse support growth

The resolver SHALL use deterministic ordered indexes built once from full validated correlation evidence, SHALL store scenario-support sets sparsely — creating `(operation, ScenarioId)` entries only for discovered positive relationships and never eagerly allocating an operation-times-scenario matrix — and SHALL terminate Phase A1 when no support membership is added. A productive round SHALL add at least one previously absent support membership, giving a formal upper bound of N operations times S scenarios memberships. Phase B cycle analysis SHALL be bounded linear graph work per scenario (strongly connected components plus edge removal). There SHALL be no recursion. Internal state SHALL stay proportional to the input set plus discovered support rather than arbitrary external identifier cardinalities. Correctness SHALL never be traded for these bounds.

#### Scenario: Indexed lookups replace global scans

- **WHEN** resolution processes many operations and concurrent ingress requests
- **THEN** predicate evaluation consults ordered indexes keyed by relation fields and reference
- **AND** no step iterates every operation against every ingress against every evidence item

#### Scenario: Support storage stays sparse

- **WHEN** recordings contain many scenarios but most operations hold few or no relationships
- **THEN** allocated support entries track discovered positives only
- **AND** no full operation-by-scenario structure is materialized

#### Scenario: Termination is guaranteed by monotonic growth

- **WHEN** Phase A1 runs on any input set
- **THEN** each productive round adds at least one new `(operation, ScenarioId)` membership within the N-times-S bound
- **AND** propagation terminates deterministically

### Requirement: Invalid inputs fail closed while insufficient evidence resolves normally

Typed resolver errors — failing closed — SHALL cover: duplicate full operation references; reference lineage or recording-scope mismatch; invalid supplied `InteractionRoleResolution`; malformed `CorrelationEvidence`; caller-supplied `ScenarioRoot` in ANY caller-controlled container in EITHER channel; correlation-context join violations; and impossible internal graph invariants after construction. Ordinary evidence situations SHALL always yield an outcome rather than an error: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without an edge; cyclic parent relations → edges omitted with ownership preserved; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant because temporal evidence is contextual only.

#### Scenario: Broken inputs are typed errors

- **WHEN** resolver inputs contain duplicate references, mismatched recording scope, invalid roles, malformed evidence, smuggled root claims in either channel, or join violations
- **THEN** the resolver rejects the input set with a typed error naming the violation
- **AND** it neither silently drops the offending data nor guesses substitutes

#### Scenario: Insufficient evidence is a normal outcome

- **WHEN** inputs are valid but carry no relational evidence, several competing relations, uncertain parenthood, or cyclic parent relations
- **THEN** the resolver returns `Uncorrelated`, `Ambiguous`, `Resolved` without an edge, or the cycle-safe edge set respectively
- **AND** no error is raised for any of these situations

### Requirement: Output validation uses explicit session lineage context

Resolver output SHALL satisfy all correlation graph structural/domain invariants and SHALL succeed under `validate_against_sessions(...)` when the required canonical-session lineage context is supplied. This change SHALL NOT require context-free `validate()` success for non-empty graphs, because the existing foundation defines `validate()` as `validate_against_sessions(&[])` and intentionally fails non-empty graphs with missing-session errors; that contract remains unchanged and lineage validation SHALL NOT be weakened. Implementation MAY expose an additive public structural-validation API equivalent to the existing private structural check if needed, without altering `validate()` semantics.

#### Scenario: Full validation succeeds with supplied sessions

- **WHEN** a non-empty resolver-produced graph is validated against the canonical sessions that own its referenced operations
- **THEN** `validate_against_sessions(valid_sessions)` succeeds

#### Scenario: Context-free validate semantics remain unchanged

- **WHEN** `validate()` is called directly on a non-empty graph under the current foundation API
- **THEN** it fails with missing-session context exactly as the foundation defines today
- **AND** this behavior is expected, tested, and not treated as a resolver defect

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
