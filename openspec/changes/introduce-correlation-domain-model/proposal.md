## Why

Chronicle already turns capture evidence into protocol-neutral `CanonicalOperation` values and publishes deterministic `CanonicalSession` artifacts. It does not yet define how those logical interactions relate to the application Chronicle is recording, how one scenario references operations published in different epoch sessions, or who owns resolved, ambiguous, and uncorrelated outcomes. Without those boundaries, packet direction, sockets, tracing providers, or session artifacts could be mistaken for application scenario identity.

Current architecture assigns observation to `chronicle-capture`, transport reconstruction to `chronicle-session`, protocol-local message pairing to `chronicle-protocol`, canonical operations and validation to `chronicle-canonical`, and complete publication ordering to `chronicle-etl`. A recording may publish one canonical session per finalized epoch, and existing canonical provenance already records contributing epoch ranges and a completion-owner epoch. `CaptureEvent` v1 contains socket/lifecycle/payload evidence; it does not contain an event identity. `Direction` describes byte flow, not application ownership.

## What Changes

- Define Chronicle-owned `InteractionRole` semantics with `Ingress` and `Egress` values. The role is attached to, or deterministically derived for, each logical canonical interaction from application-relative ownership evidence; it is not packet direction.
- Keep `CanonicalOperation` and `OperationId` as the logical interaction and interaction identity. Add no `InteractionId`.
- Define a scoped `CanonicalOperationRef` containing recording lineage, owning epoch/session scope, and `OperationId`, so correlation can safely reference operations outside their owning `CanonicalSession` and across epoch boundaries.
- Introduce a recording-scoped `CorrelationGraph` aggregate. It owns `Scenario` children, role assignments, resolutions, and selected causal edges; resolved, ambiguous, and uncorrelated interactions remain represented in one canonical result.
- Make only an interaction classified as `Ingress` eligible to be a scenario root. Preserve protocol direction, transport carriers, infrastructure identifiers, and external trace identifiers as evidence only.
- Preserve framework-neutral evidence, optional trace enrichment, concurrency safety, ambiguity, and replayability/completeness independence.
- Explicitly omit `EventId` from this foundation. Existing `OperationId` plus scoped references and existing provenance are sufficient for correlation. Any future event identity requires a dedicated deterministic-identity or versioned-compatibility change.
- Keep this change algorithm-neutral: it defines values and validation for supplied resolutions, not a resolver, heuristic, temporal inference, runtime lineage algorithm, or automatic ownership from raw interleaved traffic.

## Capabilities

### New Capabilities

- `correlation-domain-model`: Chronicle-owned interaction roles, scoped canonical operation references, recording-scoped correlation ownership, scenario membership, causal relationships, framework-neutral evidence, and unresolved outcomes.

### Modified Capabilities

None. Existing protocol specifications continue to own protocol request/response pairing. This change adds cross-interaction scenario semantics without changing HTTP, database, WAL, capture, replay, or CLI behavior.

## Compatibility and Impact

This is a planning-only revision. It does not modify production Rust, manifests, tracing dependencies, crate boundaries, Capture Event v1, Canonical Session v1, WAL v1, Session Manifest v1, public JSON, CLI behavior, or replay behavior.

The public 0.1 compatibility boundary remains unchanged. Correlation data must not be added implicitly to frozen v1 artifacts. A later implementation may first publish a separately versioned correlation sidecar/new artifact, or may introduce an explicit 0.2 versioned contract through a dedicated compatibility/migration OpenSpec change. That decision is not authorized by this foundation.

`chronicle-common` may eventually own only neutral `ScenarioId` primitives; correlation semantics and `CanonicalOperationRef` remain in the canonical domain. No standalone correlation crate is introduced. Core/domain APIs expose no OpenTelemetry, W3C, B3, Datadog, AWS X-Ray, or other tracing SDK types.

## Out of Scope and Future Changes

- Correlation resolver or algorithm, scoring, weighted evidence, temporal inference, runtime instrumentation, or automatic scenario selection. Track in `correlate-ingress-and-egress-interactions` or equivalent.
- Pluggable trace evidence providers and concurrent-ingress correlation implementation.
- Persisted scenario/correlation artifact design, if not covered by the sidecar decision.
- Event identity for capture evidence. A future change must choose deterministic derivation from stable immutable provenance or an explicit versioned contract; it must not alter 0.1 schemas silently.
- Scenario artifact UX, replay v2, dependency matching, exports, assertions, and test generation.
