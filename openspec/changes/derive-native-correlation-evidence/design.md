## Context

See `proposal.md` for motivation. This design is grounded in current `main` at PR #6 (`c9e940d`), not an assumed future recorder.

Current path:

```text
CaptureEvent v1 -> WAL v1 -> chronicle-session reconstruction
  -> per-connection protocol stream -> protocol canonicalization
  -> CanonicalSession v1 -> explicit CorrelationContext join
  -> chronicle-canonical resolver -> CorrelationGraph
```

Current facts and their verified semantics:

| Layer | Current fact | Safe meaning | Not safe as ownership support |
| --- | --- | --- | --- |
| `chronicle-capture` | `SocketIdentity { socket_cookie, first_seen, network_namespace }` | Reuse-resistant identity of one observed kernel socket generation within its boot/namespace scope | Parent request, task lineage, or ingress ownership |
| `chronicle-capture` | `SocketEvidence` endpoints, active/passive role, `ProcessMetadata` PID/TID/executable, cgroup | Observation and endpoint/role provenance | Operation identity, process/task causal lineage, or scenario identity |
| `chronicle-capture` | payload timestamp, direction, TCP/continuation sequence, truncation | Per-observation transport evidence | Global timeline, nearest-ingress selection, or causal order |
| `chronicle-session` | `ReconstructionConnectionIdentity::Socket`, ordered directional fragments, loss windows | Bounded reconstruction of one socket generation | Cross-connection execution relationship |
| `chronicle-protocol` | `SourceConnectionGeneration` on decoded frames and `ProtocolStream` per reconstructed connection | Connection-generation and protocol-stream provenance | Cross-stream parenthood; stream/message sequence is not application causality |
| HTTP builtin | request/response pairing, request sequence, pipeline depth, protocol bytes | One protocol exchange's canonical operation and completeness | Relationship between an ingress operation and another connection's egress |
| `chronicle-canonical` | `CanonicalOperation` connection/WAL/epoch provenance and relative offsets | Scoped operation and publication provenance | Native relational evidence; no task or handoff field exists |
| `chronicle-etl` | independently reconstructs connections and invokes protocol canonicalizers; explicit correlation composition is on demand | Complete ETL and exact session/reference join | Native evidence generation from absent facts |

Current code therefore exposes no proven task creation, execution continuation, inherited execution context, process parent/child operation binding, or cross-protocol-stream causal relation. `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, and `ProtocolStream` already exist as Chronicle-owned evidence shapes, but their current producers do not define causal semantics. The conservative PR #6 boundary is correct: without trace relationships or a future explicit native relation, non-root operations normally remain `Uncorrelated`.

The resolver already owns support closure, `Resolved`/`Ambiguous`/`Uncorrelated` materialization, confidence, witnesses, and fixed parent-edge construction. It consumes only the explicit correlation-evidence channel; role evidence remains classification provenance. This change must add facts at that boundary without moving selection into a producer.

## Goals / Non-Goals

**Goals:**

- Add one real, narrow native relationship family: explicit Chronicle-owned execution handoff from one fully scoped canonical operation to another.
- Make the relationship candidate-specific, provider-neutral, generation-safe, bounded, serializable as a runtime value, and deterministic.
- Derive evidence at ETL/canonical composition after exact operation verification, then reuse the existing resolver unchanged as selection authority except for the named predicate it must understand.
- Prove concurrent A/B/C ingress isolation, database and HTTP children, post-response async continuation, ambiguity, identifier reuse, retry determinism, and trace coexistence without provider packages.
- State honestly what current passive capture cannot prove and leave absent native input as a safe no-support result.

**Non-Goals:**

- Mining task/PID/TID/process/socket/connection/stream/timestamp equality into support.
- Adding task creation, process inheritance, socket-generation history, protocol-specific cross-stream logic, or provider integrations in this first slice.
- Changing Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public CLI JSON, replay safety, or adding `EventId`.
- Persisting scenario graphs or scenario artifacts.
- Making runtime handoff facts durable across a restart. A future evidence-durability change must define that separately; this change is deterministic when the same fact set is supplied and is deterministically empty when it is absent.

## Decisions

### 1. First native predicate is explicit execution handoff, not identifier equality

Introduce one candidate-relative domain value:

```text
CorrelationEvidenceKind::NativeExecutionLineage {
    parent: CanonicalOperationRef,
    relation: ExecutionContinuation,
}
```

The item is attached to the child operation's explicit correlation-evidence collection. Its meaning is narrow: a Chronicle-owned runtime handoff source observed an explicit execution-context transfer from `parent` operation to this child. `parent` is the candidate/reference scope; it is never inferred from a task, process, socket, connection, stream, time, or role value. The item is non-temporal and does not contain a scenario, confidence, selected edge, or role.

The raw composition fact has the same explicit scope:

```text
NativeExecutionHandoffFact {
    parent: CanonicalOperationRef,
    child: CanonicalOperationRef,
    relation: ExecutionContinuation,
    provenance: EvidenceProvenance,
}
```

A Chronicle-owned runtime source may implement the handoff as a capability/token transfer, but token equality alone is not evidence. The source must record the parent-to-child handoff event. No external trace SDK, provider context, or vendor wire format is part of this contract.

The first implementation accepts facts after canonical operation references are bound. A pre-canonical source that only has a raw anchor is not allowed to guess a reference: a later change may define an anchor containing a reuse-safe `SourceConnectionGeneration` plus a protocol-local operation position. Bare sequence, socket cookie, PID/TID, task ID, file descriptor, connection ID, or stream ID is never an anchor.

### 2. Current passive capture remains honest

No current `CaptureEvent` or reconstructed session fact is promoted merely because it exists. In particular:

- `SocketIdentity` proves same socket generation only. It is useful to reject stale bindings and distinguish reuse, not to assign an egress to an ingress.
- `ProcessMetadata` is a point-in-time owner observation. There is no process-generation or operation-execution binding.
- Relative offsets and monotonic clock identity support observation context only. They do not establish a recording-global causal timeline.
- TCP/WAL/protocol sequences establish bounded provenance/order in their owning scope only.
- `ProtocolStream` is per connection. HTTP currently pairs request and response inside one operation and emits no cross-connection causal fact.
- Passive/active socket role and `ProtocolOwnership` can support application-relative role classification; role evidence cannot double as correlation evidence.

This means a current passive recording with no handoff fact and no trace evidence remains mostly uncorrelated. That is an explicit capability boundary, not a license to use a heuristic. The production-shaped first slice is a Chronicle-owned runtime handoff source supplying facts at composition time; wiring a durable passive capture source for those facts is deferred because it cannot be represented in frozen v1 artifacts safely.

### 3. Derive at ETL/canonical composition, not capture, session, or protocol builtins

Responsibilities stay aligned with existing ownership:

1. `chronicle-capture` and `chronicle-capture-ebpf` remain observation-only and unchanged. They do not emit application-operation or handoff claims in Capture Event v1.
2. `chronicle-session` remains transport reconstruction-only. It supplies connection-generation and loss context but does not assign lineage.
3. `chronicle-protocol` remains the generic SPI. `chronicle-protocol-builtins` remains protocol implementation; current HTTP has no proven cross-operation causal relation and is not changed in this slice.
4. `chronicle-etl` owns a native-fact derivation/composition step after canonical operations and exact session lineage are available. It verifies both full references, normalizes facts, and attaches `NativeExecutionLineage` to the child.
5. `chronicle-canonical` owns the evidence variant, graph/domain placement validation, and the resolver's named native predicate. It never imports capture, session, WAL, protocol implementation, or provider vocabulary.
6. `chronicle-application` may own optional runtime-source wiring and pass facts into ETL. CLI remains unaware of correlation and protocol semantics.

The derived result is a `CorrelationContext` input to the existing resolver. The producer emits evidence and bounded diagnostics only. It does not construct `Scenario`, `CorrelationResolution`, `SelectedCausalEdge`, or confidence values.

### 4. Native relation participates in existing resolver phases

The resolver adds one named relation index over validated `NativeExecutionLineage` items. For child C and named parent P:

- **Admission:** require C and P to be admitted full references in the same recording, reject malformed/orphan/cross-scope input, and retain role evidence separately.
- **Phase A1:** `C -> P` inherits P's previous-round support set as transitive candidate-specific support. Support remains sparse and monotonic; no scenario is chosen by the native producer.
- **Phase A2:** no native-only direct trace support is fabricated. A unique native-only chain uses existing `Strong` transitive confidence and a native lineage witness. Multiple supported scenarios materialize `Ambiguous`; no confidence is assigned.
- **Phase B:** the same relation is an eligible direct-parent candidate. It goes through the existing invalid-relation pre-filter, unique-parent mapping, complete provisional graph, global SCC computation, and internal-cycle-edge removal. Multiple parent candidates produce no provisional edge; cycles lose only internal edges.
- **Retention:** native evidence is retained in the correlation channel under the existing deterministic evidence key/cap rules. Role evidence remains byte-for-byte verbatim. A native relation never replaces a direct trace witness merely because it sorts first.

This preserves resolver authority and avoids a second native selector. Existing temporal rules, root pinning, ScenarioRoot reservation, support closure, ambiguity, and confidence semantics remain intact.

### 5. Scope and identifier reuse are explicit

Every derived item carries the complete parent reference. The child scope comes from the evidence map key and is also checked against the fact. Reference verification must prove recording, owner epoch, session lineage, and exactly one operation occurrence. Cross-epoch parent/child facts are valid when both sessions are supplied and verified; an epoch boundary does not break a causal relation.

The following never create a relation by equality:

- `OperationId` without its recording/epoch/session scope;
- PID, TGID, TID, process generation, task/worker string;
- file descriptor, socket cookie, five-tuple, `ConnectionId`, or `SourceConnectionGeneration` alone;
- protocol stream ID, request sequence, WAL sequence, byte direction, or timestamp.

If a future source binds facts before canonicalization, it must use an explicit generation-safe anchor and exact operation position. A stale/reused anchor maps to zero or multiple operations and is rejected/omitted with bounded diagnostics. The first slice avoids this uncertainty by accepting already bound canonical references.

### 6. Ambiguity and source coexistence are additive, never voting

A raw fact that cannot map to exactly one parent/child reference produces no positive relation and a bounded diagnostic. It is not resolved by nearest time, first/last observation, or one remaining candidate. Conversely, two independently proven facts naming parents in different scenarios both enter the resolver and naturally produce `Ambiguous`; the producer does not collapse them.

Trace and native evidence share the same correlation channel. They only add candidate-specific positive support:

- same scenario: both provenance sources remain inspectable; existing confidence and witness class rules apply;
- different scenarios: support remains `Ambiguous`; no provider wins;
- different direct parents: existing unique-parent rule suppresses the selected edge;
- no source contradicts another through a negative vote. An invalid or missing source only removes its own support.

### 7. Bounds and retention

Use explicit implementation limits:

- at most `32` native handoff facts attached to one child per derivation batch;
- at most `4096` native handoff facts in one bounded derivation batch;
- at most `4096` active runtime handoff contexts in the source.

Limits are admission/resource limits, not semantic truncation. On overflow, report typed bounded loss or fail closed for affected facts; do not keep an arbitrary prefix and claim complete evidence. The producer retains one-hop facts only. It does not retain transitive ancestry, all paths, an operation×scenario matrix, or lifetime-wide socket/task-generation history. The resolver's existing sparse N×S support bound and proof derivation rules handle transitivity.

Facts can be streamed in bounded batches. Across a long recording, per-operation direct facts and full scoped references are the compact retained relationship; recording length does not create a transitive explanation graph. Runtime handoff state is released after explicit handoff or bounded terminal cleanup. A restart that cannot restore runtime facts emits no native relation rather than guessing.

### 8. Deterministic normalization

Derivation is a pure normalization step over verified sessions and facts:

1. reject or classify scope/shape errors without fallback;
2. key facts by `(child_ref, parent_ref, relation_kind, provenance.source, provenance.observation)`;
3. deduplicate exact keys idempotently;
4. emit child evidence in canonical order by child reference, parent reference, relation discriminator, then complete provenance;
5. pass the resulting context to the existing resolver.

No hash-map iteration order, worker scheduling, retry count, recorder restart, random ID, or processing-order identifier affects output. Native relation evidence is serializable as a Chronicle-owned runtime value. Given identical canonical sessions and identical fact input, output and bounded diagnostics are byte-identical. Given canonical artifacts without the optional fact input, output is deterministically native-empty; durable rehydration of the fact stream is a separate change.

### 9. Explicit answers to repository-inspection questions

1. **Raw facts available today:** socket generation and endpoint/role/process snapshots; payload timestamps/directions/transport sequence; reconstructed per-socket streams, loss, and termination; protocol frames and per-connection stream; canonical operation connection/WAL/epoch provenance, relative offsets, request/response protocol data, completeness; ETL session lineage and deterministic operation references.
2. **Actual causal semantics today:** socket generation identifies one observed physical socket generation; canonical full references identify one operation occurrence after lineage verification; HTTP request/response pairing identifies the two halves of one operation. None proves ingress-to-egress execution causality.
3. **Contextual-only facts:** PID/TID/task/process labels, connection/socket/FD/five-tuple equality, stream/message/WAL sequence, wire direction, socket role, relative time/lifetime, protocol name, and custom values.
4. **New evidence required:** one additive `NativeExecutionLineage { parent, relation: ExecutionContinuation }` variant plus a serializable `NativeExecutionHandoffFact` composition input. No existing contextual variant is overloaded.
5. **Existing variants reused:** `EvidenceProvenance`, `CanonicalOperationRef`, existing correlation channel, resolver retention, and `CorrelationContext`. `ExecutionTaskLineage`, `ProcessThreadGeneration`, `ConnectionSocketGeneration`, and `ProtocolStream` remain contextual.
6. **Derivation location:** ETL/canonical composition after canonical operations and session lineage are available; not capture, WAL, session, or protocol builtins.
7. **Crate ownership:** canonical owns domain/predicate/resolver; ETL owns exact join/derivation; application may wire runtime facts; capture/session/protocol remain fact/context producers; CLI remains outer adapter.
8. **Provider neutrality:** only Chronicle types cross the boundary; provider adapters, if any, translate at the outer boundary and are separately distributed.
9. **Reuse/generations:** full references are authoritative; any pre-canonical anchor must include reuse-safe source generation plus exact local position; bare identifiers never bind.
10. **Boundedness:** 32 facts/child, 4096 facts/batch, 4096 active handoffs; one-hop facts only; overflow fails closed; resolver keeps existing sparse support bound.
11. **Determinism:** canonical total ordering and exact deduplication over full references, relation kind, and provenance; no derived IDs or arrival order.
12. **Ambiguous native facts:** unmappable raw fact yields no relation plus bounded diagnostic; independently proven competing relations reach resolver and remain `Ambiguous`.
13. **Native plus trace:** both contribute positive support through one channel; neither contradicts or overrides the other; disagreement remains ambiguity.
14. **Safe resolver predicates:** only explicit child-side `NativeExecutionLineage` naming an admitted parent; it supports transitive ownership and eligible direct-parent construction under existing phases.
15. **Forbidden predicates:** timing/proximity/overlap, same or nearest ingress, task/PID/TID/process equality, process inheritance alone, socket/FD/connection/stream equality, byte direction, protocol name, WAL/sequence order, absence of contradiction, weighted voting, or provider priority.

### 10. First vertical slice input and proof

The first end-to-end proof uses canonical fixture sessions plus a production-shaped Chronicle-owned handoff source, not a fake resolver shortcut. It creates overlapping known-ingress roots A/B/C and explicit one-hop facts:

```text
A -> database A1
A -> HTTP A2
A -> async continuation A3
B -> database B1
B -> HTTP B2
C -> database C1
```

The source supplies complete operation references. The ETL composition step derives child evidence, then invokes the existing resolver. Tests permute operations/facts, overlap lifetimes, place A3 after A's completion, reuse contextual task/socket/connection identifiers, and omit all trace evidence. Expected graph ownership follows only handoff facts. The scenario remains uncorrelated if the handoff fact is removed, and ambiguous if valid facts name competing parents.

## Risks / Trade-offs

- **[Current passive recorder has no handoff producer] →** do not claim that current recordings magically gain native correlation. Add the explicit runtime/composed source contract in this slice; leave no-fact recordings safely uncorrelated. Define durable capture in a later compatibility/evidence-durability change.
- **[Runtime facts can be lost across restart] →** facts are optional, serializable, and deterministic when replayed with the same input; missing facts fail closed. No guessed reconstruction is allowed.
- **[A single explicit handoff source is narrower than broad task/socket lineage] →** narrow semantics are reviewable and safe. Defer other families until concrete producers prove candidate-specific meaning.
- **[Conflicting parents reduce selected edges] →** ambiguity and no-edge outcomes are preferable to silently choosing a parent; existing resolver cycle and multi-parent rules remain authoritative.
- **[Input caps can reduce completeness] →** overflow is typed/bounded loss, never silent semantic truncation. Capture/reliability reporting remains separate from correlation outcomes.
- **[Evidence retention can hide optional context] →** resolver semantics operate on complete validated input; existing required-witness retention and deterministic cap rules remain unchanged.

## Migration Plan

1. Implement domain additions and resolver predicate behind existing runtime APIs; do not alter v1 serializers or default CLI flow.
2. Implement ETL native-fact verification/normalization and optional application composition wiring. Existing callers pass no facts and retain current behavior.
3. Add unit, integration, deterministic, boundedness, identifier-reuse, and end-to-end vertical-slice tests before enabling any production call path.
4. Run architecture and dependency validation; no dependency edge or crate addition is expected.
5. Review documentation/AGENTS impact. Update durable guidance only if implementation makes the explicit handoff predicate a repository-wide invariant; do not update user-facing correlation UX because none is introduced.
6. Roll back by omitting the optional fact input or disabling the composition call. Existing capture, WAL, canonical publication, storage, replay, and CLI artifacts remain readable because no frozen contract changes.

No production code is part of this task; these steps are implementation tasks for a later apply phase.
