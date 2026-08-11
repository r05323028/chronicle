## Why

Chronicle's 13-crate workspace already forms an acyclic capture-to-replay pipeline, but ownership rules are spread across crate names, manifests, source comments, and feature specs. Several edges now bypass intended boundaries—most notably `chronicle-session -> chronicle-wal` and `chronicle-cli -> {chronicle-protocol, chronicle-protocol-builtins, chronicle-replay}`—and no CI check prevents new leakage as release work expands.

This change makes existing responsibilities and dependency direction explicit before more features land. It is architecture hygiene only: preserve formats, behavior, reliability authority, and crate count while making future human- and agent-driven changes safer.

## Current Architecture Assessment

Repository review covered root and crate manifests, every crate source layout and public surface, `AGENTS.md`, current architecture/validation docs, active and archived OpenSpec artifacts, and the Cargo path-dependency graph. Root workspace contains 13 Chronicle crates; `ebpf/` and `ebpf-feasibility/` are separate excluded workspaces. Current Chronicle graph has 43 normal path-dependency declarations, 4 dev declarations, no build-dependency declarations, and no dependency cycle.

### Current crate responsibilities

| Crate | Responsibility observed in current code | Boundary assessment |
| --- | --- | --- |
| `chronicle-common` | IDs, timestamps, endpoints, directions, protocol IDs, small formatting/parsing helpers | Healthy dependency leaf; presentation helpers are minor ownership ambiguity but not a required split. |
| `chronicle-canonical` | Canonical session/operation/connection model, provenance, completeness, replay metadata, validation | Healthy: depends only on common and does not know capture, WAL, storage backend, or CLI. |
| `chronicle-capture` | Protocol-neutral socket/payload/loss evidence, capture-source lifecycle, fixtures, event codec | Healthy: capture implementation and application semantics remain outside. |
| `chronicle-capture-ebpf` | Optional Linux/Aya source, preflight, private kernel ABI decoding, normalization into capture events | Healthy and must remain adapter-only; raw ABI is private. |
| `chronicle-wal` | Append-only framing, group commit, locking, recovery, manifests, retention lifecycle | Core durability boundary is healthy; public surface is broad, but no crate split is required. Depends on capture clock types for terminal-loss evidence. |
| `chronicle-session` | Bounded connection reconstruction, TCP ordering/deduplication, loss classification, protocol-neutral streams | Needs correction: public `ReconstructionInput::TerminalWalLoss(chronicle_wal::TerminalWalLoss)` creates infrastructure leakage. WAL provenance fields remain evidence but should use transport-neutral types. |
| `chronicle-protocol` | Detector/decoder/canonicalizer/replay/verifier SPI plus registry | Healthy: concrete built-ins do not flow inward. Its broad SPI is intentional. |
| `chronicle-protocol-builtins` | HTTP implementation and honest planned registrations for PostgreSQL, MySQL/MariaDB, MongoDB and other protocols | Healthy direction: built-ins depend on SPI, never reverse. Planned registrations must not be represented as implemented capability. |
| `chronicle-etl` | Validated WAL extraction, session reconstruction, protocol interpretation, canonicalization, incremental artifact publication, and publication/checkpoint helpers | Storage dependency is healthy and must remain, but one-shot final publication/checkpoint sequencing still lives partly in application. Target consolidates that Load transaction behind an ETL-owned API without changing storage or format authority. |
| `chronicle-storage` | Canonical session publication/inspection plus immutable recording-artifact storage abstractions and filesystem/in-memory implementations | Direction is healthy; overlapping APIs need documentation, not a new crate. Backend details must stay behind storage-owned APIs. |
| `chronicle-replay` | Default-deny target mapping, planning, execution, result accounting, verification through protocol SPI | Healthy: consumes canonical/protocol/common only and does not know WAL, capture, ETL, or storage. |
| `chronicle-application` | Composition root and user-facing use-case orchestration for record, recorder lifecycle, ETL, replay, inspect, doctor, recovery and publication | Correct layer, but root module/public re-export surface is too broad and monolithic; internal use-case ownership needs cleanup without new crates. |
| `chronicle-cli` | Clap parsing, compatibility command mapping, rendering, signal/runtime setup, exit mapping | Needs correction: normal dependencies and imports reach into common, protocol, built-ins, and replay instead of using only application-owned request/result/error APIs. |

### Current dependency direction

Healthy backbone:

```text
chronicle-cli
  -> chronicle-application
       -> evidence, interpretation, processing, and domain crates

chronicle-capture-ebpf -> chronicle-capture -> chronicle-common
chronicle-wal          -> chronicle-capture -> chronicle-common
chronicle-session      -> chronicle-capture -> chronicle-common
chronicle-protocol-builtins -> chronicle-protocol
chronicle-etl -> WAL/capture/session/protocol/canonical/storage
chronicle-replay -> canonical/protocol/common
chronicle-storage -> canonical/common
chronicle-canonical -> common
```

Problem edges and ownership gaps:

- `chronicle-session -> chronicle-wal`: reconstruction accepts concrete WAL loss type; transport-neutral evidence must replace it.
- `chronicle-cli -> chronicle-protocol`, `chronicle-protocol-builtins`, and `chronicle-replay`: CLI constructs/matches lower-layer types and error taxonomies; application API must own this seam.
- `chronicle-cli -> chronicle-common`: CLI uses shared escaping/timestamp types directly; application-owned render-safe DTOs should remove the normal dependency.
- `chronicle-application/src/lib.rs`: thousands of lines and broad re-exports mix use cases with implementation primitives; internal ownership is unclear even though crate placement is correct.
- Architecture docs describe intended flow but do not enumerate the actual manifest graph, and `scripts/validate.sh fast` has no dependency-direction check.

Boundaries that must not change:

- Kernel ABI/Aya details remain private to `chronicle-capture-ebpf`; only normalized capture events cross outward.
- WAL commit markers and recovery rules remain durability authority; manifest remains descriptive/rebuildable.
- ETL remains a complete Extract-Transform-Load pipeline and keeps storage dependencies.
- Canonical model remains independent from capture implementation, WAL implementation, storage backend, and CLI.
- Protocol core never depends on concrete built-ins.
- Replay remains independent from capture, WAL, and ETL internals.
- Existing crate set, recording/WAL formats, canonical schema, CLI behavior, and product behavior remain unchanged.

## What Changes

- Document one primary responsibility, owned public concepts, forbidden knowledge, and allowed dependency direction for every Chronicle crate in `docs/architecture/crate-boundaries.md`.
- Add explicit machine-readable workspace dependency policy in `validation/architecture.toml`, covering normal, dev, build, optional, and target-specific path dependencies.
- Add a dependency-free architecture check using Cargo metadata/manifests and run it from bounded fast/release validation so CI rejects forbidden or unclassified edges.
- Remove `chronicle-session`'s dependency on WAL-specific implementation types by introducing/reusing transport-neutral persistence-loss/provenance evidence at the existing evidence boundary.
- Move current application-owned one-shot final session publication and recording-local checkpoint sequencing behind an ETL-owned API, preserving application quota/domain policy and exact artifact/order behavior.
- Refactor `chronicle-application` internally around record, recorder, ETL, replay, inspect, and doctor use cases; curate its public facade without creating crates or changing behavior.
- Remove CLI normal and dev dependencies on protocol/replay/common/capture/WAL internals; route parsing-to-request, test fixture preparation, and result/error-to-rendering through application-owned APIs. CLI's sole Chronicle dependency is application in every dependency kind.
- Update `AGENTS.md` with crate ownership, layering, dependency direction, and forbidden-pattern rules. `AGENTS.md` is normative guidance; `validation/architecture.toml` is its executable mirror.
- Preserve all current formats, product behavior, supported protocols, ETL load semantics, and reliability authority.

## Capabilities

### New Capabilities

- `workspace-dependency-boundaries`: Chronicle crate ownership, allowed/forbidden workspace dependency direction, architecture documentation, and portable manifest-graph enforcement.

### Modified Capabilities

None. Existing product capability requirements do not change; this change codifies and enforces architecture used to implement them.

## Impact

Affected planning/implementation areas: all crate manifests as policy inputs; `chronicle-session`, `chronicle-etl`, `chronicle-application`, `chronicle-storage`, and `chronicle-cli` for boundary cleanup; shared evidence types only where needed to remove WAL leakage; `docs/architecture/`, `AGENTS.md`, `validation/`, `scripts/validate.sh`, validation-tool tests, and CI through the existing fast validation entrypoint.

Expected public Rust API movement is internal architecture cleanup within the unreleased workspace; serialized recording, WAL, manifest, checkpoint, canonical, and CLI JSON/human contracts remain unchanged. No new external dependency, crate, protocol, backend, command, or user-visible behavior is introduced.

## Non-Goals

Do NOT:

- rewrite the architecture completely;
- create unnecessary crates or split `chronicle-application` into new crates;
- change recording format, WAL format, canonical schema, checkpoint schema, or storage artifact format;
- change CLI commands, arguments, output, exit codes, or behavior;
- add protocols or claim planned protocol implementations are complete;
- make ETL transform-only, remove its storage dependency, or change persisted ETL/publication formats;
- weaken WAL durability, recovery, loss accounting, retention proof, replay safety, or platform boundaries;
- add privileged acceptance work for a portable architecture-policy check.

## Acceptance Criteria

Completion state remains unchecked until implementation evidence exists, per repository task rules:

- [ ] Every crate has one documented primary responsibility.
- [ ] Dependency direction is documented.
- [ ] Architecture validation exists.
- [ ] Forbidden dependency edges fail validation.
- [ ] CLI only depends on the application boundary for Chronicle APIs across normal, dev, and build dependencies.
- [ ] Application responsibilities are separated internally.
- [ ] Session does not depend on WAL-specific implementation types.
- [ ] `AGENTS.md` contains architecture rules.
- [ ] Existing tests continue passing.
- [ ] No user-visible behavior changes.
