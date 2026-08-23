## Context

The `correlation-domain-model` capability (archived 2026-08-23) shipped Chronicle-owned `InteractionRole`/`InteractionRoleResolution`, scoped `CanonicalOperationRef`, `CorrelationGraph`, `CorrelationResolution::{Resolved,Ambiguous,Uncorrelated}`, tagged `CorrelationEvidence`, selected-edge invariants, and strict validation — but deliberately no algorithm.

Repository facts this design is built on (verified on current main):

- **OperationId is randomly generated at canonicalization time.** The production HTTP canonicalizer assigns `id: OperationId::new()` (`crates/chronicle-protocol-builtins/src/lib.rs`, `fn operation`); `uuid_id!` backs all IDs with `Uuid::new_v4()`. Stability therefore comes only from persistence: incremental ETL checkpoints preserve generated IDs, and cross-epoch continuation preserves one logical operation identity across rollover. Re-canonicalizing the same WAL from scratch generates different IDs. OperationId is stable and unique within a recording lineage's persisted artifacts — nothing more.
- **`RelativeTimeNanos(u64)` carries no documented recording-global timeline guarantee.** Canonicalizer offsets are derived per reconstructed stream; no artifact asserts cross-session comparability semantics. Any temporal elimination rule would rest on unproven comparability.
- **Evidence kinds are operation-local records.** `ExecutionTaskLineage { task, worker }`, `TraceRelationship { provider, trace_id, span_id, parent_span_id }`, etc., describe one observation; correlation needs relational predicates comparing child-side items against candidate-side items.
- **`ScenarioId` is a UUID-backed neutral primitive** (`chronicle-common`), with no derivation rule today. `sha2` is already a workspace dependency used by six crates; `chronicle-canonical` does not yet declare it.
- Dependency policy (`validation/architecture.toml`) forbids provider packages in core/domain crates and the default-distribution closure; an ordinary hashing crate is unaffected. `chronicle-etl -> chronicle-canonical` already exists.

## Goals and Non-Goals

### Goals

- Deterministically reconstruct scenario ownership from positively supported, candidate-specific relationships — including concurrent ingress, cross-epoch membership, and chained causality — while preserving ambiguity and uncorrelated outcomes.
- Separate scenario ownership from direct causal-parent selection, matching the domain's existing split between `CorrelationResolution` and `SelectedCausalEdge`.
- Define every consumed evidence kind as an explicit candidate-relative predicate; the implementation invents no semantics.
- Derive scenario identity from stable logical scope with exactly specified bytes and versioning.
- Keep the resolver provider-neutral, SDK-free, bounded in complexity, and independent of tracing presence.
- Fail closed on broken inputs; never error on merely insufficient or conflicting evidence.

### Non-Goals

- Producing native correlation evidence from real recordings (`derive-native-correlation-evidence`).
- Temporal elimination of any kind in this change (no proven timeline comparability).
- Persistence of correlation results; any v1 artifact change; `EventId`; provider adapters or plugin infrastructure.
- Weighted/scoring/probabilistic correlation; distributed multi-host correlation.
- Changes to capture, WAL, session reconstruction, protocol pairing, replay, or publication behavior.

## Decisions

### 1. Resolver placement: `chronicle-canonical`

The resolver is correlation semantics; the foundation invariant assigns those to `chronicle-canonical`. It extends the existing `correlation` module. No standalone crate: nothing justifies a new dependency node. ETL composes; it never implements selection rules.

### 2. Inputs, outputs, and CorrelationContext

```text
CorrelationInput {
    reference: CanonicalOperationRef,
    operation_view: OperationCorrelationView,   // lifetimes + provenance from CanonicalOperation
    role: InteractionRoleResolution,            // supplied verbatim
    evidence: Vec<CorrelationEvidence>,         // Chronicle-owned, operation-local items
}

correlation_context = {
    role_resolutions: Map<CanonicalOperationRef, InteractionRoleResolution>,
    evidence:        Map<CanonicalOperationRef, Vec<CorrelationEvidence>>,
}

resolve_correlation(recording_id, inputs) -> Result<CorrelationGraph, CorrelationResolverError>
```

Canonical sessions supply references, lifetime views, and lineage verification. Role resolutions and evidence arrive exclusively through supplied context; sessions alone cannot fabricate them (frozen v1 sessions contain neither). Output is a fully populated `CorrelationGraph` that must pass existing foundation validation unchanged.

### 3. Scenario identity contract: `scenario-id-v1`

Identity = recording identity + stable logical root-operation identity:

```text
scenario-id-v1 = UUID-128( SHA-256(
    "chronicle-correlation/scenario-id/v1"   // fixed ASCII domain separator, 37 bytes
 || recording_uuid_be                        // RecordingId as Uuid::as_bytes(), 16 bytes
 || root_operation_uuid_be                   // OperationId  as Uuid::as_bytes(), 16 bytes
))
```

- Field order, encoding, and sizes are fixed: ASCII separator, then big-endian UUID byte arrays (`Uuid::as_bytes()`). Fixed lengths make delimiters unnecessary.
- Digest: first 16 bytes of the SHA-256 output become the UUID octets; then version field set to 8 (custom, RFC 9562): `octets[6] = (octets[6] & 0x0F) | 0x80`; variant set to RFC 4122: `octets[8] = (octets[8] & 0x3F) | 0x80`.
- Epoch and session identities are NOT inputs. Regrouping the same identified operations across different session/epoch publications preserves scenario identity.
- Guarantee boundary (explicit): stability holds within one recording lineage's persisted operation identities — restarts, retries, resumed ETL, replay over the same published artifacts, any input ordering. It does NOT hold across independent re-canonicalization that regenerates random `OperationId`s from raw WAL; that stronger cross-republication identity requires a future identity/versioning change and is not claimed anywhere in this capability.
- Versioning: the domain separator embeds `v1`. Any change to fields, order, encoding, hash, truncation, or bit-fixups ships a new separator version and new derivation name; existing derivations never change meaning.

### 4. Two-stage resolution pipeline

Stage A and Stage B answer different questions and write different graph state:

```text
all admitted inputs
   |
[Stage A] scenario ownership per operation
   |    construct candidate-specific positive relationships against scenario roots/members
   |    apply named safe contradiction checks to supported candidates only
   |    -> Resolved(scenario, Exact|Strong) | Ambiguous(candidates+evidence) | Uncorrelated(evidence)
   |
[Stage B] direct causal parents for Resolved operations only
        evaluate direct-parent predicates within the owning scenario
        -> exactly one sufficient parent: SelectedCausalEdge
        -> otherwise: no edge; ownership unchanged; evidence stays in the resolution
```

Ownership never requires pointing at the root: an operation may bind to any member of a scenario and may inherit membership transitively through uniquely supported parent relations to already-resolved members. Chaining rounds for transitive binding are bounded by scenario member count and processed in canonical input order.

### 5. Relational evidence predicates

Every rule below is normative; the implementation encodes exactly these comparisons. Child side = the operation being resolved; candidate side = a scenario root or already-resolved member. A relation exists only when both sides carry the referenced items; a missing side yields no relation, never a default.

| Predicate (name) | Kind(s) | Match condition (child × candidate) | Positive support meaning | Contradiction | Supports ownership | Supports direct parent |
| --- | --- | --- | --- | --- | --- | --- |
| `SharedTraceIdentity` | `TraceRelationship` | same non-empty `provider` AND same non-empty `trace_id` | child shares one trace identity with that scenario — scenario-level relationship evidence | none | yes (`Exact` when it is the sole basis) | no |
| `ExplicitParentSpan` | `TraceRelationship` | child `parent_span_id` non-empty AND equals candidate's non-empty `span_id` with same `provider`+`trace_id` | declared direct span parenthood | none | yes (via member chain inheritance) | yes — sufficient alone |
| `SameTaskLineage` | `ExecutionTaskLineage` | exact string equality of non-empty `task` values | co-execution within one task generation — scenario-level relationship evidence | none (different tasks never contradict: spawned work gets new task identities) | yes (`Exact` when sole basis) | no |
| `SharedSpanAmbiguity` | `TraceRelationship` | multiple distinct operations share one `provider`+`trace_id`+`span_id` | marks the span as ambiguous for direct-parent matching | — | n/a | blocks `ExplicitParentSpan` sufficiency for that span |
| Context-only kinds | `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom` | — | retained as retained/contextual evidence on outcomes | none ever | never | never |

Additional predicate rules:

- Provider namespaces are part of trace identity: identical opaque IDs under different providers never match. Trace evidence stays opaque and provider-neutral while remaining namespaced.
- Missing `span_id`/`parent_span_id`/`trace_id` degrade gracefully: the corresponding predicate simply does not fire.
- Same `trace_id` alone NEVER implies a selected direct edge; `ExplicitParentSpan` is the only trace-derived direct-parent predicate, and `SharedSpanAmbiguity` suppresses it when several operations share the candidate's span.
- Connection/socket/stream/process/thread equality supports nothing by itself; shared carriers are normal. They remain attached evidence for inspection.
- `Custom` evidence is contextual by default; promoting any namespaced custom relation requires a specification change naming its predicate.

If a future producer cannot express these relations with current kinds, the smallest additive adjustment extends `CorrelationEvidenceKind`; this change adds none.

### 6. Supported-candidate model (Stage A)

Three concepts, strictly ordered:

- **Possible owner** — any eligible scenario root. Existence alone creates nothing.
- **Supported candidate** — a possible owner for which at least one positive ownership predicate (`SharedTraceIdentity`, `SameTaskLineage`, or inherited membership via `ExplicitParentSpan` chains) binds THIS operation to THAT scenario specifically.
- **Sufficiently supported owner** — a supported candidate that survives safe contradiction checks.

Outcome selection over sufficiently supported owners:

- zero → `Uncorrelated { evidence }` (retained items include whatever contextual evidence exists);
- exactly one → `Resolved { scenario, confidence }`;
- two or more → `Ambiguous { candidates }`, each candidate carrying the specific evidence that supports it.

Absence of eliminating evidence manufactures nothing. Non-candidate-specific context (shared process, connection environment, temporal overlap, protocol family) never creates supported candidates. Every entry in `Ambiguous.candidates` must be explainable by its own evidence list.

Contradiction checks operate only on supported candidates and, in this revision, contain exactly one instance: **cross-scenario decisive conflict** — if `ExplicitParentSpan` relations bind one operation decisively into two different scenarios' chains, both scenarios stay supported candidates and the outcome is ambiguity; nothing discards either side. No temporal contradiction exists in this change (Decision 7). Future contradiction checks must each arrive as a named, spec-defined predicate with proven timeline/identity guarantees.

### 7. Temporal evidence: contextual only, no elimination

Repository inspection found no documented recording-global timeline guarantee behind `RelativeTimeNanos` offsets (per-stream derivation, undocumented cross-session semantics). Therefore this resolver performs **no temporal elimination of any kind**:

- overlap selects nothing;
- non-overlap eliminates nothing — an ingress that completed before downstream asynchronous work began remains fully eligible;
- incomparable timelines need no special case because no comparison influences outcomes;
- temporal items ride along as retained contextual evidence.

Normative precedence rule: safe contradiction evidence may reject an asserted causal relationship only when the contradiction is logically incompatible with that relationship under documented, trustworthy, comparable timeline semantics. Ordinary lifetime non-overlap is not such a contradiction. A future safe rule (for example, candidate start strictly after child start on a proven common timeline) must be introduced by a dedicated change that first establishes the timeline guarantee in the canonical model.

### 8. Confidence from semantics, not counts

- `Exact` — ownership rests directly on at least one positive ownership predicate evaluated between the operation and the owning scenario's root/members (`SharedTraceIdentity`, `SameTaskLineage`, or direct `ExplicitParentSpan` binding).
- `Strong` — ownership rests solely on transitive inheritance: the operation binds through `ExplicitParentSpan` (or equivalent future named chain relations) to already-resolved members without any direct predicate of its own.
- `Inferred` — reserved for externally supplied graphs; this resolver never emits it.

No numeric scores, vote counts, or dimension tallies exist anywhere in the resolver. Two weak contextual agreements still support nothing.

### 9. Stage B: direct causal-parent selection

For each `Resolved` operation, candidate parents are members of its own scenario (root included). `ExplicitParentSpan` is the currently defined sufficient direct-parent predicate; a fired `ExplicitParentSpan` whose target span is not shared (`SharedSpanAmbiguity` absent) selects exactly one edge. Outcomes:

- unique sufficient parent → emit `SelectedCausalEdge { scenario, parent, child, evidence }`;
- multiple sufficient parents, insufficient relationships, or contextual-only relations → emit NO edge; ownership stays `Resolved`; the unresolved-parenthood detail lives in the absence itself plus the resolution's retained evidence — the domain has no second ambiguity channel, and this change does not invent one;
- proposed parents outside the owning scenario → ignored for edge construction; scenario ownership is never rewritten to make an edge valid.

Foundation edge validation runs unchanged: full scoped endpoints, same scenario, acyclic, ≤1 parent per child, root never a child, ambiguous operations never in edges.

### 10. Role preservation

Supplied `InteractionRoleResolution` values copy verbatim into the output graph. Unknown/ambiguous-role operations participate like any non-root operation (never roots); correlation never promotes roles or rewrites classification.

### 11. Boundedness and complexity

- All indexes deterministic: inputs sorted by full reference tuple; relation indexes are `BTreeMap`s keyed by `(provider, trace_id)`, `task`, `(provider, trace_id, span_id)`, and reference.
- Candidate lookup uses index hits, never full scans: cost ≈ O((N + R) log N + Σ bucket-hits), where N = operations, R = relational items; worst case degrades only when adversarial inputs share single keys (bounded by bucket size), which correctness never trades away.
- Transitive chaining rounds are bounded by the owning scenario's member count; recursion depth is explicitly bounded — no unbounded recursion.
- Retained evidence per outcome is capped at a documented constant (64 items), selected deterministically in canonical item order; overflow drops lowest-priority contextual items last-in-first-out and never drops predicate-bearing items that justify the outcome. Resolver state never scales with arbitrary external identifier cardinalities beyond the input set.
- Scenario creation is bounded by root-eligible operation count.

### 12. ETL composition boundary

A `chronicle-etl` helper joins published `CanonicalSession` values with a caller-supplied `CorrelationContext` and produces resolver inputs:

- resolves and verifies each full operation reference against session lineage (existing `resolve_in_session`);
- enriches references with lifetime/provenance views;
- joins context entries by exact full reference.

Join rules (explicit):

- session operation missing a role-resolution context entry → composition error (suppliers must classify every admitted operation; silence is not `Unknown` by default);
- missing evidence-map entry → treated as empty evidence (valid: little evidence is a normal situation);
- context entry referencing no session operation → composition error (mismatched scope fails closed);
- duplicate context keys → impossible (map keyed by reference); duplicate session-operation references → composition error surfaced from verification.

The helper contains zero ownership-selection semantics and is invoked explicitly — the default publication/checkpoint path never calls it (low production overhead). No dependency-policy change is required.

### 13. Failure taxonomy

Typed `CorrelationResolverError` (input/invariant failures, fail closed): duplicate full operation references; reference lineage/recording-scope mismatch; invalid supplied `InteractionRoleResolution`; malformed `CorrelationEvidence` (fails existing evidence validation); context join violations (Decision 12); internal graph invariant impossibility after construction.

Ordinary evidence situations — never errors: missing trace context; no positive ownership evidence → `Uncorrelated`; several supported candidates → `Ambiguous`; uncertain direct parent → `Resolved` without edge; conflicting valid causal evidence between scenarios → `Ambiguous`; incomparable timing → irrelevant (contextual only).

### 14. Compatibility, persistence, and follow-up boundaries

In-memory runtime capability only; nothing persists; frozen v1 contracts untouched. Deferred follow-ups, in roadmap order:

```text
introduce-correlation-domain-model (done)
        ↓
correlate-ingress-and-egress-interactions      ← this change
        ↓
derive-native-correlation-evidence             ← populates the evidence this resolver consumes
        ↓
persist-correlation-scenario-artifacts
        ↓
integrate-pluggable-trace-evidence-providers
        ↓
trace-provider-plugin-installation
        ↓
scenario inspect / replay / test generation
```

`derive-native-correlation-evidence` owns deriving provider-neutral evidence from Chronicle-controlled sources (application/process/task lineage, socket/connection generations, protocol stream relationships, capture/session reconstruction facts). Until it lands, ordinary recordings legitimately produce mostly `Uncorrelated` results — the resolver consumes evidence; it does not mine sessions for it. A separate future identity/versioning change must precede or accompany any cross-republication scenario-identity claim, and a timeline-guarantee change must precede any temporal contradiction rule.

## Risks and Trade-offs

- Conservative supported-candidate gating means weak-but-real signals stay uncorrelated until producers emit relational evidence. Intended: wrong attribution is worse than missing attribution.
- Identity scoped to persisted operation identities: regrouping preserved, regeneration not — honestly bounded rather than falsely promised.
- No temporal elimination forfeits some cheap pruning; correctness first, and a timeline-guarantee change can add named contradictions later.
- Fixed evidence caps trade completeness of retained diagnostics for bounded memory; predicate-bearing justification items are never dropped.

## Future Changes

- `derive-native-correlation-evidence`: Chronicle-native evidence derivation from capture/session/task/socket sources.
- `persist-correlation-scenario-artifacts`: versioned persistence of resolved graphs.
- `integrate-pluggable-trace-evidence-providers` / `trace-provider-plugin-installation`: outer adapters translating OTel/Datadog/X-Ray/B3 data into `CorrelationEvidence`, then install/discovery/loading UX.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Identity/versioning change for cross-republication scenario stability if ever required.
- Scenario inspect/UX, scenario replay, test-case generation/assertions.
