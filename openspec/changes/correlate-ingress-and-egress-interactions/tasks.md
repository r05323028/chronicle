# Implementation tasks

Ordered by dependency. Every task stays inside the approved surfaces: `crates/chronicle-canonical/src/correlation.rs`, a new `crates/chronicle-etl` composition helper, domain/integration tests, and documentation. No production behavior outside these surfaces may change.

## 1. Domain/API adjustments required by the resolver

- [ ] 1.1 Add a resolver input value in `correlation.rs`: one admitted operation's full scoped `CanonicalOperationRef`, its owning `CanonicalOperation` view (lifetimes via `started_at_offset`/`completed_at_offset`, provenance), the supplied `InteractionRoleResolution`, and the supplied `Vec<CorrelationEvidence>`.
- [ ] 1.2 Add deterministic `ScenarioId` derivation from a fixed namespace constant plus the root reference bytes (`recording_id || owner_epoch_id || session_id || operation_id`), using an existing deterministic UUID mechanism available to the workspace or a documented equivalent; document that derivation is stable across restarts and never random.
- [ ] 1.3 Add a resolver error type distinct from graph validation errors for invalid resolver inputs (duplicate references, role-validation failures, references whose recording scope mismatches); resolver output itself must still pass `CorrelationGraph::validate_against_sessions` when sessions are supplied.
- [ ] 1.4 Keep all existing public correlation types and their semantics unchanged; the resolver is additive.

## 2. Candidate-set construction

- [ ] 2.1 Implement deterministic candidate construction: sort inputs by full reference tuple (`recording_id, owner_epoch_id, session_id, operation_id`) before any processing; never iterate caller order or hash order.
- [ ] 2.2 Create one scenario candidate per root-eligible operation (`InteractionRoleResolution::is_root_eligible()`, i.e. `Known(Ingress)`), with its deterministically derived `ScenarioId`; keep scenarios as graph entities even when no egress resolves into them.
- [ ] 2.3 Reject or preserve-with-error operations whose supplied role resolution fails foundation validation; never default a missing role so it can become a candidate owner.

## 3. Deterministic resolver core

- [ ] 3.1 Classify evidence per the normative tiers of the `correlation-resolver` capability: decisive causal-link evidence (`TraceRelationship` whose parent context resolves exactly to the candidate's trace identity), narrowing evidence (temporal disjointness, execution-task lineage mismatch/generation disjointness), supporting/contextual evidence, and never-independent evidence (temporal overlap, PID/TID equality, wire direction, socket role).
- [ ] 3.2 For each non-root operation, build the viable-owner set from scenario roots (and later resolved members for chaining rounds); apply only eliminating evidence to remove impossible candidates.
- [ ] 3.3 Select outcomes: exactly one survivor with decisive support -> `Resolved(Exact)`; unique survivor after non-temporal narrowing with at least two independent supporting dimensions -> `Resolved(Strong)`; multiple survivors -> `Ambiguous { candidates }` with candidate-specific evidence; zero survivors -> `Uncorrelated` with retained evidence. Never emit `Inferred` from this resolver.
- [ ] 3.4 Enforce the temporal rule mechanically: temporal overlap never eliminates nothing and never selects anything; temporal disjointness may eliminate only when both lifetimes come from comparable timelines; cross-session comparisons without comparable offsets are treated as non-eliminating.
- [ ] 3.5 On conflicting decisive evidence pointing at different owners, produce `Ambiguous` with each candidate carrying its own supporting evidence; never pick the first-processed evidence source.

## 4. Ambiguity and uncorrelated handling

- [ ] 4.1 Preserve every ambiguous candidate set with per-candidate evidence via the existing `CorrelationResolution::Ambiguous` shape; ambiguous operations stay outside all scenario membership and selected edges.
- [ ] 4.2 Preserve `Uncorrelated` operations with their evidence; create no synthetic owners, roots, or membership entries.
- [ ] 4.3 Verify output graphs pass foundation validation unchanged (`CorrelationGraph::validate`, and `validate_against_sessions` where session lineage is available) with zero modifications to that validator.

## 5. Causal-edge construction

- [ ] 5.1 Emit selected edges only between operations resolved into the same scenario; derive parent chains from the resolution outcome (root first, then chained members), keeping at most one selected parent per child and no cycles by construction.
- [ ] 5.2 Support nested/chained structure: run bounded fixed-point rounds where an operation's decisive/narrowing evidence points at an already-resolved member rather than the root; attach grandchild edges without changing scenario ownership semantics.
- [ ] 5.3 Never convert ambiguous candidates into selected edges and never make a scenario root a child; cross-session/cross-epoch edges use full scoped references on both endpoints.

## 6. Concurrent-ingress support

- [ ] 6.1 Test three overlapping ingress lifetimes with two egresses where evidence uniquely assigns each egress: two/three independent scenarios resolve correctly regardless of overlap degree.
- [ ] 6.2 Prove timing proximity is inert: construct cases where the temporally closest ingress is evidence-linked to a different egress and verify no test can distinguish ordering heuristics because none exist.
- [ ] 6.3 Test partial overlap ambiguity: one egress viable under two overlapping ingresses yields `Ambiguous { A, B }` while the other resolves.

## 7. Cross-epoch resolution

- [ ] 7.1 Test root published in epoch N session and egress in epoch N+1 session resolving into one scenario through full scoped references.
- [ ] 7.2 Test an operation whose canonical completion owner is the successor epoch participating under its terminal reference, not a predecessor continuation reference.
- [ ] 7.3 Test that identical `OperationId` values under different session scopes never collide during resolver lookup.

## 8. ETL integration boundary

- [ ] 8.1 Add a composition helper in `chronicle-etl` that converts published `CanonicalSession` values of one recording into resolver inputs and returns the `CorrelationGraph`; ETL owns no correlation semantics and adds no dependency edges beyond the existing `chronicle-etl -> chronicle-canonical` allowlist entry.
- [ ] 8.2 Do not wire the resolver into the default publication/checkpoint hot path; invocation stays explicit/on-demand so capture-path overhead remains zero unless requested.
- [ ] 8.3 Test the helper end to end with protocol-builtins fixtures at the lowest conclusive integration layer; no privileged capture tests are required by this change.

## 9. Deterministic restart/order tests

- [ ] 9.1 Test identical inputs across two resolver invocations (simulated restart/retry/replay) produce byte-identical graphs including derived `ScenarioId`s.
- [ ] 9.2 Test permuted input iteration orders produce semantically identical graphs (identical after canonical re-sorting of representation collections).
- [ ] 9.3 Test results do not depend on map/hash iteration order by construction review plus a shuffled-order property test over a multi-scenario fixture.

## 10. Architecture validation

- [ ] 10.1 Verify `validation/architecture.toml` requires no changes (no new crate, no new edge); run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture` and record the result.
- [ ] 10.2 Verify no provider SDK package enters any touched crate's dependencies and no provider type name appears in canonical APIs.

## 11. Documentation updates

- [ ] 11.1 Update `docs/architecture/crate-boundaries.md` (`chronicle-canonical` and `chronicle-etl` ownership sections) with resolver placement and the ETL composition-only boundary.
- [ ] 11.2 Update `docs/canonical-model.md` with resolver inputs, evidence tiers, outcome selection rules, and deterministic scenario identity derivation.
- [ ] 11.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; keep additions to one paragraph if needed.
- [ ] 11.4 Add concise durable invariants to `AGENTS.md` correlation section: temporal overlap never selects a scenario owner; resolver lives in `chronicle-canonical`; persistence remains deferred.
- [ ] 11.5 Review user-facing documentation impact; this change adds no CLI/website behavior, so no `zh-tw`/`ja` localization updates are triggered — record that determination.
- [ ] 11.6 Run `graphify update .` after production source changes land.

## 12. Full validation

- [ ] 12.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent.
- [ ] 12.2 Run `./scripts/validate.sh fast` and targeted changed-path validation; fix findings before completion.
- [ ] 12.3 Verify the final diff contains no frozen v1 contract mutation, no persisted correlation artifact, no new crate, no `EventId`, no provider SDK dependency, and no scoring/heuristic weighting.
- [ ] 12.4 Run direct behavioral probes covering acceptance scenarios 1–14 of this capability and retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `persist-correlation-scenario-artifacts`: versioned sidecar/new artifact decision for persisted graphs.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`: adapters translating external trace data into `CorrelationEvidence` and their installation UX.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`) compatibility change, only if a future feature proves it necessary.
