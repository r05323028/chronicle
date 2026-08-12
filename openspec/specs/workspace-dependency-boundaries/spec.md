## Purpose

Chronicle crate ownership, allowed/forbidden workspace dependency direction, architecture documentation, and portable manifest-graph enforcement.

## Requirements

### Requirement: Every Chronicle crate has one primary owner

Chronicle SHALL document one primary responsibility, owned public concepts, allowed dependencies, and forbidden knowledge for each root-workspace crate. Documentation SHALL describe actual current APIs rather than infer responsibility from crate names and SHALL distinguish public composition APIs from internal implementation details.

The primary responsibilities SHALL be:

- `chronicle-common`: transport-neutral shared primitives, IDs, timestamps, endpoints, and small value objects;
- `chronicle-canonical`: protocol-independent canonical recording/replay model, provenance, completeness, and validation;
- `chronicle-capture`: protocol-neutral capture evidence, socket evidence, payload fragments, and loss/truncation metadata;
- `chronicle-capture-ebpf`: Linux eBPF/kernel interaction and normalization into capture events;
- `chronicle-wal`: append-only durable evidence framing, commit authority, recovery, manifests, and retention mechanics;
- `chronicle-session`: ordered bidirectional stream reconstruction, TCP ordering, connection reconstruction, and loss handling;
- `chronicle-protocol`: detector, decoder, canonicalizer, replay-adapter, and verifier SPI/registry;
- `chronicle-protocol-builtins`: built-in protocol implementations and honest planned registrations;
- `chronicle-etl`: complete Extract-Transform-Load from recovery-authoritative evidence through canonical publication;
- `chronicle-storage`: persistence abstractions and owned filesystem/in-memory artifact/session implementations;
- `chronicle-replay`: replay planning, target mapping, execution, result accounting, and verification orchestration;
- `chronicle-application`: user-facing use-case orchestration and composition;
- `chronicle-cli`: argument parsing, application dispatch, output writing, and process exit mapping.

#### Scenario: Maintainer looks up crate ownership

- **WHEN** maintainer or agent considers adding a public type or dependency to any Chronicle crate
- **THEN** `AGENTS.md` and crate-boundary architecture documentation identify one primary owner and forbidden knowledge for that crate

#### Scenario: Ownership does not follow name alone

- **WHEN** documented responsibility is compared with Cargo manifests and public source APIs
- **THEN** documentation reflects actual responsibilities, including ETL load/publication and WAL recovery/retention ownership


### Requirement: Domain and evidence boundaries remain infrastructure-independent

`chronicle-common` SHALL remain a workspace dependency leaf and MUST NOT depend on WAL, ETL, storage, replay, protocol implementations, application, or CLI. `chronicle-canonical` SHALL depend only on common primitives among Chronicle crates and MUST NOT know capture implementation, WAL implementation, storage backend implementation, application, or CLI.

`chronicle-capture` SHALL depend only on common primitives among Chronicle crates and MUST NOT know HTTP, database protocols, canonical operations, WAL mechanics, ETL, replay, application, or CLI. `chronicle-capture-ebpf` SHALL depend only on capture/common among Chronicle crates; Aya handles, kernel ABI records, and raw observations MUST remain private, and only normalized capture-domain events SHALL cross its public adapter boundary.

`chronicle-wal` MAY depend on capture/common evidence primitives required by its stable wire contracts but MUST NOT depend on session reconstruction, protocol decoding, canonical model, ETL, storage publication, replay, application, or CLI. Commit markers and validated WAL bytes SHALL remain durability/recovery authority; manifests SHALL NOT replace that authority.

#### Scenario: Protocol logic proposed in capture

- **WHEN** a change attempts to parse HTTP or a database protocol in capture or eBPF adapter code
- **THEN** ownership review and focused boundary tests reject the semantic violation and require interpretation through protocol interfaces downstream; Cargo architecture validation additionally rejects any forbidden crate edge

#### Scenario: Canonical model proposed to import WAL

- **WHEN** canonical code or manifest adds a dependency on WAL implementation types
- **THEN** architecture validation fails before merge

#### Scenario: Raw kernel observation crosses adapter

- **WHEN** eBPF adapter output is exposed to another Chronicle crate
- **THEN** output is a normalized capture event and contains no Aya handle or private kernel ABI type


### Requirement: Interpretation dependencies point toward neutral evidence and interfaces

`chronicle-session` SHALL depend only on capture/common among Chronicle crates. Its reconstruction inputs MAY carry transport-neutral persistence ordering, placement, and loss evidence but MUST NOT import or expose concrete `chronicle-wal` types.

`chronicle-protocol` MAY depend on canonical/common/session and SHALL own protocol capability interfaces and registry contracts. It MUST NOT depend on `chronicle-protocol-builtins` or any concrete protocol implementation.

`chronicle-protocol-builtins` MAY depend on canonical/common/protocol and SHALL implement protocol interfaces without making protocol core depend on it. Registrations without working capability implementations SHALL remain explicitly planned/research/unavailable rather than claiming support.

#### Scenario: WAL terminal loss reaches reconstruction

- **WHEN** ETL reads a committed WAL terminal-loss record
- **THEN** it converts that wire-owned type into transport-neutral loss evidence before calling session reconstruction

#### Scenario: Protocol core imports a built-in

- **WHEN** protocol SPI code or manifest adds a dependency on built-in HTTP/database implementations
- **THEN** architecture validation fails and composition remains application-owned

#### Scenario: Planned database protocol registration

- **WHEN** PostgreSQL, MySQL, MariaDB, MongoDB, or another planned registration lacks a detector/decoder/canonicalizer/replay/verifier implementation
- **THEN** capability metadata remains honest and no architecture cleanup claims new protocol behavior


### Requirement: ETL remains a complete Extract-Transform-Load pipeline

`chronicle-etl` SHALL own the complete deterministic pipeline:

```text
Extract: WAL -> capture evidence -> session reconstruction
Transform: detection -> decoding -> canonicalization
Load: canonical model -> storage publication -> publication verification/checkpoint ordering
```

ETL MAY depend on WAL, capture, session, protocol, canonical, storage, and common. ETL test code MAY depend on protocol built-ins. ETL MUST NOT depend on CLI, application, or eBPF implementation. Architecture cleanup MUST NOT remove ETL's storage dependency, move publication-before-checkpoint authority out of ETL, or redesign ETL as transform-only.

Current one-shot final session publication and recording-local checkpoint sequencing performed directly by application SHALL move behind an ETL-owned API. Application SHALL retain domain lock, quota policy/reservation, use-case selection, and presentation, while ETL SHALL own concrete storage publication, publication verification, and checkpoint advancement order. This move MUST preserve deterministic IDs, no-replace semantics, quota accounting, final session/checkpoint bytes, and failure ordering.

#### Scenario: Storage dependency audit

- **WHEN** ETL Cargo dependencies are checked against architecture policy
- **THEN** `chronicle-storage` is allowed and documented as required by Load responsibilities

#### Scenario: Transform-only refactor proposed

- **WHEN** architecture cleanup proposes returning transformed values while application takes over publication/checkpoint ordering
- **THEN** proposal is rejected as outside this change and ETL remains complete Extract-Transform-Load


### Requirement: Storage and replay hide upstream implementation details

`chronicle-storage` MAY depend on canonical/common and SHALL hide filesystem, future object-store, and database-client mechanics behind storage-owned APIs. It MUST NOT depend on capture, eBPF, WAL append/recovery, session, protocol implementations, ETL, replay, application, or CLI. Existing canonical-session and immutable-artifact storage APIs MAY remain distinct where their durability authorities differ; this change SHALL NOT introduce another crate or speculative abstraction.

`chronicle-replay` MAY depend on canonical/common/protocol. It SHALL accept canonical recording input and produce replay plans/results. It MUST NOT depend on capture, eBPF, WAL, session reconstruction internals, ETL, storage backend, application, or CLI.

#### Scenario: Replay without source WAL

- **WHEN** replay plans or executes a persisted canonical recording after original capture/WAL input is unavailable
- **THEN** replay remains functional through canonical/protocol inputs and requires no capture, WAL, or ETL dependency

#### Scenario: Backend implementation used outside storage

- **WHEN** filesystem/object/database persistence details are needed by another layer
- **THEN** they are accessed through storage-owned APIs rather than reimplemented or imported from backend clients directly


### Requirement: Application owns use-case composition without becoming a dumping ground

`chronicle-application` MAY depend on every non-CLI Chronicle crate needed to compose user-facing workflows, including target-gated optional eBPF implementation. It SHALL organize implementation ownership around record, recorder lifecycle, ETL execution, replay, inspect, and doctor use cases. Exact file nesting MAY vary, but unrelated use-case implementations MUST NOT accumulate indefinitely in one root module.

Application SHALL expose curated request/result/error APIs for outer adapters. It MUST NOT create new crates, duplicate ETL publication semantics, duplicate replay planning/execution, duplicate WAL durability logic, or introduce a generic service/factory abstraction without an actual second implementation or consumer.

#### Scenario: New user-facing use case

- **WHEN** application adds behavior for record, recorder, ETL, replay, inspect, or doctor
- **THEN** code is placed with that use-case owner and root exports expose only the intended outer seam

#### Scenario: Low-level primitive is re-exported

- **WHEN** application considers publicly re-exporting a quota, retention, scope, WAL, or transition primitive
- **THEN** a real cross-crate consumer or documented extension seam is required; otherwise primitive stays internal

#### Scenario: New application crate split proposed

- **WHEN** internal organization can solve ownership ambiguity
- **THEN** implementation refactors modules in `chronicle-application` and creates no new crate


### Requirement: CLI communicates through application APIs only

For all normal, dev, and build Chronicle dependencies, `chronicle-cli` SHALL depend only on `chronicle-application`. CLI production code SHALL parse arguments into application-owned requests, invoke application APIs, render or write application-owned results, and map application-owned outcome/error classifications to exit codes.

CLI MUST NOT directly import protocol/built-in/replay/WAL/ETL/capture/storage/session/canonical/common implementation types. CLI SHALL NOT contain protocol decoding, replay policy, WAL scanning/recovery, ETL orchestration, storage publication, eBPF loading, or business safety decisions. Binary-specific Clap grammar, checked output boundary, runtime setup, signal forwarding, and final process exit remain CLI-owned.

#### Scenario: Replay exit mapping

- **WHEN** CLI maps replay completion, policy stop, transport stop, or verification stop to a process exit code
- **THEN** it uses application-owned outcome/error classification and has no direct protocol or replay dependency

#### Scenario: CLI fixture contract test

- **WHEN** CLI tests require capture/WAL-backed artifacts
- **THEN** tests create them through application services or application-owned test support and add no direct CLI dev dependency on capture or WAL

#### Scenario: Forbidden CLI dependency

- **WHEN** any CLI Cargo dependency kind adds another Chronicle crate besides application
- **THEN** architecture validation fails with source, target, and dependency kind


### Requirement: Machine-readable architecture policy rejects dependency leakage

Repository SHALL contain `validation/architecture.toml` defining every root-workspace Chronicle crate, allowed dependencies by normal/dev/build kind, and explicit critical forbidden relationships. Every unlisted workspace edge SHALL be forbidden. Optional and target-specific declarations SHALL be checked independent of current host platform. Package renames SHALL be evaluated by package identity. The policy SHALL reject dependency cycles, missing/unknown policy members, dependency on CLI, session-to-WAL coupling, protocol-to-built-ins coupling, common upward coupling, and CLI-to-non-application coupling.

Deleting an allowed dependency SHALL remain valid without first changing policy; policy defines maximum permitted coupling, not required edges. Adding a legitimate edge SHALL require architecture documentation, `AGENTS.md`, policy, and rationale updates in the same change.

#### Scenario: Allowed graph

- **WHEN** Cargo dependency graph contains only declared allowed edges and remains acyclic
- **THEN** architecture validation exits successfully with deterministic output

#### Scenario: Forbidden normal edge

- **WHEN** session adds a normal dependency on WAL or protocol adds a normal dependency on built-ins
- **THEN** validation exits nonzero and identifies exact forbidden edge

#### Scenario: Forbidden dev or build edge

- **WHEN** a forbidden relationship is added through dev-dependencies or build-dependencies
- **THEN** validation rejects it rather than treating test/build code as layering escape hatch

#### Scenario: Target-specific optional edge

- **WHEN** a forbidden dependency is optional or declared only for Linux while validation runs on macOS
- **THEN** validation still discovers and rejects declaration

#### Scenario: New workspace crate lacks policy

- **WHEN** root workspace membership gains a Chronicle crate without ownership/policy entry
- **THEN** validation fails until responsibility and allowed direction are documented


### Requirement: Architecture validation is portable, bounded, and part of CI

Architecture validation SHALL use existing repository-standard bounded command execution and standard-library tooling plus Cargo metadata; it SHALL add no external package solely for graph enforcement. `scripts/validate.sh fast` and release validation SHALL execute it, making existing CI fail on architecture violations. Focused standard-library tests SHALL cover accepted graph, forbidden normal/dev/build edges, target-specific optional edges, package identity/rename handling, unknown members, and cycles.

OpenSpec validation SHALL prove specification structure only and MUST NOT be reported as runtime or architecture-check success. Architecture check is portable repository validation and MUST NOT be placed in privileged P1/P2 acceptance.

#### Scenario: Fast CI validation

- **WHEN** CI runs existing `./scripts/validate.sh fast`
- **THEN** dependency architecture check runs beneath configured timeout and fails CI on a forbidden edge

#### Scenario: Architecture helper hangs

- **WHEN** Cargo metadata or helper execution exceeds configured command deadline
- **THEN** existing timeout wrapper terminates process tree and validation fails with timeout evidence

#### Scenario: Privileged gate selection

- **WHEN** only architecture policy/docs/helper change and no production runtime behavior changes
- **THEN** portable validation runs and no privileged acceptance claim is required


### Requirement: Architecture guidance is durable for future agents

`AGENTS.md` SHALL contain a mandatory crate architecture section with primary ownership, allowed dependency direction, forbidden dependency patterns, application/CLI rules, ETL complete-pipeline rule, eBPF privacy rule, and instruction to update architecture policy/docs when dependencies change. `docs/architecture/crate-boundaries.md` SHALL provide detailed current/target graph and rationale and SHALL be linked from existing architecture documentation.

`AGENTS.md` SHALL be normative guidance for future human/agent changes; `validation/architecture.toml` SHALL be its executable mirror. Contradictory guidance or policy SHALL block completion until reconciled.

#### Scenario: Agent proposes new dependency

- **WHEN** future agent changes a Chronicle Cargo manifest
- **THEN** agent checks ownership rules, updates documentation/policy when legitimate, and runs bounded architecture validation

#### Scenario: Guidance and policy disagree

- **WHEN** `AGENTS.md` allows an edge that machine policy rejects or vice versa
- **THEN** change is incomplete until both express same rule


### Requirement: Architecture migration preserves behavior and formats

Implementation SHALL preserve recording, WAL, manifest, checkpoint, canonical, storage, replay, CLI human/JSON, and protocol behavior. Session neutralization SHALL keep WAL wire codecs and persisted field compatibility unchanged and SHALL prove equivalent reconstruction from neutral evidence. Application/CLI module and API cleanup SHALL preserve commands, arguments, output, exit codes, safety gates, and side effects.

Existing automated tests SHALL continue passing. New tests SHALL verify architecture policy and boundary conversion without claiming new product behavior. Privileged supported-Linux evidence is required only if implementation exceeds architecture-only scope and changes behavior that existing task rules assign to privileged acceptance.

#### Scenario: Persisted artifact comparison

- **WHEN** existing WAL/checkpoint/canonical fixtures run through boundary-cleaned code
- **THEN** accepted bytes, identities, completeness, provenance, and publication ordering remain unchanged

#### Scenario: CLI contract comparison

- **WHEN** existing CLI contract tests run before and after dependency cleanup
- **THEN** help, arguments, human/JSON output, exit codes, and safety behavior are unchanged

#### Scenario: Architecture-only completion

- **WHEN** OpenSpec, architecture check, focused tests, and fast validation pass with no user-visible behavior delta
- **THEN** change is accepted as architecture hygiene and makes no new product capability claim
