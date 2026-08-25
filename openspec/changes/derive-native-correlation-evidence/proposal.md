## Why

PR #6 made correlation selection deterministic and conservative, but current recordings provide no Chronicle-native candidate-specific causal relationship for most non-root operations. The resolver therefore correctly leaves database, outbound HTTP, and asynchronous work `Uncorrelated` when no external trace evidence is supplied. Existing capture, reconstruction, and protocol facts identify socket generations, process snapshots, stream order, and timing, but none proves which ingress caused a downstream operation; promoting any of them by equality or proximity would reintroduce unsafe attribution.

This change closes that gap with one narrow, provider-neutral vertical slice: an explicit Chronicle-owned execution-handoff fact that names a fully scoped parent and child operation. It does not mine task, process, socket, connection, stream, or temporal identifiers. Those facts remain contextual until a later change proves stronger semantics.

## What Changes

- Inventory current capture, WAL, session, protocol, canonical, and ETL facts and record which facts have causal semantics versus contextual-only meaning.
- Add one additive Chronicle-owned correlation evidence variant for an explicitly proven execution continuation, with the child-side evidence carrying the complete parent `CanonicalOperationRef`.
- Define a bounded, serializable `NativeExecutionHandoffFact` input produced by a Chronicle-owned runtime handoff source. The source records an explicit context handoff, not task/process/socket equality and not a tracing-provider context.
- Derive native evidence at the ETL/canonical composition boundary after exact operation-reference verification. The producer emits relations only; the existing resolver remains responsible for support closure, ambiguity, confidence, witness retention, and causal-parent selection.
- Authorize the existing resolver to treat explicit native execution continuation as a named transitive parent predicate. Native and trace evidence enter the same Chronicle-owned correlation channel; neither source receives priority or negative-vote semantics.
- Preserve `Uncorrelated` and `Ambiguous` when handoff facts are absent, genuinely conflicting, unmappable, or insufficient. Async continuation remains eligible even when it begins after ingress completion.
- Make operation scope and generation handling explicit: full recording/owner-epoch/session/operation references are authoritative; socket cookies, file descriptors, PIDs/TIDs, task labels, connection IDs, stream IDs, WAL sequence, and timestamps never become ownership predicates.
- Keep derivation bounded and deterministic under input permutation, worker scheduling, retry, restart, and repeated resolution. No transitive ancestry history, scenario artifact, `EventId`, or frozen v1 field is added.
- Prove concurrent native correlation with a portable Chronicle-owned handoff source for overlapping A/B/C ingress scenarios, including database, HTTP, and post-response continuation children, without any trace-provider package.

## Capabilities

### New Capabilities

- `native-correlation-evidence`: derives bounded, provider-neutral Chronicle-native relational evidence from explicit execution-handoff facts and defines the capability boundary for all other current recording identifiers.

### Modified Capabilities

- `correlation-domain-model`: adds the additive, non-frozen `NativeExecutionLineage` evidence variant with explicit parent reference and validation rules.
- `correlation-resolver`: adds the named native execution-continuation predicate while preserving the existing three-phase resolver authority and evidence-channel separation.

## Impact

Implementation is expected to touch the existing `chronicle-canonical` domain/resolver and `chronicle-etl` composition boundary, with optional `chronicle-application` wiring for the Chronicle-owned runtime handoff source. `chronicle-session`, `chronicle-protocol`, and `chronicle-protocol-builtins` remain fact producers only; no current protocol implementation is claimed to prove cross-operation causality. Capture, eBPF, WAL, storage, replay, and CLI behavior remain unchanged in this slice.

No new crate or dependency edge is proposed. No OpenTelemetry, Datadog, X-Ray, B3, or other provider SDK enters any Chronicle crate or the default executable closure. Capture Event v1, WAL v1, Canonical Session v1, Session Manifest, persisted checkpoints, public CLI JSON, and replay-safety contracts remain unchanged. Native handoff facts are runtime/composed input in this change; durable capture of that side channel is explicitly deferred rather than smuggled into a frozen format. Re-running canonical artifacts without that input deterministically produces no native support.
