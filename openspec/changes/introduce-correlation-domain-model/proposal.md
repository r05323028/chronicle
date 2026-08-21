## Why

Chronicle's current pipeline reconstructs protocol-neutral connections and canonical operations from `CaptureEvent` evidence, but it has no Chronicle-owned domain model for correlating independent logical interactions into one replayable application scenario. Chronicle 0.2.0 needs that boundary before adding tracing adapters, runtime lineage capture, correlation algorithms, scenario replay, or test generation; otherwise a tracing framework or infrastructure carrier could accidentally become scenario ownership.

Repository inspection on `origin/main` (`75051fc`) found the existing ownership seam: `chronicle-common` owns shared IDs and timestamps; `chronicle-capture` owns socket/process/payload evidence; `chronicle-session` groups transport evidence into connection streams; `chronicle-canonical` owns `CanonicalSession`/`CanonicalOperation`; `chronicle-protocol` and built-ins own protocol request/response correlation; and `chronicle-etl` transforms committed WAL evidence into canonical publication. The current checkout is one unrelated release commit ahead of `origin/main`; no inspected correlation boundary differs. No OpenTelemetry, W3C, B3, Datadog, or AWS X-Ray dependency exists.

## What Changes

- Define a Chronicle-owned correlation domain model for scenarios, logical interactions, causal relationships, evidence provenance, confidence, and unresolved outcomes.
- Reuse `CanonicalOperation`/`OperationId` as Chronicle's existing logical interaction concept instead of creating a duplicate operation/interaction identity.
- Establish stable event and scenario identity ownership, with infrastructure IDs remaining evidence only and session/recording/epoch identities remaining distinct.
- Define a framework-neutral `CorrelationEvidence` vocabulary covering trace, protocol, execution, process/thread, connection/socket, stream, temporal, and custom evidence without exposing tracing SDK types.
- Define categorical resolution semantics for exact, strong, inferred, ambiguous, and uncorrelated results; preserve candidates and evidence instead of forcing unsafe ownership.
- Define the conservative 0.2 scenario boundary: one root ingress, causally correlated synchronous interactions, and the ingress response; post-response background work is excluded.
- Specify where the model fits the current crate graph and how WAL, reconstruction, protocol canonicalization, ETL, storage, and future adapters interact with it.
- Add implementation tasks for domain validation, concurrency cases, architecture/dependency checks, and durable documentation updates.
- **Do not implement production code, correlation algorithms, tracing adapters, replay, exports, or test generation in this change.**

## Capabilities

### New Capabilities

- `correlation-domain-model`: Chronicle-owned scenario/interaction identities, framework-neutral correlation evidence, causal relationships, explainable resolution, conservative scenario boundaries, and safety invariants for concurrent application behavior.

### Modified Capabilities

None. Existing protocol specifications continue to own request/response pairing (for example HTTP/1.1 FIFO correlation); this change defines cross-interaction scenario ownership and does not alter that behavior.

## Impact

The future implementation is expected to extend `chronicle-common` with only neutral identity primitives needed by the domain and add a correlation module to `chronicle-canonical`, reusing existing `OperationId`/`CanonicalOperation`. `chronicle-capture`, `chronicle-session`, and protocol adapters remain evidence/reconstruction/operation producers; `chronicle-etl` remains the complete transformation and publication owner. No new crate, tracing SDK, external adapter, WAL format, Capture Event v1 format, Canonical Session v1 format, CLI contract, or production dependency is changed by this planning-only task. Architecture documentation and `AGENTS.md` will eventually receive concise durable invariants; detailed rationale remains in this change and the canonical architecture docs.
