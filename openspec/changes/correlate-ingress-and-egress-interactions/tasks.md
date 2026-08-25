# Implementation tasks

Ordered by dependency. Approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, the OpenSpec change artifacts (including the `correlation-domain-model` delta), domain/integration tests, and documentation. No production behavior outside these surfaces may change.

## 1. Full-reference scenario-id-v1 and known-answer identity tests

- [x] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit.
- [x] 1.2 Implement `scenario_id_v1(root: CanonicalOperationRef) -> ScenarioId`: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII (36 bytes) || recording_be || owner_epoch_be || session_be || operation_be)` via `Uuid::as_bytes()` in ref declaration order; first 16 digest bytes as UUID octets; version 8 + RFC 4122 variant bit fixups. Bare/partial-scope variants forbidden.
- [x] 1.3 Known-answer tests hashing the actual separator bytes (never a hard-coded length); fixed UUID quadruples -> exact expected outputs; bit assertions.
- [x] 1.4 Duplicate-`OperationId` test: same operation ID under two owner-epoch/session scopes -> distinct refs -> distinct scenario ids.
- [x] 1.5 Document the guarantee boundary: stable across restarts/retries/replay-over-same-artifacts/scheduling/iteration orders; NOT stable across regrouping changing the authoritative reference, republication into another session/epoch, independent re-canonicalization, regenerated IDs.

## 2. correlation-domain-model delta: ScenarioRoot variant and placement consistency validation

- [x] 2.1 Add additive non-frozen `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }`; non-temporal (`is_temporal() == false`); serde-tagged like siblings.
- [x] 2.2 Implement the placement-consistency invariant from the delta: `ScenarioRoot { root: R }` is valid only inside the `Resolved` outcome of R, selecting scenario S where S.root == R, with role[R] `Known(Ingress)`; graph/domain validation rejects every other placement.
- [x] 2.3 Invalid-placement tests: wrong-operation item (`root: B` inside resolution[A]); non-root member carrying its own `ScenarioRoot`; root reference mismatched to the selected scenario's root; item inside `Uncorrelated` evidence; inside any `Ambiguous.candidates[*].evidence`; inside `SelectedCausalEdge.evidence`. All fail validation.
- [x] 2.4 Valid-placement test per scenario J: S.root = A, role[A] Known(Ingress), resolution[A] Resolved { Exact, [ScenarioRoot { root: A }] } passes `validate_against_sessions(supplied)`.
- [x] 2.5 Keep existing evidence validation otherwise untouched; no redesign beyond the new variant's consistency rule.

## 3. Distinct role-evidence and correlation-evidence channels

- [x] 3.1 Enforce the channel boundary in code structure: correlation predicate indexes are built ONLY from `CorrelationInput.evidence` (joined from `CorrelationContext.evidence[ref]`); role-channel items flow only into foundation role validation and verbatim output preservation.
- [x] 3.2 Channel-isolation tests: trace relationship present ONLY in role evidence produces no support; the same relationship moved into correlation evidence correlates normally; supplied role evidence (items and order) appears byte-for-byte in output — never reordered by canonical retention sorting.
- [x] 3.3 Review indexes/types so no API accepts or flattens merged evidence collections that could blur the channels.

## 4. ScenarioRoot input reservation across both channels and root pinning

- [x] 4.1 Pre-resolution validation rejects caller-supplied `ScenarioRoot` in EVERY caller-controlled container — context evidence maps AND nested role evidence (`Known`, `Unknown`, `Ambiguous.candidates[*]`) — as typed input errors; tests cover all injection paths in both channels.
- [x] 4.2 Root construction: derive id from full ref -> create `Scenario { id, root, members: [root] }` -> pin root support to {id} -> resolution `Resolved { Exact, [ScenarioRoot] }`. Root-only graphs pass `validate_against_sessions(supplied)` unchanged.
- [x] 4.3 Root-pinning tests: two roots sharing one trace stay two scenarios; shared span identity between roots does not merge them; contextual overlap/task/process agreement cannot pull a root across scenarios; role state byte-identical after establishment.

## 5. Immutable full-correlation-evidence relation indexes and predicates

- [x] 5.1 Build ordered indexes (`BTreeMap`) once over the COMPLETE validated correlation-channel evidence: `(provider, trace_id)`, `(provider, trace_id, span_id)`, full reference; inputs sorted by full reference tuple first; indexes immutable during resolution.
- [x] 5.2 Predicates as pure comparisons on the correlation channel only: `SharedTraceIdentity` (equal non-empty provider+trace_id), `ExplicitParentSpan` (child parent_span_id == candidate span_id under same provider+trace), shared-span detection. Everything else contextual-only.
- [x] 5.3 Unit tests: match, missing-field non-match, cross-provider non-match, shared-span behavior (blocks Phase B sufficiency only), same-task-string-alone adds nothing.

## 6. Stage A1: sparse support closure WITHOUT witness-path storage

- [x] 6.1 Sparse `(operation, ScenarioId)` memberships created only by discovered positive relationships; never allocate an operation-times-scenario matrix; NEVER store distinct transitive witness paths (exponentially many under repeated diamonds).
- [x] 6.2 Per-membership state stays bounded: at most a semantic flag such as `has_direct_support`; termination depends ONLY on newly added memberships, never on discovering a better path.
- [x] 6.3 Snapshot rounds: roots pinned + direct relations at round 0; each round evaluates against previous-round snapshot; union direct `SharedTraceIdentity` and transitive `ExplicitParentSpan` through snapshot-supported candidates; support only grows; terminate when a round adds nothing (bound N x S). No recursion.
- [x] 6.4 Diamond stress test: repeated diamond structures with exponentially many routes confirm bounded internal state and correct closure.

## 7. Post-closure canonical proof derivation, then Stage A2 materialization

- [x] 7.1 After closure, derive each materialized outcome's canonical ownership witness from the immutable relation indexes plus final support sets — NOT from first-discovery state: later shorter paths win.
- [x] 7.2 Derive the canonical transitive proof via deterministic shortest-path/dynamic-programming search over simple paths (visited-set pruned; no repeated references), ordering candidate paths shortest-first, then lexicographic candidate full-reference-tuple sequences, then canonical per-relation evidence keys. No enumeration of all paths; temporary search state proportional to the visited frontier; terminates on cyclic raw relation graphs.
- [x] 7.3 Materialize outcomes only after derivation inputs are available: empty support -> `Uncorrelated`; one scenario -> `Resolved`; multiple -> `Ambiguous` keyed by ScenarioId with per-candidate witnesses. No final values assigned during propagation.
- [x] 7.4 Tests: helper-level shortest/lexical path rules and a full `resolve_correlation` late-shorter-path regression prove final post-closure witness selection; equal-length paths break by lexicographic reference sequence regardless of presentation order; cyclic raw relation graphs terminate derivation with finite proofs; aggregation yields ONE scenario-keyed entry whose derived witness is canonical.

## 8. Confidence-consistent ownership-witness selection (unchanged semantics)

- [x] 8.1 Confidence post-closure: `Exact` for direct `SharedTraceIdentity` support or pinned `ScenarioRoot`; `Strong` for purely transitive parent-span inheritance; ambiguous skips confidence; `Inferred` never emitted.
- [x] 8.2 Semantic selection within required classes only: root Exact keeps `ScenarioRoot`; non-root Exact keeps the canonical DIRECT `SharedTraceIdentity` witness (a canonical transitive proof never replaces it); Strong keeps the canonical derived transitive path; ambiguous candidates prefer direct else canonical transitive.
- [x] 8.3 Tests: Exact-with-both-paths retains the direct witness; Strong retains the derived transitive path.

## 9. Total canonical evidence key and deterministic contextual retention

- [x] 9.1 Implement the TOTAL canonical evidence key covering the entire value: kind discriminator -> every semantic field of the kind in declaration order (UTF-8 byte string comparison; Option ordered None < Some) -> candidate full-reference tuple when candidate-relative (empty/None scope for non-relative items) -> `provenance.source` -> `provenance.observation`. For `TraceRelationship`: discriminator, provider, trace_id, span_id, parent_span_id, candidate scope, source, observation. No two distinct serialized items may tie.
- [x] 9.2 Contextual fill: after complete semantic witnesses, sort unused correlation-channel items by the canonical key and retain at most `CORRELATION_CONTEXTUAL_RETENTION_CAP` (64 items, applied per ambiguity candidate); required semantic witnesses are never truncated, so total evidence may exceed 64 for long proofs.
- [x] 9.3 Role-evidence exception test: nested role evidence remains byte-for-byte exactly as supplied (including order) regardless of correlation-retention sorting.
- [x] 9.4 Permutation-stability tests: same correlation-evidence multiset in several orders -> identical retained correlation evidence after contextual-fill cap application; prefix strings (`"a"`/`"aa"`, `"otel"`/`"otel2"`) order lexicographically; witnesses differing only in parent_span_id / provenance.source / provenance.observation resolve identically under permutation.

## 10. Stage B: fixed normative construction sequence with deterministic cycle safety

- [x] 10.1 Implement the exact per-scenario sequence: (1) collect candidate `ExplicitParentSpan` relations from correlation-channel evidence only; (2) reject relations where parent==child, child==S.root, parent/child not final members of S, target parent span shared, or predicate otherwise fails; (3) group survivors by child; (4) zero sufficient parents -> none, exactly one -> provisional edge, more than one -> none; (5) build the COMPLETE provisional graph for S; (6) compute directed cyclic SCCs globally (>1 node; self-loops impossible post-filter); (7) remove every provisional edge whose parent AND child are BOTH inside the same cyclic SCC; (8) emit surviving edges in canonical order (child ref tuple, then parent ref tuple).
- [x] 10.2 SCC removal semantics tests: only internal component edges removed; cycle participant's outgoing edge to an outside child survives when otherwise valid; acyclic incoming structure from outside untouched; membership never changes.
- [x] 10.3 Cycle regression tests: pure two-node X⇄Y loses both internal edges with ownership intact; three-node X→Y→Z→X same; corrected surrounding case D→A plus B⇄C plus C→E→F removes exactly B⇄C while retaining D→A, C→E, E→F.
- [x] 10.4 Multi-parent-before-cycle test (corrected outcome): A→B, C→B, B→C gives B parents {A,C} so BOTH A→B and C→B vanish before mapping; C keeps B as its single parent; provisional graph is exactly B→C; emitted edges are exactly {B→C}; no B⇄C SCC forms.
- [x] 10.5 Self-parent test: operation whose `span_id` equals its own `parent_span_id` produces no selected edge, keeps Resolved ownership, leaves evidence inspectable, and never reaches SCC analysis.
- [x] 10.6 Root-as-child test: root R with `parent_span_id` equal to member X's span emits no X→R edge; R stays root and Resolved(Scenario A); R's evidence (including `ScenarioRoot`) never mutated or discarded.
- [x] 10.7 Order-independence tests: permuted input/evidence orders over every cyclic, multi-parent, and tie case produce identical final edge sets and retained witnesses.
- [x] 10.8 Assert no incremental add-if-still-valid traversal exists and no code path drops edges based on validator failure; foundation validation confirms construction output unchanged.

## 11. Separate ownership-witness and edge-witness retention

- [x] 11.1 Ownership witness lives in `CorrelationResolution.evidence`; direct-parent witness lives on `SelectedCausalEdge.evidence`; extra parent detail inside resolutions is optional contextual enrichment only.
- [x] 11.2 Retention runs strictly after A2+B over closed support structure; required ownership and ambiguity-candidate witnesses are retained in full; `CORRELATION_CONTEXTUAL_RETENTION_CAP` bounds only optional contextual fill per outcome slot and ambiguity candidate; Uncorrelated keeps contextual/input items canonically with no manufactured witness.
- [x] 11.3 Long-proof regression: a 70-hop Strong transitive witness remains complete and deterministic under input permutation while optional contextual fill remains bounded.

## 12. Concurrent-ingress, async, temporal, cross-epoch cases (unchanged contracts)

- [x] 12.1 Three overlapping ingresses with uniquely related egresses resolve independently regardless of overlap/order; timing proximity influences nothing; partial-overlap ambiguity keeps unrelated ingresses out.
- [x] 12.2 Async/post-completion causality: disjoint lifetimes never block propagation; temporal-only inputs stay uncorrelated; reverse ordering performs no elimination.
- [x] 12.3 Epoch-N root + epoch-N+1 egress resolve into one scenario identified from the epoch-N root reference; terminal completion-owner scope participation; equal operation-ID roots under different sessions stay distinct end to end.

## 13. CorrelationContext and ETL composition API

- [x] 13.1 Define the correlation context value in `chronicle-canonical`; sessions alone are not valid resolver input; `ScenarioRoot` rejected in all supplied containers in both channels.
- [x] 13.2 `chronicle-etl` helper: verify references via existing lineage resolution, enrich lifetime/provenance views, join by exact reference; zero selection semantics.
- [x] 13.3 Join-rule tests: missing role entry -> typed error; missing evidence entry -> empty evidence; orphan entry -> typed error; smuggled root claims (both channels) -> typed errors.
- [x] 13.4 Default publication/checkpoint path never calls the helper; integration test at the composition layer with explicitly supplied hand-built fixtures (protocol-builtins end-to-end fixtures arrive with `derive-native-correlation-evidence`, the first producer of real correlation evidence).

## 14. Determinism and boundedness tests

- [x] 14.1 Repeated invocations across restart/retry/replay produce byte-identical graphs including ids, witnesses, edges.
- [x] 14.2 Permuted and hash-adversarial input/evidence orders produce identical results after canonical re-sorting; cycle-edge removal, witness ties, and contextual fill identical under permutation.
- [x] 14.3 High path-count graphs (repeated diamonds) stay within bounded internal state; proof derivation visits frontier-proportional state only; no structure grows with distinct possible paths.

## 15. Validator contract and architecture/provider checks

- [x] 15.1 Assert output graphs satisfy structural invariants and pass `validate_against_sessions(supplied_sessions)`; assert context-free `validate()` on non-empty graphs fails with `MissingSessionContext` per existing foundation semantics; change nothing in foundation validation; expose an additive structural API ONLY if implementation proves it necessary.
- [x] 15.2 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture --root .`; expect pass with no policy edits beyond the inherently allowed external hashing dependency.
- [x] 15.3 Verify no provider SDK enters touched crates, no new workspace edges, no new crate; `ScenarioRoot` stays inside the non-frozen domain enum authorized by this change's `correlation-domain-model` delta.

## 16. Documentation and AGENTS.md updates

- [x] 16.1 Update `docs/architecture/crate-boundaries.md`: resolver placement, three-phase semantics, evidence-channel separation, composition-only ETL boundary, full-reference scenario-id-v1 scope.
- [x] 16.2 Update `docs/canonical-model.md`: two-channel evidence model, predicate table, root establishment/pinning/reservation/placement-validation rules, support-set closure without witness-path storage, post-closure canonical proof derivation, normative eight-step Phase B with SCC removal semantics, total canonical evidence key + transitive path ordering, canonical contextual fill, temporal contextual-only rule, honest native-evidence capability boundary, CorrelationContext join rules, validator contract.
- [x] 16.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [x] 16.4 Update `AGENTS.md` durable invariants concisely: role evidence is classification provenance, never correlation input; positive candidate-specific support required; ownership materializes only after support closure with witnesses derived post-closure; parent edges constructed through the fixed pre-filter/unique-parent/SCC sequence; `ScenarioRoot` placement validated by the domain; temporal evidence contextual until timeline guarantees exist; native evidence production deferred.
- [x] 16.5 Record user-facing documentation determination: no CLI/website changes, so no `zh-tw`/`ja` localization updates triggered.
- [x] 16.6 Run `graphify update .` after production source changes land.

## 17. Full repository/OpenSpec validation

- [x] 17.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent (both capability deltas included).
- [x] 17.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [x] 17.3 Verify final diff: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/votes/heuristics/timing rules, no temporal elimination, no recursion, no bare-`OperationId` derivation, no early-finalized resolutions, no retention or derivation before closure, no witness-path enumeration/storage, no role-evidence leakage into correlation indexes, no self-edge/root-child emission paths, no order-dependent cycle breaking, no change to `validate()` semantics.
- [x] 17.4 Direct behavioral probes covering acceptance scenarios A–J of this revision plus prior equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: owns Chronicle-native relational-semantics contracts — logical task generation identity, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage — and promotion of safe named predicates via specification change. Until then this revision defines no native positive ownership predicate beyond resolver-generated `ScenarioRoot`.
- Durable logical-operation / scenario identity surviving publication scope changes.
- Timeline-guarantee change enabling named safe temporal contradictions.
- Additive public structural-validation API on `CorrelationGraph`, only if implementation needs it.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
