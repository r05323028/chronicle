# Crate Boundaries

This document enumerates one primary responsibility, owned public concepts, allowed Chronicle dependencies, forbidden knowledge, and "must not change" reliability boundaries for every crate in the root workspace. It is the detailed counterpart to the normative `AGENTS.md` crate architecture section; `validation/architecture.toml` is the executable mirror of the dependency rules. When dependencies change, update all three in the same change.

## Current graph (implementation start, 2026-08-11)

Bounded `cargo metadata --format-version 1 --no-deps` captures 13 root-workspace crates, 43 normal path-dependency declarations, 4 dev declarations, 0 build declarations, and no dependency cycle.

```text
chronicle-common -> {}
chronicle-canonical -> {chronicle-common}
chronicle-capture -> {chronicle-common}
chronicle-capture-ebpf -> {chronicle-capture, chronicle-common}
chronicle-wal -> {chronicle-capture, chronicle-common}
chronicle-session -> {chronicle-capture, chronicle-common, chronicle-wal}   # problem edge
chronicle-protocol -> {chronicle-canonical, chronicle-common, chronicle-session}
chronicle-protocol-builtins -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-storage -> {chronicle-canonical, chronicle-common}
chronicle-replay -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-etl -> {canonical, capture, common, protocol, session, storage, wal}
chronicle-application -> {canonical, capture, common, etl, protocol, protocol-builtins, replay, session, storage, wal, capture-ebpf[optional,linux]}
chronicle-cli -> {application, common, protocol, protocol-builtins, replay}  # problem edges
```

Dev-only declarations: `application -> wal` (test-support, also normal), `cli -> capture`, `cli -> wal`, `etl -> protocol-builtins` (protocol integration tests).

## Target graph

```text
chronicle-common -> {}
chronicle-canonical -> {chronicle-common}
chronicle-capture -> {chronicle-common}
chronicle-capture-ebpf -> {chronicle-capture, chronicle-common}
chronicle-wal -> {chronicle-capture, chronicle-common}
chronicle-session -> {chronicle-capture, chronicle-common}                  # wal edge removed
chronicle-protocol -> {chronicle-canonical, chronicle-common, chronicle-session}
chronicle-protocol-builtins -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-storage -> {chronicle-canonical, chronicle-common}
chronicle-replay -> {chronicle-canonical, chronicle-common, chronicle-protocol}
chronicle-etl -> {canonical, capture, common, protocol, session, storage, wal}  (+ dev builtins)
chronicle-application -> {canonical, capture, common, etl, protocol, protocol-builtins, replay, session, storage, wal, capture-ebpf[optional,linux]}
chronicle-cli -> {chronicle-application}                                    # sole Chronicle edge, every kind
```

No workspace build dependency is allowed in the target graph.

## Crate ownership

### chronicle-common

- **Primary responsibility**: transport-neutral shared primitives: IDs, timestamps, endpoints, directions, protocol IDs, and small value/formatting helpers.
- **Owned public concepts**: `RecordingId`, `SessionId`, `ConnectionId`, `OperationId`, `ProtocolId`, `Endpoint`, `Timestamp`, directions, and small value objects.
- **Allowed Chronicle dependencies**: none (dependency leaf).
- **Forbidden knowledge**: WAL, ETL, storage, replay, protocol implementations, application, CLI.
- **Must not change**: identity and timestamp semantics shared by every crate.

### chronicle-canonical

- **Primary responsibility**: protocol-independent canonical recording/replay model, provenance, completeness, and validation.
- **Owned public concepts**: `CanonicalSession`, `CanonicalConnection`, `CanonicalOperation`, `TimelineEntry`, `Completeness`, `ReplayMetadata`, `CanonicalValidationError`.
- **Allowed Chronicle dependencies**: `chronicle-common`.
- **Forbidden knowledge**: capture implementation, WAL implementation, storage backend, application, CLI.
- **Must not change**: canonical schema v1, validation fail-closed guard, serialized field names.

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

## Reliability boundaries that must not change

- Kernel ABI/Aya details remain private to `chronicle-capture-ebpf`; only normalized capture events cross outward.
- WAL commit markers and recovery rules remain durability authority; manifests remain descriptive/rebuildable.
- ETL remains a complete Extract-Transform-Load pipeline and keeps storage dependencies.
- Canonical model remains independent from capture implementation, WAL implementation, storage backend, and CLI.
- Protocol core never depends on concrete built-ins.
- Replay remains independent from capture, WAL, and ETL internals.
- Existing crate set, recording/WAL formats, canonical schema, CLI behavior, and product behavior remain unchanged.
