## Why

The `correlation-domain-model` capability defines Chronicle-owned interaction roles, scoped canonical operation references, `CorrelationGraph` ownership, and validation for supplied resolutions. It deliberately does not compute those resolutions: today every scenario assignment must be supplied by hand, so Chronicle cannot yet reconstruct the application-level scenarios it exists to produce.

Naive correlation is unsafe under concurrent ingress. When several ingress requests overlap in time — the normal case for any server — an egress interaction overlaps many candidate ingresses, and choosing the oldest, newest, closest, or currently active ingress is a guess that silently misattributes behavior. Wrong scenario ownership produces confidently wrong regression tests, which is worse than no test.

Correlation selection must also stay provider-neutral. Trace context (OpenTelemetry, Datadog, AWS X-Ray, B3) is high-quality evidence when present, but Chronicle must correlate without any tracing SDK installed, and provider SDK types or adapters must never enter the resolver's domain boundary. Adapters translate external data into Chronicle-owned `CorrelationEvidence`; the resolver consumes only that evidence and never changes.

## What Changes

- Add a deterministic, ambiguity-safe resolver to the existing canonical correlation domain (`chronicle-canonical`). It consumes canonical operations, supplied `InteractionRoleResolution` values, and Chronicle-owned `CorrelationEvidence`, and produces a validated `CorrelationGraph`.
- Classify evidence into normative tiers: decisive causal-link evidence capable of establishing ownership, narrowing evidence capable only of eliminating impossible candidates, supporting/contextual evidence, and evidence that must never independently resolve ownership. Temporal overlap alone never selects a scenario owner.
- Emit exactly one of `Resolved`, `Ambiguous { candidates }`, or `Uncorrelated` per operation: one sufficiently supported owner resolves; multiple viable owners preserve ambiguity with candidate-specific evidence; zero viable owners remain uncorrelated. No egress is ever forced into a scenario.
- Derive `ScenarioId` deterministically from stable scope (recording plus root operation reference) so results are stable across process restarts, ETL retries/replay, input iteration order, map/hash order, worker scheduling, and epoch publication boundaries.
- Support concurrent ingress as a first-class case, cross-session/cross-epoch scenarios via full scoped `CanonicalOperationRef`s, and chained/nested causal edges within the existing tree invariants.
- Preserve role resolution verbatim: correlation never rewrites `Unknown`/`Ambiguous` roles, promotes timing to ingress, or lets an egress become a root.
- Add an ETL composition helper that turns published canonical sessions into resolver inputs. ETL owns no correlation semantics; capture/WAL/publication behavior stays unchanged; the resolver runs on demand, not in the default publication hot path.
- Keep the resolver in-memory only. No persisted correlation artifact, no field added to Capture Event v1 / Canonical Session v1 / WAL v1, no new crate, no provider SDK dependency, no `EventId`.

## Capabilities

### New Capabilities

- `correlation-resolver`: deterministic, provider-neutral, ambiguity-safe selection of scenario ownership for canonical operations from Chronicle-owned roles and evidence, producing valid `CorrelationGraph` output with preserved ambiguity and uncorrelated outcomes.

### Modified Capabilities

None. The foundation's algorithm-neutrality requirement already names this resolver as a separate capability; its validation semantics are unchanged and remain authoritative for resolver output.

## Compatibility and Impact

No frozen contract changes. The resolver operates on non-v1 canonical-domain values at runtime and persists nothing. Persisted correlation/scenario artifacts remain unauthorized until a dedicated change (for example `persist-correlation-scenario-artifacts`) selects a separately versioned sidecar/new artifact or an explicit 0.2 compatibility/migration path.

No new crate: correlation semantics stay in `chronicle-canonical`; composition belongs to `chronicle-etl` under its existing dependency edges; architecture policy (`validation/architecture.toml`) needs no changes.

## Out of Scope and Future Changes

- OpenTelemetry/provider SDK integration, provider-specific adapters, trace-context plugin installation, registries, discovery, loading runtimes, and CLI plugin management.
- Persisted correlation/scenario artifacts (`persist-correlation-scenario-artifacts`).
- Test-case generation, assertions, replay v2, dependency mocking, scenario export formats, UI/website work.
- Probabilistic or ML-based correlation, arbitrary weighted scoring, distributed multi-host correlation.
- Capture-schema `EventId` and modifications to frozen v1 artifacts.
- Scenario inspect/UX commands and scenario replay.
