# Implementation tasks

Ordered by dependency. Approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, domain/integration tests, documentation. No production behavior outside these surfaces may change.

## 1. Full-reference scenario-id-v1 and known-answer identity tests

- [ ] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit.
- [ ] 1.2 Implement `scenario_id_v1(root: CanonicalOperationRef) -> ScenarioId`: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII (36 bytes) || recording_be || owner_epoch_be || session_be || operation_be)` via `Uuid::as_bytes()` in ref declaration order; first 16 digest bytes as UUID octets; version 8 + RFC 4122 variant bit fixups. Bare/partial-scope variants forbidden.
- [ ] 1.3 Known-answer tests hashing the actual separator bytes (never a hard-coded length); fixed UUID quadruples -> exact expected outputs; bit assertions.
- [ ] 1.4 Duplicate-`OperationId` test: same operation ID under two owner-epoch/session scopes -> distinct refs -> distinct scenario ids.
- [ ] 1.5 Document the guarantee boundary: stable across restarts/retries/replay-over-same-artifacts/scheduling/iteration orders; NOT stable across regrouping changing the authoritative reference, republication into another session/epoch, independent re-canonicalization, regenerated IDs.

## 2. Distinct role-evidence and correlation-evidence channels

- [ ] 2.1 Enforce the channel boundary in code structure: correlation predicate indexes are built ONLY from `CorrelationInput.evidence` (joined from `CorrelationContext.evidence[ref]`); role-channel items (`Known.evidence`, `Unknown.evidence`, `Ambiguous.candidates[*].evidence`) flow only into foundation role validation and verbatim output preservation.
- [ ] 2.2 Channel-isolation tests: trace relationship present ONLY in role evidence produces no support; the same relationship moved into correlation evidence correlates normally; supplied role evidence (items and order) appears byte-for-byte in output.
- [ ] 2.3 Review indexes/types so no API accepts or flattens merged evidence collections that could blur the channels.

## 3. ScenarioRoot evidence kind, input reservation across both channels, and root pinning

- [ ] 3.1 Add additive non-frozen `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`; non-temporal; document meaning.
- [ ] 3.2 Input reservation scans BOTH channels — context evidence maps AND nested role evidence (`Known`, `Unknown`, `Ambiguous.candidates[*]`) — rejecting any caller-supplied `ScenarioRoot` as a typed input error; tests cover all injection paths in both channels.
- [ ] 3.3 Root construction: derive id from full ref -> create `Scenario { id, root, members: [root] }` -> pin root support to {id} with `ScenarioRoot` witness -> resolution `Resolved { Exact, [ScenarioRoot] }`. Root-only graphs pass `validate_against_sessions(supplied)` unchanged.
- [ ] 3.4 Root-pinning tests: two roots sharing one trace stay two scenarios; shared span identity between roots does not merge them; contextual overlap/task/process agreement cannot pull a root across scenarios; role state byte-identical after establishment.

## 4. Immutable full-correlation-evidence relation indexes and predicates

- [ ] 4.1 Build ordered indexes (`BTreeMap`) once over the COMPLETE validated correlation-channel evidence: `(provider, trace_id)`, `(provider, trace_id, span_id)`, full reference; inputs sorted by full reference tuple first; indexes immutable during resolution.
- [ ] 4.2 Predicates as pure comparisons on the correlation channel only: `SharedTraceIdentity` (equal non-empty provider+trace_id), `ExplicitParentSpan` (child parent_span_id == candidate span_id under same provider+trace), shared-span detection. Everything else contextual-only.
- [ ] 4.3 Unit tests: match, missing-field non-match, cross-provider non-match, shared-span behavior (blocks Phase B sufficiency only), same-task-string-alone adds nothing.

## 5. Stage A1: sparse scenario-support-set fixpoint (unchanged contracts)

- [ ] 5.1 Sparse `(operation, ScenarioId)` memberships created only by discovered positive relationships; never allocate an operation-times-scenario matrix.
- [ ] 5.2 Snapshot rounds: roots pinned + direct relations at round 0; each round evaluates against previous-round snapshot; union direct `SharedTraceIdentity` and transitive `ExplicitParentSpan` through snapshot-supported candidates; support only grows.
- [ ] 5.3 Terminate when a round adds no membership; productive rounds each add >=1 membership (bound N x S); internal per-membership witnesses retained (direct item or transitive path). No recursion.

## 6. Stage A2: materialize outcomes only after closure (unchanged contracts)

- [ ] 6.1 Empty support -> `Uncorrelated`; one scenario -> `Resolved`; multiple -> `Ambiguous` keyed by ScenarioId with per-candidate witnesses. No final values assigned during propagation.
- [ ] 6.2 Chain-depth ambiguity tests: short-path A vs long-path B to Z materializes `Ambiguous(A, B)` regardless of discovery order; reversed operation/session/evidence orders produce identical results.
- [ ] 6.3 Aggregation test: several members of one scenario matching one child yield ONE scenario-keyed entry with multiple witness paths.

## 7. Confidence-consistent ownership-witness selection

- [ ] 7.1 Confidence post-closure: `Exact` for direct `SharedTraceIdentity` support or pinned `ScenarioRoot`; `Strong` for purely transitive parent-span inheritance; ambiguous skips confidence; `Inferred` never emitted.
- [ ] 7.2 Semantic witness selection matching materialized confidence: root Exact keeps `ScenarioRoot`; non-root Exact keeps a canonical DIRECT `SharedTraceIdentity` witness (never substitute an earlier-sorting transitive proof); Strong keeps a canonical transitive span path; ambiguous candidates prefer direct candidate-specific witnesses else canonical transitive proofs.
- [ ] 7.3 Canonical ordering key for equivalent proofs: provider, then trace id, then span id where applicable, then candidate full-reference tuple; selection independent of discovery/iteration order.
- [ ] 7.4 Tests: Exact-with-both-paths retains the direct witness (scenario F); Strong retains the transitive witness (scenario G); permuted presentation changes nothing.

## 8. Stage B: global deterministic cycle-safe parent construction

- [ ] 8.1 Compute ALL sufficient direct-parent relations among each resolved scenario's final members; derive the provisional unique-parent mapping (zero sufficient relations -> none; multiple equally sufficient -> none).
- [ ] 8.2 Evaluate the COMPLETE provisional directed graph globally; identify directed cycles via strongly connected components (>1 node) per scenario; remove every provisional edge inside cyclic components; acyclic edges outside cycles survive when the remaining graph stays a valid forest.
- [ ] 8.3 No incremental add-if-acyclic traversal anywhere; validity holds by construction, never by validator-rejection triage; foundation validation passes unchanged (tree invariants, root never child, ambiguous never in edges).
- [ ] 8.4 Cycle tests: two-node cycle X⇄Y loses both edges while ownership stays Resolved; three-node cycle X→Y→Z→X same; surrounding acyclic structure (D→A→B with B⇄C) keeps D→A→B; permuted input/evidence orders yield identical edge sets (scenarios C, D, E).

## 9. Separate ownership-witness and edge-witness retention

- [ ] 9.1 Ownership witness lives in `CorrelationResolution.evidence`; direct-parent witness lives on `SelectedCausalEdge.evidence`; extra parent detail inside resolutions is optional contextual enrichment only — never merged conceptually or representationally.
- [ ] 9.2 Retention runs strictly after A2+B over the closed support structure; documented constant 64 applied per outcome slot and PER AMBIGUITY CANDIDATE; justification-first fill then canonical contextual fill; Uncorrelated keeps contextual/input items canonically with no manufactured witness.
- [ ] 9.3 Cap-pressure tests: exceeding the cap leaves support sets, outcomes, and edges identical to uncapped runs; wide ambiguity retains every candidate's confidence-consistent witness without drops or errors.

## 10. Concurrent-ingress, async, and temporal cases (unchanged contracts)

- [ ] 10.1 Three overlapping ingresses with uniquely related egresses resolve independently regardless of overlap/order; timing proximity influences nothing; partial-overlap ambiguity keeps unrelated ingresses out.
- [ ] 10.2 Async/post-completion causality: disjoint lifetimes never block propagation; temporal-only inputs stay uncorrelated; reverse ordering performs no elimination.

## 11. Cross-epoch / scoped-reference handling (unchanged contracts)

- [ ] 11.1 Epoch-N root + epoch-N+1 egress resolve into one scenario identified from the epoch-N root reference; cross-session edge validates with full scoped endpoints.
- [ ] 11.2 Terminal completion-owner scope participation; equal operation-ID roots under different sessions stay distinct end to end.

## 12. CorrelationContext and ETL composition API

- [ ] 12.1 Define the correlation context value in `chronicle-canonical`; sessions alone are not valid resolver input; `ScenarioRoot` rejected in all supplied containers in both channels.
- [ ] 12.2 `chronicle-etl` helper: verify references via existing lineage resolution, enrich lifetime/provenance views, join by exact reference; zero selection semantics.
- [ ] 12.3 Join-rule tests: missing role entry -> typed error; missing evidence entry -> empty evidence; orphan entry -> typed error; smuggled root claims (both channels) -> typed errors.
- [ ] 12.4 Default publication/checkpoint path never calls the helper; integration test at lowest conclusive layer with protocol-builtins fixtures and explicitly supplied contexts.

## 13. Determinism tests

- [ ] 13.1 Repeated invocations across restart/retry/replay produce byte-identical graphs including ids, witnesses, edges.
- [ ] 13.2 Permuted and hash-adversarial input/evidence orders produce identical results after canonical re-sorting; cycle-edge removal identical under permutation (see 8.4).

## 14. Validator contract and architecture/provider checks

- [ ] 14.1 Assert output graphs satisfy structural invariants and pass `validate_against_sessions(supplied_sessions)`; assert context-free `validate()` on non-empty graphs fails with `MissingSessionContext` per existing foundation semantics; change nothing in foundation validation; expose an additive structural API ONLY if implementation proves it necessary.
- [ ] 14.2 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture --root .`; expect pass with no policy edits beyond the inherently allowed external hashing dependency.
- [ ] 14.3 Verify no provider SDK enters touched crates, no new workspace edges, no new crate; `ScenarioRoot` stays inside the non-frozen domain enum.

## 15. Documentation and AGENTS.md updates

- [ ] 15.1 Update `docs/architecture/crate-boundaries.md`: resolver placement, three-phase semantics, evidence-channel separation, composition-only ETL boundary, full-reference scenario-id-v1 scope.
- [ ] 15.2 Update `docs/canonical-model.md`: two-channel evidence model, predicate table, root establishment/pinning/reservation, support-set closure, global cycle-safe Phase B, confidence-consistent witness retention with ownership-vs-edge separation, temporal contextual-only rule, honest native-evidence capability boundary, CorrelationContext join rules, validator contract.
- [ ] 15.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [ ] 15.4 Update `AGENTS.md` durable invariants concisely: role evidence is classification provenance, never correlation input; positive candidate-specific support required; ownership materializes only after support closure; parent edges constructed globally with deterministic cycle refusal; temporal evidence contextual until timeline guarantees exist; native evidence production deferred to `derive-native-correlation-evidence`.
- [ ] 15.5 Record user-facing documentation determination: no CLI/website changes, so no `zh-tw`/`ja` localization updates triggered.
- [ ] 15.6 Run `graphify update .` after production source changes land.

## 16. Full repository/OpenSpec validation

- [ ] 16.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent.
- [ ] 16.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [ ] 16.3 Verify final diff: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/votes/heuristics/timing rules, no temporal elimination, no recursion, no bare-`OperationId` derivation, no early-finalized resolutions, no retention before semantic closure, no role-evidence leakage into correlation indexes, no order-dependent cycle breaking, no change to `validate()` semantics.
- [ ] 16.4 Direct behavioral probes covering acceptance scenarios A–H of this revision plus prior equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: owns Chronicle-native relational-semantics contracts — logical task generation identity, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage — and promotion of safe named predicates via specification change. Until then this revision defines no native positive ownership predicate beyond resolver-generated `ScenarioRoot`.
- Durable logical-operation / scenario identity surviving publication scope changes.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Additive public structural-validation API on `CorrelationGraph`, only if implementation needs it.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
