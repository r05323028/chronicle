## MODIFIED Requirements

### Requirement: Versioned checksummed filesystem layout

Each published epoch session SHALL retain the existing `<root>/sessions/<session-id>/manifest.json`, `session.json`, and `payloads/<sha256-hex>` layout and manifest v1 integrity rules. Manifest/source provenance SHALL identify the stable parent `RecordingId`, `EpochId`, epoch ordinal, epoch-local WAL identity/ranges, any cross-epoch contributing ranges, completion-owner epoch, commit-marker boundary, WAL snapshot digest, source status/reason, capture range, continuation-in/out artifact references, completeness, and replayability. Parent aggregation SHALL reference session IDs and verified manifest/continuation digests but SHALL not replace the session manifest, mutate a published predecessor, or promote WAL bytes. A finalized epoch session remains an independently valid/replayable artifact for its terminal operations; continuation-only evidence is explicitly partial/non-executable rather than silently omitted. Readers SHALL continue to reject unsupported manifest versions and checksum/provenance conflicts.

#### Scenario: Epoch provenance manifest

- **WHEN** ETL publishes a finalized epoch session from a recovered WAL
- **THEN** the manifest contains parent/epoch identity, committed boundary, digest, completeness, and source status without raw payload/header values

#### Scenario: Distinct sessions for repeated ranges

- **WHEN** two epochs have equal marker ranges or identical operation payloads
- **THEN** deterministic session/artifact identity includes epoch lineage so sessions do not overwrite one another

#### Scenario: Parent aggregate mismatch

- **WHEN** parent index references a session whose manifest parent/epoch/digest disagrees
- **THEN** inspect/replay reports inconsistency and does not treat the session as published parent evidence

#### Scenario: Deterministic lookup

- **WHEN** session ID is known
- **THEN** store derives manifest/session/payload paths without scanning unrelated sessions

#### Scenario: Payload size integrity

- **WHEN** payload file size differs from manifest/ref
- **THEN** metadata-only inspect reports corruption and replay does not execute

#### Scenario: Payload digest integrity

- **WHEN** payload has expected size but digest differs during hydration
- **THEN** hydration reports corruption and replay performs zero network

#### Scenario: Corrupt canonical session

- **WHEN** `session.json` checksum differs from manifest
- **THEN** load fails before exposing session or replaying

#### Scenario: Production provenance manifest

- **WHEN** the recording ETL publishes a recovered recording
- **THEN** manifest v1 contains recording/checkpoint/provenance/completeness metadata without raw payload/header values

#### Scenario: Read P0 manifest

- **WHEN** valid manifest v1 session is loaded
- **THEN** store loads it with absent production provenance defaults

### Requirement: Inspect without original capture artifacts

Inspect SHALL load parent/epoch index, session manifests, canonical sessions, and payload file metadata without reading/printing body contents. It SHALL verify session/manifest integrity and payload existence/size, expose embedded parent/epoch/WAL provenance, and report raw epoch source availability from durable tombstone/retention evidence rather than from an untrusted path. Inspect SHALL succeed from published immutable session artifacts after raw epoch WAL is safely removed, while reporting that raw source is unavailable.

#### Scenario: Raw epoch WAL absent

- **WHEN** a valid epoch session remains after retention deletes its verified source segments
- **THEN** inspect succeeds from canonical files, identifies parent/epoch/session provenance, and reports intentional raw-source expiration honestly

#### Scenario: Parent inspection without active lock

- **WHEN** the parent has multiple finalized sessions and no live recorder lock
- **THEN** inspect returns deterministic ordered epoch summaries without scanning unrelated paths or re-running ETL

#### Scenario: Unknown epoch session

- **WHEN** an epoch/session reference has no final directory or its parent association is absent
- **THEN** inspect returns typed inconsistency/not-found without scanning staging files

#### Scenario: WAL absent or relocated

- **WHEN** valid published session is inspected without original WAL at any known path
- **THEN** inspect succeeds from canonical files and reports embedded source ID/commit-marker boundary/digest without claiming live WAL availability

#### Scenario: Recording materialized into another root

- **WHEN** same deterministic session is published into another output root
- **THEN** inspect in either root reports identical embedded provenance independent of source path alias

#### Scenario: Unknown session ID

- **WHEN** session ID has no final directory
- **THEN** inspect returns typed not-found error without scanning staging files

### Requirement: Typed canonical provenance and completeness

Canonical Session MVP v1 SHALL continue to represent source status/reason, connection generation, WAL ranges/commit-marker provenance, loss windows, and typed completeness. New epoch output SHALL append optional parent/epoch/completion-owner, typed `epoch_wal_ranges`, and continuation-reference provenance fields under the current mutable-v1 append-only policy; legacy single-epoch `wal_ranges` remain readable. A WAL epoch boundary SHALL not mark a connection or operation complete or incomplete by itself. A pending request/connection is represented by immutable continuation evidence; the epoch where decoding terminates owns the canonical operation. Parent aggregation SHALL not merge completeness maps into a false complete operation and SHALL expose unsupported/lost continuation explicitly.

#### Scenario: Complete epoch operation provenance

- **WHEN** an operation is decoded from several WAL records inside one epoch
- **THEN** canonical data identifies stable parent, epoch, connection generation, source ranges, and complete state

#### Scenario: Capture loss affects completeness

- **WHEN** an epoch reports loss overlapping or ambiguously affecting a connection
- **THEN** the affected connection/operation is degraded while provably outside evidence remains eligible

#### Scenario: Connection crosses boundary

- **WHEN** one observed connection continues after epoch rollover
- **THEN** each epoch session preserves its own typed completeness and no parent aggregate claims a complete cross-epoch operation without explicit future support

#### Scenario: Complete operation provenance

- **WHEN** operation is decoded from several WAL records
- **THEN** canonical data identifies recording, connection generation, and all source ranges with complete state

## ADDED Requirements

### Requirement: Per-epoch session publication is parent-addressable

Every finalized epoch with publishable committed evidence SHALL have at most one deterministic session associated with one parent and epoch identity. Continuation-only evidence is referenced immutably and does not create an additional replay operation.

#### Scenario: One epoch one session

- **WHEN** an epoch is finalized and ETL succeeds
- **THEN** one session is published and parent aggregation references it by verified session/digest/epoch identity

#### Scenario: Retry after rollover

- **WHEN** the recorder rolls over and retries finalization of the old epoch
- **THEN** the same epoch session is adopted or verified and no duplicate session is created

### Requirement: Canonical operation ownership is completion-based

A cross-epoch logical operation SHALL be owned by the epoch where its protocol decoder reaches terminal completion. Its immutable provenance SHALL include the completion-owner epoch plus every contributing parent/epoch/WAL range. Continuation-only evidence remains inspectable metadata and is not an executable replay operation. This rule preserves immutable predecessor sessions, deterministic retry, bounded ETL, and one operation per logical correlation.

#### Scenario: Completion-owned operation

- **WHEN** request/partial-frame evidence begins in one epoch and response/completion arrives in the next
- **THEN** the next epoch owns one canonical operation with cross-epoch provenance and the predecessor remains immutable

#### Scenario: Run ends without completion

- **WHEN** continuation remains pending at terminal shutdown
- **THEN** final trusted owner emits one incomplete/non-replayable operation with all known ranges
