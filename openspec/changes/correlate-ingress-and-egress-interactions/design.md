## Context

The `correlation-domain-model` capability (archived 2026-08-23) shipped Chronicle-owned `InteractionRole`/`InteractionRoleResolution`, scoped `CanonicalOperationRef`, `CorrelationGraph`, `CorrelationResolution::{Resolved,Ambiguous,Uncorrelated}`, tagged `CorrelationEvidence`, selected-edge invariants, and strict validation — but deliberately no algorithm.

Repository facts this design is built on (verified on current main):

- **OperationId is randomly generated at canonicalization time** (`OperationId::new()` in the production HTTP canonicalizer; `uuid_id!` backs IDs with `Uuid::new_v4()`). Stability comes only from persistence: incremental ETL checkpoints preserve generated IDs and cross-epoch continuation preserves one logical operation identity across rollover. Re-canonicalizing raw WAL from scratch generates different IDs.
- **Bare OperationId uniqueness outside its owning session is forbidden by the foundation**: equal `OperationId` values may legally exist under different session/epoch scopes; external references must use the complete `CanonicalOperationRef`. Scenario derivation therefore consumes the full authoritative root scope.
- **The public validator contract is `validate_against_sessions(sessions)`**: `CorrelationGraph::validate()` delegates to `validate_against_sessions(&[])`, so any non-empty graph fails with `MissingSessionContext` under it — existing foundation tests assert exactly this. Resolver requirements demand structural invariants plus successful `validate_against_sessions(...)` with supplied lineage context, never context-free `validate()` success.
- **Foundation validation requires non-empty, non-temporal-only correlation evidence for every `Resolved` outcome** (`CorrelationResolution::validate`). A scenario root therefore needs Chronicle-owned correlation-level evidence establishing its membership — role evidence cannot double as correlation evidence.
- **The foundation separates role classification from scenario correlation**, so role evidence nested inside `InteractionRoleResolution` must never become correlation input.
- **`RelativeTimeNanos(u64)` carries no documented recording-global timeline guarantee**, so temporal elimination is unsound in this change.
- **Evidence kinds are operation-local records with no proven causal-execution identity semantics**: `ExecutionTaskLineage.task` is an opaque string whose producer-side meaning is undefined; equality cannot be trusted as causal identity today.
- **`ScenarioId` is UUID-backed** (`chronicle-common`); `sha2` is already a workspace dependency used by six crates; `chronicle-canonical` does not yet declare it.

## Goals and Non-Goals

### Goals

- Deterministically reconstruct scenario ownership from positively supported, candidate-specific relationships — concurrent ingress, cross-epoch membership, chained causality — preserving ambiguity and uncorrelated outcomes regardless of discovery order or chain depth.
- Separate role-evidence and correlation-evidence input channels; separate scenario ownership from direct causal-parent selection; separate support propagation from outcome materialization.
- Construct Phase B parent edges globally per scenario with deterministic cycle-safe removal — valid graphs by construction, never validator-rejection triage.
- Define every consumed evidence kind as an explicit candidate-relative predicate; the implementation invents nothing.
- Derive scenario identity from the complete scoped root reference with exactly specified bytes and versioning.
- Retain witnesses consistent with materialized confidence, keeping ownership witnesses and direct-parent witnesses distinct.

### Non-Goals

- Producing native correlation evidence from real recordings or defining native positive predicates (`derive-native-correlation-evidence` owns those contracts).
- Temporal elimination of any kind in this change.
- Persistence of correlation results; any v1 artifact change; `EventId`; provider adapters or plugin infrastructure.
- Weighted/scoring/probabilistic correlation; distributed multi-host correlation.
- Changes to capture, WAL, session reconstruction, protocol pairing, replay, or publication behavior.
- Changing the meaning of `CorrelationGraph::validate()` or weakening lineage validation (an additive structural-validation API may be exposed only if implementation needs it).
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

Phase A1 propagates scenario-support sets to a fixed point without finalizing outcomes; Phase A2 materializes outcomes once after closure; Phase B constructs parent edges globally from final ownership:

```text
all admitted inputs
   |
role-channel validation + ScenarioRoot reservation scan (both channels)
   |
immutable correlation-channel relation indexes (FULL validated correlation evidence)
   |
[A1] scenario support propagation to fixed point
   |    support[operation] : Set<ScenarioId> — grows monotonically, never shrinks
   |    snapshot rounds; internal support witnesses retained per (operation, scenario)
   |
[A2] materialize CorrelationResolution values once
   |    {} -> Uncorrelated ; {A} -> Resolved(A) ; {A,B,...} -> Ambiguous(A,B,...)
|    construct final scenario memberships
   |
[B] GLOBAL direct-parent construction per scenario (see Decision 10)
   |
deterministic minimal-witness retention (output only)
   |
CorrelationGraph
```

Monotonicity is stated over support: *support sets may only gain scenarios during Phase A1*; final resolutions do not exist during propagation. This prevents the early-resolution hazard where Z binds to Scenario A in round 2 while its longer Scenario B chain completes only in round 3: at closure both paths are present and Z materializes `Ambiguous(A, B)`.

A productive round strictly grows at least one `(operation, ScenarioId)` support membership. Termination is guaranteed: support sets only grow and the scenario count is finite.

Support witnesses: for each `(operation, ScenarioId)` entry, A1 retains enough internal witness information to explain why that scenario entered the set — a direct predicate item, or a transitive path (child-to-member `ExplicitParentSpan` plus member support). Multiple members of one scenario aggregate into ONE scenario-keyed support entry carrying all witness paths.

### 7. Relational evidence predicates (correlation channel only)

Every rule below is normative. Child side = the operation being resolved; candidate side = a scenario root or already-supported member. A relation exists only when both sides carry the referenced items in the CORRELATION channel; missing sides yield no relation, never defaults. Role-channel items are invisible to all of this.

Ownership-relevant predicates:

| Predicate | Match condition (child × candidate) | Meaning | Powers |
| --- | --- | --- | --- |
| `SharedTraceIdentity` | same non-empty `provider` AND same non-empty `trace_id` in correlation-channel `TraceRelationship` items | child shares one trace identity with that scenario — DIRECT scenario-level support | adds scenario to child's support set; never direct parent |
| `ExplicitParentSpan` | child non-empty `parent_span_id` equals candidate's non-empty `span_id` under same `provider`+`trace_id` | declared direct span parenthood | child inherits candidate's previous-round support entries transitively; sufficient for Phase B direct parenthood unless span shared |
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

When several proof paths reach the same scenario: direct beats transitive; among equals a canonical ordering applies (Decision 11). Chain-discovery order never influences confidence.

### 9. Temporal evidence: contextual only, no elimination

No documented recording-global timeline guarantee exists behind `RelativeTimeNanos`, so this resolver performs no temporal elimination: overlap selects nothing; non-overlap eliminates nothing (asynchronous downstream work may start after its cause completes); incomparable timelines are irrelevant because no comparison influences outcomes; temporal items ride along as retained contextual evidence. Safe contradiction evidence may reject an asserted relationship only when logically incompatible under documented comparable timeline semantics; ordinary lifetime non-overlap is never such a contradiction. A future safe rule requires a dedicated timeline-guarantee change first.

### 10. Phase B: global, cycle-safe direct-parent construction after closure

Phase B runs exclusively on final Phase A2 ownership — never during propagation. It is deterministic and GLOBAL within each scenario; there is no incremental "add if still acyclic" loop whose result depends on traversal order:

```text
final resolved membership of Scenario S
   ↓
compute ALL sufficient direct-parent relations among S's members
   (ExplicitParentSpan fired; target span unshared; endpoints inside S)
   ↓
derive provisional unique-parent mapping
   (a child with zero sufficient relations gets none;
    a child with more than one equally sufficient relation gets none)
   ↓
build the complete provisional directed graph over S
   ↓
globally identify directed cycles (strongly connected components with >1 node)
   ↓
remove every provisional edge INSIDE a cyclic component
   (cycle participants end with NO selected parent;
    acyclic edges touching a cycle component only from outside survive)
   ↓
emit the final selected-edge set — valid by construction
```

Cycle semantics: when otherwise-sufficient unique relations form a directed cycle, Chronicle preserves all scenario ownership, guesses nothing, breaks nothing by input order/timestamps/lexical/span/operation order, emits NO selected edge for cycle participants whose parent choice cannot stand without violating graph invariants, and retains the relevant direct-parent evidence for inspection. The safe default removes every edge within the cyclic strongly-connected component; operations outside the component keep their edges when the remaining graph stays a forest (acyclic, ≤1 parent, roots never children) — e.g. in `D → A → B ⇄ C`, the `B ⇄ C` edges vanish while `D → A → B` survives. Removed-edge children keep their `Resolved` ownership and their parent evidence becomes inspectable context, never a second ambiguity channel.

Foundation edge validation runs unchanged; the emitted set is valid by construction rather than by post-hoc dropping on validator failure.

### 11. Full-evidence semantics, confidence-consistent witness retention

**Semantics/representation boundary:** all predicate evaluation, indexes, support propagation, ambiguity detection, and parent selection operate on the complete validated CORRELATION-channel evidence set. Retention/truncation happens only after semantic resolution is complete, as output representation. Caps can never alter outcomes.

**Witnesses must explain the materialized confidence** — selection is semantic, not one global priority list:

- Root `Exact` → retain the `ScenarioRoot` item.
- Non-root `Exact` → retain a canonical DIRECT `SharedTraceIdentity` witness proving Exact ownership; a transitive span proof must never be substituted merely because it sorts earlier.
- `Strong` → retain a canonical transitive `ExplicitParentSpan` support proof.
- `Ambiguous` candidate → prefer a direct candidate-specific ownership witness when one exists, else a canonical transitive proof; every candidate's retained witness matches why it is supported.

Canonical ordering keys (byte-stable): within the required class, order by provider, then trace id, then span id where applicable, then candidate full-reference tuple (`recording, epoch, session, operation`); ties impossible because references are unique. Discovery order and input iteration order never matter.

**Ownership vs parent witnesses stay separate:** `CorrelationResolution.evidence` carries the ownership witness; `SelectedCausalEdge.evidence` carries the direct-parent witness. Any additional parent detail inside a resolution is optional contextual enrichment, never the primary ownership proof. Which-scenario and which-direct-parent remain separable through output provenance as well as algorithm phases.

Global constant documented (64 items per slot, applied per ambiguity candidate). Justification-first fill, then canonical contextual fill; recency/discovery-order never determines priority.

### 12. Boundedness: sparse support sets

- All indexes deterministic and ordered (`BTreeMap`) keyed by `(provider, trace_id)`, `(provider, trace_id, span_id)`, and full reference; inputs sorted by full reference tuple first; built once from full validated correlation evidence.
- Support stored sparsely: entries created only by discovered positive relationships; no eager operation×scenario matrix.
- Monotonic unit: a productive round adds ≥1 previously absent `(operation, ScenarioId)` membership; formal bound N×S; typical usage far lower.
- Rounds terminate when no membership is added; no recursion; correctness never traded. Phase B cycle analysis is linear-time graph work per scenario (SCC computation), bounded by scenario size.

### 13. ETL composition boundary

A `chronicle-etl` helper joins published `CanonicalSession` values with a caller-supplied `CorrelationContext`: verifies full references against session lineage, enriches lifetime/provenance views, joins context by exact reference. Join rules: admitted session operation missing a role-resolution entry → composition error; missing evidence entry → empty evidence; context entry referencing no verified session operation → composition error; duplicate/mismatched scoped references fail from verification; `ScenarioRoot` appearing anywhere in supplied context — evidence map OR nested role evidence — → typed error. Zero selection semantics; explicitly invoked; default publication/checkpoint path never calls it.

### 14. Failure taxonomy

Typed `CorrelationResolverError` (fail closed): duplicate full operation references; reference lineage/recording-scope mismatch; invalid supplied roles; malformed evidence; caller-supplied `ScenarioRoot` in ANY caller-controlled container in EITHER channel; correlation-context join violations; impossible internal graph invariants. Ordinary situations — never errors: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant.

### 15. Validator contract and compatibility boundaries

Resolver output SHALL satisfy all correlation graph structural/domain invariants and SHALL succeed under `validate_against_sessions(...)` when the required canonical-session lineage context is supplied. Context-free `validate()` intentionally fails non-empty graphs today (`MissingSessionContext`); this change neither requires nor claims otherwise and does not weaken lineage validation. An additive public structural-validation API MAY be exposed if implementation needs it, without changing `validate()` semantics.

In-memory runtime capability only; frozen v1 contracts untouched; nothing persists. Roadmap:

```text
introduce-correlation-domain-model (done)
        ↓
correlate-ingress-and-egress-interactions      ← this change
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

Until native evidence lands, this revision defines no Chronicle-native positive ownership predicate beyond resolver-generated `ScenarioRoot`: non-root operations without supported trace relationships normally remain `Uncorrelated`. That is a capability boundary of THIS revision, not a limitation of provider neutrality — the resolver consumes Chronicle-owned evidence, trace providers remain outer adapters, future native producers add named predicates through separate specs, and no SDK dependency exists anywhere in the resolver. Until native evidence lands, ordinary recordings legitimately produce mostly `Uncorrelated` results.

## Risks and Trade-offs

- Conservative gating (contextual task lineage, no temporal elimination, no native predicates yet) leaves weak-but-real signals unresolved until producers emit relational evidence. Intended.
- Cycle removal forfeits some derivable structure inside cyclic components; guessing a break point would be worse.
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
