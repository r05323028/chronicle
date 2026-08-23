## Why

The `correlation-domain-model` capability defines Chronicle-owned interaction roles, scoped canonical operation references, `CorrelationGraph` ownership, and validation for supplied resolutions. It deliberately does not compute those resolutions: today every scenario assignment must be supplied by hand, so Chronicle cannot yet reconstruct the application-level scenarios it exists to produce.

Naive correlation is unsafe under concurrent ingress. When several ingress requests overlap in time — the normal case for any server — an egress interaction overlaps many candidate ingresses, and choosing the oldest, newest, closest, or currently active ingress is a guess that silently misattributes behavior. Wrong scenario ownership produces confidently wrong regression tests, which is worse than no test.

Two further safety properties shape this design. First, absence of disproof is not support: an egress with no candidate-specific evidence overlapping three ingresses must stay `Uncorrelated`, not become ambiguous across all three. Only positively evidenced, candidate-specific relationships may create supported scenario candidates. Second, lifetime non-overlap does not disprove causality: an ingress can complete before asynchronous downstream work begins, so temporal evidence never eliminates a candidate in this change. Repository inspection shows canonical operation time offsets carry no documented recording-global timeline guarantee, so any stronger temporal rule would be unsound.

Correlation selection must also stay provider-neutral. Trace context (OpenTelemetry, Datadog, AWS X-Ray, B3) is high-quality relationship evidence when present, but Chronicle must correlate without any tracing SDK installed, and provider SDK types or adapters must never enter the resolver's domain boundary. Adapters translate external data into Chronicle-owned `CorrelationEvidence`; the resolver consumes only that evidence and never changes.

## What Changes

- Add a deterministic, ambiguity-safe two-stage resolver to the existing canonical correlation domain (`chronicle-canonical`). Stage A resolves scenario ownership (`Resolved` / `Ambiguous` / `Uncorrelated`) from positively supported, candidate-specific relationships; Stage B separately constructs selected direct causal-parent edges for already-resolved operations when exactly one sufficient parent exists.
- Define candidate-relative relational predicates for every evidence kind the resolver uses: what sits on the child, what sits on the candidate, what counts as a match, what counts as a contradiction, and whether the relation can support scenario ownership, direct parenthood, or neither. Trace identity equality supports scenario-level relationships only; explicit parent-span matches support direct parenthood. The two are never conflated.
- Restrict temporal evidence to contextual/supporting use. Lifetime overlap selects nothing; lifetime non-overlap eliminates nothing. No temporal elimination exists in this change because current canonical offset semantics cannot prove the required timeline comparability; introducing a safe temporal contradiction requires a future change with explicit timeline guarantees.
- Replace heuristic confidence with semantic confidence: `Exact` for directly bound relations, `Strong` for purely transitively inherited ownership through resolved members; `Inferred` remains valid only for externally supplied graphs. No evidence counting, weighting, or scores.
- Derive `ScenarioId` deterministically as `scenario-id-v1`: SHA-256 over a fixed domain separator plus the recording UUID bytes plus the root operation UUID bytes, truncated to 128 bits with RFC 9562 version-8/variant bits. Identity excludes epoch/session scope, so regrouping the same identified operations across publications preserves scenario identity. Stability is promised only within one recording lineage's persisted operation identities — independent re-canonicalization regenerates random `OperationId`s, so cross-republication stability is explicitly deferred to a future identity change.
- Require an explicit provider-neutral `CorrelationContext` (role resolutions and correlation evidence keyed by full operation reference) alongside canonical sessions for ETL composition. Missing context follows explicit join rules instead of silent synthesis.
- Preserve role resolution verbatim, keep only `Known(Ingress)` eligible as scenario roots, keep connection/socket/stream identities as non-identity evidence, and keep ambiguity and uncorrelated outcomes fully represented.
- Keep the resolver on-demand only: ETL gains a composition helper containing zero selection semantics; capture/WAL/publication behavior stays unchanged.
- Keep the resolver in-memory only. No persisted correlation artifact, no field added to Capture Event v1 / Canonical Session v1 / WAL v1, no new crate, no provider SDK dependency, no `EventId`.
- Explicitly defer production of Chronicle-native correlation evidence to `derive-native-correlation-evidence`. Shipping this resolver alone does not make ordinary captured traffic auto-correlate without tracing: today nothing populates the relational evidence the resolver consumes except tests and outer adapters.

## Capabilities

### New Capabilities

- `correlation-resolver`: deterministic, provider-neutral, ambiguity-safe selection of scenario ownership and selected causal-parent structure for canonical operations from Chronicle-owned roles and relationship evidence, producing valid `CorrelationGraph` output with preserved ambiguity and uncorrelated outcomes.

### Modified Capabilities

None. The foundation's algorithm-neutrality requirement already names this resolver as a separate capability; its validation semantics are unchanged and remain authoritative for resolver output.

## Compatibility and Impact

No frozen contract changes. The resolver operates on non-v1 canonical-domain values at runtime and persists nothing. Persisted correlation/scenario artifacts remain unauthorized until a dedicated change (for example `persist-correlation-scenario-artifacts`) selects a separately versioned sidecar/new artifact or an explicit 0.2 compatibility/migration path.

No new crate: correlation semantics stay in `chronicle-canonical` (which gains the workspace-standard `sha2` dependency for scenario-id derivation — an ordinary hashing crate, not a provider package); composition belongs to `chronicle-etl` under its existing dependency edges; architecture policy (`validation/architecture.toml`) needs no changes.

## Out of Scope and Future Changes

- `derive-native-correlation-evidence`: deriving provider-neutral correlation evidence from Chronicle-controlled sources (application/process/task lineage, socket/connection generations, protocol stream relationships, capture/session reconstruction facts). Without it, real recordings supply no relational evidence and the resolver correctly yields mostly `Uncorrelated` outcomes.
- OpenTelemetry/provider SDK integration, provider-specific adapters, trace-context plugin installation, registries, discovery, loading runtimes, and CLI plugin management.
- Persisted correlation/scenario artifacts (`persist-correlation-scenario-artifacts`).
- Cross-republication scenario-identity stability beyond persisted operation identities (future identity/versioning change).
- Safe temporal-contradiction elimination backed by explicit timeline guarantees (future timeline-semantics change).
- Test-case generation, assertions, replay v2, dependency mocking, scenario export formats, UI/website work.
- Probabilistic or ML-based correlation, weighted scoring, distributed multi-host correlation.
- Capture-schema `EventId` and modifications to frozen v1 artifacts.
- Scenario inspect/UX commands and scenario replay.
