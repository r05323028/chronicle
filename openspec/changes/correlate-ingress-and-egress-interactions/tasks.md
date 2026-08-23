# Implementation tasks

Ordered by dependency. Approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, domain/integration tests, documentation. No production behavior outside these surfaces may change.

## 1. Full-reference scenario-id-v1 and known-answer identity tests

- [ ] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit (ordinary crate, no workspace-edge change).
- [ ] 1.2 Implement `scenario_id_v1(root: CanonicalOperationRef) -> ScenarioId` exactly as specified: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII || recording_uuid_be || owner_epoch_uuid_be || session_uuid_be || operation_uuid_be)` using `Uuid::as_bytes()` in ref declaration order; first 16 digest bytes as UUID octets; `octets[6] = (octets[6] & 0x0F) | 0x80`; `octets[8] = (octets[8] & 0x3F) | 0x80`. Bare-operation or partial-scope variants are forbidden.
- [ ] 1.3 Known-answer tests: fixed UUID quadruples -> exact expected output UUIDs (vectors computed once from the algorithm); separator-bytes verbatim test; version/variant bit assertions.
- [ ] 1.4 Duplicate-`OperationId` test: same operation ID value under two different owner-epoch/session scopes yields distinct `CanonicalOperationRef`s and distinct derived `ScenarioId`s — proving compatibility with the foundation's no-bare-uniqueness invariant.
- [ ] 1.5 Document the guarantee boundary in API docs: stable across restarts, retries, replay over the same persisted artifacts, scheduling, and iteration orders; NOT guaranteed across regrouping that changes the authoritative root reference, republication into a different owning session/epoch, independent re-canonicalization, or regenerated IDs. No doc claims more.

## 2. Scenario-root self-resolution and the ScenarioRoot evidence kind

- [ ] 2.1 Add additive `CorrelationEvidenceKind::ScenarioRoot { root: CanonicalOperationRef }` to the non-frozen domain enum; non-temporal (`is_temporal() == false`); serde-tagged like siblings; document meaning: resolver establishes this known ingress as root of the scenario derived from its full reference.
- [ ] 2.2 Root construction per spec: derive scenario id from root's full reference -> create `Scenario { id, root, members: [root] }` -> admit resolution `Resolved { scenario, Exact, evidence: [ScenarioRoot { root }] }`. Root-only scenarios must pass foundation validation unchanged.
- [ ] 2.3 Tests: single-ingress input produces valid graph with one root-only scenario; root carries exactly one resolution witnessed by its `ScenarioRoot` item; role resolution byte-identical before/after establishment; foundation validator accepts without modification.

## 3. Evidence predicate cleanup

- [ ] 3.1 Implement named predicates as pure comparison functions over child-side and candidate-side evidence: `SharedTraceIdentity` (equal non-empty provider AND trace_id), `ExplicitParentSpan` (child parent_span_id equals candidate span_id under same provider+trace), shared-span detection (same provider+trace+span carried by multiple operations).
- [ ] 3.2 Contextual-only classification for everything else — `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, `ProtocolStream`, `ProtocolOwnership`, `WireDirection`, `SocketRole`, `TemporalLifetime`, `Custom`: support nothing, contradict nothing, retained only.
- [ ] 3.3 Unit tests per predicate: match, missing/empty-field non-match, cross-provider non-match, shared-span behavior (blocks Stage B sufficiency only, never Stage A ownership); same-task-string-alone resolves nothing.

## 4. Stage A: ownership resolution with candidate aggregation by ScenarioId

- [ ] 4.1 Deterministic ordered indexes (`BTreeMap`) keyed by `(provider, trace_id)`, `(provider, trace_id, span_id)`, and full reference; inputs sorted by full reference tuple first.
- [ ] 4.2 Construct supported candidates ONLY from fired `SharedTraceIdentity` relations or inherited membership via `ExplicitParentSpan` chains to resolved members, plus resolver-generated root resolutions; aggregate multiple member matches of one scenario into ONE candidate keyed by `ScenarioId` carrying the union witness set — never duplicate candidates per matching member.
- [ ] 4.3 Outcomes over sufficiently supported owners: zero -> `Uncorrelated { retained }`; one -> `Resolved { scenario, Exact }`; two or more -> `Ambiguous { candidates }` with per-candidate witnesses. Absence of eliminating evidence contributes nothing; contextual agreement manufactures nothing.
- [ ] 4.4 Single contradiction check this change: cross-scenario decisive conflict keeps both candidates ambiguous; structured so future named predicates plug in without reshaping. Temporal items never participate.

## 5. Iterative monotonic chaining fixpoint

- [ ] 5.1 Implement snapshot-round iteration: evaluate each round against previous round's resolved set only; admit new members next round; repeat until a round admits nothing; monotonic — resolved membership never removed/reassigned within a run; productive rounds bounded by admitted-operation count. No recursion anywhere.
- [ ] 5.2 Emit `Strong` confidence for operations resolved purely through transitive inheritance without direct identity match; `Exact` otherwise.
- [ ] 5.3 Tests: chain longer than any plausible depth heuristic resolves fully through iterative rounds; forward-vs-reversed input order on a multi-level chain yields identical graphs; stalled chains terminate leaving uncorrelated/ambiguous states intact.

## 6. Stage B: direct causal-parent selection

- [ ] 6.1 For each `Resolved` operation evaluate `ExplicitParentSpan` against members of its own scenario only; emit exactly one edge when one unshared target matches, retaining the predicate witness on the edge.
- [ ] 6.2 Emit NO edge for multiple/equally sufficient parents, insufficient relations, contextual-only relations, or cross-scenario proposed parents; ownership stays `Resolved`; unresolved parenthood represented solely by edge absence plus retained witnesses. Shared spans block sufficiency.
- [ ] 6.3 Verify every output graph passes unchanged foundation validation (`validate`, `validate_against_sessions`): tree invariants, root never child, ambiguous operations never in edges.

## 7. Deterministic minimal-witness retention

- [ ] 7.1 Implement documented constant (64 items) applied PER AMBIGUITY CANDIDATE and per outcome slot: Resolved keeps fired-predicate witness(es)/`ScenarioRoot` + selected-edge witness, then canonical contextual fill; Ambiguous keeps >=1 candidate-specific witness per candidate then canonical fill; Uncorrelated keeps contextual/input items canonically (no manufactured witness).
- [ ] 7.2 Witness ordering: justification-first in canonical predicate order, then canonical input-order contextual fill; no recency priority.
- [ ] 7.3 Cap-pressure tests: raw evidence exceeding the cap leaves outcomes correct, deterministic, and explained; wide ambiguity retains every candidate's witness with no drops and no boundedness errors; uncorrelated retention contains no ownership witness.

## 8. Concurrent-ingress behavior

- [ ] 8.1 Three mutually overlapping ingresses with uniquely related egresses resolve independently regardless of start order/duration/overlap.
- [ ] 8.2 Timing proximity/recency/openness/processing order influence nothing (closest-ingress-differs cases).
- [ ] 8.3 Candidate-specific ambiguity: `Ambiguous(A, B)` while unrelated Ingress C never appears; another egress resolves normally in the same run.

## 9. Async / post-completion cases

- [ ] 9.1 Ingress completing before egress start with trace relation binding them resolves; disjointness ignored.
- [ ] 9.2 Explicit parent-span relation over entirely earlier candidate lifetimes retains full force.
- [ ] 9.3 Temporal-only inputs stay uncorrelated; overlap count/proximity matter nowhere; reverse-ordering performs no elimination.

## 10. Cross-epoch / scoped-reference handling

- [ ] 10.1 Root owned by epoch N session, egress by epoch N+1 session: one scenario identified from the epoch-N root reference; cross-session edge with full scoped endpoints validates.
- [ ] 10.2 Terminal completion-owner scope participation; continuation provenance never a second subject.
- [ ] 10.3 Equal operation-ID roots under different sessions: distinct subjects, distinct scenario ids end to end (extends task 1.4 through full resolution).

## 11. CorrelationContext and ETL composition API

- [ ] 11.1 Define the correlation context value (role resolutions + evidence keyed by full `CanonicalOperationRef`) in `chronicle-canonical`; sessions alone are not valid resolver input; caller-supplied `ScenarioRoot` content is invalid context.
- [ ] 11.2 Add the `chronicle-etl` composition helper joining published sessions with supplied context: verify references via existing lineage resolution, enrich lifetime/provenance views, join context by exact reference; zero selection semantics; resolver generates all root evidence itself.
- [ ] 11.3 Join-rule tests: missing role entry -> typed error naming the reference; missing evidence entry -> empty evidence; orphan context entry -> typed error; caller-supplied root evidence -> typed error; duplicate/mismatched scoped references fail from verification.
- [ ] 11.4 Confirm default publication/checkpoint path never calls the helper; integration test at lowest conclusive layer using protocol-builtins fixtures with explicitly supplied contexts (no privileged capture tests).

## 12. Determinism tests

- [ ] 12.1 Repeated invocations across simulated restart/retry/replay produce byte-identical graphs including ids and retained witnesses.
- [ ] 12.2 Permuted and hash-adversarial input orders produce identical semantics after canonical re-sorting.
- [ ] 12.3 Known-answer vectors re-verified; guarantee boundary documented exactly (stable list vs not-guaranteed list).

## 13. Architecture/provider dependency checks

- [ ] 13.1 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture`; expect pass, no policy edits beyond inherently allowed external hashing dependency.
- [ ] 13.2 Verify no provider SDK enters touched crates, no new workspace edges, no new crate; `ScenarioRoot` stays inside the non-frozen domain enum without touching frozen contracts.

## 14. Documentation and AGENTS.md updates

- [ ] 14.1 Update `docs/architecture/crate-boundaries.md` (`chronicle-canonical`, `chronicle-etl`): resolver placement, two-stage semantics, composition-only ETL boundary, full-reference scenario-id-v1 scope.
- [ ] 14.2 Update `docs/canonical-model.md`: supported-candidate model aggregated by scenario, predicate table (task lineage contextual), root-establishment semantics, temporal contextual-only rule, confidence definitions, fixpoint chaining, witness retention policy, CorrelationContext join rules, derive-native-correlation-evidence boundary incl. future lineage-semantics contracts.
- [ ] 14.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [ ] 14.4 Update `AGENTS.md` durable invariants concisely: positive candidate-specific support required; absence of eliminating evidence is not support; temporal evidence contextual until timeline guarantees exist; scenario ownership and direct parenthood separate; scenario identity derives from full scoped root reference; native evidence production deferred.
- [ ] 14.5 Record user-facing documentation determination: no CLI/website changes, so no `zh-tw`/`ja` localization updates triggered.
- [ ] 14.6 Run `graphify update .` after production source changes land.

## 15. Full repository/OpenSpec validation

- [ ] 15.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent.
- [ ] 15.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [ ] 15.3 Verify final diff: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/votes/heuristics/timing rules, no temporal elimination code paths, no recursion, no bare-`OperationId` derivation.
- [ ] 15.4 Direct behavioral probes covering acceptance scenarios A–H and prior 1–14 equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: owns Chronicle-native relational semantics — logical task generation, causal execution lineage, process/task inheritance, socket/connection generation relationships, protocol-stream lineage — and may promote named predicates (e.g. a proven task-lineage ownership predicate) via specification change. Until then infrastructure equality fields stay contextual.
- Durable logical-operation / scenario identity surviving publication scope changes (regrouping, republication, regeneration).
- Timeline-guarantee change enabling named safe temporal contradictions.
- `persist-correlation-scenario-artifacts`.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
