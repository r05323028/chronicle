## Context

The `correlation-domain-model` capability (archived 2026-08-23) shipped Chronicle-owned `InteractionRole`/`InteractionRoleResolution`, scoped `CanonicalOperationRef`, `CorrelationGraph`, `CorrelationResolution::{Resolved,Ambiguous,Uncorrelated}`, tagged `CorrelationEvidence`, selected-edge invariants, and strict validation — but deliberately no algorithm.

Repository facts this design is built on (verified on current main):

- **OperationId is randomly generated at canonicalization time** (`OperationId::new()` in the production HTTP canonicalizer; `uuid_id!` backs IDs with `Uuid::new_v4()`). Stability comes only from persistence: incremental ETL checkpoints preserve generated IDs and cross-epoch continuation preserves one logical operation identity across rollover. Re-canonicalizing raw WAL from scratch generates different IDs.
- **Bare OperationId uniqueness outside its owning session is forbidden by the foundation**: equal `OperationId` values may legally exist under different session/epoch scopes; external references must use the complete `CanonicalOperationRef`. Scenario derivation must therefore consume the full authoritative root scope.
- **Foundation validation requires non-empty, non-temporal-only correlation evidence for every `Resolved` outcome** (`CorrelationResolution::validate`). A scenario root therefore needs Chronicle-owned correlation-level evidence establishing its membership — role evidence cannot double as correlation evidence.
- **`RelativeTimeNanos(u64)` carries no documented recording-global timeline guarantee**, so temporal elimination is unsound in this change.
- **Evidence kinds are operation-local records with no proven causal-execution identity semantics**: `ExecutionTaskLineage.task` is an opaque string whose producer-side meaning is undefined; equality cannot be trusted as causal identity today.
- **`ScenarioId` is UUID-backed** (`chronicle-common`); `sha2` is already a workspace dependency used by six crates; `chronicle-canonical` does not yet declare it.

## Goals and Non-Goals

### Goals

- Deterministically reconstruct scenario ownership from positively supported, candidate-specific relationships — concurrent ingress, cross-epoch membership, chained causality — preserving ambiguity and uncorrelated outcomes.
- Separate scenario ownership from direct causal-parent selection.
- Define every consumed evidence kind as an explicit candidate-relative predicate; the implementation invents nothing.
- Derive scenario identity from the complete scoped root reference with exactly specified bytes and versioning.
- Establish scenario roots explicitly without touching role state and without weakening foundation validation.
- Keep resolution an iterative monotonic process with deterministic minimal-witness retention.

### Non-Goals

- Producing native correlation evidence from real recordings (`derive-native-correlation-evidence` owns task/process lineage contracts).
- Temporal elimination of any kind in this change.
- Persistence of correlation results; any v1 artifact change; `EventId`; provider adapters or plugin infrastructure.
- Weighted/scoring/probabilistic correlation; distributed multi-host correlation.
- Changes to capture, WAL, session reconstruction, protocol pairing, replay, or publication behavior.
- Durable logical-operation identity that survives republication scope changes (dedicated future change).

## Decisions

### 1. Resolver placement: `chronicle-canonical`

The resolver is correlation semantics; the foundation invariant assigns those to `chronicle-canonical`. It extends the existing `correlation` module. No standalone crate. ETL composes; it never implements selection rules.

### 2. Inputs, outputs, and CorrelationContext

```text
CorrelationInput {
    reference: CanonicalOperationRef,
    operation_view: OperationCorrelationView,
    role: InteractionRoleResolution,            // supplied verbatim
    evidence: Vec<CorrelationEvidence>,         // operation-local items
}

correlation_context = {
    role_resolutions: Map<CanonicalOperationRef, InteractionRoleResolution>,
    evidence:        Map<CanonicalOperationRef, Vec<CorrelationEvidence>>,
}

resolve_correlation(recording_id, inputs) -> Result<CorrelationGraph, CorrelationResolverError>
```

Canonical sessions supply references, lifetime views, and lineage verification; roles and evidence arrive exclusively through supplied context. Root-establishment evidence is generated inside the resolver — callers never place synthetic `ScenarioRoot` items into context. Output passes existing foundation validation unchanged.

### 3. Scenario identity contract: `scenario-id-v1` over the full root reference

Identity = the complete authoritative root scope, because bare `OperationId` uniqueness outside its owning session is not assumed by the foundation:

```text
scenario-id-v1 = UUID-128( SHA-256(
    "chronicle-correlation/scenario-id/v1"   // fixed ASCII domain separator, 37 bytes
 || recording_uuid_be                        // RecordingId,   Uuid::as_bytes(), 16 bytes
 || owner_epoch_uuid_be                      // EpochId,       Uuid::as_bytes(), 16 bytes
 || session_uuid_be                          // SessionId,     Uuid::as_bytes(), 16 bytes
 || operation_uuid_be                        // OperationId,   Uuid::as_bytes(), 16 bytes
))
```

- Field order and encoding are fixed: separator, then the four UUID byte arrays of the root's authoritative `CanonicalOperationRef` in declaration order (`recording_id, owner_epoch_id, session_id, operation_id`), each big-endian via `Uuid::as_bytes()`. Fixed lengths make delimiters unnecessary.
- Digest: first 16 output bytes become the UUID octets; version field 8 per RFC 9562 (`octets[6] = (octets[6] & 0x0F) | 0x80`); RFC 4122 variant (`octets[8] = (octets[8] & 0x3F) | 0x80`).
- Two roots sharing an `OperationId` value under different sessions produce distinct scenario identities because their scoped references differ — directly compatible with the foundation's duplicate-ID invariant.
- Stability guarantee (exact): stable across resolver restarts, retries, replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash iteration order.
- Not currently guaranteed (and nowhere claimed): canonical-session regrouping that changes the authoritative root reference; publication into a different owning session/epoch; independent re-canonicalization; regenerated OperationIds. If Chronicle later needs publication-scope-independent scenario identity, a dedicated durable logical-operation identity/scenario identity change must define it.
- Versioning: the domain separator embeds `v1`; any change to fields, order, encoding, hash, truncation, or bit-fixups ships a new separator version. Existing derivations never change meaning.

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
    -> resolutions[R.reference] = Resolved { scenario, confidence: Exact,
                                             evidence: [ScenarioRoot { root: R.reference }] }
```

The variant is additive on a non-frozen 0.2 domain enum; no validator weakening occurs. The resolver generates this evidence itself; `CorrelationContext` callers never fabricate it. Role resolutions are copied verbatim — inspecting or changing root membership never mutates `InteractionRoleResolution`. A scenario containing only its root validates immediately.

### 5. Relational evidence predicates

Every rule below is normative. Child side = the operation being resolved; candidate side = a scenario root or already-resolved member. A relation exists only when both sides carry the referenced items; missing sides yield no relation, never defaults.

Positive ownership predicates (the only items that can create supported candidates):

| Predicate | Match condition (child × candidate) | Meaning | Powers |
| --- | --- | --- | --- |
| `SharedTraceIdentity` | same non-empty `provider` AND same non-empty `trace_id` in `TraceRelationship` items | child shares one trace identity with that scenario — scenario-level relationship support | supports ownership (candidate keyed by ScenarioId); never direct parent |
| `ExplicitParentSpan` | child `parent_span_id` equals candidate's non-empty `span_id` under same `provider`+`trace_id` | declared direct span parenthood | supports transitive membership inheritance; sufficient for Stage B direct parenthood unless span shared |
| `ScenarioRoot` (resolver-generated) | established for every created scenario's root | root owns the scenario it creates | resolves the root itself |

Contextual-only kinds — support nothing, contradict nothing, retained for inspection: `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom`.

Predicate rules:

- Provider namespaces are part of trace identity; identical opaque IDs under different providers never match.
- Missing/non-empty violations degrade to no relation; no synthesized defaults.
- Same `trace_id` alone never implies a selected edge; shared span identity (`provider`+`trace_id`+`span_id` carried by multiple operations) blocks only Stage B sufficiency, never Stage A ownership.
- Task/process/socket/connection/stream equality is contextual because the repository defines no causal-execution identity contract behind those strings today. `derive-native-correlation-evidence` owns defining logical task generation / causal lineage semantics; promoting any such predicate requires a specification change naming its comparison rule. Equal task labels across unrelated requests must resolve nothing.
- `Custom` evidence stays contextual unless a spec change names its predicate.

If future producers cannot express these relations with current kinds, the smallest additive adjustment extends `CorrelationEvidenceKind`; beyond `ScenarioRoot` this change adds none.

### 6. Supported-candidate model (Stage A)

Three concepts:

- **Possible owner** — any eligible scenario root; existence alone creates nothing.
- **Supported candidate** — a possible owner bound to THIS operation by at least one fired positive ownership predicate, aggregated **keyed by ScenarioId**: multiple members of one scenario matching the same child aggregate into ONE candidate carrying the union of matching witnesses, never duplicates.
- **Sufficiently supported owner** — a supported candidate surviving contradiction checks.

Outcomes over sufficiently supported owners: zero → `Uncorrelated { evidence }`; exactly one → `Resolved { scenario, Exact }`; two or more → `Ambiguous { candidates }` with candidate-specific witnesses. Absence of eliminating evidence manufactures nothing; context-only agreement creates no candidates.

Contradiction checks operate only on supported candidates and contain exactly one instance this change: cross-scenario decisive conflict — `ExplicitParentSpan` relations binding one operation into different scenarios keep both candidates as ambiguity; nothing discards either side. No temporal contradiction exists. Future checks arrive as named spec-defined predicates.

### 7. Temporal evidence: contextual only, no elimination

No documented recording-global timeline guarantee exists behind `RelativeTimeNanos`, so this resolver performs no temporal elimination: overlap selects nothing; non-overlap eliminates nothing (asynchronous downstream work may start after its cause completes); incomparable timelines are irrelevant because no comparison influences outcomes; temporal items ride along as retained contextual evidence. Safe contradiction evidence may reject an asserted relationship only when logically incompatible under documented comparable timeline semantics; ordinary lifetime non-overlap is never such a contradiction. A future safe rule requires a dedicated timeline-guarantee change first.

### 8. Confidence from semantics, not counts

- `Exact` — ownership rests on a direct positive scenario-level relationship between the operation and the owning scenario using a named exact predicate: `SharedTraceIdentity` against the scenario, or resolver-generated `ScenarioRoot` establishment for the root itself.
- `Strong` — ownership inherited transitively through a uniquely established `ExplicitParentSpan` member chain without any direct scenario-level identity match of its own.
- `Inferred` — reserved for externally supplied graphs; never emitted here.

Contextual evidence is never `Strong`; counts of matches, dimensions, temporal proximity, connection reuse, and process/thread equality produce nothing.

### 9. Iterative monotonic fixpoint for transitive inheritance

Chaining is an iterative worklist-free fixpoint; no recursive traversal exists:

```text
repeat:
    snapshot = resolved-set from previous round
    evaluate operations against snapshot only
    admit all newly resolved operations
until a round admits nothing new
```

Monotonicity: resolved membership is never removed or reassigned within one run. Each productive round admits at least one previously unresolved operation, bounding successful rounds by the admitted-operation count. After a no-progress round, remaining unsupported operations stay `Uncorrelated`, multi-candidate-supported operations stay `Ambiguous`, and no fallback fires. Snapshot evaluation prevents input order from changing intra-round eligibility depth; reversing input order on long chains yields identical output. There is no arbitrary recursion-depth cap because there is no recursion.

### 10. Stage B: direct causal-parent selection

For each `Resolved` operation, candidate parents are members of its own scenario (root included). `ExplicitParentSpan` is the defined sufficient direct-parent predicate; a fired relation whose target span is unshared selects one edge. Multiple equally sufficient parents, insufficient relations, or contextual-only relations emit NO edge — ownership stays `Resolved`, unresolved parenthood lives in edge absence plus retained witnesses, and no second ambiguity channel is invented. Proposed parents outside the owning scenario are ignored without rewriting ownership. Foundation edge validation runs unchanged.

### 11. Role preservation

Supplied `InteractionRoleResolution` values copy verbatim. Unknown/ambiguous-role operations participate as ordinary non-root members; correlation never promotes roles. Root establishment does not read or write role state.

### 12. Deterministic minimal-witness retention

Retention keeps what explains the semantic outcome, not every matching item. Per outcome:

- **Resolved**: retain the deterministic minimal witness set proving chosen-scenario ownership and confidence (the fired predicate item(s); `ScenarioRoot` for roots), plus the selected direct-parent witness when Stage B emitted an edge. Fill remaining capacity with contextual/input evidence in canonical input order up to the cap.
- **Ambiguous**: retain, for EVERY emitted candidate, at least one deterministic candidate-specific witness proving that scenario's support; then fill remaining capacity canonically. Capacity applies PER CANDIDATE, so ordinary valid ambiguity can never overflow into dropped candidates or typed errors.
- **Uncorrelated**: retain contextual/input evidence deterministically up to the cap; no ownership witness exists.
- **Selected edge**: retain the direct-parent predicate witness on the edge.

The global retention constant is documented (64 items per outcome slot, applied per ambiguity candidate where applicable). Witness selection order: justification witnesses first in canonical predicate order, then contextual fill in canonical input order; no recency-based prioritization. Correctness never trades against these bounds.

### 13. ETL composition boundary

A `chronicle-etl` helper joins published `CanonicalSession` values with a caller-supplied `CorrelationContext`: verifies full references against session lineage, enriches lifetime/provenance views, joins context by exact reference. Join rules: admitted session operation missing a role-resolution entry → composition error; missing evidence entry → empty evidence; context entry referencing no verified session operation → composition error; duplicate/mismatched scoped references fail from verification. Zero selection semantics; explicitly invoked; default publication/checkpoint path never calls it. Callers supply roles/evidence only — the resolver owns scenario creation and root evidence.

### 14. Failure taxonomy

Typed `CorrelationResolverError` (fail closed): duplicate full operation references; reference lineage/recording-scope mismatch; invalid supplied roles; malformed evidence; context join violations; impossible internal graph invariants. Ordinary situations — never errors: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant.

### 15. Compatibility, persistence, and follow-up boundaries

In-memory runtime capability only; frozen v1 contracts untouched; nothing persists. Roadmap:

```text
introduce-correlation-domain-model (done)
        ↓
correlate-ingress-and-egress-interactions      ← this change
        ↓
derive-native-correlation-evidence             ← defines native relational semantics incl. logical task generation,
                                                 causal execution lineage, process/task inheritance,
                                                 socket/connection generation, protocol-stream lineage;
                                                 may promote named predicates via spec change
        ↓
persist-correlation-scenario-artifacts
        ↓
integrate-pluggable-trace-evidence-providers
        ↓
trace-provider-plugin-installation
        ↓
scenario inspect / replay / test generation
```

Durable logical-operation identity (publication-scope-independent scenario stability) remains a separate dedicated change. Until native evidence lands, ordinary recordings legitimately produce mostly `Uncorrelated` results.

## Risks and Trade-offs

- Conservative gating (contextual task lineage, no temporal elimination) leaves weak-but-real signals unresolved until producers emit relational evidence. Intended.
- Full-reference identity means regrouping operations across publications changes scenario identities. Honest: the foundation provides no stronger invariant to build on.
- Per-candidate retention capacity slightly raises worst-case memory for wide ambiguity; explainability of every candidate outweighs it.
- Snapshot-round fixpoint can take more rounds than eager intra-round admission; bounded by operation count and immune to ordering effects.

## Future Changes

- `derive-native-correlation-evidence`: Chronicle-native evidence derivation and lineage-semantics contracts; may promote named predicates.
- Durable logical-operation identity / scenario identity surviving publication scope changes.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` / `trace-provider-plugin-installation`.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Scenario inspect/UX, replay, test generation/assertions.
