## Context

Chronicle's current ownership chain is:

```text
CaptureEvent v1 -> WAL v1 -> chronicle-session reconstruction
              -> chronicle-protocol decoding/canonicalization
              -> chronicle-etl -> CanonicalSession v1 -> storage
```

`chronicle-common` owns neutral IDs, endpoints, directions, and timestamps. `chronicle-capture` owns socket/lifecycle/payload evidence. `chronicle-session` reconstructs bounded connection streams. `chronicle-protocol` and built-ins own protocol-local request/response pairing and produce `CanonicalOperation`. `chronicle-canonical` owns `CanonicalSession`, `CanonicalConnection`, `CanonicalOperation`, provenance, completeness, replay metadata, and validation. `chronicle-etl` owns the complete Extract-Transform-Load path, canonical publication, and checkpoint ordering.

A recording may publish one deterministic canonical session per finalized epoch. A logical operation can have contributing ranges in more than one epoch; `OperationProvenance` records those ranges and its completion-owner epoch. Continuation-only predecessor evidence remains non-replayable evidence, while the terminal canonical operation is the authoritative operation occurrence. This means session, epoch, and recording boundaries are publication/lineage boundaries, not scenario boundaries.

Current capture semantics intentionally separate `SocketRole` (`Active`/`Passive`) from `Direction` (`ClientToServer`/`ServerToClient`). `SocketRole` can be evidence about whether the recorded application accepted or initiated a connection; neither lower-layer value is itself a scenario identity. The current v1 canonical operation has no Chronicle-owned application ingress/egress role. This change defines that missing semantic without silently adding fields to v1 artifacts.

The public 0.1 compatibility policy freezes the meaning and supported reading of persisted Capture Event v1 where persisted/versioned, Canonical Session v1, WAL v1, manifests, public JSON, CLI behavior, and replay safety. A planning document cannot authorize additive correlation fields in those artifacts.

## Goals and Non-Goals

### Goals

- Define application-relative ingress/egress semantics independent of protocol and byte-flow direction.
- Reuse `CanonicalOperation`/`OperationId` as the logical interaction and identity.
- Make references outside a session unambiguous across recording and epoch publication boundaries.
- Give resolved, ambiguous, and uncorrelated interactions one explicit top-level owner.
- Preserve candidate-specific evidence and selected causal-edge invariants.
- Keep trace context optional and framework-neutral.
- Make foundation validation accept supplied resolutions without implementing correlation selection.
- Resolve the `EventId` question by omitting it from this foundation and documenting its future compatibility boundary.

### Non-Goals

- Production Rust implementation in this planning revision.
- A correlation engine, heuristic, score, temporal inference, runtime lineage capture, or automatic ownership from raw interleaved events.
- Protocol framing or request/response pairing changes.
- Tracing SDK integration, scenario replay, exports, assertions, test generation, or Web UI.
- Mutation of Capture Event v1, Canonical Session v1, WAL v1, public JSON, or other 0.1 contracts.
- A new Chronicle crate.

## Decisions

### 1. Chronicle-owned interaction role

Define two separate Chronicle-owned concepts:

```text
enum InteractionRole {
    Ingress,
    Egress,
}

InteractionRoleResolution =
    Known {
        role: InteractionRole,
        evidence: [CorrelationEvidence],
    }
  | Ambiguous {
        candidates: [
            {
                role: InteractionRole,
                evidence: [CorrelationEvidence],
            }
        ],
    }
  | Unknown {
        evidence: [CorrelationEvidence],
    }
```

`InteractionRole` contains only actual application-relative roles. `Ingress` means the logical interaction is accepted by the recorded application from a remote/client side. `Egress` means the recorded application initiates the logical interaction toward a remote service/dependency. `InteractionRoleResolution` records whether Chronicle knows that role; uncertainty is classification state, not a third role. In prose, `Known(Ingress)` and `Known(Egress)` are shorthand for the corresponding `Known { role, evidence }` records.

Every operation supplied to correlation has one role-resolution state attached in the correlation input or deterministically derived before graph validation from Chronicle-owned, application-relative evidence. The state may be represented in a future correlation sidecar/aggregate rather than as a new Canonical Session v1 field. A known classification retains its supporting evidence. An `Ambiguous` classification contains candidate records, never bare roles plus one shared evidence list: every candidate retains its own evidence collection explaining why that role remains viable. Candidate evidence remains attached to its candidate and is independently inspectable; it is never swapped, flattened, or used to select a role merely to eliminate ambiguity.

An ambiguous candidate set SHALL contain at least two distinct viable roles. Duplicate candidate roles, an empty candidate set, and a single-candidate set are invalid. If supplied evidence supports only one role, the result is `Known` for that role with its evidence. The current `InteractionRole` domain has only `Ingress` and `Egress`, so a valid ambiguous result currently contains both distinct roles. This is a current-domain consequence, not a shape constraint: if future role values are introduced, validation SHALL continue to require at least two distinct viable candidates with candidate-specific evidence rather than assuming exactly two candidates.

Derivation is semantic normalization, not correlation selection:

- validated passive HTTP server evidence with remote-client-to-recorded-application ownership yields `Known(Ingress)`;
- validated active HTTP client evidence initiated by the recorded application yields `Known(Egress)`;
- validated active PostgreSQL, MySQL, Redis, or other dependency evidence initiated by the recorded application yields `Known(Egress)`.

The same logical HTTP operation keeps one known role while request and response bytes travel in both directions. `Direction` and byte-flow direction remain wire evidence and never substitute for `InteractionRole`; if application ownership is otherwise unknown, `ClientToServer`/`ServerToClient` labels leave the state `Unknown`. Active/passive socket evidence may contribute only when application ownership is established. `SocketRole`, connection, stream, PID, TID, and timing remain evidence, not interaction or scenario identity. Conflicting ownership evidence becomes `Ambiguous` with separately retained candidate evidence, not a timing or ordering tie-break.

Only `Known(Ingress)` is eligible for `Scenario.root`. `Known(Egress)` may be a selected child when supplied correlation permits it; `Unknown` and `Ambiguous` role states cannot become roots. Role classification and scenario-correlation resolution are independent: `Unknown + Uncorrelated`, `Known(Egress) + Ambiguous`, `Known(Ingress) + Resolved`, and `Ambiguous + Ambiguous` are all representable combinations. The role semantics are protocol-neutral and belong to Chronicle's canonical domain.

### 2. Identity vocabulary and scoped operation references

Keep identity meanings distinct:

| Concept | Identity/shape | Meaning | Not interchangeable with |
| --- | --- | --- | --- |
| Logical interaction | existing `CanonicalOperation` / `OperationId` | one protocol request/response exchange or equivalent canonical operation | packet, message fragment, connection, stream, session |
| Operation reference | `CanonicalOperationRef` | durable lookup scope for an operation outside its session | bare `OperationId` |
| Interaction role | `InteractionRole` | application-relative ingress/egress classification | `Direction`, `SocketRole` |
| Scenario | `ScenarioId` | one correlation scenario owned by the graph | `SessionId`, `EpochId`, `RecordingId`, trace ID |
| Canonical session | `SessionId` | one published canonical session artifact, normally one finalized epoch publication | scenario identity |
| Recording | `RecordingId` | recording/run lineage | scenario identity |
| Epoch | `EpochId` | bounded publication/capture lineage within a recording | scenario identity |
| Transport carrier | connection/socket/stream evidence | carrier used to observe/reconstruct operations | operation or scenario identity |
| External trace | opaque provider-labelled evidence | optional correlation evidence | Chronicle scenario identity |

Use this scoped reference for durable recording-scoped correlation:

```text
CanonicalOperationRef {
    recording_id: RecordingId,
    owner_epoch_id: EpochId,
    session_id: SessionId,
    operation_id: OperationId,
}
```

`owner_epoch_id` is the finalized epoch/session publication that owns the authoritative canonical operation occurrence. It is not a replacement for the operation's contributing `epoch_ranges`; a cross-epoch operation retains all contributing ranges and its completion-owner information in canonical provenance. `session_id` identifies the published artifact scope; `operation_id` is resolved only inside that session. The full tuple is the external reference. `OperationId` remains the interaction identity and is not redefined as globally unique.

A durable graph validates that:

1. the referenced session exists under `recording_id`;
2. its lineage identifies `owner_epoch_id`;
3. the session contains exactly one operation with `operation_id`; and
4. the operation's provenance is consistent with its owner and any contributing epochs.

Fixtures or in-memory domain tests must supply explicit recording/epoch/session scope when current fixture artifacts omit lineage. No implementation may infer recording or epoch identity from a UUID, session name, WAL position, or operation ID.

This shape permits two sessions to contain the same `OperationId` value without collision: their references differ in `session_id` and lineage. A lookup by bare `OperationId` is invalid outside its owning session.

### 3. Epoch rollover and scenario identity

`CorrelationGraph` is recording-scoped and may reference operations from any finalized session in that recording. It can therefore contain:

```text
Recording
├── Epoch N   -> CanonicalSession A
└── Epoch N+1 -> CanonicalSession B

CorrelationGraph(recording)
└── Scenario X
    ├── CanonicalOperationRef(..., A, ingress)
    ├── CanonicalOperationRef(..., A, child)
    └── CanonicalOperationRef(..., B, child)
```

If an ingress operation begins in epoch N and completes after rollover in N+1, canonicalization retains one logical `OperationId`. Its authoritative reference uses the session/owner epoch containing the terminal canonical operation, while its provenance retains the bounded ranges from both epochs. Continuation-only predecessor evidence is not a second operation. If a child egress operation is published in the successor session, it receives its own full reference there. A selected causal edge may connect any two references in the same scenario, even when their sessions and owner epochs differ.

`ScenarioId` is owned by the recording-scoped graph and remains stable when new epoch sessions are appended. Session or epoch identity must never be used as scenario identity. A restarted ETL resolves references by exact recording/owner-epoch/session/operation lookup and lineage verification; it does not regenerate IDs, scan for a matching bare operation ID, or infer lineage. Missing, conflicting, or unverifiable references fail closed as invalid/unresolved data.

### 4. Top-level correlation aggregate

Use a Chronicle-owned `CorrelationGraph` as the aggregate root. The conceptual shape is:

```text
CorrelationGraph {
    recording_id: RecordingId,
    role_resolutions: Map<CanonicalOperationRef, InteractionRoleResolution>,
    scenarios: [Scenario],
    resolutions: Map<CanonicalOperationRef, CorrelationResolution>,
    causal_edges: [SelectedCausalEdge],
}

Scenario {
    id: ScenarioId,
    root: CanonicalOperationRef,
    members: [CanonicalOperationRef],
}
```

`Scenario` is a child entity owned by `CorrelationGraph`, not an independent owner. The graph is authoritative for role classification and correlation outcomes. Scenario membership is a selected view of graph state and must agree with `Resolved` entries; it is not a second way to hide unresolved operations.

Every operation admitted to the graph has exactly one `InteractionRoleResolution` and one correlation-resolution entry. The classification state and correlation outcome are independent dimensions:

```text
role: Known(Ingress) | Known(Egress) | Unknown | Ambiguous(...)
correlation: Resolved(...) | Ambiguous(...) | Uncorrelated
```

An admitted operation with unknown or ambiguous role remains in `role_resolutions`, with evidence inspectable, even when its correlation outcome is ambiguous or uncorrelated. It cannot become a root unless its role state is `Known(Ingress)`. `Known(Egress)` may be a supplied scenario child; unknown/ambiguous role state is not silently converted into a role or deleted. The graph cannot be valid if any admitted operation disappears merely because Chronicle cannot classify its application-relative role or scenario ownership.

The three correlation outcomes are:

```text
Resolved {
    scenario: ScenarioId,
    confidence: Exact | Strong | Inferred,
    evidence: [CorrelationEvidence],
}
Ambiguous {
    candidates: [{ scenario: ScenarioId, evidence: [CorrelationEvidence] }],
}
Uncorrelated {
    evidence: [CorrelationEvidence],
}
```

A resolved operation belongs to exactly one scenario. An ambiguous operation remains outside selected scenario membership but retains every candidate and candidate-specific evidence. An uncorrelated operation remains in `resolutions` with no synthetic owner. A scenario root must be a reference with `Known(Ingress)` classification. Candidate scenarios in an ambiguous correlation resolution do not make the ambiguous operation a member.

### 5. Selected causal edges

The graph owns selected edges explicitly:

```text
SelectedCausalEdge {
    scenario: ScenarioId,
    parent: CanonicalOperationRef,
    child: CanonicalOperationRef,
    evidence: [CorrelationEvidence],
}
```

Validation requires both endpoints to resolve to the same scenario, both references to resolve exactly through their full scope, no self-edge, no cycle, and at most one selected parent per child in the 0.2 tree. A root has no selected parent. An unresolved/ambiguous operation may exist without a parent, but it cannot participate in a selected edge. Candidate relationships are retained only in resolution evidence; they never become graph edges or scenario members.

### 6. Framework-neutral evidence and resolution provenance

`CorrelationEvidence` is a Chronicle-owned tagged value. It may represent trace relationship, protocol ownership, execution/task lineage, process/thread generation, connection/socket generation, protocol stream, temporal/lifetime, and bounded namespaced custom evidence. Each item records source/provenance and relation sufficiently to explain a supplied resolution.

Trace values are opaque/provider-labelled enrichment. Core/domain APIs do not expose OpenTelemetry SDK types, W3C libraries, B3 libraries, Datadog types, AWS X-Ray types, or equivalent provider types. External adapters map into Chronicle-owned evidence at an outer boundary. No trace context is required, and missing trace parents are never synthesized.

Temporal overlap, PID, TID, task worker, socket, connection, stream, and timestamps are evidence only. Model validation records resolution provenance and rejects any selected resolution whose only basis is temporal overlap. Temporal evidence may accompany a supplied resolution supported by other Chronicle-owned evidence; the foundation does not decide how a future resolver weights it.

### 7. Completeness and replayability stay separate

Correlation membership, causal resolution, operation completeness, and replayability are separate decisions. Assigning an incomplete, lost, malformed, unsupported, or otherwise non-replayable operation to a scenario cannot make its payload complete or authorize replay. Uncorrelated and ambiguous operations remain inspectable regardless of replayability.

### 8. Canonical/ETL ownership and dependency direction

The future flow remains layered:

1. capture observes socket/lifecycle/payload evidence and does not assign scenarios;
2. WAL preserves durability and recovery authority and does not assign scenarios;
3. session reconstruction preserves bounded carrier/stream evidence;
4. protocol implementations perform protocol-local pairing and produce canonical operations;
5. canonical correlation validates Chronicle-owned roles, references, supplied resolutions, and selected edges;
6. ETL composes correlation with complete canonical publication and checkpoint ordering;
7. storage persists/verifies a future correlation artifact without deciding ownership;
8. application composes use cases and CLI remains an outer adapter.

Correlation semantics stay in `chronicle-canonical`; neutral IDs stay in `chronicle-common`. No standalone correlation crate is justified. The repository's existing architecture validation mechanism currently checks Chronicle workspace dependency direction, critical forbids, and semantic/API boundaries; it does not currently enforce an external tracing-SDK/package denylist. Future implementation SHALL extend `validation/architecture.toml`, `scripts/validation.py architecture`, and its existing standard-library fixture tests with the smallest maintainable direct-dependency guard for provider SDKs/integrations (using actual Cargo package identity and covering renames/target-specific declarations as appropriate). Do not create a parallel validator. Outer provider adapters may translate SDK data into Chronicle-owned `CorrelationEvidence`; core/domain crates must not depend on those provider packages. Generic logging/tracing facades are not provider SDKs and must be handled according to the inspected dependency policy rather than blindly banned.

### 9. EventId decision and compatibility boundary

This foundation does not introduce `EventId`. Correlation does not need an identity for every raw capture event: `CanonicalOperation`/`OperationId` identifies logical interactions, `CanonicalOperationRef` scopes them across published sessions, and existing connection/WAL/epoch provenance explains their source. Adding a random event identity would create restart and persistence obligations without helping scenario membership.

Therefore:

- no `EventId` primitive is added to `chronicle-common` by this change;
- no EventId field is added to Capture Event v1 or Canonical Session v1;
- no correlation API requires EventId;
- any future event identity begins at a separately approved capture/evidence contract boundary.

If future requirements make event identity necessary, a dedicated OpenSpec change must choose either a deterministic derived identity from stable immutable provenance (restart/re-read stable, collision-safe in declared scope, not timestamp/WAL-byte-position/order derived, and unchanged by WAL segmentation or ETL processing order) or an explicit versioned 0.2 persisted contract/sidecar. It must define reader/writer, migration/no-migration, lineage, and rollback policy. This foundation authorizes neither option and leaves 0.1 contracts untouched.

### 10. Algorithm-neutral implementation boundary

The foundation implementation accepts already constructed canonical operations, roles, and supplied `CorrelationResolution` values. It validates identity scope, graph invariants, evidence provenance, and outcome preservation. It does not consume raw interleaved traffic to derive Scenario A/B/C, select candidates, score evidence, infer temporal ownership, or implement runtime lineage. Those behaviors belong to a future resolver change.

## Risks and Trade-offs

- A full operation reference is more verbose than `OperationId`, but it prevents collisions across session artifacts and makes restart lookup explicit.
- Requiring application-relative role evidence can leave an operation ineligible for root selection; that is safer than treating byte direction or a carrier as application ownership.
- Keeping ambiguous and uncorrelated outcomes outside scenario membership lowers apparent scenario completeness but preserves truth and evidence.
- A recording-scoped graph requires a future persistence decision; a sidecar/new artifact avoids mutating v1, while a new versioned canonical contract requires a separate migration proposal.
- Deferring EventId avoids an unnecessary persisted identity. A future event-level feature must carry its own deterministic/compatibility proof.

## Compatibility and Migration Plan

This change is planning-only and has no production migration or rollback. It does not edit any persisted artifact or public API.

Future implementation order:

1. Add neutral `ScenarioId` support without changing existing v1 serialization; keep `OperationId` as the only interaction identity.
2. Implement canonical-domain role, `CanonicalOperationRef`, `CorrelationGraph`, resolution, evidence, and validation values in a non-v1 integration surface.
3. Choose a separately versioned correlation sidecar/new artifact for persisted graph data, or submit a dedicated 0.2 contract migration before adding fields to any persisted canonical/capture artifact.
4. Preserve protocol-local pairing, ETL publication/checkpoint ordering, WAL recovery authority, replay safety, and unresolved outcomes.
5. Add only supplied-resolution domain tests and architecture checks; add resolver tests in the future resolver change.
6. Update architecture documentation and `AGENTS.md` with concise durable invariants after implementation.

No reader may guess missing lineage, use bare operation lookup, default a missing role to ingress, add v1 fields silently, or make a correlation result replayable.

## Future Changes

- `correlate-ingress-and-egress-interactions`: deterministic resolver and evidence selection for concurrent ingress/egress traffic.
- Pluggable trace evidence providers at outer boundaries.
- Concurrent ingress and runtime lineage capture.
- Versioned correlation/scenario artifact and user-facing scenario commands.
- Replay v2 and test generation.
- Dedicated event identity/compatibility change, only if raw-event identity becomes necessary.
