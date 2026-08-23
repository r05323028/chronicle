# Implementation tasks

Ordered by dependency. Every task stays inside the approved surfaces: `crates/chronicle-canonical/src/correlation.rs` (+ its Cargo.toml for the workspace hashing dependency), a new `crates/chronicle-etl` composition helper, domain/integration tests, and documentation. No production behavior outside these surfaces may change.

## 1. Scenario identity contract and deterministic derivation

- [ ] 1.1 Add `sha2.workspace = true` to `crates/chronicle-canonical/Cargo.toml`; confirm `validation/architecture.toml` needs no edit (ordinary crate, not a provider package; no workspace edge changes).
- [ ] 1.2 Implement `scenario_id_v1(recording_id: RecordingId, root_operation_id: OperationId) -> ScenarioId` exactly as specified: `SHA-256("chronicle-correlation/scenario-id/v1" ASCII || Uuid::as_bytes() of recording || Uuid::as_bytes() of root operation)`, first 16 bytes as UUID octets, `octets[6] = (octets[6] & 0x0F) | 0x80`, `octets[8] = (octets[8] & 0x3F) | 0x80`. No other hash, encoding, or field order is permitted.
- [ ] 1.3 Add fixed known-answer tests: exact input UUID pairs -> exact expected output UUIDs (computed once from the algorithm), plus a test asserting the domain separator bytes are used verbatim, plus version/variant bit assertions.
- [ ] 1.4 Document the guarantee boundary in the API docs: stable within one recording lineage's persisted operation identities (including regrouping across epoch/session publications); NOT stable across independent re-canonicalization that regenerates `OperationId`s. No doc may claim cross-republication stability.

## 2. Candidate-relative evidence comparison APIs

- [ ] 2.1 Implement the named predicates from the capability spec as pure comparison functions over child-side and candidate-side `CorrelationEvidence`: `SharedTraceIdentity`, `ExplicitParentSpan`, `SameTaskLineage`, shared-span detection, and a contextual-only classification for all remaining kinds. Missing sides yield no relation; no defaults.
- [ ] 2.2 Encode provider namespacing: `(provider, trace_id)` equality requires both fields non-empty and equal across both sides; cross-provider identical values never match.
- [ ] 2.3 Unit-test each predicate's match, non-match (empty/missing fields), cross-provider non-match, and shared-span blocking behavior against spec examples verbatim.

## 3. Positive candidate construction vs contradiction checks

- [ ] 3.1 Build deterministic indexes (`BTreeMap`) keyed by `(provider, trace_id)`, `task`, `(provider, trace_id, span_id)`, and full reference; sort inputs by full reference tuple before processing.
- [ ] 3.2 Construct supported candidates ONLY from fired positive ownership predicates (`SharedTraceIdentity`, `SameTaskLineage`) or inherited membership via `ExplicitParentSpan` chains to resolved members; absence of eliminating evidence contributes nothing.
- [ ] 3.3 Implement the single contradiction check in this change (cross-scenario decisive conflict -> keep both candidates, outcome ambiguous); structure the code so future named contradiction predicates plug in without reshaping the pipeline. Temporal items never participate in any check.

## 4. Stage A: scenario ownership resolution

- [ ] 4.1 Create one scenario candidate per root-eligible operation (`InteractionRoleResolution::is_root_eligible()`), identified by `scenario_id_v1(recording_id, root_operation_id)`.
- [ ] 4.2 Select outcomes over sufficiently supported owners: zero -> `Uncorrelated { retained evidence }`; one -> `Resolved { scenario, Exact }` for direct predicate bindings; two or more -> `Ambiguous { candidates }` with each candidate's own explaining evidence.
- [ ] 4.3 Implement bounded transitive inheritance through explicit parent relations to already-resolved members, emitting `Resolved { scenario, Strong }` when no direct predicate exists; rounds bounded by owning-scenario member count, canonical input order within rounds, explicit recursion cap.
- [ ] 4.4 Copy supplied role resolutions verbatim into the output graph; unknown/ambiguous-role operations resolve as ordinary members, never roots.

## 5. Stage B: direct causal-parent selection

- [ ] 5.1 For each `Resolved` operation, evaluate `ExplicitParentSpan` against members of its own scenario only; emit exactly one `SelectedCausalEdge` when one non-shared target matches.
- [ ] 5.2 Emit NO edge when parents are multiple/equally sufficient, relations are insufficient or contextual-only, or proposed parents belong to another scenario; ownership stays `Resolved` and unresolved parenthood is represented solely by edge absence plus retained resolution evidence.
- [ ] 5.3 Verify emitted graphs pass unchanged foundation validation (`CorrelationGraph::validate`, `validate_against_sessions`) including tree invariants, root-never-child, and ambiguous-never-in-edges.

## 6. Concurrent-ingress behavior

- [ ] 6.1 Test three mutually overlapping ingresses with uniquely related egresses: independent scenarios regardless of start order/duration/overlap.
- [ ] 6.2 Test that timing proximity, recency, openness, and processing order cannot influence any outcome (construct closest-ingress-differs-from-bound-owner cases).
- [ ] 6.3 Test partial-overlap candidate-specific ambiguity: `Ambiguous(A, B)` while unrelated Ingress C never appears and another egress resolves normally in the same run.

## 7. Async / post-completion causal cases

- [ ] 7.1 Test ingress completing before egress start with task/trace relation binding them: resolves with lifetime disjointness ignored.
- [ ] 7.2 Test decisive linkage over entirely earlier candidate lifetimes: full predicate force retained.
- [ ] 7.3 Test temporal-only inputs: uncorrelated with temporal evidence retained; overlap count/proximity never matters; reverse-ordering cases perform no elimination.

## 8. Cross-epoch reference handling

- [ ] 8.1 Test root owned by epoch N session and egress by epoch N+1 session resolving into one scenario; selected edge carries both full scoped references.
- [ ] 8.2 Test terminal completion-owner scope participation and continuation-provenance exclusion as second subjects.
- [ ] 8.3 Test identical `OperationId`s under different session scopes remain distinct subjects; test publication regrouping preserves derived scenario IDs.

## 9. CorrelationContext and ETL composition API

- [ ] 9.1 Define the correlation context value (role resolutions + evidence keyed by full `CanonicalOperationRef`) in `chronicle-canonical`; sessions alone are not valid resolver input.
- [ ] 9.2 Add the `chronicle-etl` composition helper joining published sessions with supplied context: verify references via existing lineage resolution, enrich lifetime/provenance views, join context by exact reference; zero selection semantics.
- [ ] 9.3 Implement and test the join rules: missing role entry -> typed error naming the reference; missing evidence entry -> empty evidence; orphan context entry -> typed error; duplicate/invalid session operations surface existing verification errors.
- [ ] 9.4 Confirm the default publication/checkpoint path does not call the helper; add an integration test at the lowest conclusive layer using protocol-builtins fixtures with explicitly supplied contexts (no privileged capture tests).

## 10. Determinism tests

- [ ] 10.1 Repeat resolution across simulated restart/retry/replay invocations: byte-identical graphs including derived `ScenarioId`s.
- [ ] 10.2 Permuted and hash-adversarial iteration orders produce semantically identical results after canonical re-sorting.
- [ ] 10.3 Known-answer identity vectors re-verified; regrouping same identified operations into different session groupings keeps IDs; regenerated-operation re-canonicalization is documented as legitimately unstable.

## 11. Boundedness/complexity tests

- [ ] 11.1 Property test: work grows with index bucket hits, not operation-times-ingress-times-evidence scans (assert via instrumented counters on synthetic wide inputs).
- [ ] 11.2 Chaining terminates on deep chains within member-count bounds with recursion capped; results match fixpoint semantics.
- [ ] 11.3 Evidence retention cap: overflow drops contextual items last-in-first-out deterministically and never drops predicate-bearing justification items; outcomes stay stable under cap pressure.

## 12. Architecture/provider-dependency validation

- [ ] 12.1 Run `./scripts/run-with-timeout.sh 300 python3 scripts/validation.py architecture`; expect pass with no policy edits beyond the new external hashing dependency being inherently allowed.
- [ ] 12.2 Verify no provider SDK package enters any touched crate and no provider type name appears in canonical APIs; verify no new workspace edges and no new crate.

## 13. Documentation updates

- [ ] 13.1 Update `docs/architecture/crate-boundaries.md` (`chronicle-canonical`, `chronicle-etl` sections): resolver placement, two-stage semantics, composition-only ETL boundary, scenario-id-v1 identity scope.
- [ ] 13.2 Update `docs/canonical-model.md`: supported-candidate model, relational predicates table, temporal contextual-only rule, confidence semantics, CorrelationContext join rules, boundedness contract, derive-native-correlation-evidence boundary.
- [ ] 13.3 Update `docs/architecture/overview.md` only if its runtime flow mentions correlation; one paragraph maximum.
- [ ] 13.4 Update `AGENTS.md` durable invariants concisely: absence of eliminating evidence is not support; temporal evidence is contextual only until timeline guarantees exist; scenario ownership and direct parenthood are separate decisions; native evidence production is a deferred capability.
- [ ] 13.5 Record user-facing documentation determination: no CLI/website behavior changes, so no `zh-tw`/`ja` localization updates triggered.
- [ ] 13.6 Run `graphify update .` after production source changes land.

## 14. Full repository/OpenSpec validation

- [ ] 14.1 Run `openspec validate --all --strict --no-interactive` after implementation edits keep artifacts consistent.
- [ ] 14.2 Run `./scripts/validate.sh fast` plus targeted changed-path validation; fix findings before completion.
- [ ] 14.3 Verify the final diff contains: no frozen v1 mutation, no persisted artifact, no new crate, no `EventId`, no provider SDK dependency, no scores/weights/heuristics, no temporal elimination code paths.
- [ ] 14.4 Direct behavioral probes covering acceptance scenarios A–J and scenarios 1–14 equivalents; retain evidence at the lowest conclusive layer.

## Explicitly deferred follow-ups

- `derive-native-correlation-evidence`: derive provider-neutral correlation evidence from Chronicle-controlled sources (task/process lineage, socket/connection generations, protocol streams, capture/session reconstruction facts). Until it lands, real recordings supply no relational evidence and mostly-`Uncorrelated` outcomes are correct.
- `persist-correlation-scenario-artifacts`: versioned sidecar/new artifact decision for persisted graphs.
- Identity/versioning change for cross-republication scenario stability if ever required (beyond persisted operation identities).
- Timeline-guarantee change enabling named safe temporal contradictions.
- `integrate-pluggable-trace-evidence-providers` then `trace-provider-plugin-installation`.
- Scenario inspect/UX commands, scenario replay, test-case generation, assertions.
- Event identity (`EventId`), only if a future feature proves it necessary.
