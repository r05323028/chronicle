## Purpose

Filesystem persistence and safe inspection of versioned canonical sessions and content-addressed binary payloads.

## Requirements

### Requirement: Filesystem-backed protocol-neutral session store

Storage crate SHALL provide filesystem adapter implementing existing metadata and artifact boundaries. Application publication SHALL use concrete protocol-neutral `publish(PublishSession { session, checkpoint, issues, replayability })` because existing metadata save trait cannot carry manifest processing metadata. Publishing any canonical session SHALL externalize non-empty inline payloads and SHALL NOT depend on fixture, WAL, HTTP parser, or replay implementation.

#### Scenario: Save canonical session

- **WHEN** record service saves HTTP canonical session
- **THEN** session metadata and operations persist under deterministic session-ID path and bodies become backend-neutral Artifact refs

#### Scenario: Save non-HTTP inline payload

- **WHEN** another protocol session contains inline payload
- **THEN** same filesystem externalization applies without HTTP branch

#### Scenario: Existing session destination

- **WHEN** final session directory already exists
- **THEN** save fails without overwrite

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

### Requirement: Atomic session publication

Store SHALL write payloads and session into unique staging directory, sync files where supported, write manifest last, and atomically rename complete staging directory to absent final path. Failures SHALL leave final path absent and cleanup staging best-effort. On Unix, all store/session/payload directories SHALL be mode `0700` and manifest/session/payload files SHALL be mode `0600`; non-Unix deployments SHALL enforce equivalent private ACLs or fail closed.

#### Scenario: Successful atomic publish

- **WHEN** every write/sync succeeds
- **THEN** final session directory appears only after manifest/session/payloads are complete

#### Scenario: Failure before rename

- **WHEN** payload, session, manifest, or sync fails
- **THEN** no final session directory is visible

#### Scenario: Concurrent same-ID publish

- **WHEN** two writers attempt same session ID
- **THEN** at most one rename succeeds and existing data is never replaced

#### Scenario: Private filesystem permissions

- **WHEN** session is published on Unix
- **THEN** directories are `0700` and files are `0600` before final rename

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

### Requirement: Safe human-readable inspection

Default inspect output SHALL show session ID, source recording ID/status/shutdown reason, protocol, operation order/count, capture time range, source/destination metadata, methods/targets, response statuses, request/response payload sizes, typed completeness and loss-window/malformed/unmatched counts, recovery/warning summary, artifact integrity, schema versions, replayability (`fully_replayable`, `partially_replayable`, `not_replayable`), executable/non-executable counts, and blocking reasons. It SHALL NOT print payload bodies, authorization/cookie values, or arbitrary captured header values.

#### Scenario: Complete replayable session

- **WHEN** a complete valid session is inspected
- **THEN** output reports complete, integrity valid, replay ready, operation order, and provenance summary without payload values

#### Scenario: Unsupported or truncated session

- **WHEN** session contains unsupported/truncated/malformed/unmatched evidence
- **THEN** output reports counts, loss provenance, partial/not replayable classification, and stable per-operation blockers without hiding complete operations

#### Scenario: Sensitive captured header

- **WHEN** persisted session contains Authorization, Cookie, or secret-like custom header
- **THEN** default human output shows no captured value

#### Scenario: Corrupt artifact

- **WHEN** manifest/session/payload metadata integrity check fails
- **THEN** output reports invalid integrity and replay not ready

### Requirement: Safe machine-readable inspection

JSON inspect output SHALL expose stable versioned structure for same safe summary fields as human output, including deterministic operation ordering, recording/provenance, loss windows, completeness counts, integrity, schema versions, warnings, and partial replayability. It SHALL exclude payload and arbitrary/secret header values and SHALL use shared atomic command JSON renderer.

#### Scenario: JSON inspect

- **WHEN** user requests JSON for valid session
- **THEN** stdout is one valid documented JSON object with deterministic field/array ordering and no sensitive values

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

### Requirement: Sensitive production data boundary

WAL and session documentation/metadata SHALL state that stored headers and bodies can contain sensitive production data. Filesystem adapter SHALL use restrictive supported permissions and default logs/inspect SHALL remain value-safe. Chronicle SHALL NOT claim encryption at rest, secret detection, comprehensive redaction, or tenant isolation.

#### Scenario: Private publication

- **WHEN** production session and payloads are published on supported Unix filesystem
- **THEN** directories/files use existing private modes and no wider permissions are introduced

#### Scenario: Security documentation

- **WHEN** user follows recording documentation
- **THEN** sensitive-data warning and non-guarantees appear before runnable production workflow

### Requirement: Atomic deterministic re-publication

Filesystem store SHALL retain staging, sync, manifest-last, and atomic no-replace rename behavior. When deterministic ETL session already exists, application SHALL verify matching manifest/provenance/checksum and report already processed; any mismatch SHALL fail without overwrite.

#### Scenario: Matching idempotent publication

- **WHEN** ETL reruns and final session matches deterministic output
- **THEN** no new session is created and existing session remains unchanged

#### Scenario: Conflicting deterministic publication

- **WHEN** same session ID path contains different content or provenance
- **THEN** publication fails closed and retains existing final directory unchanged

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
