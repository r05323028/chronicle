# Crate Boundaries

This document enumerates one primary responsibility, owned public concepts, allowed Chronicle dependencies, forbidden knowledge, and "must not change" reliability boundaries for every crate in the root workspace. It is the detailed counterpart to the normative `AGENTS.md` crate architecture section; `validation/architecture.toml` is the executable mirror of the dependency rules. When dependencies change, update all three in the same change.

## Current graph

The following graph is the current root-workspace Chronicle dependency graph. `validation/architecture.toml` is the executable dependency-policy authority.

```text
chronicle-common -> {}
chronicle-canonical -> {chronicle-common}
chronicle-capture -> {chronicle-common}
chronicle-capture-ebpf -> {chronicle-capture, chronicle-common}
chronicle-wal -> {chronicle-capture, chronicle-common}
chronicle-session -> {chronicle-capture, chronicle-common}
chronicle-protocol -> {chronicle-canonical, chronicle-common, chronicle-session}
chronicle-protocol-builtins -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-storage -> {chronicle-canonical, chronicle-common}
chronicle-replay -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-etl -> {canonical, capture, common, protocol, session, storage, wal}
chronicle-application -> {canonical, capture, common, etl, protocol, protocol-builtins, replay, session, storage, wal, capture-ebpf[optional,linux]}
chronicle-cli -> {chronicle-application}
```

Additional dev declarations: `application -> wal` (test-support; also a normal edge), `etl -> protocol-builtins` (protocol integration tests). No workspace build dependency is allowed.

## Crate ownership

### chronicle-common

- **Primary responsibility**: transport-neutral shared primitives: IDs, timestamps, endpoints, directions, protocol IDs, and small value/formatting helpers.
- **Owned public concepts**: `RecordingId`, `SessionId`, `ConnectionId`, `OperationId`, `ScenarioId`, `ProtocolId`, `Endpoint`, `Timestamp`, directions, and small value objects.
- **Allowed Chronicle dependencies**: none (dependency leaf).
- **Forbidden knowledge**: WAL, ETL, storage, replay, protocol implementations, application, CLI.
- **Must not change**: identity and timestamp semantics shared by every crate.

### chronicle-canonical

- **Primary responsibility**: protocol-independent canonical recording/replay model, provenance, completeness, correlation foundation, and validation.
- **Owned public concepts**: `CanonicalSession`, `CanonicalConnection`, `CanonicalOperation`, `CanonicalOperationRef`, `InteractionRole`, `InteractionRoleResolution`, `CorrelationGraph`, `CorrelationEvidence`, `TimelineEntry`, `Completeness`, `ReplayMetadata`, `CanonicalValidationError`.
- **Allowed Chronicle dependencies**: `chronicle-common`.
- **Forbidden knowledge**: capture implementation, WAL implementation, storage backend, application, CLI, and external tracing/provider SDKs.
- **Must not change**: canonical schema v1, validation fail-closed guard, serialized field names, or v1 persistence boundaries.

### chronicle-capture

- **Primary responsibility**: protocol-neutral capture evidence: socket evidence, payload fragments, loss/truncation metadata, capture-source lifecycle, fixtures, event codec.
- **Owned public concepts**: `CaptureEvent`, `CaptureEventKind`, `SocketIdentity`, `SocketEvidence`, `PayloadFragment`, `LossWindowObserved`, `TruncationMetadata`, `FixtureCaptureSource`.
- **Allowed Chronicle dependencies**: `chronicle-common`.
- **Forbidden knowledge**: HTTP/database protocols, canonical operations, WAL mechanics, ETL, replay, application, CLI.
- **Must not change**: Capture Event v1 wire codec and transport-neutral evidence semantics.

### chronicle-capture-ebpf

- **Primary responsibility**: optional Linux eBPF/kernel interaction and normalization into capture events.
- **Owned public concepts**: the adapter boundary emitting normalized `CaptureEvent`s; preflight diagnostics.
- **Allowed Chronicle dependencies**: `chronicle-capture`, `chronicle-common`.
- **Forbidden knowledge**: Aya handles, kernel ABI records, and raw observations must stay private; only normalized capture-domain events cross the adapter.
- **Must not change**: raw kernel ABI privacy; optional platform-gated build.

### chronicle-wal

- **Primary responsibility**: append-only durable evidence framing, group commit, locking, recovery, manifests, retention mechanics, and terminal-loss wire records.
- **Owned public concepts**: WAL segments, commit markers, `TerminalWalLoss` and its v1 codec, recovery, manifests, retention.
- **Allowed Chronicle dependencies**: `chronicle-capture`, `chronicle-common` (evidence primitives required by stable wire contracts).
- **Forbidden knowledge**: session reconstruction, protocol decoding, canonical model, ETL, storage publication, replay, application, CLI.
- **Must not change**: commit-marker durability/recovery authority; manifests remain descriptive/rebuildable; WAL format v1 bytes.

### chronicle-session

- **Primary responsibility**: ordered bidirectional stream reconstruction, TCP ordering/deduplication, connection reconstruction, and loss handling.
- **Owned public concepts**: `ReconstructionAssembler`, reconstruction inputs/events, protocol-neutral streams/connections.
- **Allowed Chronicle dependencies**: `chronicle-capture`, `chronicle-common`.
- **Forbidden knowledge**: concrete WAL types (terminal-loss wire records are converted to neutral evidence before reconstruction), protocol implementations, ETL, storage, replay, application, CLI.
- **Must not change**: reconstruction determinism and loss-classification semantics.

### chronicle-protocol

- **Primary responsibility**: detector/decoder/canonicalizer/replay-adapter/verifier SPI and registry.
- **Owned public concepts**: `ProtocolRegistry`, detector/decoder/canonicalizer/replay/verifier interfaces, protocol streams.
- **Allowed Chronicle dependencies**: `chronicle-canonical`, `chronicle-common`, `chronicle-session`.
- **Forbidden knowledge**: concrete built-in protocol implementations (`chronicle-protocol-builtins`).
- **Must not change**: protocol SPI contracts used by built-ins and ETL.

### chronicle-protocol-builtins

- **Primary responsibility**: built-in protocol implementations and honest planned registrations.
- **Owned public concepts**: HTTP implementation; planned/research PostgreSQL, MySQL/MariaDB, MongoDB registrations (explicitly not claimed as implemented).
- **Allowed Chronicle dependencies**: `chronicle-canonical`, `chronicle-common`, `chronicle-protocol`.
- **Forbidden knowledge**: protocol core must never depend on this crate; planned registrations must not be represented as implemented capability.
- **Must not change**: registration honesty and direction (built-ins depend on SPI, never reverse).

### chronicle-etl

- **Primary responsibility**: complete Extract-Transform-Load from recovery-authoritative evidence through canonical publication.
- **Owned public concepts**: validated WAL extraction, session reconstruction, protocol interpretation, canonicalization, incremental artifact publication, one-shot final session publication, publication verification, and checkpoint advancement ordering.
- **Allowed Chronicle dependencies**: `chronicle-canonical`, `chronicle-capture`, `chronicle-common`, `chronicle-protocol`, `chronicle-session`, `chronicle-storage`, `chronicle-wal`; dev `chronicle-protocol-builtins`.
- **Forbidden knowledge**: CLI, application, eBPF implementation.
- **Must not change**: ETL remains complete Extract-Transform-Load; storage dependency and publication-before-checkpoint authority stay in ETL; persisted ETL/checkpoint formats unchanged.

### chronicle-storage

- **Primary responsibility**: canonical session publication/inspection plus immutable recording-artifact storage abstractions and filesystem/in-memory implementations.
- **Owned public concepts**: `FilesystemSessionStore`, `RecordingStore`, artifact storage traits, metadata repository.
- **Allowed Chronicle dependencies**: `chronicle-canonical`, `chronicle-common`.
- **Forbidden knowledge**: capture, eBPF, WAL append/recovery, session reconstruction, protocol implementations, ETL, replay, application, CLI.
- **Must not change**: no-replace publication semantics and artifact immutability; existing API families remain distinct where durability authorities differ.

### chronicle-replay

- **Primary responsibility**: replay planning, target mapping, execution, result accounting, and verification orchestration.
- **Owned public concepts**: replay options, plans, outcomes, result accounting, verification.
- **Allowed Chronicle dependencies**: `chronicle-canonical`, `chronicle-common`, `chronicle-protocol`.
- **Forbidden knowledge**: capture, eBPF, WAL, session reconstruction internals, ETL, storage backend, application, CLI.
- **Must not change**: default-deny safety model; replay independence from capture/WAL/ETL internals.

### chronicle-application

- **Primary responsibility**: user-facing use-case orchestration and composition (record, recorder, ETL, replay, inspect, doctor).
- **Owned public concepts**: request/result/error APIs for outer adapters; domain lock, quota policy, supervised scope, composition.
- **Allowed Chronicle dependencies**: every non-CLI Chronicle crate needed for composition, including optional target-gated `chronicle-capture-ebpf`.
- **Forbidden knowledge**: none beyond not depending on CLI; must not duplicate ETL publication semantics, replay planning, or WAL durability logic.
- **Must not change**: exact domain-lock acquisition, quota accounting, and reliability authority; no new crates.

### chronicle-cli

- **Primary responsibility**: argument parsing, application dispatch, output writing, and process exit mapping.
- **Owned public concepts**: Clap grammar, command selection, rendering/writing, runtime/signal setup, exit codes.
- **Allowed Chronicle dependencies**: `chronicle-application` only, for every dependency kind.
- **Forbidden knowledge**: protocol decoding, replay policy, WAL scanning/recovery, ETL orchestration, storage publication, eBPF loading, business safety decisions.
- **Must not change**: CLI commands, arguments, output, exit codes, or behavior.

Correlation foundation remains a canonical-domain concern, not a new crate or a persisted v1 field. `InteractionRole` contains only `Ingress` and `Egress`; `InteractionRoleResolution` keeps known, unknown, and candidate-specific ambiguous states separate from `Direction` and `SocketRole`. `CorrelationGraph` owns recording-scoped `Scenario` children, role/correlation indexes, and selected causal edges. `CanonicalOperationRef` scopes `OperationId` by recording, owner epoch, and session; epoch rollover remains a publication boundary while `ScenarioId` remains recording-scoped. Ambiguous and uncorrelated operations remain discoverable without synthetic ownership, and only `Known(Ingress)` can be a scenario root. Correlation evidence is Chronicle-owned and provider-neutral; optional external trace values are opaque enrichment.

## Semantic/API boundaries

Dependency edges alone do not define a boundary: `chronicle-cli -> chronicle-application` is technically satisfied even when the CLI consumes replay/protocol vocabulary re-exported by application. The semantic boundary is: **outer adapters operate only on application-owned contracts**. This section is the documentary counterpart of the `[semantic]` table in `validation/architecture.toml`.

### Invariants

- **S1 — Outer adapters do not consume lower-layer vocabulary.** CLI (and any future external adapter) does not name, construct, or pattern-match lower-layer vocabulary, even when reachable through application re-exports. It passes plain request data and consumes application-owned requests/results/errors/rendering.
- **S2 — Application does not re-export lower-layer vocabulary as an escape hatch.** `pub use` of replay/protocol/WAL/ETL/capture/session/storage/canonical/built-ins/eBPF-adapter items from `chronicle-application` is forbidden except (a) neutral primitives from `chronicle-common` and (b) explicitly allowlisted reviewed contracts (`allowed_re_exports` entries with rationale, added in the same change as their documentation).
- **S3 — Application-owned view models expose application-owned classification.** Fields an outer adapter needs to interpret (outcome, replayability, operation state) are application-owned types with stable serialization; JSON and exit codes remain unchanged.
- **S4 — Replay policy stays in replay/application composition.** Options, timing, target mapping, and execution authorization are constructed inside application (`ReplayRequest` -> `LoopbackReplayOptions`); the CLI never builds replay policy.
- **S6 — Provider integrations are separately distributed optional adapters.** No crate in the transitive normal/build dependency closure of the default distribution roots (`chronicle-cli`, the public `chronicle` executable) may declare a forbidden provider package — renamed, optional, target-specific, or feature-gated declarations included — and the fully resolved dependency graph of those roots must not reach one either, so providers entering through third-party transitive crates are equally forbidden. Provider SDKs live only in adapter/plugin crates outside that closure, which translate provider data into Chronicle-owned evidence contracts. Lightweight trace-context wire-format parsers remain provider-neutral and are not provider SDKs.

### CLI forbidden vocabulary (enforced)

`LoopbackReplayOptions`, `ReplayOutcome`, `Replayability`, `TimingMode`, `OperationExecutionState`, `ReplayError`, `ProtocolError`, `TransportErrorCategory`. The vocabulary scan covers `crates/chronicle-cli/src/**` and `crates/chronicle-cli/tests/**` (word-boundary matches, comment lines stripped).

### Reviewed access seams (intentional, allowed)

- `protocol_registry()` returns a protocol-owned registry that the CLI passes through to application functions without invoking registry methods; application owns the built-ins dependency.
- `chronicle-common` primitives (`RecordingId`, `SessionId`, `Timestamp`, `escape_control`) are re-exported as neutral domain vocabulary.
- `InspectSessionResult.replayability` is a serialized-only field: the CLI never names the replay-owned type; application render functions own presentation. If a future adapter must interpret it, application translates.

### Enforcement

`scripts/validation.py architecture` scans application re-export lines and CLI source against the `[semantic]` table, checks protected core/domain direct package dependencies against `[external_dependencies]`, walks the transitive normal/build workspace-edge closure of `default_distribution_roots` to reject declared forbidden provider packages anywhere in the default distribution graph, and additionally walks the fully resolved Cargo metadata graph (`resolve.nodes`, third-party crates included) from the same roots so providers entering through external transitive dependencies are also rejected (diagnostics include the dependency path); wired into `validate.sh fast` and release. Violation messages state the invariant, location, rationale, and remediation.

## Reliability boundaries that must not change

- Kernel ABI/Aya details remain private to `chronicle-capture-ebpf`; only normalized capture events cross outward.
- WAL commit markers and recovery rules remain durability authority; manifests remain descriptive/rebuildable.
- ETL remains a complete Extract-Transform-Load pipeline and keeps storage dependencies.
- Canonical model remains independent from capture implementation, WAL implementation, storage backend, and CLI.
- Protocol core never depends on concrete built-ins.
- Replay remains independent from capture, WAL, and ETL internals.
- Existing crate set, recording/WAL formats, canonical schema, CLI behavior, and product behavior remain unchanged.
