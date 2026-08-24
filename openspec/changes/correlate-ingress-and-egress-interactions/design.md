## Context

The `correlation-domain-model` capability (archived 2026-08-23) shipped Chronicle-owned `InteractionRole`/`InteractionRoleResolution`, scoped `CanonicalOperationRef`, `CorrelationGraph`, `CorrelationResolution::{Resolved,Ambiguous,Uncorrelated}`, tagged `CorrelationEvidence`, selected-edge invariants, and strict validation — but deliberately no algorithm.

Repository facts this design is built on (verified on current main):

- **OperationId is randomly generated at canonicalization time** (`OperationId::new()` in the production HTTP canonicalizer; `uuid_id!` backs IDs with `Uuid::new_v4()`). Stability comes only from persistence: incremental ETL checkpoints preserve generated IDs and cross-epoch continuation preserves one logical operation identity across rollover. Re-canonicalizing raw WAL from scratch generates different IDs.
- **Bare OperationId uniqueness outside its owning session is forbidden by the foundation**: equal `OperationId` values may legally exist under different session/epoch scopes; external references must use the complete `CanonicalOperationRef`. Scenario derivation therefore consumes the full authoritative root scope.
- **The public validator contract is `validate_against_sessions(sessions)`**: `CorrelationGraph::validate()` delegates to `validate_against_sessions(&[])`, so any non-empty graph fails with `MissingSessionContext` under it — existing foundation tests assert exactly this. Resolver requirements demand structural invariants plus successful `validate_against_sessions(...)` with supplied lineage context, never context-free `validate()` success.
- **Foundation validation requires non-empty, non-temporal-only correlation evidence for every `Resolved` outcome** (`CorrelationResolution::validate`). A scenario root therefore needs Chronicle-owned correlation-level evidence establishing its membership — role evidence cannot double as correlation evidence.
- **The foundation separates role classification from scenario correlation**, so role evidence nested inside `InteractionRoleResolution` must never become correlation input.
- **No validator prohibits `span_id == parent_span_id`** inside a single `TraceRelationship`, so a self-parent relation (`A -> A`) is representable in evidence and must be rejected explicitly during Phase B construction rather than left to cycle detection.
- **`EvidenceProvenance { source: String, observation: Option<String> }`** participates in evidence equality — canonical witness ordering must cover provenance or byte-stable output breaks under permutation.
- **`RelativeTimeNanos(u64)` carries no documented recording-global timeline guarantee**, so temporal elimination is unsound in this change.
- **Evidence kinds are operation-local records with no proven causal-execution identity semantics**: `ExecutionTaskLineage.task` is an opaque string whose producer-side meaning is undefined; equality cannot be trusted as causal identity today.
- **`ScenarioId` is UUID-backed** (`chronicle-common`); `sha2` is already a workspace dependency used by six crates; `chronicle-canonical` does not yet declare it.

## Goals and Non-Goals

### Goals

- Deterministically reconstruct scenario ownership from positively supported, candidate-specific relationships — concurrent ingress, cross-epoch membership, chained causality — preserving ambiguity and uncorrelated outcomes regardless of discovery order or chain depth.
- Separate role-evidence and correlation-evidence input channels; separate scenario ownership from direct causal-parent selection; separate support propagation from outcome materialization.
- Construct Phase B parent edges through a fixed normative sequence per scenario — invalid-relation pre-filter, unique-parent mapping, whole-graph SCC analysis, internal-cycle-edge removal — valid by construction, never by validator-rejection triage.
- Define every consumed evidence kind as an explicit candidate-relative predicate; the implementation invents nothing.
- Define a total canonical ordering over equivalent witnesses (semantic fields + provenance + candidate scope) and over transitive proof paths, so retained output is byte-stable under any presentation order.
- Derive scenario identity from the complete scoped root reference with exactly specified bytes and versioning.

### Non-Goals

- Producing native correlation evidence from real recordings or defining native positive predicates (`derive-native-correlation-evidence` owns those contracts).
- Temporal elimination of any kind in this change.
- Persistence of correlation results; any v1 artifact change; `EventId`; provider adapters or plugin infrastructure.
- Weighted/scoring/probabilistic correlation; distributed multi-host correlation.
- Changes to capture, WAL, session reconstruction, protocol pairing, replay, or publication behavior.
- Changing the meaning of `CorrelationGraph::validate()` or weakening lineage validation (an additive structural-validation API may be exposed only if implementation needs it).
- A new parent-ambiguity domain type; arbitrary cycle tie-breakers; temporal reasoning; native task/process predicates.
- Durable logical-operation identity that survives republication scope changes (dedicated future change).

## Decisions

### 1. Resolver placement: `chronicle-canonical`

The resolver is correlation semantics; the foundation invariant assigns those to `chronicle-canonical`. It extends the existing `correlation` module. No standalone crate. ETL composes; it never implements selection rules.

### 2. Inputs, outputs, CorrelationContext, and the two evidence channels

```text
CorrelationInput {
    reference: CanonicalOperationRef,
    operation_view: OperationCorrelationView,
    role: InteractionRoleResolution,            // supplied verbatim; role evidence stays inside
    evidence: Vec<CorrelationEvidence>,        // THE correlation channel for this operation
}

correlation_context = {
    role_resolutions: Map<CanonicalOperationRef, InteractionRoleResolution>,
    evidence:        Map<CanonicalOperationRef, Vec<CorrelationEvidence>>,
}

resolve_correlation(recording_id, inputs) -> Result<CorrelationGraph, CorrelationResolverError>
```

Two distinct evidence channels exist:

- **Role evidence** — items nested inside `InteractionRoleResolution::Known.evidence`, `Unknown.evidence`, and `Ambiguous.candidates[*].evidence` — is consumed only for validating the supplied role classification and preserving role provenance for inspection. It NEVER enters relation indexes, `SharedTraceIdentity`, `ExplicitParentSpan`, Phase A1 support propagation, Phase A2 ownership, or Phase B parent selection.
- **Correlation evidence** — the explicit per-operation collection (`CorrelationInput.evidence`, joined from `CorrelationContext.evidence`) — is the only input indexed and consumed by correlation predicates.

One deliberate exception crosses both channels without merging them: the `ScenarioRoot` input-reservation scan inspects role evidence too, because caller-supplied synthetic root claims are invalid anywhere in caller-controlled input. Scanning for a forbidden value does not make role evidence correlation evidence. Role evidence reaches the output byte-for-byte.

Canonical sessions supply references, lifetime views, and lineage verification. Output passes structural graph invariants and succeeds under `validate_against_sessions(supplied_sessions)`.

### 3. Scenario identity contract: `scenario-id-v1` over the full root reference

Identity = the complete authoritative root scope, because bare `OperationId` uniqueness outside its owning session is not assumed by the foundation:

```text
scenario-id-v1 = UUID-128( SHA-256(
    "chronicle-correlation/scenario-id/v1"   // fixed ASCII domain separator, exactly 36 bytes
 || recording_uuid_be                        // RecordingId,   Uuid::as_bytes(), 16 bytes
 || owner_epoch_uuid_be                      // EpochId,       Uuid::as_bytes(), 16 bytes
 || session_uuid_be                          // SessionId,     Uuid::as_bytes(), 16 bytes
 || operation_uuid_be                        // OperationId,   Uuid::as_bytes(), 16 bytes
))
```

- Field order and encoding are fixed: separator (the exact ASCII bytes above — known-answer tests hash the actual bytes, never a hard-coded length), then the four UUID byte arrays of the root's authoritative `CanonicalOperationRef` in declaration order (`recording_id, owner_epoch_id, session_id, operation_id`), each big-endian via `Uuid::as_bytes()`. Fixed lengths make delimiters unnecessary.
- Digest: first 16 output bytes become the UUID octets; version field 8 per RFC 9562 (`octets[6] = (octets[6] & 0x0F) | 0x80`); RFC 4122 variant (`octets[8] = (octets[8] & 0x3F) | 0x80`).
- Two roots sharing an `OperationId` value under different sessions produce distinct scenario identities because their scoped references differ.
- Stability guarantee (exact): stable across resolver restarts, retries, replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash iteration order.
- Not currently guaranteed (and nowhere claimed): canonical-session regrouping that changes the authoritative root reference; publication into a different owning session/epoch; independent re-canonicalization; regenerated OperationIds. A dedicated durable logical-operation identity/scenario identity change would have to define that.
- Versioning: the domain separator embeds `v1`; any change ships a new separator version; existing derivations never change meaning.

### 4. Scenario-root self-resolution via Chronicle-owned `ScenarioRoot` evidence

Every created scenario needs its root admitted as a member with a valid `Resolved` outcome, and foundation validation demands non-temporal correlation evidence. Role evidence answers "is this ingress or egress"; it must not pretend to answer "which scenario does this root own". Therefore this change adds exactly one additive domain fact:

```text
CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }
```

Chronicle-owned, provider-neutral, non-temporal, meaning: *the resolver deterministically establishes this known-ingress operation as the root of the scenario derived from its full reference*. Construction is normative:

```text
Known(Ingress) operation R
    -> scenario_id = scenario-id-v1(R.reference)
    -> Scenario { id: scenario_id, root: R.reference, members: [R.reference] }
    -> support[R.reference] pinned to { scenario_id } (witness: ScenarioRoot item)
    -> resolutions[R.reference] = Resolved { scenario, confidence: Exact,
                                             evidence: [ScenarioRoot { root: R.reference }] }
```

The variant is additive on a non-frozen 0.2 domain enum; no validator weakening occurs. The resolver generates this evidence itself; callers never fabricate it (Decision 5). Role resolutions — including their nested evidence — are copied verbatim. A scenario containing only its root validates through `validate_against_sessions` immediately.

### 5. ScenarioRoot reservation and root pinning

Two safety rules surround root establishment:

- **Input reservation:** before resolution begins, validation rejects caller-supplied `ScenarioRoot` items in BOTH channels — correlation-context evidence maps AND role evidence nested inside `Known`/`Unknown`/`Ambiguous.candidates[*]` — as typed input errors. `ScenarioRoot` remains a valid graph-domain value in resolver output; it is reserved from caller input so no path can smuggle pre-established scenarios past deterministic root creation. The resolver creates `ScenarioRoot` items only after input validation identifies actual `Known(Ingress)` roots.
- **Root pinning:** once established, a scenario root's Stage A1 support set is pinned to exactly its own `ScenarioId`. Generic ownership predicates add nothing to it; it can never become ambiguous, migrate into another scenario, or be overridden by trace/task/context evidence. Two known-ingress roots sharing one trace stay two independently owned scenarios. Pinning applies only to roots; non-root operations propagate ordinarily.

### 6. Three-phase resolution pipeline

Phase A1 propagates scenario-support sets to a fixed point without finalizing outcomes; Phase A2 materializes outcomes once after closure; Phase B constructs parent edges globally from final ownership through the fixed sequence of Decision 10:

```text
all admitted inputs
   |
role-channel validation + ScenarioRoot reservation scan (both channels)
   |
immutable correlation-channel relation indexes (FULL validated correlation evidence)
   |
[A1] scenario support propagation to fixed point
   |    support[operation] : Set<ScenarioId> — grows monotonically, never shrinks
   |    sparse memberships + bounded per-membership flag; NO witness-path storage
[A2] canonical witness derivation from closed graph + immutable indexes,
   |    then materialize CorrelationResolution values once
   |    {} -> Uncorrelated ; {A} -> Resolved(A) ; {A,B,...} -> Ambiguous(A,B,...)
|    construct final scenario memberships
   |
[B] GLOBAL direct-parent construction per scenario (Decision 10)
   |
deterministic minimal-witness retention (output only)
   |
CorrelationGraph
```

Monotonicity is stated over support: *support sets may only gain scenarios during Phase A1*; final resolutions do not exist during propagation. This prevents the early-resolution hazard where Z binds to Scenario A in round 2 while its longer Scenario B chain completes only in round 3: at closure both paths are present and Z materializes `Ambiguous(A, B)`.

A productive round strictly grows at least one `(operation, ScenarioId)` support membership. Termination is guaranteed: support sets only grow and the scenario count is finite.

Witness state stays bounded: Phase A1 persists ONLY the sparse `(operation, ScenarioId)` memberships plus, per membership, at most a bounded semantic flag such as whether direct (`SharedTraceIdentity`) support exists. It NEVER stores the witness paths themselves — raw relation graphs can contain exponentially many distinct transitive paths (repeated diamonds yield 2^k routes into one scenario) while operations and memberships stay O(N)/O(N×S). Termination depends ONLY on newly added support memberships, never on discovering a "better" path. After closure, the canonical ownership witness for each materialized outcome is DERIVED on demand from the immutable relation indexes plus the final support sets, so a later-discovered shorter proof automatically wins and no stale first-discovery witness can persist.

### 7. Relational evidence predicates (correlation channel only)

Every rule below is normative. Child side = the operation being resolved; candidate side = a scenario root or already-supported member. A relation exists only when both sides carry the referenced items in the CORRELATION channel; missing sides yield no relation, never defaults. Role-channel items are invisible to all of this.

Ownership-relevant predicates:

| Predicate | Match condition (child × candidate) | Meaning | Powers |
| --- | --- | --- | --- |
| `SharedTraceIdentity` | same non-empty `provider` AND same non-empty `trace_id` in correlation-channel `TraceRelationship` items | child shares one trace identity with that scenario — DIRECT scenario-level support | adds scenario to child's support set; never direct parent |
| `ExplicitParentSpan` | child non-empty `parent_span_id` equals candidate's non-empty `span_id` under same `provider`+`trace_id` | declared direct span parenthood | child inherits candidate's previous-round support entries transitively; candidate for Phase B direct parenthood subject to Decision 10 filters |
| `ScenarioRoot` (resolver-generated) | established per created scenario's root | pins the root to its own scenario | resolves the root itself |

Contextual-only kinds — add nothing anywhere, contradict nothing, retained for inspection: `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom`.

Predicate rules:

- Provider namespaces are part of trace identity; identical opaque IDs under different providers never match.
- Missing/non-empty violations degrade to no relation; no synthesized defaults.
- Same `trace_id` alone never implies a selected edge; shared span identity blocks only Phase B sufficiency, never support propagation.
- Task/process/socket/connection/stream equality is contextual because the repository defines no causal-execution identity contract behind those strings today. `derive-native-correlation-evidence` owns defining logical task generation / causal execution lineage / process-task inheritance / socket-generation / protocol-stream contracts and may promote named predicates via specification change. Equal task labels across unrelated requests resolve nothing.
- Roots are exempt from generic predicate effects via pinning; two roots sharing a trace remain two scenarios.
- `Custom` evidence stays contextual unless a spec change names its predicate.

Beyond `ScenarioRoot`, future producers needing new shapes require additive `CorrelationEvidenceKind` adjustments specified separately.

### 8. Materializing outcomes after closure (Phase A2)

Only after A1 terminates:

- empty support → `Uncorrelated { retained evidence }`;
- exactly one scenario → `Resolved { scenario, confidence }`;
- multiple scenarios → `Ambiguous { candidates }`, one candidate per supported scenario, each retaining candidate-specific witnesses matching why that scenario is supported.

Absence of eliminating evidence manufactures nothing; context-only agreement creates no support. Genuinely conflicting support paths simply leave multiple scenarios in the set and materialize as ambiguity; nothing discards either side.

Confidence from final support proofs (not counts, not discovery order):

- `Exact` — the final support entry contains a direct scenario-level relationship for the operation itself (`SharedTraceIdentity`) or the operation is a pinned `ScenarioRoot`;
- `Strong` — the operation has no direct scenario-level relationship and its unique final scenario support exists solely through one or more transitive `ExplicitParentSpan` support paths;
- `Inferred` — reserved for externally supplied graphs; never emitted here;
- multiple supported scenarios → `Ambiguous`; confidence is not materialized.

Ownership witnesses are not stored during propagation; they are derived AFTER closure from the immutable relation graph and final support sets using the deterministic best-path rules of Decision 11 — shortest valid simple path first, lexicographic candidate-reference sequence next, canonical evidence keys per relation last. A shorter valid path discovered late therefore wins over an earlier longer one, equal-length paths break by the lexicographic reference sequence, and chain-discovery order never influences either confidence or the retained witness.

When several proof paths reach the same scenario: direct beats transitive; among equals the canonical path ordering of Decision 11 applies. Chain-discovery order never influences confidence.

### 9. Temporal evidence: contextual only, no elimination

No documented recording-global timeline guarantee exists behind `RelativeTimeNanos`, so this resolver performs no temporal elimination: overlap selects nothing; non-overlap eliminates nothing (asynchronous downstream work may start after its cause completes); incomparable timelines are irrelevant because no comparison influences outcomes; temporal items ride along as retained contextual evidence. Safe contradiction evidence may reject an asserted relationship only when logically incompatible under documented comparable timeline semantics; ordinary lifetime non-overlap is never such a contradiction. A future safe rule requires a dedicated timeline-guarantee change first.

### 10. Phase B: fixed normative construction sequence, global and cycle-safe

Phase B runs exclusively on final Phase A2 ownership — never during propagation — and is deterministic and GLOBAL within each resolved scenario. No incremental "add if still acyclic" logic is allowed anywhere; no validator-driven edge dropping is allowed; foundation validation confirms correctness instead of choosing semantics. For each finally resolved Scenario S, in exactly this order:

```text
1. collect every candidate ExplicitParentSpan relation
   from CORRELATION-channel evidence only
2. reject relations where:
   - parent == child                                    (self-parent)
   - child == S.root                                    (root never becomes a child)
   - parent is not a final member of S                  (membership/scope)
   - child is not a final member of S                   (membership/scope)
   - the target parent span is shared by >1 operation   (shared-span ambiguity)
   - the relation otherwise fails the defined predicate
3. group surviving relations by child
4. per child:
   zero sufficient parents     -> no provisional edge
   exactly one                 -> one provisional edge
   more than one               -> no provisional edge
5. build the COMPLETE provisional directed graph for S
6. compute all directed cyclic SCCs globally
   (strongly connected components containing more than one node;
    self-loops cannot occur because step 2 rejected them)
7. remove every provisional edge whose parent AND child
   are BOTH members of the same cyclic SCC
8. emit the remaining selected edges in canonical order
   (sorted by child reference tuple, then parent reference tuple)
```

Precise SCC removal semantics: every node inside a cyclic SCC loses exactly the provisional parent edge that made it part of the cycle — the edge internal to the component. Edges are removed ONLY when parent and child are both members of the same cyclic SCC. An outgoing provisional edge from a cycle participant to a child OUTSIDE the component MAY survive when otherwise valid; incoming acyclic structure from outside is untouched; membership never changes. Example that genuinely reaches the SCC phase (all children have unique sufficient parents):

```text
D -> A      B -> C      C -> B      C -> E      E -> F

SCC = { B, C }
removed:  B -> C, C -> B          (internal to the cyclic component)
retained: D -> A, C -> E, E -> F  (acyclic; C stays parent of outside child E)
```

Cycle participants keep their `Resolved` ownership; dropped edges' direct-parent evidence becomes inspectable non-selected context — never a second ambiguity channel. Self-parent relations were already rejected at step 2 as ordinary uncertain/invalid-parent situations (no error, no ownership impact). Root-as-child relations likewise never reach mapping, and root establishment evidence (`ScenarioRoot`) is never mutated or discarded to enforce the invariant. Foundation edge validation runs unchanged and confirms what construction already guarantees.

### 11. Full-evidence semantics, total canonical witness ordering, confidence-consistent retention

**Semantics/representation boundary:** all predicate evaluation, indexes, support propagation, ambiguity detection, and parent selection operate on the complete validated CORRELATION-channel evidence set. Retention/truncation happens only after semantic resolution is complete, as output representation. Caps can never alter outcomes.

**Total canonical evidence key.** Any two semantically equivalent witness items are ordered by a key covering the ENTIRE value, so distinct serialized values never tie:

```text
canonical_evidence_key(item, candidate_ref?) =
    kind_discriminator            // canonical variant name, lexicographic
 || every semantic field of the kind,
    in declaration order,
    strings compared by UTF-8 bytes,
    Option values ordered None < Some(value)
 || candidate CanonicalOperationRef tuple when the witness is
    candidate-relative           // (recording, epoch, session, operation)
 || provenance.source            // UTF-8 bytes
 || provenance.observation       // None < Some, UTF-8 bytes
```

For `TraceRelationship` concretely: discriminator → provider → trace_id → span_id (None<Some) → parent_span_id (None<Some) → candidate ref → provenance.source → provenance.observation. Two witnesses differing in ANY field — including `parent_span_id` or either provenance value — order deterministically; insertion/presentation order is irrelevant.

**Transitive proof-path ordering and derivation.** For a `Strong` (or fallback ambiguous) transitive `ExplicitParentSpan` proof, candidate paths order by:

1. shortest valid proof path (fewest relations);
2. lexicographic sequence of candidate `CanonicalOperationRef` tuples along the path;
3. canonical evidence keys of each relation along the path.

References are unique, so the ordering is total. The implementation MUST DERIVE the canonical best path — via deterministic shortest-path/dynamic-programming search over the closed support/relation graph — rather than enumerate or store every possible path: raw relation graphs may be cyclic and exponentially path-rich, so search state stays proportional to the visited frontier (visited-set-pruned simple paths), never to the number of distinct paths. A proof path is a FINITE SIMPLE path with no repeated operation reference; cycles in raw relations cannot produce unbounded derivation because revisiting any reference is pruned before extension.

**Confidence-consistent selection (unchanged semantics):** root `Exact` keeps its `ScenarioRoot`; non-root `Exact` keeps a canonical DIRECT `SharedTraceIdentity` witness — a canonical transitive proof must NEVER replace an available direct witness merely by ordering; `Strong` keeps the canonical transitive path; `Ambiguous` candidates prefer a direct candidate-specific witness when one exists, else the canonical transitive proof. The total ordering operates ONLY within the required semantic class.

**Ownership vs parent witnesses stay separate:** `CorrelationResolution.evidence` carries the ownership witness; `SelectedCausalEdge.evidence` carries the direct-parent witness; extra parent detail inside a resolution is optional contextual enrichment. Which-scenario and which-direct-parent remain separable through output provenance as well as algorithm phases.

Contextual fill is CANONICAL, not caller-ordered: remaining capacity fills from the operation's unused correlation-channel items sorted by the total canonical evidence key (non-candidate-relative items take an empty/None candidate scope in the key), truncated to the documented constant (64 items per slot, applied per ambiguity candidate). Caller evidence presentation order therefore cannot change retained output; the resolver does not preserve correlation-evidence `Vec` order. Role evidence is the deliberate exception — preserved byte-for-byte exactly as supplied, including item order, because role-resolution preservation is a separate foundation invariant. Recency/discovery-order never determines priority anywhere.

### 12. Boundedness: sparse support sets

- All indexes deterministic and ordered (`BTreeMap`) keyed by `(provider, trace_id)`, `(provider, trace_id, span_id)`, and full reference; inputs sorted by full reference tuple first; built once from full validated correlation evidence.
- Support stored sparsely: entries created only by discovered positive relationships; no eager operation×scenario matrix.
- Monotonic unit: a productive round adds ≥1 previously absent `(operation, ScenarioId)` membership; formal bound N×S; typical usage far lower.
- Rounds terminate when no membership is added; no recursion; correctness never traded. Phase B work is linear-time graph analysis per scenario (relation filtering, unique-parent mapping, SCC computation), bounded by scenario size.
- Internal state is bounded by: validated input evidence/indexes; sparse discovered `(operation, ScenarioId)` support memberships; at most a bounded per-membership semantic flag (e.g. direct-support boolean); Phase B provisional graph state; and temporary proof-search state proportional to the visited frontier. The resolver SHALL NOT retain all distinct transitive proof paths — the N×S membership bound does NOT extend to arbitrary path enumeration, which can be exponential in diamond-rich relation graphs.

### 13. ETL composition boundary

A `chronicle-etl` helper joins published `CanonicalSession` values with a caller-supplied `CorrelationContext`: verifies full references against session lineage, enriches lifetime/provenance views, joins context by exact reference. Join rules: admitted session operation missing a role-resolution entry → composition error; missing evidence entry → empty evidence; context entry referencing no verified session operation → composition error; duplicate/mismatched scoped references fail from verification; `ScenarioRoot` appearing anywhere in supplied context — evidence map OR nested role evidence — → typed error. Zero selection semantics; explicitly invoked; default publication/checkpoint path never calls it.

### 14. Failure taxonomy

Typed `CorrelationResolverError` (fail closed): duplicate full operation references; reference lineage/recording-scope mismatch; invalid supplied roles; malformed evidence; caller-supplied `ScenarioRoot` in ANY caller-controlled container in EITHER channel; correlation-context join violations; impossible internal graph invariants. Ordinary situations — never errors: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain, insufficient, self-parent, or cyclic direct-parent relations → `Resolved` without an edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant.

### 15. Validator contract and compatibility boundaries

Resolver output SHALL satisfy all correlation graph structural/domain invariants and SHALL succeed under `validate_against_sessions(...)` when the required canonical-session lineage context is supplied. Context-free `validate()` intentionally fails non-empty graphs today (`MissingSessionContext`); this change neither requires nor claims otherwise and does not weaken lineage validation. An additive public structural-validation API MAY be exposed if implementation needs it, without changing `validate()` semantics.

In-memory runtime capability only; frozen v1 contracts untouched; nothing persists. The one modified-capability surface is the additive `ScenarioRoot` variant in `correlation-domain-model` (see its delta spec). Roadmap:

```text
introduce-correlation-domain-model (done)
        ↓
correlate-ingress-and-egress-interactions      ← this change (adds correlation-resolver;
                                                 extends correlation-domain-model with ScenarioRoot)
        ↓
derive-native-correlation-evidence             ← owns native relational-semantics contracts (logical task generation,
                                                 causal execution lineage, process/task inheritance,
                                                 socket/connection generations, protocol-stream lineage)
                                                 and promotion of safe native predicates via spec change
        ↓
persist-correlation-scenario-artifacts
        ↓
integrate-pluggable-trace-evidence-providers
        ↓
trace-provider-plugin-installation
        ↓
scenario inspect / replay / test generation
```

Until native evidence lands, this revision defines no Chronicle-native positive ownership predicate beyond resolver-generated `ScenarioRoot`: non-root operations without supported trace relationships normally remain `Uncorrelated`. That is a capability boundary of THIS revision, not a limitation of provider neutrality — the resolver consumes Chronicle-owned evidence, trace providers remain outer adapters, future native producers add named predicates through separate specs, and no SDK dependency exists anywhere in the resolver.

## Risks and Trade-offs

- Conservative gating (contextual task lineage, no temporal elimination, no native predicates yet) leaves weak-but-real signals unresolved until producers emit relational evidence. Intended.
- Cycle removal forfeits derivable structure inside cyclic components while preserving outgoing acyclic edges; guessing a break point would be worse.
- Full-reference identity means regrouping operations across publications changes scenario identities. Honest: the foundation provides no stronger invariant to build on.
- Snapshot-round closure may need more rounds than eager admission; bounded by N×S memberships and immune to ordering effects.
- Per-candidate retention capacity slightly raises worst-case memory for wide ambiguity; explainability of every candidate outweighs it.

## Future Changes

- `derive-native-correlation-evidence`: Chronicle-native evidence derivation and lineage-semantics contracts; promotion of safe native predicates.
- Durable logical-operation identity / scenario identity surviving publication scope changes.
- Additive public structural-validation API on `CorrelationGraph`, only if implementation needs it.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` / `trace-provider-plugin-installation`.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Scenario inspect/UX, replay, test generation/assertions.
