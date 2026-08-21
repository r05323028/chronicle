## Context

See `proposal.md` for motivation. Inspection of `origin/main` at `75051fc` (the working branch is one unrelated release commit ahead) shows a clear existing layering:

```text
CaptureEvent -> WAL -> chronicle-session reconstruction
           -> chronicle-protocol decoding/canonicalization
           -> chronicle-etl -> CanonicalSession -> chronicle-storage
```

`chronicle-common` owns transport-neutral IDs, endpoints, directions, and timestamps. `chronicle-capture` owns socket, process, payload, lifecycle, and loss evidence. `chronicle-session` groups evidence into protocol-neutral connection streams keyed by socket/fixture evidence. `chronicle-canonical` owns `CanonicalSession`, `CanonicalConnection`, `CanonicalOperation`, operation provenance, completeness, and replay metadata. `chronicle-protocol`/built-ins own protocol-local request/response pairing, while ETL owns the complete Extract-Transform-Load path and publication ordering.

The current model therefore has two relevant seams but no scenario domain: protocol canonicalizers already produce logical operations, and reconstruction already exposes connection/stream evidence. Existing `http11-operations` correlation is FIFO request/response pairing inside HTTP/1.1; it is not cross-connection scenario ownership. Current architecture policy allows `chronicle-canonical -> chronicle-common` and forbids upward or adapter leakage. Current manifests contain no OpenTelemetry, W3C Trace Context, B3, Datadog, or AWS X-Ray dependency; CLI `tracing-subscriber` is logging setup, not domain trace context.

## Goals / Non-Goals

**Goals:**

- Put the correlation domain in the existing canonical ownership boundary without creating a new crate.
- Reuse `CanonicalOperation`/`OperationId` for logical interactions and add no parallel operation identity.
- Define Chronicle-owned scenario/event identities and keep recording, epoch, session, connection, socket, process, thread, stream, WAL, and time identities distinct.
- Preserve evidence provenance and candidate sets so every selected or unresolved relationship is explainable.
- Make trace enrichment optional and replaceable through an outer adapter boundary.
- Preserve concurrent, multiplexed, pooled, migrated, missing-trace, conflicting, and uncorrelated cases.
- Leave current 0.1 formats and production behavior untouched until an explicit 0.2 implementation/compatibility decision.

**Non-Goals:**

- Implementing production Rust types or changing any crate in this planning task.
- Choosing or implementing a correlation algorithm, weighted score, runtime instrumentation, or tracing adapter.
- Replacing protocol-local request/response correlation.
- Adding scenario replay, dependency matching, export artifacts, assertions, test generation, AI/LLM assistance, or Web UI.
- Adding a new crate or any tracing SDK dependency.

## Decisions

### 1. Extend existing canonical ownership; do not add a correlation crate

Future domain types belong in a `correlation` module owned by `chronicle-canonical`, because that crate already owns the protocol-independent replay model, operation identity, provenance, completeness, and validation. `chronicle-common` remains the dependency leaf and owns only neutral identity primitives. No new crate is justified before an adapter has independently distributable behavior.

This keeps the intended dependency direction:

```text
future trace/protocol/runtime adapters
                  -> Chronicle-owned CorrelationEvidence
                  -> chronicle-canonical correlation domain
                  -> chronicle-common identity primitives
```

A future adapter may live at an outer/application boundary or in a separately approved crate. The core must never import the adapter or its SDK.

**Alternative rejected:** Put scenario ownership in `chronicle-capture` or `chronicle-session`. Those crates own observation and transport reconstruction; doing so would make a socket/stream carrier look like application ownership and would force lower layers to depend upward on canonical semantics.

**Alternative rejected:** Add a seventh/standalone correlation crate now. The current workspace has no independent implementation boundary for it; a crate split would add coupling without behavior.

### 2. Reuse current operation terminology

`CanonicalOperation` already represents a logical request/response exchange or equivalent protocol operation and has stable `OperationId`, completeness, offsets, protocol data, and source provenance. In this design, “interaction” is the domain term for that existing concept. Future implementation SHALL add scenario relationships around it rather than create `Interaction` plus `CanonicalOperation` with two IDs.

`chronicle-protocol` remains responsible for turning reconstructed protocol streams into operations and pairing protocol messages. Scenario correlation consumes those operations. This separates message framing/response pairing from application-causality correlation.

**Alternative rejected:** Rename `CanonicalOperation` to `Interaction` immediately. It would create unnecessary API and artifact churn before the 0.2 persisted contract is designed. A semantic mapping is sufficient for this foundation.

### 3. Identity ownership and lifecycle

The future implementation uses these identity roles:

| Domain concept | Existing/new identity | Owner | Meaning | Forbidden substitute |
| --- | --- | --- | --- | --- |
| observed event | new `EventId` | `chronicle-common` | one observed evidence event carried through capture/WAL/ETL | WAL sequence, frame offset, socket, PID/TID |
| logical interaction | existing `OperationId` | `chronicle-common` / canonical operation | one protocol request/response exchange or equivalent operation | connection, stream, packet, thread |
| application scenario | new `ScenarioId` | `chronicle-common` | one root-ingress scenario aggregate | session, recording, trace ID, process |
| canonical session | existing `SessionId` | `chronicle-common` | current persisted/session artifact identity | scenario identity |
| recording lineage | existing `RecordingId`/`EpochId` | `chronicle-common` | capture-run and epoch lineage | scenario identity |
| carrier | existing `ConnectionId` plus capture/socket/stream evidence | owning lower layer | transport/protocol carrier | scenario owner |

`EventId` and `ScenarioId` are Chronicle IDs, not values copied from external systems. An identity is assigned once at its owning boundary, persisted/carried through subsequent stages, and never regenerated merely because ETL restarts, a WAL epoch changes, a connection is reused, or resolution confidence changes. The exact derivation (random allocation versus deterministic allocation from verified lineage) is an implementation detail constrained by Chronicle's deterministic/recovery rules; it MUST be recorded wherever restart/replay needs stable identity.

Adding an event field to Capture Event v1 or adding scenario fields to Canonical Session v1 would be a persisted-contract change. This planning change makes no such edit. Implementation must use the repository's explicit 0.2 compatibility/versioning policy rather than silently rewriting 0.1 artifacts or adding an implicit reader fallback.

### 4. Model evidence as a tagged Chronicle-owned value

The domain should use one Chronicle-owned `CorrelationEvidence` value with a tagged kind and source/provenance. Exact Rust field names and serialization are implementation work, but the semantic shape is:

```text
CorrelationEvidence {
    source: EvidenceSource,       // provider/adapter + version/provenance
    relation: EvidenceRelation,   // how subject and candidate are related
    observed_at: optional time,   // evidence time, not identity
    kind: {
        Trace { opaque trace/span/parent references, completeness },
        Protocol { protocol, request/response ownership, operation key },
        Execution { runtime, task/coroutine lineage },
        Process { pid, process generation },
        Thread { tid, thread generation },
        Connection { connection/socket generation },
        Stream { protocol stream identity },
        Temporal { clock/lifetime interval, quality: weak },
        Custom { namespace, bounded value }
    }
}
```

The representation is deliberately not an OpenTelemetry `SpanContext`, W3C SDK object, B3 object, Datadog object, or AWS X-Ray object. Trace IDs may be retained as opaque normalized values with a format/provider label; an adapter owns parsing and normalization. Evidence records source and relation provenance so resolution can be explained without a score-only decision. Sensitive custom/trace values remain subject to existing Chronicle redaction and safe-output rules.

Temporal evidence is valid evidence but has an explicit weak role. The model does not make temporal overlap mathematically sufficient for ownership. Process/thread/socket/connection/stream values are useful carriers and provenance, not scenario keys.

**Alternative rejected:** Store `HashMap<ScenarioId, f64>` or one opaque confidence score. It cannot explain assignments, preserve conflicting candidates, or support future provider replacement.

**Alternative rejected:** Store external SDK structs behind `Box<dyn Any>`. It leaks provider coupling, prevents stable serialization, and makes adapters non-replaceable.

### 5. Separate resolution outcome from confidence

The semantic resolution shape is:

```text
CorrelationResolution =
    Resolved {
        scenario: ScenarioId,
        confidence: Exact | Strong | Inferred,
        evidence: [CorrelationEvidence],
        parent_interaction: optional OperationId,
    }
  | Ambiguous {
        candidates: [
            { scenario: ScenarioId, evidence: [CorrelationEvidence] }
        ],
    }
  | Uncorrelated {
        evidence: [CorrelationEvidence],
    }
```

`CorrelationConfidence` is categorical and applies only to a selected resolution. `Ambiguous` and `Uncorrelated` are resolution outcomes, not low numeric confidence. Candidate evidence is retained per candidate. A future algorithm can choose whether and when to emit `Exact`, `Strong`, or `Inferred`, but it cannot erase the distinction or turn ambiguity into a selected owner.

A `Scenario` conceptually contains:

```text
Scenario {
    id: ScenarioId,
    root_ingress: OperationId,
    members: [ScenarioInteraction],
    causal_edges: [InteractionCausalEdge],
    boundary: ingress-response completion,
}

ScenarioInteraction {
    interaction: OperationId,
    resolution: CorrelationResolution,
}
```

Only selected resolved edges become scenario membership/parent edges. Ambiguous and uncorrelated interactions remain in the canonical correlation result/index even when they are not members of a selected scenario. This preserves evidence without fabricating tree structure. Exact persisted nesting, indexing, and bounded storage are implementation choices; all choices must preserve these semantics.

### 6. Define the 0.2 boundary around ingress lifetime

A root interaction is the ingress request/response exchange. Synchronous child interactions are those causally initiated while handling that ingress and admitted by future Chronicle correlation policy. The ingress response is the conservative close boundary. Work initiated after that response is not automatically a child, even if it shares a process, task, connection, trace ID, or time window.

This boundary intentionally avoids promising background-task lineage before runtime-specific evidence exists. Future work may add explicit asynchronous continuation scenarios, but it must introduce its own specification and relationship semantics rather than weakening the 0.2 boundary.

### 7. Place correlation at the canonical/ETL seam

The future pipeline remains:

1. `chronicle-capture` emits observed events and transport evidence; it does not assign scenarios.
2. `chronicle-wal` durably frames/recoveries evidence; WAL records do not become interactions or scenarios.
3. `chronicle-session` reconstructs bounded connection streams and preserves carrier evidence; it does not choose scenario owners.
4. `chronicle-protocol`/built-ins decode protocol frames and perform protocol-local request/response correlation into `CanonicalOperation`/`OperationId`, emitting protocol ownership evidence where available.
5. `chronicle-etl` composes future correlation over canonical interactions/evidence and owns canonical publication/checkpoint ordering; the algorithm itself is a later change.
6. `chronicle-storage` persists and verifies the canonical scenario representation; it does not decide correlation.
7. `chronicle-application` composes use cases; `chronicle-cli` remains an outer adapter and sees no lower-layer vocabulary.

This placement preserves ETL's complete Extract-Transform-Load responsibility and avoids making scenario ownership depend on the current co-located process topology.

### 8. Enforce provider independence through existing architecture validation

Use `scripts/validation.py architecture` and `validation/architecture.toml`, not a parallel dependency checker. The future implementation task should extend this existing policy/check to inspect direct package dependencies for core/domain processing crates (`chronicle-common`, `chronicle-canonical`, `chronicle-capture`, `chronicle-session`, `chronicle-protocol`, and `chronicle-etl`) and reject tracing SDK/provider packages (including OpenTelemetry SDK/bridge packages and provider-specific correlation packages). Adapter-owned outer crates remain separate from this denylist. Existing CLI logging dependencies remain owned by CLI and are not domain evidence.

The current checker already verifies all Chronicle workspace path edges, dependency kinds, optional/target-specific edges, and semantic boundaries. Add the smallest policy field and standard-library fixture tests needed to express the external denylist. Do not add an OpenTelemetry dependency to test the rule; use temporary Cargo manifests/metadata fixtures in the existing validation test style.

### 9. Preserve current compatibility and reliability authority

No production source, manifest, WAL bytes, Capture Event v1 bytes, Canonical Session v1 artifacts, checkpoint, storage layout, CLI command, or replay behavior changes in this planning change. A later implementation that persists event/scenario identities or resolution data must define the 0.2 reader/writer/version policy explicitly and keep WAL commit markers, recovery authority, canonical validation, ETL publication-before-checkpoint ordering, and replay safety unchanged.

No correlation result may make an incomplete/lost operation replayable merely because it has a scenario owner. Scenario grouping and operation completeness remain separate concerns.

## Risks / Trade-offs

- **Existing `CanonicalOperation` terminology may be less discoverable than `Interaction`.** Reusing it avoids duplicate identity and schema churn; documentation will explicitly map interaction language to the existing type.
- **A neutral evidence union can grow.** Start with the listed evidence categories and bounded provenance; add new kinds only through domain-owned extensions, not provider SDK types.
- **Ambiguity reduces scenario completeness.** This is intentional: an incomplete/ambiguous scenario is safer and more explainable than a confidently wrong one.
- **Trace IDs and custom metadata may be sensitive.** Reuse canonical redaction and safe-output rules; retain only bounded, provenance-bearing values needed for explanation.
- **Current architecture validation checks workspace edges, not external package policy.** Extend that same command/policy with focused tests rather than creating another validator; failure to do so leaves SDK leakage possible.
- **Scenario identity across epoch rollover is difficult.** Carry explicit lineage/continuation evidence and never infer parentage from UUID equality, matching existing recording/epoch rules.
- **Persisted schema impact is deferred.** No 0.1 artifacts change now; future implementation must land explicit 0.2 versioning and migration/no-migration decisions before writing scenario data.

## Migration Plan

This task is planning-only; it has no production migration or rollback.

Future implementation should proceed in this order:

1. Add/reuse neutral `EventId` and `ScenarioId` primitives in `chronicle-common`; confirm `OperationId` is the sole interaction identity.
2. Add the canonical correlation value types, validation, and bounded evidence/provenance rules in `chronicle-canonical` without importing capture, WAL, protocol implementations, or tracing SDKs.
3. Define the 0.2 persisted integration point and reader/writer policy before modifying Capture Event or Canonical Session artifacts. Preserve current v1 readers/writers unless the explicit policy says otherwise.
4. Add protocol/ETL seams that pass Chronicle-owned evidence and retain unresolved outcomes; do not add a correlation algorithm in the domain change.
5. Extend the existing architecture validator/policy and its portable tests to reject forbidden external tracing dependencies in core/domain crates.
6. Add unit/integration tests from the validation matrix, then update canonical architecture docs and `AGENTS.md` with concise invariants.
7. Run strict OpenSpec validation, targeted fast validation, and graph update after any production code changes. Privileged capture gates are not required for pure domain/architecture behavior.

Rollback before implementation is deletion/revert of this change directory. A future persisted 0.2 migration must define its own rollback/read policy; no compatibility shim is authorized by this foundation.

## Open Questions

None affect the domain contract. Exact Rust field names, serialization layout, bounded collection limits, and whether scenarios are embedded in `CanonicalSession` or published as a separate canonical artifact are implementation decisions, but either choice must preserve the specified identities, evidence provenance, causal edges, resolution outcomes, and 0.1 compatibility policy.
