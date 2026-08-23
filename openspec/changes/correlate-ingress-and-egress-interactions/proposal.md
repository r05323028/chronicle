## Why

The `correlation-domain-model` capability defines Chronicle-owned interaction roles, scoped canonical operation references, `CorrelationGraph` ownership, and validation for supplied resolutions. It deliberately does not compute those resolutions: today every scenario assignment must be supplied by hand, so Chronicle cannot yet reconstruct the application-level scenarios it exists to produce.

Naive correlation is unsafe under concurrent ingress. When several ingress requests overlap in time — the normal case for any server — an egress interaction overlaps many candidate ingresses, and choosing the oldest, newest, closest, or currently active ingress is a guess that silently misattributes behavior. Wrong scenario ownership produces confidently wrong regression tests, which is worse than no test.

Two further safety properties shape this design. First, absence of disproof is not support: an egress with no candidate-specific evidence overlapping three ingresses must stay `Uncorrelated`, not become ambiguous across all three. Only positively evidenced, candidate-specific relationships may create supported scenario candidates. Second, lifetime non-overlap does not disprove causality: an ingress can complete before asynchronous downstream work begins, so temporal evidence never eliminates a candidate in this change. Repository inspection shows canonical operation time offsets carry no documented recording-global timeline guarantee, so any stronger temporal rule would be unsound.

Correlation selection must also stay provider-neutral. Trace context (OpenTelemetry, Datadog, AWS X-Ray, B3) is high-quality relationship evidence when present, but Chronicle must correlate without any tracing SDK installed, and provider SDK types or adapters must never enter the resolver's domain boundary. Adapters translate external data into Chronicle-owned `CorrelationEvidence`; the resolver consumes only that evidence and never changes.

## What Changes

- Add a deterministic, ambiguity-safe three-phase resolver to the existing canonical correlation domain (`chronicle-canonical`). Phase A1 propagates monotonically growing scenario-support sets to a fixed point without finalizing outcomes; Phase A2 materializes `Resolved` / `Ambiguous` / `Uncorrelated` only after support closure so causal-chain discovery depth can never change results; Phase B selects direct causal-parent edges solely from final ownership, constructing the complete provisional parent graph globally and refusing unsafe selections (including causal-parent cycles) deterministically instead of relying on validator rejection.
- Keep role classification and scenario correlation on separate evidence channels: evidence nested inside `InteractionRoleResolution` validates and preserves role state only and is never indexed for correlation; only the explicit per-operation correlation-evidence collection feeds relationship indexes, support propagation, and parent selection. `ScenarioRoot` input reservation scans both channels because caller-supplied synthetic root claims are invalid anywhere in caller-controlled input.
- Define candidate-relative relational predicates with exact comparison semantics: `SharedTraceIdentity` (same provider+trace identity) supports scenario-level ownership; explicit parent-span matches support direct parenthood and transitive inheritance; shared spans block only direct-parent sufficiency. This revision defines no Chronicle-native positive ownership predicate beyond resolver-generated `ScenarioRoot`: until `derive-native-correlation-evidence` defines and supplies such predicates, non-root operations without supported trace relationships normally remain `Uncorrelated`. That is a capability boundary, not a provider-neutrality limitation.
- Restrict temporal evidence to contextual/supporting use. Lifetime overlap selects nothing; lifetime non-overlap eliminates nothing.
- Establish scenario roots explicitly: creating a scenario emits Chronicle-owned `ScenarioRoot` correlation evidence so the root's own resolution satisfies foundation validation with role classification untouched. Roots are pinned to their own scenario during support propagation, and every caller-controlled evidence path rejects synthetic `ScenarioRoot` items with typed errors. Task and process lineage equality is contextual-only until a future change proves causal-execution identity semantics.
- Derive confidence from final support proofs, and retain witnesses consistent with the materialized confidence: root/direct proofs keep `Exact` witnesses (`ScenarioRoot` / direct trace identity), purely transitive proofs keep `Strong` transitive-span witnesses, and ambiguous candidates keep per-candidate witnesses matching why each scenario is supported. Ownership witnesses live in the correlation resolution; direct-parent witnesses live on the selected edge — separate facts, separate output provenance.
- Derive `ScenarioId` deterministically as `scenario-id-v1`: SHA-256 over a fixed versioned domain separator plus the complete authoritative root scope — recording, owner-epoch, session, and operation UUID bytes in canonical order — truncated to 128 bits with RFC 9562 version-8/variant bits. The derivation uses the full scoped `CanonicalOperationRef` because the foundation forbids assuming bare `OperationId` uniqueness outside its owning session; equal operation IDs under different sessions therefore yield distinct scenario identities. Scenario identity is stable across resolver restarts, retries, replay over the same persisted canonical artifacts, worker scheduling, input iteration order, and map/hash order. It is NOT currently guaranteed across canonical-session regrouping that changes the authoritative root reference, republication into a different owning session/epoch, independent re-canonicalization, or regenerated `OperationId`s; a durable logical-operation identity remains a dedicated future change.
- Preserve role resolution verbatim, keep only `Known(Ingress)` eligible as scenario roots, keep connection/socket/stream identities as non-identity evidence, and keep ambiguity and uncorrelated outcomes fully represented.
- Keep the resolver on-demand only: ETL gains a composition helper containing zero selection semantics; capture/WAL/publication behavior stays unchanged.
- Keep the resolver in-memory only. No persisted correlation artifact, no field added to Capture Event v1 / Canonical Session v1 / WAL v1, no new crate, no provider SDK dependency, no `EventId`.
- Explicitly defer production of Chronicle-native correlation evidence to `derive-native-correlation-evidence`, which owns logical task generation identity, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage, and promotion of safe native predicates through specification changes. No equality-based native correlation is pre-authorized here.

## Capabilities

### New Capabilities

- `correlation-resolver`: deterministic, provider-neutral, ambiguity-safe selection of scenario ownership and selected causal-parent structure for canonical operations from Chronicle-owned roles and relationship evidence, producing valid `CorrelationGraph` output with preserved ambiguity and uncorrelated outcomes.

### Modified Capabilities

None. The foundation's algorithm-neutrality requirement already names this resolver as a separate capability; its validation semantics are unchanged and remain authoritative for resolver output.

## Compatibility and Impact

No frozen contract changes. The resolver operates on non-v1 canonical-domain values at runtime and persists nothing. Persisted correlation/scenario artifacts remain unauthorized until a dedicated change (for example `persist-correlation-scenario-artifacts`) selects a separately versioned sidecar/new artifact or an explicit 0.2 compatibility/migration path.

No new crate: correlation semantics stay in `chronicle-canonical` (which gains the workspace-standard `sha2` dependency for scenario-id derivation — an ordinary hashing crate, not a provider package); composition belongs to `chronicle-etl` under its existing dependency edges; architecture policy (`validation/architecture.toml`) needs no changes.

## Out of Scope and Future Changes

- `derive-native-correlation-evidence`: deriving provider-neutral correlation evidence from Chronicle-controlled sources and defining native relational-semantics contracts (logical task generation, causal execution lineage, process/task inheritance, socket/connection generations, protocol-stream lineage) plus promotion of safe native predicates. Without it, real recordings supply no relational evidence and the resolver correctly yields mostly `Uncorrelated` outcomes.
- OpenTelemetry/provider SDK integration, provider-specific adapters, trace-context plugin installation, registries, discovery, loading runtimes, and CLI plugin management.
- Persisted correlation/scenario artifacts (`persist-correlation-scenario-artifacts`).
- Cross-republication scenario-identity stability beyond persisted operation identities (future identity/versioning change).
- Safe temporal-contradiction elimination backed by explicit timeline guarantees (future timeline-semantics change).
- Test-case generation, assertions, replay v2, dependency mocking, scenario export formats, UI/website work.
- Probabilistic or ML-based correlation, weighted scoring, distributed multi-host correlation.
- Capture-schema `EventId` and modifications to frozen v1 artifacts.
- Scenario inspect/UX commands and scenario replay.
