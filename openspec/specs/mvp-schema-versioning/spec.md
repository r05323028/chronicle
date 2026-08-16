# Mvp Schema Versioning

## Purpose

Define mutable-v1 schema policy for internal MVP artifacts.

## Requirements

### Requirement: Internal artifact domains have one current model
Before the first public release, each internal artifact domain SHALL have exactly one active runtime model, reader, and writer path. Capture Event, private eBPF ABI, WAL, canonical session, session manifest, recording metadata, epoch catalog, rollover transition, ETL checkpoint, continuation, fixture, and machine-readable report evolution SHALL modify the current model directly; unreleased internal persisted schemas MAY be replaced rather than supported through permanent runtime compatibility layers. Persisted structures MAY retain explicit schema version numbers (for example epochs.json version 2), but version numbers SHALL NOT be forced into Rust type names unless multiple versions genuinely need to coexist at runtime.

#### Scenario: Pre-freeze schema evolution
- **WHEN** an internal artifact needs a new field or corrected meaning before 0.1.0
- **THEN** the current model, writer, reader, fixtures, and tests are updated together
- **AND** no parallel versioned runtime implementation is introduced for unreleased internal formats

#### Scenario: Different capture sources
- **WHEN** fixture and production capture provide equivalent evidence
- **THEN** both emit the same active Capture Event model
- **AND** source type does not select schema version

### Requirement: MVP readers have no obsolete compatibility dispatch
Each internal artifact domain SHALL expose one current reader/writer path. V1/V2/V3 dispatch enums, legacy readers, migration adapters, fallback defaults for obsolete models, and dual-write paths MUST NOT be retained solely for repository history. Superseded repository-only formats SHALL be rejected with typed unsupported-schema or unsupported-format error.

#### Scenario: Obsolete repository artifact
- **WHEN** bytes or JSON from superseded repository-only format are presented
- **THEN** current reader rejects them
- **AND** it does not convert, default, or partially interpret them

#### Scenario: Repository source inspection
- **WHEN** schema implementation is reviewed
- **THEN** no active V1/V2/V3 dispatch enum or legacy compatibility reader exists for internal artifacts

### Requirement: Repository artifacts migrate atomically
Schema edits SHALL migrate checked-in fixtures, golden files, embedded objects, documentation, tests, and active planning artifacts in same change. Repository tests MUST exercise only current models except explicit rejection tests for unsupported versions.

#### Scenario: Current checkout validation
- **WHEN** repository test suite runs from one commit
- **THEN** all checked-in artifacts are readable by the current readers
- **AND** no test depends on an obsolete internal format remaining readable

### Requirement: Compatibility freeze requires explicit change
Schema versions SHALL increase only after explicit OpenSpec change declares compatibility freeze, names frozen artifact contracts, defines supported reader/writer behavior, and provides migration and deprecation policy. Stable wire formats intentionally preserved by the project (WAL v1 framing, commit markers, Capture Event v1, Canonical Session v1, replay safety contracts) remain versioned separately and are out of scope for this pre-freeze replacement policy. Architecture documentation SHALL state this gate.

#### Scenario: Proposed version increase without freeze
- **WHEN** change proposes freezing an internal schema without compatibility-freeze OpenSpec change
- **THEN** proposal fails architecture acceptance
- **AND** evolution remains on the current pre-freeze model

#### Scenario: Architecture policy lookup
- **WHEN** contributor reads project architecture documentation
- **THEN** one-current-model policy and explicit freeze gate are documented as authoritative pre-0.1 policy
