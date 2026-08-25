## 1. Current-fact inventory and contract placement

- [ ] 1.1 Record verified current identity facts from `chronicle-session`, `chronicle-protocol`, protocol builtins, and ETL: `SourceConnectionGeneration`, protocol-local sequence, `WalByteRange`, epoch ranges, completion-owner epoch, and late deterministic `OperationId` assignment.
- [ ] 1.2 Confirm no existing execution-context or handoff hook exists in recorder/application composition; document passive-only behavior and the exact missing boundary receipt.
- [ ] 1.3 Place non-frozen observation, anchor, binding-result, bound-fact, and diagnostic contracts at the existing `chronicle-etl` composition boundary without adding a crate or reverse dependency.
- [ ] 1.4 Define application-owned source wiring against the ETL contract; keep `chronicle-canonical` limited to domain evidence and resolver semantics.

## 2. Cooperative Chronicle-native source

- [ ] 2.1 Implement the opt-in Chronicle execution-context carrier with generation-safe context lifecycle; do not use PID/TID/task/socket/connection/stream equality or provider context as the carrier.
- [ ] 2.2 Implement explicit parent-to-child `ExecutionContinuation` handoff recording before final canonical references exist; source emits observations, never scenarios or resolver outcomes.
- [ ] 2.3 Implement the Chronicle transport/application boundary adapter that emits an exact operation-boundary receipt for each supported protocol path.
- [ ] 2.4 Carry recording scope, source epoch placement, source generation, protocol identity, direction, protocol-local operation position, and exact source/reconstruction provenance into each receipt.
- [ ] 2.5 Deliver observations through a bounded non-frozen application/ETL side channel; define typed overflow, missing-receipt, restart, and loss diagnostics.
- [ ] 2.6 Prove source can retain a pending observation until boundary provenance is complete, then either complete the receipt or fail closed; never emit a guessed anchor.
- [ ] 2.7 Keep passive capture and existing recorder/WAL publication unchanged when cooperative integration is absent.

## 3. Pre-canonical anchor and exact binder

- [ ] 3.1 Implement `NativeOperationAnchor` as a complete pre-canonical boundary identity, not a `CanonicalOperationRef` alias and not a bare identifier.
- [ ] 3.2 Reject anchors missing generation-safe source scope, exact protocol-local operation position, or exact source/reconstruction provenance; reject PID/TID/task/FD/socket/connection/stream/WAL/timestamp-only anchors.
- [ ] 3.3 Build deterministic canonical-operation candidates from supplied sessions, owning connections, operation provenance, source epoch ranges, protocol boundaries, and full session scope.
- [ ] 3.4 Bind anchor to exactly one canonical operation using all required composite predicates; use verified completion-owner epoch only when constructing final `CanonicalOperationRef`.
- [ ] 3.5 Emit distinct deterministic `AnchorBindingUnresolved` and `AnchorBindingAmbiguous` diagnostics for zero and multiple matches; never apply nearest/first/last/active/order/role/protocol fallbacks.
- [ ] 3.6 Bind parent and child independently, permit cross-epoch source/completion ownership, and reject cross-recording, self-parent, orphan, duplicate-occurrence, or incomplete-scope results.
- [ ] 3.7 Normalize exact duplicate observations by canonical anchor/relation/provenance ordering; retain independently valid conflicting facts for resolver ambiguity.
- [ ] 3.8 Add a protocol-adapter contract proving each supported boundary receipt maps to one canonical operation or fails closed; unsupported adapters remain unresolved.

## 4. Bound facts and canonical evidence

- [ ] 4.1 Create `BoundNativeExecutionHandoffFact` only after both endpoints bind exactly and same-recording, direction, scope, and operation validation pass.
- [ ] 4.2 Derive only child-side `NativeExecutionLineage { parent, ExecutionContinuation }` correlation evidence with Chronicle-owned provenance; do not emit scenarios, confidence, witnesses, edges, or roles.
- [ ] 4.3 Keep binding diagnostics outside `CorrelationResolution::Ambiguous`; emit no positive evidence for unresolved or multi-match anchors.
- [ ] 4.4 Ensure missing native observations yield empty native evidence without changing default ETL publication, checkpoint, completeness, replayability, or WAL recovery behavior.

## 5. Existing resolver integration

- [ ] 5.1 Index only validated child-to-parent native evidence from the correlation channel; preserve role-channel exclusion and caller-supplied `ScenarioRoot` reservation.
- [ ] 5.2 Feed native relations into existing Phase A1 sparse previous-round support closure with deterministic ordering and no second selector.
- [ ] 5.3 Preserve existing Phase A2 outcome, `Strong` transitive confidence, witness retention, and no fabricated `Exact` semantics.
- [ ] 5.4 Feed native direct-parent candidates through existing pre-filter → unique-parent → complete provisional graph → SCC → internal-edge removal sequence.
- [ ] 5.5 Union native and trace positive support without weights, priority, negative votes, or source-order effects; preserve genuine cross-scenario ambiguity.
- [ ] 5.6 Keep contextual variants, role evidence, roots, completeness, replayability, and graph validation semantics unchanged.

## 6. Domain and resolver unit tests

- [ ] 6.1 Test native evidence tagging, serialization, non-temporal classification, child-channel placement, full parent scope, and provenance preservation.
- [ ] 6.2 Test invalid orphan, cross-recording, wrong epoch/session, duplicate occurrence, self-parent, and binding-diagnostic inputs fail closed.
- [ ] 6.3 Test native-shaped role evidence cannot enter correlation support and supplied role evidence remains unchanged.
- [ ] 6.4 Test contextual-only PID/TID/task/FD/socket/connection/stream/protocol/timing/direction evidence remains uncorrelated without explicit native lineage.
- [ ] 6.5 Test native-only transitive ownership yields existing `Strong`, not fabricated `Exact`.
- [ ] 6.6 Test native parent cycles, multiple parents, root-as-child, SCC edge removal, and resolver root generation follow existing rules.
- [ ] 6.7 Test native and trace agreement corroborates without priority; disagreement remains resolver `Ambiguous`.
- [ ] 6.8 Test zero/multiple pre-canonical binding matches produce diagnostics with no resolver candidate, distinct from true causal ambiguity.

## 7. ETL composition/integration proof

- [ ] 7.1 Test already-bound fact → ETL composition → native evidence → existing resolver → validated `CorrelationGraph`; label as composition/integration only.
- [ ] 7.2 Test anchor observation before canonicalization, later exact binding, cross-epoch parent/child, missing receipt, zero match, multiple match, duplicate delivery, and orphan context.
- [ ] 7.3 Test shuffled observations, canonical sessions, map insertion orders, retry delivery, and worker scheduling produce byte-identical facts, diagnostics, evidence, witnesses, memberships, and edges.
- [ ] 7.4 Test canonical artifacts without cooperative observations remain deterministic and produce no native support.
- [ ] 7.5 Test default ETL publication/checkpoint path does not invoke native composition unless explicitly configured.

## 8. Native production E2E proof

- [ ] 8.1 Build a real opt-in cooperative runtime/transport harness that starts from explicit runtime handoffs and pre-canonical observations, not manually constructed `CanonicalOperationRef` facts.
- [ ] 8.2 Exercise overlapping A/B/C ingress operations and assert parent/child isolation through observation, anchor binding, evidence, resolver, and graph validation.
- [ ] 8.3 Exercise an asynchronous/post-response continuation and assert explicit handoff correlation without timing or proximity selection.
- [ ] 8.4 Exercise reuse of PID/TID/task, FD, socket, connection, and stream values across generations; assert stale anchors do not bind to new operations.
- [ ] 8.5 Exercise missing native observation with one active ingress and no trace evidence; assert non-root egress remains `Uncorrelated`.
- [ ] 8.6 Exercise one binding-ambiguous anchor and two independently bound competing parents; assert first emits no relation while second yields resolver `Ambiguous`.
- [ ] 8.7 Exercise native/trace agreement and disagreement under the same existing resolver rules.
- [ ] 8.8 Exercise cooperative mode with no OpenTelemetry or provider package and prove default dependency closure remains provider-free.
- [ ] 8.9 Label any pre-bound fixture path as composition proof and prevent it from satisfying production-E2E acceptance alone.

## 9. Bounds, loss, restart, and compatibility

- [ ] 9.1 Bound active context, pending observations, per-child facts, batch work, diagnostics, and one-hop retained relations without adding normative arbitrary count constants.
- [ ] 9.2 Test configured overflow and side-channel loss fail closed or emit typed bounded-loss diagnostics; never silently truncate positive relations while claiming completeness.
- [ ] 9.3 Test no transitive ancestry storage, no operation-by-scenario matrix, and stable memory behavior across long handoff chains.
- [ ] 9.4 Test restart without durable native side-channel recovery produces no guessed relation and leaves WAL authority, checkpoints, completeness, replayability, and loss accounting unchanged.
- [ ] 9.5 Assert Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public CLI JSON, replay safety, and no-`EventId` rules remain unchanged.

## 10. Architecture, documentation, and validation

- [ ] 10.1 Run architecture validation and confirm observation source, ETL binding, canonical domain/resolver, session/protocol fact exposure, and CLI boundaries use existing dependency direction.
- [ ] 10.2 If implementation requires a new dependency edge, update `validation/architecture.toml`, `docs/architecture/crate-boundaries.md`, and `AGENTS.md` together; otherwise record no edge change.
- [ ] 10.3 Update affected architecture/operations/canonical-model documentation to describe passive-only versus cooperative-native capability, exact binding, and fail-closed loss behavior.
- [ ] 10.4 Review user-facing documentation impact; update localized website pages only if canonical user-facing English documentation changes.
- [ ] 10.5 Run `openspec validate --all --strict --no-interactive`, relevant architecture/documentation checks, and `./scripts/validate.sh fast`; retain separate composition and native production-E2E evidence.
