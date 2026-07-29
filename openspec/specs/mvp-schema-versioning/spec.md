# Mvp Schema Versioning

## Purpose

Define mutable-v1 schema policy for internal MVP artifacts.

## Requirements

### Requirement: MVP artifact domains have one mutable v1
Until compatibility freeze, each internal artifact domain SHALL have exactly one active schema or format version and that value SHALL be 1. Capture Event, private eBPF ABI, WAL, canonical session, session manifest, recording metadata, ETL checkpoint, fixture, and machine-readable report evolution SHALL modify current v1 directly.

#### Scenario: Pre-freeze schema evolution
- **WHEN** an internal artifact needs a new field or corrected meaning before compatibility freeze
- **THEN** current v1 model, writer, reader, fixtures, and tests are updated together
- **AND** no v2 or parallel source-specific schema is introduced

#### Scenario: Different capture sources
- **WHEN** fixture and production capture provide equivalent evidence
- **THEN** both emit same active Capture Event v1 model
- **AND** source type does not select schema version

### Requirement: MVP readers have no obsolete compatibility dispatch
Each internal artifact domain SHALL expose one current reader/writer path. V1/V2/V3 dispatch enums, legacy readers, migration adapters, fallback defaults for obsolete models, and dual-write paths MUST NOT be retained solely for repository history. Inputs declaring any version other than 1 SHALL fail with typed unsupported-schema or unsupported-format error.

#### Scenario: Obsolete repository artifact
- **WHEN** bytes or JSON from superseded repository-only format are presented
- **THEN** current reader rejects them
- **AND** it does not convert, default, or partially interpret them

#### Scenario: Repository source inspection
- **WHEN** schema implementation is reviewed
- **THEN** no active V1/V2/V3 dispatch enum or legacy compatibility reader exists for internal artifacts

### Requirement: Repository artifacts migrate atomically
Schema edits SHALL migrate checked-in fixtures, golden files, embedded objects, documentation, tests, and active planning artifacts in same change. Repository tests MUST exercise only current v1 except explicit rejection tests for non-v1 values.

#### Scenario: Current checkout validation
- **WHEN** repository test suite runs from one commit
- **THEN** all checked-in artifacts are readable by matching v1 readers
- **AND** no test depends on obsolete internal format remaining readable

### Requirement: Compatibility freeze requires explicit change
Schema versions SHALL increase only after explicit OpenSpec change declares compatibility freeze, names frozen artifact contracts, defines supported reader/writer behavior, and provides migration and deprecation policy. Architecture documentation SHALL state this gate.

#### Scenario: Proposed version increase without freeze
- **WHEN** change proposes internal schema version greater than 1 without compatibility-freeze OpenSpec change
- **THEN** proposal fails architecture acceptance
- **AND** evolution remains on v1

#### Scenario: Architecture policy lookup
- **WHEN** contributor reads project architecture documentation
- **THEN** mutable-v1 policy and explicit freeze gate are documented as authoritative MVP policy
