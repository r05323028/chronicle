## Purpose

Define the public compatibility line introduced by Chronicle 0.1 so durable recordings, replay safety, documented CLI behavior, and stable machine-readable output evolve without silently breaking users or replaying unsafe traffic.

## ADDED Requirements

### Requirement: 0.1 frozen contract scope is explicit

Starting with the first public 0.1 release, the following contracts SHALL be compatibility-sensitive within the 0.1.x release line: WAL v1 segment and envelope framing plus in-WAL commit-marker semantics; Capture Event v1 when it is intentionally persisted or versioned across the capture/WAL/fixture boundary; Canonical Session v1; Session Manifest v1; replay safety and authorization semantics; the documented public `record`, `list`, `inspect`, `replay`, and `doctor` command hierarchy, flags, defaults, exit meanings, and safety behavior; and documented public machine-readable output schemas declared as version 1. The hidden `internal` operational namespace, private eBPF ABI, transient runtime state, advisory catalog/metadata files, test-only fixtures not used as compatibility evidence, and other implementation details are not frozen by this requirement unless another public contract explicitly says so.

#### Scenario: Public artifact is in the frozen scope

- **WHEN** a 0.1.x change modifies WAL v1 bytes, persisted Capture Event v1 meaning, Canonical Session v1, Session Manifest v1, replay authorization, documented public CLI behavior, or a declared public JSON schema
- **THEN** the change is reviewed as a compatibility-sensitive change and cannot silently alter the existing contract

#### Scenario: Advisory implementation detail changes

- **WHEN** a 0.1.x change modifies private eBPF ABI details, transient state, or an advisory file that is not named as a public contract
- **THEN** the change does not claim that the implementation detail is permanently frozen, provided all frozen contracts remain valid

### Requirement: 0.1.x readers and writers preserve v1 contracts

A 0.1.x writer SHALL emit the declared v1 representation for every frozen persisted or machine-readable contract. A 0.1.x reader SHALL accept every valid v1 representation produced by a supported 0.1.x writer and SHALL preserve its defined meaning; it SHALL reject malformed data without guessing. A v1 reader SHALL NOT interpret an unknown format or schema version as v1, and a v1 writer SHALL NOT emit a newer version under a v1 label. Public command defaults, safety gates, exit meanings, and stable JSON field semantics SHALL remain compatible throughout 0.1.x.

#### Scenario: Current release reads a valid v1 artifact

- **WHEN** a supported 0.1.x reader opens a valid WAL, Capture Event, Canonical Session, or Session Manifest v1 artifact from another supported 0.1.x writer
- **THEN** it reads the artifact with the v1 meaning and does not require a same-build or same-host writer

#### Scenario: v1 reader sees a newer version

- **WHEN** a reader encounters a declared format or schema version newer than the frozen v1 contract
- **THEN** it returns a typed unsupported-version result before interpreting payload or mutating durable state

### Requirement: Unsupported versions and migrations fail closed

Unsupported or incompatible versions SHALL produce an explicit, machine-detectable failure with safe human diagnostics. Replay SHALL never execute an artifact that cannot be validated under a supported frozen contract. A migration from a frozen public contract SHALL be introduced only by an explicit OpenSpec change that names source and destination versions, preserves or accounts for provenance and integrity, defines reader/writer combinations, and provides a bounded migration or an explicit no-migration policy. No silent field defaults, heuristic reinterpretation, hidden compatibility alias, or dual writer SHALL substitute for that change.

#### Scenario: Unsupported persisted input

- **WHEN** WAL, Capture Event, Canonical Session, or Session Manifest input declares an unsupported version or fails its v1 integrity/contract validation
- **THEN** the reader fails closed, reports the unsupported or invalid condition, and leaves source evidence unchanged

#### Scenario: Incompatible migration is proposed

- **WHEN** a future change needs a representation that cannot preserve the 0.1 v1 contract
- **THEN** the change first declares an explicit versioned migration decision and validation evidence in OpenSpec rather than adding an implicit reader fallback

### Requirement: Deprecation and future incompatible changes are explicit

A public contract deprecation SHALL be declared by OpenSpec, documented in user-facing and release documentation, and preserve the 0.1.x behavior for the announced compatibility window; a deprecation SHALL NOT remove or silently reinterpret a frozen contract inside 0.1.x. Safety or integrity fixes MAY block unsafe behavior immediately, but SHALL state the compatibility impact and retain fail-closed diagnostics. Any future incompatible public change SHALL use a new compatibility line and explicit versioned contract, reader/writer policy, migration expectation, deprecation plan, tests, and documentation before implementation. Version increases SHALL never be introduced ad hoc in code.

#### Scenario: Patch release deprecation

- **WHEN** a 0.1.x feature or field is deprecated
- **THEN** its existing documented behavior remains available for the announced 0.1.x window, and the deprecation is visible without changing stable machine-readable output unexpectedly

#### Scenario: Future incompatible contract

- **WHEN** a future release cannot preserve a frozen 0.1 contract
- **THEN** an explicit OpenSpec change defines the new compatibility line and its migration/deprecation policy before any incompatible writer, reader, CLI, or replay behavior ships
