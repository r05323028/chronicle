## Context

The `correlation-domain-model` capability (archived 2026-08-23) shipped Chronicle-owned `InteractionRole`/`InteractionRoleResolution`, scoped `CanonicalOperationRef`, `CorrelationGraph`, `CorrelationResolution::{Resolved,Ambiguous,Uncorrelated}`, tagged `CorrelationEvidence`, selected-edge invariants, and strict validation — but deliberately no algorithm. `CorrelationGraph::from_operations` accepts already-supplied resolutions; nothing computes them.

Current code facts this design builds on:

- `crates/chronicle-canonical/src/correlation.rs` owns all correlation values. `CorrelationEvidenceKind` variants: `TraceRelationship`, `ProtocolOwnership`, `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `TemporalLifetime`, `WireDirection`, `SocketRole`, `Custom`. `CorrelationResolution::Resolved` carries `CorrelationConfidence { Exact, Strong, Inferred }`. `CanonicalOperationRef` verifies lineage via `resolve_in_session`.
- `CanonicalOperation` exposes `started_at_offset`/`completed_at_offset` (`RelativeTimeNanos`) on the recording-relative timeline, plus sequence and provenance — sufficient lifetimes for candidate elimination; no new capture fields are needed.
- `ScenarioId` is a UUID-based neutral primitive in `chronicle-common`; today it has no deterministic derivation rule.
- `CorrelationGraph` stores maps keyed by full reference in `BTreeMap`s — deterministic representation already exists.
- Dependency policy (`validation/architecture.toml`): `chronicle-canonical` depends only on `chronicle-common`; `chronicle-etl` already depends on `chronicle-canonical`. Provider packages are denied in core/domain crates and the whole default-distribution closure.

## Goals and Non-Goals

### Goals

- Deterministically reconstruct `Ingress → children` scenarios from canonical operations, roles, and evidence, including concurrent ingress, cross-epoch membership, and chained causality.
- Preserve ambiguity and uncorrelated outcomes instead of guessing.
- Keep the resolver provider-neutral, SDK-free, and independent of tracing presence.
- Make results stable across restarts, retries, replay, iteration order, scheduling, and epoch boundaries.
- Fit entirely inside existing crate ownership and dependency policy.

### Non-Goals

- Persistence of correlation results; any v1 artifact change; `EventId`.
- Provider adapters, plugin installation/discovery, CLI surfaces.
- Weighted/scoring/probabilistic correlation; distributed multi-host correlation.
- Changes to capture, WAL, session reconstruction, protocol pairing, replay, or publication behavior.

## Decisions

### 1. Resolver placement: `chronicle-canonical`

The resolver is correlation semantics; the foundation invariant assigns correlation semantics to `chronicle-canonical`. It extends the existing `correlation` module. No standalone crate: nothing here justifies a new dependency node, and creating one would force new edges for every consumer. ETL composes; it never implements selection rules.

### 2. Inputs and outputs

```text
CorrelationInput {
    reference: CanonicalOperationRef,
    lifetime: OperationLifetimeView,        // from CanonicalOperation offsets/provenance
    role: InteractionRoleResolution,        // supplied verbatim
    evidence: Vec<CorrelationEvidence>,     // Chronicle-owned only
}

resolve_correlation(recording_id, inputs) -> Result<CorrelationGraph, CorrelationResolverError>
```

Output is a fully populated `CorrelationGraph` — role resolutions copied verbatim from inputs, correlation resolutions computed, scenarios created for root-eligible operations, selected edges constructed — which must pass existing `CorrelationGraph::validate(_against_sessions)` unchanged. The resolver validates its own inputs (duplicate references, scope mismatch, invalid supplied roles are errors, not silent drops) and otherwise always produces an outcome per admitted operation; there is no third "resolver failed" state for ordinary evidence situations.

### 3. Evidence tiers

| Tier | Kinds / conditions | Power |
| --- | --- | --- |
| Decisive causal link | `TraceRelationship` whose declared parent context resolves exactly to the candidate operation's trace identity | Can establish ownership alone -> `Exact` |
| Narrowing | `TemporalLifetime` disjointness (comparable timelines only); `ExecutionTaskLineage` exact-match filtering; `ProcessThreadGeneration` / generation disjointness proving co-execution impossibility | May eliminate impossible candidates; never selects among survivors |
| Supporting/contextual | `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, corroborating matches of narrowing kinds | Contributes corroboration (e.g. second independent dimension for `Strong`); never eliminates, never selects alone |
| Never independent | Temporal overlap, bare PID/TID equality, `WireDirection`, `SocketRole`, shared connection identity, processing order | Cannot produce `Resolved` in any combination with only same-tier items; retained as evidence |

Rules that make the tiers safe:

- Shared connection/stream identity is *not* narrowing (connection reuse is normal); only generation-disjointness that proves impossibility narrows.
- Task-lineage matching filters parents to those sharing the child's task identity; if several viable parents share it, it does not resolve.
- Temporal overlap eliminates nothing and selects nothing. Disjointness eliminates only when both lifetimes are on a comparable timeline; cross-session comparisons without comparable offsets are treated as non-eliminating (fail-safe direction).
- `Custom` evidence is supporting/contextual by default; a future namespaced kind must be promoted by a spec change, not by resolver discretion.
- Conflicting decisive links to different owners yield `Ambiguous` with per-candidate evidence; processing order is invisible.

### 4. Outcome selection

For each non-root operation, against the current set of viable owners (scenario roots, later also resolved members):

1. Apply eliminating evidence -> survivor set.
2. Exactly one survivor supported by decisive evidence -> `Resolved(Exact)`.
3. Exactly one survivor with no decisive item but unique survival after non-temporal narrowing plus at least two independent supporting dimensions -> `Resolved(Strong)`.
4. Multiple survivors -> `Ambiguous { candidates }` (candidate-specific evidence preserved).
5. Zero survivors -> `Uncorrelated { evidence }`.

The resolver never emits `Inferred`; that confidence remains valid for externally supplied graphs. Ambiguity is never broken by ordering, recency, or count of weak signals.

### 5. Candidate construction and scenario identity

Inputs are sorted by full reference tuple before any processing. One scenario candidate is created per root-eligible operation (`Known(Ingress)` only). Unknown/ambiguous-role operations never become owners; known-egress operations never become owners.

`ScenarioId` is derived deterministically from a fixed namespace plus the root reference bytes (`recording_id || owner_epoch_id || session_id || operation_id`). Same logical scenario therefore keeps its identity across restarts, retries, replay, and epoch-boundary republication; two recordings or two different roots cannot collide. Randomness, timestamps, and processing counters never enter the derivation.

### 6. Concurrent ingress

Overlapping lifetimes create no preference. With Ingress A/B/C overlapping and Egress X/Y present, each egress is evaluated independently against all viable owners purely on evidence tiers. Cases:

- Unique evidence for X->A and Y->B: two scenarios resolve correctly regardless of start order, duration, or recency.
- Evidence viable for both A and B: `Ambiguous { A, B }` preserved even though "most" evidence leans one way.
- No viable owner: `Uncorrelated`.

No heuristic exists to get wrong: there is no oldest/newest/closest/active tie-break anywhere in the implementation.

### 7. Cross-epoch and cross-session scenarios

All ownership decisions operate on full `CanonicalOperationRef`s; session/epoch difference is invisible except for lineage verification and timeline comparability (Decision 3). An ingress owned by epoch N's session and an egress owned by N+1's session resolve into one scenario; selected edges carry both full references. Terminal canonical operations are referenced by their completion-owner scope, matching the foundation's continuation rules.

### 8. Chained causal edges

After root-level resolution settles, bounded fixed-point rounds let an operation resolve against an already-resolved member (e.g. ingress -> HTTP egress -> database egress). Edges follow the resolution chain (grandchild attaches to the member its evidence indicates), staying within foundation tree invariants: same scenario, acyclic, ≤1 selected parent, root never a child, ambiguous candidates never edges. Rounds are bounded by member count; ordering within a round is the canonical input order.

### 9. Role independence

The resolver copies supplied `InteractionRoleResolution` values verbatim into the output graph. It never promotes roles, resolves role ambiguity, or lets correlation outcomes rewrite classification. Operations with unknown/ambiguous roles participate in correlation as members/candidates like any other non-root operation (they simply can never be roots) — satisfying the foundation's requirement that both dimensions remain inspectable.

### 10. ETL composition boundary

A small helper in `chronicle-etl` converts published `CanonicalSession` values of one recording into `CorrelationInput`s and returns the graph. It contains zero selection logic. The default publication/checkpoint path does not invoke the resolver (Chronicle's low-production-overhead philosophy); invocation is explicit, so future consumers (application use case, CLI command, persistence change) opt in. No dependency-policy change is required.

### 11. Compatibility and failure behavior

In-memory runtime capability only; nothing persists. Frozen v1 contracts untouched. Invalid resolver inputs fail closed with typed errors; unverifiable references fail closed rather than resolving leniently; output graphs must pass unchanged foundation validation. If a future `persist-correlation-scenario-artifacts` change lands, it selects a separately versioned sidecar/artifact and inherits these determinism rules.

## Risks and Trade-offs

- Conservative tiers mean some genuinely related operations stay `Ambiguous`/`Uncorrelated` until richer evidence kinds exist. That is intended: wrong attribution is worse than missing attribution.
- Deterministic ID derivation couples `ScenarioId` to root-reference stability; the foundation already fixes terminal-operation scoping, so the coupling is well-defined.
- Fixed-point chaining adds mild complexity; bounded rounds keep worst-case cost linear in members × rounds.

## Future Changes

- `persist-correlation-scenario-artifacts`: versioned persistence of resolved graphs.
- `integrate-pluggable-trace-evidence-providers`: outer adapters translating OTel/Datadog/X-Ray/B3 data into `CorrelationEvidence`.
- `trace-provider-plugin-installation`: install/discovery/loading UX.
- Scenario inspect/UX, scenario replay, test-case generation/assertions.
