## Context

See `proposal.md` for motivation. Current code proves canonical identity only late in the pipeline:

```text
CaptureEvent / WAL
  → ReconstructionConnectionIdentity
  → ProtocolNeutralConnection / ProtocolStream
  → DecodedFrame
  → protocol canonicalizer returns CanonicalizedOperation
  → ETL session construction and deterministic OperationId normalization
  → CanonicalSession
  → CanonicalOperationRef
```

Relevant current facts:

- `chronicle-session` groups capture into `ReconstructionConnectionIdentity::Fixture` or generation-safe socket identity.
- `chronicle-protocol::reconstructed_frames` maps that identity to `SourceConnectionGeneration` and carries frame `sequence`, `connection_generation`, and `WalByteRange` provenance in `DecodedFrame`.
- `ProtocolStream` exposes protocol chunks and local sequence values. Existing HTTP canonicalization uses request/response frame positions, but the current pipeline has no native execution-handoff field.
- `CanonicalOperation` retains `sequence`, `OperationProvenance.connection_generation`, `wal_ranges`, `epoch_ranges`, and `completion_owner_epoch`. `CanonicalOperationRef` adds recording, owner epoch, session, and final operation scope.
- `chronicle-etl` assigns deterministic final operation IDs only after canonicalization by hashing the canonical operation with its ID cleared. `OperationId` therefore cannot be a runtime anchor.
- `CanonicalOperationRef::resolve_in_session` and `chronicle-etl::compose_correlation` already perform full recording/epoch/session/operation verification. The existing resolver receives only fully scoped `CorrelationInput` values.
- `RecorderOrchestrator` currently owns capture polling, bounded ingest, and WAL admission. CodeGraph/Graphify inspection found no existing execution-context or handoff hook.

`SourceConnectionGeneration` is generation-safe connection context, not operation identity. WAL sequence is ordering/provenance, not causal identity. Protocol position is useful only when paired with source generation and exact recording/epoch/reconstruction lineage. The current code therefore lacks one minimal non-frozen fact: a Chronicle-native operation-boundary receipt emitted by an opted-in runtime/transport integration and tied to the same source facts later retained by canonical operations.

## Goals / Non-Goals

**Goals:**

- Make every stage from runtime handoff to canonical evidence explicit.
- Select one realistic first source: cooperative Chronicle-native execution context plus a Chronicle transport/application boundary adapter.
- Bind pre-canonical anchors exactly to canonical operations or fail closed.
- Preserve positive candidate-specific evidence, role-channel isolation, existing resolver authority, ambiguity, provider neutrality, and frozen contracts.
- Prove both already-bound composition and real observation-to-resolver production flow.

**Non-Goals:**

- Passive eBPF inference of application causality.
- Generic process/task/socket/connection/stream lineage.
- Timing, proximity, active-ingress, protocol-kind, role, WAL-order, or processing-order heuristics.
- OpenTelemetry or any provider SDK integration.
- Persisting observations, bound facts, scenario graphs, or adding `EventId`.
- Changing Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, checkpoints, CLI JSON, replay safety, or resolver phase ordering.

## Decisions

### 1. First production source: cooperative Chronicle-native context carrier

The first source is an opt-in application/runtime integration, not passive recorder inference:

```text
application/runtime
    │ explicit Chronicle context transfer
    ▼
cooperative native source + transport boundary adapter
    │ NativeExecutionHandoffObservation
    ▼
bounded non-frozen side channel
    │ exact pre-canonical anchors
    ▼
chronicle-etl anchor binding against canonical sessions
    ▼
BoundNativeExecutionHandoffFact
    ▼
NativeExecutionLineage in child correlation evidence
    ▼
existing chronicle-canonical resolver
```

The source owns an opaque Chronicle execution context with a generation that prevents stale context reuse. At an explicit application/runtime handoff, the parent context is passed through a Chronicle-owned API or helper into the child operation. The transport boundary adapter used by the integration supplies the child operation-boundary receipt; a generic task, process, socket, or trace identifier cannot substitute for it.

Source lifecycle:

1. The application/runtime integration creates a Chronicle execution context at an operation boundary that it explicitly owns.
2. A child operation receives that context through the Chronicle-native carrier and calls the explicit continuation handoff API before or at child invocation.
3. The Chronicle transport boundary adapter records the exact pre-canonical source receipt for each parent and child operation. If the receipt is not complete until capture/WAL placement, the side channel retains a pending observation only until that receipt is completed.
4. The source delivers observations to ETL composition through an application-owned bounded handoff channel. ETL consumes them only after the relevant canonical sessions are available.
5. The observation remains pre-canonical until ETL binding. The source never sees or invents `CanonicalOperationRef`, `ScenarioId`, confidence, selected edges, or resolver outcomes.
6. On side-channel loss, process restart, missing receipt, or incompatible source lineage, ETL emits typed unresolved/bounded-loss diagnostics and no positive native relation. Durable observation persistence is a later versioned sidecar change.

Application/runtime requirements are explicit: opt-in Chronicle integration, explicit continuation calls, access to the Chronicle transport boundary adapter, and agreement to deliver the non-frozen side-channel records until binding. Applications without that integration remain fully supported in passive-only mode.

### 2. Pre-canonical concepts are separate

The implementation SHALL keep these semantic stages separate:

```text
NativeExecutionHandoffObservation
  ├─ parent: NativeOperationAnchor
  ├─ child:  NativeOperationAnchor
  ├─ relation: ExecutionContinuation
  └─ provenance: Chronicle-owned native source provenance

NativeOperationAnchor
  └─ complete generation-safe source/boundary identity; no canonical reference

AnchorBindingResult
  ├─ Bound(CanonicalOperationRef)
  ├─ Unresolved(diagnostic)
  └─ Ambiguous(diagnostic)

BoundNativeExecutionHandoffFact
  ├─ parent: CanonicalOperationRef
  ├─ child: CanonicalOperationRef
  ├─ relation: ExecutionContinuation
  └─ source provenance / binding diagnostics

CorrelationEvidence::NativeExecutionLineage
  └─ child-side candidate-specific evidence consumed by existing resolver
```

Conceptual non-frozen shapes:

```rust
NativeOperationBoundaryReceipt {
    recording_id: RecordingId,
    source_epoch_id: EpochId,
    source_generation: SourceConnectionGeneration,
    protocol_id: ProtocolId,
    direction: Direction,
    protocol_operation_position: u64,
    source_range: WalByteRange,
}

NativeOperationAnchor {
    boundary: NativeOperationBoundaryReceipt,
    // Chronicle execution-context generation is provenance and reuse protection;
    // it is never an ownership predicate by itself.
    context_generation: NativeExecutionContextGeneration,
}

NativeExecutionHandoffObservation {
    parent: NativeOperationAnchor,
    child: NativeOperationAnchor,
    relation: ExecutionContinuation,
    provenance: NativeObservationProvenance,
}
```

Names are semantic contract names, not required final Rust spelling. `NativeOperationBoundaryReceipt` is the minimal new non-frozen fact current code lacks. Its fields are deliberately composed from real current facts: recording/epoch placement from recorder/ETL lineage, `SourceConnectionGeneration`, protocol-local operation position, and exact `WalByteRange` source provenance. A receipt with only PID, TID, task ID, worker name, file descriptor, socket cookie, connection ID, stream ID, WAL sequence, timestamp, or position without its generation-safe scope is invalid.

The receipt is bounded: one operation-start boundary and one source-range identity, not a transitive ancestry list. An implementation may use an equivalent exact protocol-local boundary representation only when it proves the same reuse and uniqueness properties for each supported protocol adapter. Current adapters that cannot provide this receipt are unsupported by cooperative native binding and produce no relation.

### 3. Exact anchor-to-canonical binding

ETL builds a deterministic candidate index from supplied canonical sessions. Each anchor is matched against canonical operation plus owning connection using all of these checks:

1. `recording_id` equals the session recording scope.
2. `source_epoch_id` and `source_range` match one exact source placement in the operation's retained epoch/WAL provenance. This is source epoch, not necessarily `completion_owner_epoch`; an operation may complete in a later epoch.
3. `source_generation` equals the operation's retained `OperationProvenance.connection_generation`.
4. `protocol_id` and direction match the owning canonical connection and operation boundary.
5. `protocol_operation_position` matches the protocol canonicalizer's exact operation boundary and the same retained source range. Protocol kind alone never matches.
6. The candidate occurrence is unique under the canonical session's full scope. `CanonicalOperationRef::resolve_in_session` performs the final recording, owner-epoch, session, and operation uniqueness checks.

Exactly one candidate produces:

```text
CanonicalOperationRef::new(
    recording_id,
    operation.provenance.completion_owner_epoch,
    session.id,
    operation.id,
)
```

The owner epoch in the resulting reference is the canonical operation's verified completion owner. It is not guessed from the anchor's source epoch. Parent and child bind independently, so cross-epoch continuation is allowed when both references bind exactly.

Zero candidates produce `AnchorBindingUnresolved`. Multiple candidates produce `AnchorBindingAmbiguous`. Both are binding diagnostics, not `CorrelationResolution::Ambiguous`; ETL emits no bound fact and no positive evidence in either case. The binder MUST NOT choose by nearest timestamp, first/last operation, active ingress, processing order, WAL order, PID/TID equality, socket/connection equality, protocol kind, role, absence of other candidates, or any other fallback.

If either endpoint fails binding, the observation is discarded from positive derivation. If both endpoints bind, ETL verifies same recording, complete reference scope, parent/child existence, and relation direction before creating the bound fact. Exact duplicate observations are idempotent under a canonical tuple of both anchors, relation, and source provenance; conflicting observations remain separate facts and are resolved by the existing resolver.

### 4. Ownership by crate and dependency direction

- `chronicle-application` owns the opt-in cooperative source wiring, context carrier lifecycle, transport/application boundary adapter, and delivery into ETL. It does not select scenarios.
- `chronicle-etl` owns the non-frozen observation/anchor/bound-fact contracts at the composition boundary, exact binding against canonical sessions, deterministic normalization, bounded-loss diagnostics, and conversion to correlation-channel evidence.
- `chronicle-canonical` owns `NativeExecutionLineage` domain validation, role/correlation channel rules, native relation predicates, support closure, witnesses, confidence, direct-parent candidates, and final graph validation.
- `chronicle-session` and `chronicle-protocol` expose reconstruction identity, source generation, protocol-local position, and provenance. They do not infer or select causal ownership.
- `chronicle-protocol-builtins` supplies protocol-local boundary receipts only through the existing protocol-owned contracts; it does not depend on `chronicle-application` or choose scenarios.
- `chronicle-cli` remains unaware of native evidence and correlation semantics.

This split follows existing direction: application already composes ETL; ETL already depends on canonical/session/protocol; canonical owns the domain/resolver. No new crate or dependency edge is required. `validation/architecture.toml`, `docs/architecture/crate-boundaries.md`, and `AGENTS.md` need no dependency-policy changes unless implementation discovers otherwise.

### 5. Evidence and resolver integration remain additive

ETL emits only child-side `NativeExecutionLineage` for bound facts. It does not create scenarios, confidence, selected edges, role classifications, or transitive paths. The existing resolver indexes the native relation alongside existing positive trace relations:

- Phase A1 uses previous-round support snapshots for directional `child → parent` sparse closure.
- Phase A2 derives outcomes, existing `Strong` transitive confidence, and bounded witnesses without fabricating `Exact` trace evidence.
- Phase B adds validated native parent candidates to the existing pre-filter → unique-parent → complete provisional graph → SCC → internal-edge removal sequence.
- Native and trace support are unioned with no weights, priority, or negative votes. Different supported scenarios remain `Ambiguous`.
- Role evidence remains a separate channel and is preserved byte-for-byte.

### 6. Bounds, loss, restart, and determinism

The contract requires properties, not arbitrary numeric constants:

- active native source state, per-child pending state, side-channel batches, binding work, and retained one-hop facts MUST be bounded;
- observations MUST NOT retain transitive ancestry or operation-by-scenario matrices;
- overflow MUST fail closed or emit typed bounded-loss diagnostics;
- positive relations MUST NOT be silently truncated while claiming complete native evidence;
- exact duplicate retries MUST be idempotent;
- canonical ordering of anchors, diagnostics, facts, and evidence MUST be independent of input order, worker scheduling, hash order, retry, and restart over the same supplied inputs;
- missing/restarted side-channel input produces no native relation and never changes replayability, WAL authority, checkpoint ordering, or completeness;
- implementation defaults may be named/configured after resource measurement, but no default count is a normative OpenSpec semantic in this change.

### 7. Proof layers

The implementation must report two separate proofs:

**Composition/integration proof:** start with an already-bound `BoundNativeExecutionHandoffFact`; compose canonical sessions through ETL; derive `NativeExecutionLineage`; run the existing resolver; validate `CorrelationGraph`. This proves the canonical composition and resolver vertical slice only.

**Native production E2E proof:** start with an opt-in cooperative runtime handoff in an overlapping A/B/C application scenario; observe parent/child anchors through the real side channel; bind them against canonical sessions; derive the bound fact and evidence; run the resolver; validate the scenario graph. The test must not manually construct final `CanonicalOperationRef` handoff facts as its starting input. It must include exact, zero-match, multi-match, identifier-reuse, async continuation, missing-observation, binding-ambiguity, causal-ambiguity, native+trace agreement/disagreement, passive-only, and no-provider cases.

A fixture that begins at `CanonicalOperationRef` is labeled composition/integration, never production E2E.

## Risks / Trade-offs

- **[No passive causal proof]** → Passive-only mode remains conservative and documentation names cooperative integration as required for native causality.
- **[Boundary receipt unavailable or protocol adapter cannot prove exact position]** → Emit unresolved binding diagnostics and no relation; add support only after adapter-specific proof.
- **[Side-channel loss or restart]** → Typed bounded-loss evidence, no guessed relation, no frozen-artifact changes; durable sidecar is separate work.
- **[Identifier reuse]** → Require complete source generation, recording/epoch lineage, protocol position, and exact source range; reject zero/multiple matches.
- **[Cross-epoch completion]** → Match source epoch through retained epoch/WAL provenance, then use verified completion owner epoch for `CanonicalOperationRef`.
- **[Protocol canonicalizer drift]** → Add contract tests that receipt position and retained canonical provenance identify exactly one operation for each supported adapter.
- **[Unbounded producer growth]** → Bound active state and one-hop facts, make overflow typed, and never claim completeness after positive truncation.

## Migration Plan

This is additive and opt-in. Existing passive capture, WAL recovery, canonical session construction, ETL publication/checkpoints, CLI output, replay, and trace adapters continue unchanged. Implementation first adds the non-frozen source/boundary contract and composition path, then enables a cooperative integration. Disabling or removing that integration returns behavior to passive-only conservative correlation. Durable observation persistence/versioning requires a later explicit compatibility change.
