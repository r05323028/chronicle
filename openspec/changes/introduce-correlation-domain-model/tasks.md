# Future implementation plan

This task list describes the implementation of the foundation after this planning revision. It is deliberately algorithm-neutral. No task may derive Scenario A/B/C from raw interleaved traffic; tests use supplied roles, role classifications, and supplied `CorrelationResolution` values. Resolver behavior belongs to a future change.

## 1. Identity and reference model

- [ ] 1.1 Preserve `CanonicalOperation` and `OperationId` as the sole logical interaction concept/identity; add no `InteractionId` or parallel interaction type.
- [ ] 1.2 Define `CanonicalOperationRef { recording_id, owner_epoch_id, session_id, operation_id }`; validate exact session/epoch/recording lineage and reject bare `OperationId` lookup outside its owning session.
- [ ] 1.3 Define `ScenarioId` as scenario identity owned by a recording-scoped correlation aggregate; keep recording, epoch, session, connection, stream, process, thread, socket, and trace identities distinct.
- [ ] 1.4 Define cross-epoch ownership rules: terminal canonical operation uses authoritative owner session/epoch, contributing epoch ranges remain provenance, and continuation-only evidence is not a duplicate operation.
- [ ] 1.5 Explicitly omit `EventId` from this foundation. Do not add it to `chronicle-common`, Capture Event v1, Canonical Session v1, or correlation APIs; track any future deterministic/event-contract decision separately.

## 2. Interaction role classification

- [ ] 2.1 Define Chronicle-owned `InteractionRole::{Ingress,Egress}` as the two known application-relative roles.
- [ ] 2.2 Define separate `InteractionRoleResolution` state with evidence-bearing `Known { role, evidence }`, `Unknown { evidence }`, and `Ambiguous { candidates: [{ role, evidence }] }` forms; prose may use `Known(Ingress)`/`Known(Egress)` shorthand, but do not add uncertainty variants to `InteractionRole` itself.
- [ ] 2.3 Normalize validated passive application-server evidence to `Known(Ingress)` and validated active outbound HTTP/database/cache evidence to `Known(Egress)`.
- [ ] 2.4 Preserve missing ownership as `Unknown` and conflicting ownership as `Ambiguous`; retain candidate-specific supporting evidence for every ambiguous role, never flatten it into one shared list, and never choose a default through timing, PID, connection ownership, socket role, or ordering.
- [ ] 2.5 Validate that only `Known(Ingress)` can be `Scenario.root`; `Known(Egress)`, `Unknown`, and `Ambiguous` role states cannot be roots. Preserve known egress as a possible child where supplied correlation permits.
- [ ] 2.6 Keep role classification independent from scenario-correlation resolution; preserve combinations including `Known(Ingress)+Resolved`, `Known(Egress)+Ambiguous`, `Unknown+Uncorrelated`, and `Ambiguous+Ambiguous`.
- [ ] 2.7 Keep `ClientToServer`/`ServerToClient`, `SocketRole`, PID, TID, worker, socket, connection, stream, epoch, session, and timestamps as evidence only; direction must not solve an otherwise unknown role.

- [ ] 2.8 Validate ambiguous role candidates as at least two distinct viable roles with candidate-specific evidence; reject empty, single-candidate, duplicate-role, and unsupported-candidate sets. Keep validation extensible beyond the current `Ingress`/`Egress` pair.

## 3. Correlation aggregate

- [ ] 3.1 Define recording-scoped `CorrelationGraph` aggregate with role-resolution state keyed by `CanonicalOperationRef`, `Scenario` child entities, correlation-resolution index, and selected causal edges.
- [ ] 3.2 Ensure every admitted operation retains one role-resolution entry and one `CorrelationResolution` entry; unknown/ambiguous role or scenario ownership never removes the operation from graph discovery.
- [ ] 3.3 Define `Resolved`, `Ambiguous` with candidate-specific evidence, and `Uncorrelated` values; ambiguous candidates stay outside selected scenario membership.
- [ ] 3.4 Define selected edge validation: full scoped endpoints, same selected scenario, no self-edge, no cycles, at most one selected parent, and no edges from ambiguous/unresolved correlation candidates.

## 4. Evidence and compatibility boundary

- [ ] 4.1 Define Chronicle-owned, tagged, provenance-bearing evidence for trace, protocol ownership, execution/task lineage, process/thread generation, connection/socket generation, protocol streams, temporal/lifetime, and bounded namespaced custom data.
- [ ] 4.2 Keep trace context optional and provider-neutral; preserve external trace values as opaque evidence and keep tracing SDK types outside core/domain APIs.
- [ ] 4.3 Encode role and correlation provenance so temporal-only evidence cannot validate a selected correlation resolution; infrastructure identity remains evidence and never scenario identity.
- [ ] 4.4 Keep completeness/replayability separate from correlation; scenario membership cannot make incomplete, lost, malformed, unsupported, or otherwise non-replayable operations replayable.
- [ ] 4.5 Before any persisted integration, choose a separately versioned correlation sidecar/new artifact or submit a dedicated 0.2 compatibility/migration change. Do not add fields or reader fallbacks to Capture Event v1 or Canonical Session v1.

## 5. Domain-only validation

- [ ] 5.1 Test validated passive HTTP application-server evidence -> `Known(Ingress)` and root eligibility.
- [ ] 5.2 Test validated active outbound HTTP evidence -> `Known(Egress)` and root rejection.
- [ ] 5.3 Test validated active PostgreSQL/MySQL/Redis dependency evidence -> `Known(Egress)`.
- [ ] 5.4 Test insufficient application-relative ownership -> `Unknown`, preserved operation/evidence, and root rejection.
- [ ] 5.5 Test conflicting ingress/egress evidence -> `Ambiguous` candidate records with independent ingress/egress evidence and root rejection; prove candidate evidence is not shared or swapped.
- [ ] 5.6 Test bidirectional payload stability and prove `ClientToServer`/`ServerToClient` cannot turn an otherwise unknown role into a known role; test socket role alone is not scenario identity.
- [ ] 5.7 Test role/correlation independence with `Known(Ingress)+Resolved`, `Known(Egress)+Ambiguous`, `Unknown+Uncorrelated`, and candidate-specific `Ambiguous+Ambiguous` supplied outcomes; preserve role ambiguity when correlation is resolved and never let scenario evidence resolve role.
- [ ] 5.8 Test a parent ingress that begins in epoch N and completes in N+1, successor-session child egress, stable ScenarioId, and cross-session/cross-epoch causal references.
- [ ] 5.9 Test identical `OperationId` values under different scoped recording/session/epoch contexts; prove full references cannot collide through unscoped lookup.
- [ ] 5.10 Test supplied pre-resolved ownership (`op1 -> Scenario A`, `op2 -> Scenario B`, `op3 -> Scenario C`) and verify memberships without deriving ownership from raw interleaved events.
- [ ] 5.11 Test ambiguous scenario A/B candidates retain candidate-specific correlation evidence and remain outside memberships/edges; test uncorrelated operations remain in the aggregate without synthetic owners.
- [ ] 5.12 Test self-edge, cycle, multiple selected parents, cross-scenario edge, missing endpoint, duplicate membership, and invalid root rejection.
- [ ] 5.13 Test infrastructure identities and temporal overlap remain evidence only; temporal-only supplied correlation resolutions fail model validation.
- [ ] 5.14 Test operation completeness/replayability remains unchanged when a supplied resolution assigns an incomplete operation to a scenario.
- [ ] 5.15 Keep these tests at the lowest conclusive domain/integration layer. Do not add resolver, scoring, runtime-lineage, or privileged capture tests to this foundation.

- [ ] 5.16 Test role-ambiguity explainability: Ingress candidate retains passive application-server ownership evidence and Egress candidate retains active outbound ownership evidence; inspection can answer why each candidate remains viable.
- [ ] 5.17 Test duplicate candidates (`Ingress, Ingress`), a single candidate (`Ingress`), and an empty candidate list are rejected; when only one role is viable with sufficient evidence, require `Known` instead.
- [ ] 5.18 Test current-domain ambiguous roles contain both distinct `Ingress` and `Egress` candidates, while validation remains at-least-two/distinct rather than hard-coded to exactly two for future role values.
- [ ] 5.19 Test an ambiguous role containing an `Ingress` candidate remains non-root until resolution becomes `Known(Ingress)`; test resolved scenario correlation does not rewrite the role.

## 6. Architecture validation and documentation

- [ ] 6.1 Keep correlation ownership in `chronicle-canonical`, neutral primitives in `chronicle-common`, and the existing crate graph unchanged; do not add a standalone correlation crate.
- [ ] 6.2 Extend the existing architecture validation mechanism (`validation/architecture.toml` and `scripts/validation.py architecture`) with a core/domain external tracing-SDK dependency denylist. Do not claim this external package guard already exists.
- [ ] 6.3 Inspect actual Cargo dependency/package naming and implement the smallest maintainable direct-dependency policy for provider SDKs/integrations (for example OpenTelemetry SDK/bridge, Datadog, AWS X-Ray, or B3-specific packages) without blindly banning generic logging/tracing facades.
- [ ] 6.4 Add standard-library temporary-workspace/manifest tests proving forbidden provider SDK dependencies fail, renamed/target-specific direct dependencies remain covered, and approved outer adapters can translate into Chronicle evidence.
- [ ] 6.5 Do not introduce a parallel validation script; reuse the existing architecture policy/check and retain current workspace-edge and semantic-boundary checks.
- [ ] 6.6 Update `docs/architecture/crate-boundaries.md`, `docs/architecture/overview.md`, and `docs/canonical-model.md` after implementation with role classification/correlation ownership, epoch-scoped references, unresolved outcomes, and v1 compatibility boundaries.
- [ ] 6.7 Update `AGENTS.md` only with concise durable invariants: Chronicle owns correlation; trace is optional enrichment; role differs from byte direction; infrastructure identity is evidence; ambiguity remains ambiguity; replayability is separate.
- [ ] 6.8 Review user-facing documentation impact. This foundation adds no CLI or website behavior; later scenario commands or canonical English changes must update `zh-tw`/`ja` pages and run localization verification.

## 7. Validation and future boundaries

- [ ] 7.1 Run `openspec validate --all --strict --no-interactive` for this change and keep the artifacts internally consistent.
- [ ] 7.2 After production implementation, run repository bounded fast/targeted validation, architecture checks, domain tests, and direct behavioral probes; a build alone is insufficient.
- [ ] 7.3 Verify implementation diff contains no production changes outside approved canonical/common/domain/docs/validation surfaces, no tracing SDK, no new Chronicle crate, no `InteractionId`, no `EventId`, and no v1 contract mutation.
- [ ] 7.4 Run `graphify update .` after production source changes in the implementation change; this planning-only revision changes no production source.
- [ ] 7.5 Keep privileged capture gates out of foundation proof; use them only for future kernel/environment-dependent resolver behavior.

## Future changes

- `correlate-ingress-and-egress-interactions`: deterministic resolver, candidate selection, and concurrent-ingress correlation.
- Pluggable trace evidence providers and runtime lineage capture.
- Versioned persisted correlation/scenario artifact and user-facing scenario commands.
- Event identity/compatibility change, only if raw capture-event identity becomes necessary.
- Replay v2, dependency matching, exports, assertions, and test generation.
