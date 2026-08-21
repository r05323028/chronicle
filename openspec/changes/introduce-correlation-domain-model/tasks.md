# Future implementation plan

This task list describes the implementation of the foundation after this planning revision. It is deliberately algorithm-neutral. No task may derive Scenario A/B/C from raw interleaved traffic; tests use supplied roles and supplied `CorrelationResolution` values. Resolver behavior belongs to a future change.

## 1. Identity and reference model

- [ ] 1.1 Preserve `CanonicalOperation` and `OperationId` as the sole logical interaction concept/identity; add no `InteractionId` or parallel interaction type.
- [ ] 1.2 Define `CanonicalOperationRef { recording_id, owner_epoch_id, session_id, operation_id }`; validate exact session/epoch/recording lineage and reject bare `OperationId` lookup outside its owning session.
- [ ] 1.3 Define `ScenarioId` as scenario identity owned by a recording-scoped correlation aggregate; keep recording, epoch, session, connection, stream, process, thread, socket, and trace identities distinct.
- [ ] 1.4 Define cross-epoch ownership rules: terminal canonical operation uses authoritative owner session/epoch, contributing epoch ranges remain provenance, and continuation-only evidence is not a duplicate operation.
- [ ] 1.5 Explicitly omit `EventId` from this foundation. Do not add it to `chronicle-common`, Capture Event v1, Canonical Session v1, or correlation APIs; track any future deterministic/event-contract decision separately.

## 2. Interaction semantics

- [ ] 2.1 Define Chronicle-owned `InteractionRole::{Ingress,Egress}` in the canonical correlation domain and attach it to, or deterministically derive it for, each correlation input.
- [ ] 2.2 Define application-relative role derivation from validated ownership evidence: passive server interaction is ingress; active client/dependency interaction is egress; protocol and byte-flow direction are not role semantics.
- [ ] 2.3 Validate that only ingress interactions can be scenario roots; egress interactions remain valid children or unresolved interactions but never roots.
- [ ] 2.4 Preserve active/passive socket, direction, connection, stream, PID, TID, worker, and timestamp values as evidence only; conflicting role evidence must not default to ingress.

## 3. Correlation aggregate

- [ ] 3.1 Define recording-scoped `CorrelationGraph` aggregate with role assignments, `Scenario` child entities, resolution index keyed by `CanonicalOperationRef`, and selected causal edges.
- [ ] 3.2 Define `Scenario` root/membership invariants and make aggregate resolution state authoritative; every admitted operation retains one resolved, ambiguous, or uncorrelated outcome.
- [ ] 3.3 Define `Resolved`, `Ambiguous` with candidate-specific evidence, and `Uncorrelated` values; ambiguous candidates stay outside selected scenario membership.
- [ ] 3.4 Define selected edge validation: full scoped endpoints, same selected scenario, no self-edge, no cycles, at most one selected parent, and no edges from ambiguous/unresolved candidates.

## 4. Evidence and compatibility boundary

- [ ] 4.1 Define Chronicle-owned, tagged, provenance-bearing evidence for trace, protocol ownership, execution/task lineage, process/thread generation, connection/socket generation, protocol streams, temporal/lifetime, and bounded namespaced custom data.
- [ ] 4.2 Keep trace context optional and provider-neutral; preserve external trace values as opaque evidence and keep tracing SDK types outside core/domain APIs.
- [ ] 4.3 Encode resolution provenance so temporal-only evidence cannot validate a selected resolution; infrastructure identity remains evidence and never scenario identity.
- [ ] 4.4 Keep completeness/replayability separate from correlation; scenario membership cannot make incomplete, lost, malformed, unsupported, or otherwise non-replayable operations replayable.
- [ ] 4.5 Before any persisted integration, choose a separately versioned correlation sidecar/new artifact or submit a dedicated 0.2 compatibility/migration change. Do not add fields or reader fallbacks to Capture Event v1 or Canonical Session v1.

## 5. Domain-only validation

- [ ] 5.1 Test valid ingress root and rejection of egress HTTP/database/cache roots.
- [ ] 5.2 Test passive HTTP ingress, active HTTP egress, active database/cache egress, bidirectional payload stability, and rejection of byte-flow `Direction` as role.
- [ ] 5.3 Test a parent ingress that begins in epoch N and completes in N+1, successor-session child egress, stable ScenarioId, and cross-session/cross-epoch causal references.
- [ ] 5.4 Test identical `OperationId` values under different scoped recording/session/epoch contexts; prove full references cannot collide through unscoped lookup.
- [ ] 5.5 Test supplied pre-resolved ownership (`op1 -> Scenario A`, `op2 -> Scenario B`, `op3 -> Scenario C`) and verify memberships without deriving ownership from raw interleaved events.
- [ ] 5.6 Test ambiguous A/B candidates retain candidate-specific evidence and remain outside memberships/edges; test uncorrelated operations remain in the aggregate without synthetic owners.
- [ ] 5.7 Test self-edge, cycle, multiple selected parents, cross-scenario edge, missing endpoint, duplicate membership, and invalid root rejection.
- [ ] 5.8 Test infrastructure identities and temporal overlap remain evidence only; temporal-only supplied resolutions fail model validation.
- [ ] 5.9 Test operation completeness/replayability remains unchanged when a supplied resolution assigns an incomplete operation to a scenario.
- [ ] 5.10 Keep these tests at the lowest conclusive domain/integration layer. Do not add resolver, scoring, runtime-lineage, or privileged capture tests to this foundation.

## 6. Architecture and documentation

- [ ] 6.1 Keep correlation ownership in `chronicle-canonical`, neutral primitives in `chronicle-common`, and the existing crate graph unchanged; do not add a standalone correlation crate.
- [ ] 6.2 Preserve the existing architecture validation guard for tracing SDK/provider dependencies and extend only its existing policy/check if needed; do not add SDK dependencies to test the guard.
- [ ] 6.3 Update `docs/architecture/crate-boundaries.md`, `docs/architecture/overview.md`, and `docs/canonical-model.md` after implementation with canonical role/correlation ownership, epoch-scoped references, unresolved outcomes, and v1 compatibility boundaries.
- [ ] 6.4 Update `AGENTS.md` only with concise durable invariants: Chronicle owns correlation; trace is optional enrichment; roles differ from byte direction; infrastructure identity is evidence; ambiguity remains ambiguity; replayability is separate.
- [ ] 6.5 Review user-facing documentation impact. This foundation adds no CLI or website behavior; later scenario commands or canonical English changes must update `zh-tw`/`ja` pages and run localization verification.

## 7. Validation and future boundaries

- [ ] 7.1 Run `openspec validate --all --strict --no-interactive` for this change and keep the artifacts internally consistent.
- [ ] 7.2 After production implementation, run repository bounded fast/targeted validation, architecture checks, domain tests, and direct behavioral probes; a build alone is insufficient.
- [ ] 7.3 Verify implementation diff contains no production changes outside approved canonical/common/domain/docs/validation surfaces, no tracing SDK, no new Chronicle crate, no `InteractionId`, and no v1 contract mutation.
- [ ] 7.4 Run `graphify update .` after production source changes in the implementation change; this planning-only revision changes no production source.
- [ ] 7.5 Keep privileged capture gates out of foundation proof; use them only for future kernel/environment-dependent resolver behavior.

## Future changes

- `correlate-ingress-and-egress-interactions`: deterministic resolver, candidate selection, and concurrent-ingress correlation.
- Pluggable trace evidence providers and runtime lineage capture.
- Versioned persisted correlation/scenario artifact and user-facing scenario commands.
- Event identity/compatibility change, only if raw capture-event identity becomes necessary.
- Replay v2, dependency matching, exports, assertions, and test generation.
