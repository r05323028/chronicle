# Implementation tasks

Ordered by dependency. Approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, the OpenSpec change artifacts (including the new `correlation-domain-model` delta), domain/integration tests, and documentation. No production behavior outside these surfaces may change.

## 1. Full-reference scenario-id-v1 and known-answer identity tests

- [ ] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit.
- [ ] 1.2 Implement `scenario_id_v1(root: CanonicalOperationRef) -> ScenarioId`: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII (36 bytes) || recording_be || owner_epoch_be || session_be || operation_be)` via `Uuid::as_bytes()` in ref declaration order; first 16 digest bytes as UUID octets; version 8 + RFC 4122 variant bit fixups. Bare/partial-scope variants forbidden.
- [ ] 1.3 Known-answer tests hashing the actual separator bytes (never a hard-coded length); fixed UUID quadruples -> exact expected outputs; bit assertions.
- [ ] 1.4 Duplicate-`OperationId` test: same operation ID under two owner-epoch/session scopes -> distinct refs -> distinct scenario ids.
- [ ] 1.5 Document the guarantee boundary: stable across restarts/retries/replay-over-same-artifacts/scheduling/iteration orders; NOT stable across regrouping changing the authoritative reference, republication into another session/epoch, independent re-canonicalization, regenerated IDs.

## 2. correlation-domain-model delta: ScenarioRoot variant

- [ ] 2.1 Add additive non-frozen `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`; non-temporal (`is_temporal() == false`); serde-tagged like siblings.
- [ ] 2.2 Domain-level tests per the delta spec: `ScenarioRoot` accepted as non-temporal witness inside a validated `Resolved`; role resolution byte-identical in its presence; no frozen v1 surface touched.

## 3. Distinct role-evidence and correlation-evidence channels

- [ ] 3.1 Enforce the channel boundary in code structure: correlation predicate indexes are built ONLY from `CorrelationInput.evidence` (joined from `CorrelationContext.evidence[ref]`); role-channel items flow only into foundation role validation and verbatim output preservation.
- [ ] 3.2 Channel-isolation tests: trace relationship present ONLY in role evidence produces no support; the same relationship moved into correlation evidence correlates normally; supplied role evidence (items and order) appears byte-for-byte in output.
- [ ] 3.3 Review indexes/types so no API accepts or flattens merged evidence collections that could blur the channels.

## 4. ScenarioRoot input reservation across both channels and root pinning

- [ ] 4.1 Pre-resolution validation rejects caller-supplied `ScenarioRoot` in EVERY caller-controlled container — context evidence maps AND nested role evidence (`Known`, `Unknown`, `Ambiguous.candidates[*]`) — as typed input errors; tests cover all injection paths in both channels.
- [ ] 4.2 Root construction: derive id from full ref -> create `Scenario { id, root, members: [root] }` -> pin root support to {id} with `ScenarioRoot` witness -> resolution `Resolved { Exact, [ScenarioRoot] }`. Root-only graphs pass `validate_against_sessions(supplied)` unchanged.
- [ ] 4.3 Root-pinning tests: two roots sharing one trace stay two scenarios; shared span identity between roots does not merge them; contextual overlap/task/process agreement cannot pull a root across scenarios; role state byte-identical after establishment.

## 5. Immutable full-correlation-evidence relation indexes and predicates

- [ ] 5.1 Build ordered indexes (`BTreeMap`) once over the COMPLETE validated correlation-channel evidence: `(provider, trace_id)`, `(provider, trace_id, span_id)`, full reference; inputs sorted by full reference tuple first; indexes immutable during resolution.
- [ ] 5.2 Predicates as pure comparisons on the correlation channel only: `SharedTraceIdentity` (equal non-empty provider+trace_id), `ExplicitParentSpan` (child parent_span_id == candidate span_id under same provider+trace), shared-span detection. Everything else contextual-only.
- [ ] 5.3 Unit tests: match, missing-field non-match, cross-provider non-match, shared-span behavior (blocks Phase B sufficiency only), same-task-string-alone adds nothing.

## 6. Stage A1: sparse scenario-support-set fixpoint (unchanged contracts)

- [ ] 6.1 Sparse `(operation, ScenarioId)` memberships created only by discovered positive relationships; never allocate an operation-times-scenario matrix.
- [ ] 6.2 Snapshot rounds: roots pinned + direct relations at round 0; each round evaluates against previous-round snapshot; union direct `SharedTraceIdentity` and transitive `ExplicitParentSpan` through snapshot-supported candidates; support only grows.
- [ ] 6.3 Terminate when a round adds no membership; productive rounds each add >=1 membership (bound N x S); internal per-membership witnesses retained (direct item or transitive path). No recursion.

## 7. Stage A2: materialize outcomes only after closure (unchanged contracts)

- [ ] 7.1 Empty support -> `Uncorrelated`; one scenario -> `Resolved`; multiple -> `Ambiguous` keyed by ScenarioId with per-candidate witnesses. No final values assigned during propagation.
- [ ] 7.2 Chain-depth ambiguity tests: short-path A vs long-path B to Z materializes `Ambiguous(A, B)` regardless of discovery order; reversed operation/session/evidence orders produce identical results.
- [ ] 7.3 Aggregation test: several members of one scenario matching one child yield ONE scenario-keyed entry with multiple witness paths.

## 8. Confidence-consistent ownership-witness selection with total canonical ordering

- [ ] 8.1 Confidence post-closure: `Exact` for direct `SharedTraceIdentity` support or pinned `ScenarioRoot`; `Strong` for purely transitive parent-span inheritance; ambiguous skips confidence; `Inferred` never emitted.
- [ ] 8.2 Implement the TOTAL canonical evidence key: kind discriminator -> every semantic field of the kind in declaration order (UTF-8 byte string comparison; Option ordered None < Some) -> candidate full-reference tuple when candidate-relative -> `provenance.source` -> `provenance.observation`. For `TraceRelationship`: discriminator, provider, trace_id, span_id, parent_span_id, candidate scope, source, observation. No two distinct serialized items may tie.
- [ ] 8.3 Implement deterministic transitive proof-path ordering: shortest valid path, then lexicographic sequence of candidate full-reference tuples along the path, then canonical evidence keys per relation along the path.
- [ ] 8.4 Semantic selection within required classes only: root Exact keeps `ScenarioRoot`; non-root Exact keeps the canonical DIRECT `SharedTraceIdentity` witness (a canonical transitive proof never replaces it); Strong keeps the canonical transitive path; ambiguous candidates prefer direct else canonical transitive.
- [ ] 8.5 Tests: Exact-with-both-paths retains the direct witness; Strong retains the transitive path; witnesses differing ONLY in parent_span_id / provenance.source / provenance.observation resolve identically under permutation; two competing transitive paths retain the same canonical path under permutation.

## 9. Stage B: fixed normative construction sequence with deterministic cycle safety

- [ ] 9.1 Implement the exact per-scenario sequence: (1) collect candidate `ExplicitParentSpan` relations from correlation-channel evidence only; (2) reject relations where parent==child, child==S.root, parent/child not final members of S, target parent span shared, or predicate otherwise fails; (3) group survivors by child; (4) zero sufficient parents -> none, exactly one -> provisional edge, more than one -> none; (5) build the COMPLETE provisional graph for S; (6) compute directed cyclic SCCs globally (>1 node; self-loops impossible post-filter); (7) remove every provisional edge whose parent AND child are BOTH inside the same cyclic SCC; (8) emit surviving edges in canonical order (child ref tuple, then parent ref tuple).
- [ ] 9.2 SCC removal semantics tests: only internal component edges removed; cycle participant's outgoing edge to an outside child survives when otherwise valid; acyclic incoming structure from outside untouched; membership never changes.
- [ ] 9.3 Cycle regression tests: pure two-node X⇄Y loses both internal edges with ownership intact; three-node X→Y→Z→X same; corrected surrounding case D→A plus B⇄C plus C→E→F removes exactly B⇄C while retaining D→A, C→E, E→F.
- [ ] 9.4 Multi-parent-before-cycle test: A→B, C→B, B→C yields B zero provisional parents, so no B⇄C SCC forms and the acyclic A→B, B→C chain is emitted intact.
- [ ] 9.5 Self-parent test: operation whose `span_id` equals its own `parent_span_id` produces no selected edge, keeps Resolved ownership, leaves evidence inspectable, and never reaches SCC analysis.
- [ ] 9.6 Root-as-child test: root R with `parent_span_id` equal to member X's span emits no X→R edge; R stays root and Resolved(Scenario A); R's evidence (including `ScenarioRoot`) never mutated or discarded.
- [ ] 9.7 Order-independence tests: permuted input/evidence orders over every cyclic and multi-parent case produce identical final edge sets.
- [ ] 9.8 Assert no incremental add-if-still-valid traversal exists and no code path drops edges based on validator failure; foundation validation confirms construction output unchanged.

## 10. Separate ownership-witness and edge-witness retention

- [ ] 10.1 Ownership witness lives in `CorrelationResolution.evidence`; direct-parent witness lives on `SelectedCausalEdge.evidence`; extra parent detail inside resolutions is optional contextual enrichment only.
- [ ] 10.2 Retention runs strictly after A2+B over closed support structure; documented constant 64 applied per outcome slot and PER AMBIGUITY CANDIDATE; justification-first fill via section 8 rules then canonical contextual fill; Uncorrelated keeps contextual/input items canonically with no manufactured witness.
- [ ] 10.3 Cap-pressure tests: exceeding the cap leaves support sets, outcomes, and edges identical to uncapped runs; wide ambiguity retains every candidate's confidence-consistent witness without drops or errors.

## 11. Concurrent-ingress, async, and temporal cases (unchanged contracts)

- [ ] 11.1 Three overlapping ingresses with uniquely related egresses resolve independently regardless of overlap/order; timing proximity influences nothing; partial-overlap ambiguity keeps unrelated ingresses out.
- [ ] 11.2 Async/post-completion causality: disjoint lifetimes never block propagation; temporal-only inputs stay uncorrelated; reverse ordering performs no elimination.

## 12. Cross-epoch / scoped-reference handling (unchanged contracts)

- [ ] 12.1 Epoch-N root + epoch-N+1 egress resolve into one scenario identified from the epoch-N root reference; cross-session edge validates with full scoped endpoints.
- [ ] 12.2 Terminal completion-owner scope participation; equal operation-ID roots under different sessions stay distinct end to end.

## 13. CorrelationContext and ETL composition API

- [ ] 13.1 Define the correlation context value in `chronicle-canonical`; sessions alone are not valid resolver input; `ScenarioRoot` rejected in all supplied containers in both channels.
- [ ] 13.2 `chronicle-etl` helper: verify references via existing lineage resolution, enrich lifetime/provenance views, join by exact reference; zero selection semantics.
- [ ] 13.3 Join-rule tests: missing role entry -> typed error; missing evidence entry -> empty evidence; orphan entry -> typed error; smuggled root claims (both channels) -> typed errors.
- [ ] 13.4 Default publication/checkpoint path never calls the helper; integration test at lowest conclusive layer with protocol-builtins fixtures and explicitly supplied contexts.

## 14. Determinism tests

- [ ] 14.1 Repeated invocations across restart/retry/replay produce byte-identical graphs including ids, witnesses, edges.
- [ ] 14.2 Permuted and hash-adversarial input/evidence orders produce identical results after canonical re-sorting; cycle-edge removal and witness ties identical under permutation (see 8.5, 9.7).

## 15. Validator contract and architecture/provider checks

- [ ] 15.1 Assert output graphs satisfy structural invariants and pass `validate_against_sessions(supplied_sessions)`; assert context-free `validate()` on non-empty graphs fails with `MissingSessionContext` per existing foundation semantics; change nothing in foundation validation; expose an additive structural API ONLY if implementation proves it necessary.
- [ ] 15.2 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture --root .`; expect pass with no policy edits beyond the inherently allowed external hashing dependency.
- [ ] 15.3 Verify no provider SDK enters touched crates, no new workspace edges, no new crate; `ScenarioRoot` stays inside the non-frozen domain enum authorized by this change's `correlation-domain-model` delta.

## 16. Documentation and AGENTS.md updates

- [ ] 16.1 Update `docs/architecture/crate-boundaries.md`: resolver placement, three-phase semantics, evidence-channel separation, composition-only ETL boundary, full-reference scenario-id-v1 scope.
- [ ] 16.2 Update `docs/canonical-model.md`: two-channel evidence model, predicate table, root establishment/pinning/reservation, support-set closure, normative eight-step Phase B with SCC removal semantics, total canonical witness key + transitive path ordering, temporal contextual-only rule, honest native-evidence capability boundary, CorrelationContext join rules, validator contract.
- [ ] 16.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [ ] 16.4 Update `AGENTS.md` durable invariants concisely: role evidence is classification provenance, never correlation input; positive candidate-specific support required; ownership materializes only after support closure; parent edges constructed through the fixed pre-filter/unique-parent/SCC sequence; temporal evidence contextual until timeline guarantees exist; native evidence production deferred to `derive-native-correlation-evidence`.
- [ ] 16.5 Record user-facing documentation determination: no CLI/website changes, so no `zh-tw`/`ja` localization updates triggered.
- [ ] 16.6 Run `graphify update .` after production source changes land.

## 17. Full repository/OpenSpec validation

- [ ] 17.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent (both capability deltas included).
- [ ] 17.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [ ] 17.3 Verify final diff: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/votes/heuristics/timing rules, no temporal elimination, no recursion, no bare-`OperationId` derivation, no early-finalized resolutions, no retention before semantic closure, no role-evidence leakage into correlation indexes, no self-edge/root-child emission paths, no order-dependent cycle breaking, no change to `validate()` semantics.
- [ ] 17.4 Direct behavioral probes covering acceptance scenarios A–I of this revision plus prior equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: owns Chronicle-native relational-semantics contracts — logical task generation identity, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage — and promotion of safe named predicates via specification change. Until then this revision defines no native positive ownership predicate beyond resolver-generated `ScenarioRoot`.
- Durable logical-operation / scenario identity surviving publication scope changes.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Additive public structural-validation API on `CorrelationGraph`, only if implementation needs it.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
