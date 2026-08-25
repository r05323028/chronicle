# Implementation tasks

Ordered by dependency. Do not mark any task complete in this planning change. No production implementation is included here.

## 1. Domain contract and compatibility boundary

- [ ] 1.1 Add additive non-frozen `CorrelationEvidenceKind::NativeExecutionLineage { parent: CanonicalOperationRef, relation: ExecutionContinuation }` and its Chronicle-owned relation value; keep `is_temporal() == false` and serde tagging deterministic.
- [ ] 1.2 Define serializable `NativeExecutionHandoffFact { parent, child, relation, provenance }` at the existing canonical/ETL composition boundary without adding a new crate, `EventId`, or provider type.
- [ ] 1.3 Validate native evidence provenance, complete parent scope, same-recording scope, and relation shape; reject orphan/cross-recording/unverified references before resolver admission without identifier or ordering fallbacks.
- [ ] 1.4 Preserve role/correlation channel separation: native items are accepted only in explicit correlation evidence; caller-supplied `ScenarioRoot` reservation and role evidence byte-preservation remain unchanged.
- [ ] 1.5 Add compatibility assertions that Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted ETL checkpoints, public CLI JSON, replay-safety contracts, and default dependency closure are unchanged.

## 2. Chronicle-owned native fact source and ETL derivation

- [ ] 2.1 Define the bounded Chronicle-owned runtime handoff source contract and optional application wiring; source records explicit parent-to-child handoff events, not task/process/socket equality or trace-provider context.
- [ ] 2.2 Verify each fact against supplied canonical sessions and exact `CanonicalOperationRef` lineage, including recording, owner epoch, session, and unique operation occurrence; support cross-epoch parent/child references.
- [ ] 2.3 Implement deterministic fact normalization: canonical total ordering by child reference, parent reference, relation kind, and complete provenance; exact duplicate facts are idempotent.
- [ ] 2.4 Enforce `32` native facts per child per batch, `4096` facts per batch, and `4096` active handoff contexts; overflow reports bounded loss or fails closed without silently truncating positive relations.
- [ ] 2.5 Emit only child-side `NativeExecutionLineage` correlation evidence plus bounded derivation diagnostics; producer does not create scenarios, resolutions, confidence, selected edges, or role changes.
- [ ] 2.6 Make absent runtime facts a deterministic empty native-evidence result; do not add Capture Event/WAL/checkpoint persistence or a default CLI path for this slice.

## 3. Resolver integration

- [ ] 3.1 Add an immutable ordered native-parent relation index over validated correlation-channel evidence, keyed by full child/parent references; reject self/orphan/cross-recording relations before semantic evaluation.
- [ ] 3.2 Extend Phase A1 with directional native `child -> parent` transitive support using previous-round snapshots only; preserve sparse monotonic support closure and no witness-path storage.
- [ ] 3.3 Extend Phase A2 witness/confidence derivation so native-only unique support uses existing `Strong` semantics, conflicting scenario support remains `Ambiguous`, and no direct trace/`Exact` witness is fabricated.
- [ ] 3.4 Extend Phase B candidate collection to include native parent relations, then run existing pre-filter, unique-parent, global SCC, internal-edge removal, and canonical edge ordering unchanged.
- [ ] 3.5 Combine native and trace positive support in one resolver channel with no weights, source priority, negative votes, or provider coupling; preserve deterministic retention and role evidence verbatim.

## 4. Domain and resolver unit tests

- [ ] 4.1 Test native evidence tagging, round-trip serialization, non-temporal classification, full parent scope, provenance, and invalid orphan/cross-recording placement.
- [ ] 4.2 Test role-channel isolation: native-shaped role evidence never supports correlation; explicit correlation evidence does; supplied role evidence/order remains byte-identical.
- [ ] 4.3 Test contextual-only negatives for task labels, PID/TID/process generations, socket/file-descriptor/connection identifiers, protocol streams/sequences, byte direction, socket role, protocol ownership, timestamps, overlap, and absence of contradiction.
- [ ] 4.4 Test root pinning and root-as-child behavior with native relations; `ScenarioRoot` remains resolver-generated and caller-reserved.
- [ ] 4.5 Test native-only transitive ownership, `Strong` confidence, direct parent witness retention, no native relation -> `Uncorrelated`, and competing parents -> `Ambiguous`.
- [ ] 4.6 Test native parent self-relations, root-as-child relations, multi-parent filtering, cycles, SCC edge removal, and unchanged ownership/role states.

## 5. Concurrency, async, coexistence, and scope tests

- [ ] 5.1 Test three overlapping ingress roots A/B/C with explicit A1/A2/A3, B1/B2, and C1 handoffs; assert no cross-correlation under every input permutation.
- [ ] 5.2 Test mixed database and outbound HTTP children through the same native relation contract; protocol kind does not alter ownership semantics or completeness/replayability.
- [ ] 5.3 Test async/post-response child A3 with a later relative offset; assert explicit handoff correlates and timing-only control does not.
- [ ] 5.4 Test native and trace evidence agreeing on one scenario, disagreeing across scenarios, and supplying competing direct parents; assert existing ambiguity/edge rules and both provenances.
- [ ] 5.5 Test cross-epoch parent/child references and terminal completion-owner scope with full lineage verification.
- [ ] 5.6 Test same `OperationId` under different session/epoch scopes, reused task/PID/TID, reused file descriptor/socket/connection identifiers, and stale generation anchors; assert no accidental relation.

## 6. ETL integration and deterministic behavior

- [ ] 6.1 Add integration coverage for canonical sessions plus `NativeExecutionHandoffFact` composition through `chronicle-etl`, proving direct resolver invocation and composed invocation produce identical graphs.
- [ ] 6.2 Test missing role context, orphan facts, invalid session lineage, duplicate facts, ambiguous fact mapping, and bounded-loss diagnostics fail closed without guessed replacements.
- [ ] 6.3 Test forward/reverse/shuffled facts, hash-map insertion orders, duplicate retries, worker scheduling, and repeated derivation produce byte-identical evidence, diagnostics, memberships, witnesses, and edges.
- [ ] 6.4 Test restart with the same serializable canonical sessions and native fact set produces identical output; test canonical-only replay without facts produces deterministic empty native support.
- [ ] 6.5 Assert default ETL publication/checkpoint behavior remains unchanged and does not invoke native derivation unless explicitly composed.

## 7. End-to-end and acceptance proof

- [ ] 7.1 Add a portable end-to-end/acceptance fixture using the production-shaped Chronicle-owned handoff source, overlapping A/B/C traffic, database and HTTP children, and async continuation; run with no external trace package.
- [ ] 7.2 Assert acceptance output is a valid `CorrelationGraph` under supplied canonical sessions, preserves role states, keeps ambiguous/unrelated operations out of membership/edges, and leaves completeness/replayability independent.
- [ ] 7.3 Add a no-native-relationship scenario with one active ingress and assert the egress remains `Uncorrelated`; add genuine native ambiguity and assert `Ambiguous`.
- [ ] 7.4 Add an architecture/dependency probe proving no provider SDK enters core/domain or default executable closure and no new crate/edge is required.

## 8. Boundedness and reliability evidence

- [ ] 8.1 Prove active handoff state and batch/per-child limits with boundary and overflow tests; retain typed bounded-loss evidence without silent relation truncation.
- [ ] 8.2 Prove long recordings/epoch rollover retain only bounded one-hop native relations and never store transitive ancestry paths or operation-by-scenario matrices in the producer.
- [ ] 8.3 Prove native relation derivation does not change replayability, operation completeness, WAL recovery authority, checkpoint ordering, or loss accounting.

## 9. Architecture, documentation, and repository contracts

- [ ] 9.1 Run dependency-direction and semantic-boundary validation; update `validation/architecture.toml`, `docs/architecture/crate-boundaries.md`, and `AGENTS.md` together only if an implementation dependency/ownership edge genuinely changes.
- [ ] 9.2 Review `docs/architecture/overview.md`, `docs/canonical-model.md`, capture/session/protocol/ETL docs, and user-facing docs for impact; document native runtime/composition limits and no-fact behavior where affected.
- [ ] 9.3 Update `AGENTS.md` durable correlation guidance to state that only explicit candidate-scoped Chronicle-native handoff relations may support native ownership, while task/process/socket/connection/stream/timing equality remains forbidden; keep role/evidence/provider/frozen-contract invariants intact.
- [ ] 9.4 If implementation changes canonical English user-facing documentation, update corresponding `website/zh-tw` and `website/ja` pages and run website localization verification; otherwise record documentation as unaffected.
- [ ] 9.5 Run `openspec validate --all --strict --no-interactive`, the relevant architecture/documentation checks, and `./scripts/validate.sh fast` after implementation; retain direct end-to-end evidence separately from planning validation.
