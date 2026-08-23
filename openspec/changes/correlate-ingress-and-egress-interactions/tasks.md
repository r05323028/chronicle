# Implementation tasks

Ordered by dependency. Approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, domain/integration tests, documentation. No production behavior outside these surfaces may change.

## 1. Full-reference scenario-id-v1 and known-answer identity tests

- [ ] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit.
- [ ] 1.2 Implement `scenario_id_v1(root: CanonicalOperationRef) -> ScenarioId`: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII (36 bytes) || recording_be || owner_epoch_be || session_be || operation_be)` via `Uuid::as_bytes()` in ref declaration order; first 16 digest bytes as UUID octets; version 8 + RFC 4122 variant bit fixups. Bare/partial-scope variants forbidden.
- [ ] 1.3 Known-answer tests hashing the actual separator bytes (never a hard-coded length); fixed UUID quadruples -> exact expected outputs; bit assertions.
- [ ] 1.4 Duplicate-`OperationId` test: same operation ID under two owner-epoch/session scopes -> distinct refs -> distinct scenario ids.
- [ ] 1.5 Document the guarantee boundary: stable across restarts/retries/replay-over-same-artifacts/scheduling/iteration orders; NOT stable across regrouping changing the authoritative reference, republication into another session/epoch, independent re-canonicalization, regenerated IDs.

## 2. ScenarioRoot evidence kind, input reservation, and root pinning

- [ ] 2.1 Add additive non-frozen `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`; non-temporal; document meaning (resolver establishes this known ingress as root of the scenario derived from its full reference).
- [ ] 2.2 Input reservation: pre-resolution validation rejects caller-supplied `ScenarioRoot` in EVERY caller-controlled container — context evidence maps, `Known.evidence`, `Unknown.evidence`, `Ambiguous.candidates[*].evidence` — as typed input errors. Tests cover all four injection paths.
- [ ] 2.3 Root construction: derive id from full ref -> create `Scenario { id, root, members: [root] }` -> pin root support to {id} -> resolution `Resolved { Exact, [ScenarioRoot] }`. Root-only graphs pass `validate_against_sessions(supplied)` unchanged.
- [ ] 2.4 Root-pinning tests: two roots sharing one trace stay two scenarios; shared span identity between roots does not merge them; contextual overlap/task/process agreement cannot pull a root into another scenario; role state byte-identical after establishment.

## 3. Immutable full-evidence relation indexes

- [ ] 3.1 Build ordered indexes (`BTreeMap`) over the COMPLETE validated input evidence: `(provider, trace_id)`, `(provider, trace_id, span_id)`, full reference; inputs sorted by full reference tuple first. Indexes immutable during resolution.
- [ ] 3.2 Implement predicates as pure comparisons: `SharedTraceIdentity` (equal non-empty provider+trace_id), `ExplicitParentSpan` (child parent_span_id == candidate span_id under same provider+trace), shared-span detection. Everything else contextual-only (`ExecutionTaskLineage`, process/thread/socket/connection/stream/protocol/direction/custom/temporal).
- [ ] 3.3 Unit tests: match, missing-field non-match, cross-provider non-match, shared-span behavior (blocks Phase B sufficiency only), same-task-string-alone adds nothing.

## 4. Stage A1: sparse scenario-support-set fixpoint

- [ ] 4.1 Represent support sparsely as `(operation, ScenarioId)` memberships created only by discovered positive relationships; never allocate an operation-times-scenario matrix.
- [ ] 4.2 Snapshot rounds: initialize roots pinned + direct relations from round-0 evidence; each round evaluates against previous-round snapshot; union newly discoverable scenarios (direct SharedTraceIdentity; transitive ExplicitParentSpan through candidates supported in snapshot). Support only grows within a run.
- [ ] 4.3 Terminate when a round adds no membership; productive rounds each add >=1 membership (formal bound N x S). No recursion anywhere.

## 5. Stage A2: materialize outcomes only after closure

- [ ] 5.1 After termination: empty support -> `Uncorrelated { retained }`; one scenario -> `Resolved { scenario, confidence }`; multiple -> `Ambiguous { candidates }`, one candidate per supported scenario keyed by ScenarioId with per-candidate witnesses. No final values assigned during propagation.
- [ ] 5.2 Chain-depth ambiguity tests: Z reached by A via short chain and B via long chain materializes `Ambiguous(A, B)` regardless of which path becomes discoverable first; reversed operation/session/chain orders produce identical support sets, outcomes, ids, witnesses, edges.
- [ ] 5.3 Aggregation test: several members of one scenario matching the same child yield ONE scenario-keyed support entry with multiple internal witness paths.

## 6. Confidence from final support proofs

- [ ] 6.1 Compute confidence post-closure: `Exact` when final support holds a direct scenario-level relation (`SharedTraceIdentity`) or pinned `ScenarioRoot`; `Strong` when unique support exists solely through transitive explicit parent-span paths; multiple scenarios -> ambiguous, confidence not materialized; `Inferred` never emitted.
- [ ] 6.2 Deterministic internal witness selection independent of discovery order; canonical priority ScenarioRoot > ExplicitParentSpan(transitive) > SharedTraceIdentity(direct); direct beats transitive for the same scenario regardless of arrival order.
- [ ] 6.3 Tests: mixed direct+transitive paths -> Exact in every order; purely transitive -> Strong; contextual volume changes nothing.

## 7. Stage B: direct causal-parent selection after closure only

- [ ] 7.1 Run Phase B exclusively on final A2 ownership: evaluate `ExplicitParentSpan` among FINAL members of the owning scenario; emit one edge when one unshared target matches, retaining the predicate witness on the edge.
- [ ] 7.2 No edge for multiple/equally sufficient parents, insufficient/contextual-only relations, cross-scenario proposed parents, or children whose final ownership is Ambiguous/Uncorrelated; intermediate-state edges impossible by construction (edges exist only after closure).
- [ ] 7.3 Stale-edge regression test: child gaining B after earlier A-support ends Ambiguous with zero selected edges; foundation edge validation passes unchanged (tree invariants, root never child, ambiguous never in edges).

## 8. Deterministic minimal-witness retention after semantics

- [ ] 8.1 Retention runs strictly after A2+B over closed support structure; documented constant 64 applied per outcome slot and PER AMBIGUITY CANDIDATE; Resolved keeps priority witness (+edge witness) then canonical contextual fill; Ambiguous keeps >=1 per-candidate witness then canonical fill; Uncorrelated keeps contextual/input items canonically.
- [ ] 8.2 Cap-pressure tests: exceeding the cap leaves support sets, outcomes, and edges identical to uncapped runs — only optional contextual fill differs; wide ambiguity retains every candidate witness without drops or boundedness errors.

## 9. Concurrent-ingress and async cases

- [ ] 9.1 Three mutually overlapping ingresses with uniquely related egresses resolve independently regardless of start order/duration/overlap.
- [ ] 9.2 Timing proximity/recency/openness/order influence nothing; closest-ingress-differs cases.
- [ ] 9.3 Async/post-completion causality: disjoint lifetimes never block support propagation; temporal-only inputs stay uncorrelated; reverse ordering performs no elimination.

## 10. Cross-epoch / scoped-reference handling

- [ ] 10.1 Root owned by epoch N session, egress by epoch N+1 session: one scenario identified from the epoch-N root reference; cross-session edge with full scoped endpoints validates.
- [ ] 10.2 Terminal completion-owner scope participation; continuation provenance never a second subject; equal operation-ID roots under different sessions distinct end to end.

## 11. CorrelationContext and ETL composition API

- [ ] 11.1 Define the correlation context value in `chronicle-canonical`; sessions alone are not valid resolver input; `ScenarioRoot` rejected in all supplied containers (context evidence AND nested role evidence).
- [ ] 11.2 `chronicle-etl` composition helper: verify references via existing lineage resolution, enrich lifetime/provenance views, join by exact reference; zero selection semantics.
- [ ] 11.3 Join-rule tests: missing role entry -> typed error; missing evidence entry -> empty evidence; orphan entry -> typed error; smuggled root claims (all four paths) -> typed errors.
- [ ] 11.4 Default publication/checkpoint path never calls the helper; integration test at lowest conclusive layer with protocol-builtins fixtures and explicitly supplied contexts.

## 12. Determinism tests

- [ ] 12.1 Repeated invocations across restart/retry/replay produce byte-identical graphs including ids, witnesses, edges.
- [ ] 12.2 Permuted and hash-adversarial input orders produce identical results after canonical re-sorting; reversed chains identical (see 5.2).

## 13. Validator contract

- [ ] 13.1 Assert output graphs satisfy structural invariants and pass `validate_against_sessions(supplied_sessions)`; assert context-free `validate()` on non-empty graphs fails with `MissingSessionContext` per existing foundation semantics (expected, tested, not treated as defect).
- [ ] 13.2 Change nothing in foundation validation; expose an additive public structural-validation API ONLY if implementation proves it necessary, without altering `validate()` semantics.

## 14. Architecture/provider dependency checks

- [ ] 14.1 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture --root .`; expect pass, no policy edits beyond inherently allowed external hashing dependency.
- [ ] 14.2 Verify no provider SDK enters touched crates, no new workspace edges, no new crate; `ScenarioRoot` stays inside the non-frozen domain enum without touching frozen contracts.

## 15. Documentation and AGENTS.md updates

- [ ] 15.1 Update `docs/architecture/crate-boundaries.md` (`chronicle-canonical`, `chronicle-etl`): resolver placement, three-phase semantics, composition-only ETL boundary, full-reference scenario-id-v1 scope.
- [ ] 15.2 Update `docs/canonical-model.md`: support-set model (A1/A2/B), predicate table, root establishment + pinning + input reservation, temporal contextual-only rule, confidence definitions, sparse boundedness, witness retention policy, CorrelationContext join rules, validator contract, derive-native-correlation-evidence boundary.
- [ ] 15.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [ ] 15.4 Update `AGENTS.md` durable invariants concisely: positive candidate-specific support required; support monotonicity; ownership materializes only after closure; temporal evidence contextual until timeline guarantees exist; scenario identity derives from full scoped root reference; native evidence production deferred.
- [ ] 15.5 Record user-facing documentation determination: no CLI/website changes, so no `zh-tw`/`ja` localization updates triggered.
- [ ] 15.6 Run `graphify update .` after production source changes land.

## 16. Full repository/OpenSpec validation

- [ ] 16.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent.
- [ ] 16.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [ ] 16.3 Verify final diff: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/votes/heuristics/timing rules, no temporal elimination, no recursion, no bare-`OperationId` derivation, no early-finalized resolutions, no retention before semantic closure, no change to `validate()` semantics.
- [ ] 16.4 Direct behavioral probes covering acceptance scenarios A–G of this revision and prior equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: owns Chronicle-native relational semantics — logical task generation, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage — and may promote named predicates via specification change.
- Durable logical-operation / scenario identity surviving publication scope changes.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Additive public structural-validation API on `CorrelationGraph`, only if implementation needs it.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
