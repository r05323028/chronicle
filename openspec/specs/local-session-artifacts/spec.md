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

Each session SHALL use existing `<root>/sessions/<session-id>/manifest.json`, `session.json`, and `payloads/<sha256-hex>` layout. Artifact refs SHALL remain session-qualified for direct lookup. Manifest v1 SHALL include session/canonical versions, session checksum, payload counts/sizes, ETL last-valid-commit-marker boundary, pipeline version, WAL snapshot digest, source recording ID/status/shutdown reason, capture time range, WAL envelope/loss-window provenance summary, bounded issue summary, completeness counts, integrity, and replayability classification/reasons. Manifest SHALL NOT contain checkpoint-file checksum; post-publication checkpoint may contain manifest checksum. SHA-256 SHALL identify payload/session/WAL-snapshot bytes under documented scopes. Reader SHALL accept only manifest v1 and reject every other version.

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

Inspect SHALL load manifest and canonical session plus payload file metadata without reading/printing body contents. It SHALL verify manifest/session integrity and payload existence/size and expose embedded recording/WAL provenance summaries. It SHALL NOT infer current source-WAL availability from recording path: output root may differ/relocate and manifest stores no trusted live locator. Inspect SHALL succeed when fixture/WAL are absent after publication and report embedded provenance as self-contained, not claim retained/removed source state.

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

Canonical Session MVP v1 SHALL represent source recording/status/reason, source connection generation, WAL envelope/range/commit-marker provenance, ring/backend and WAL-limit loss windows, and typed operation completeness (`complete`, `incomplete`, `truncated`, `malformed`, `unmatched`, `unsupported`). Source provenance, connection/operation completeness maps, and replay attributes SHALL be direct session fields. Completeness maps SHALL be authoritative; connection/operation completeness flags and a separate operation-order list SHALL not duplicate them. Timeline SHALL own operation order. Session completeness SHALL aggregate operation/connection state and SHALL support partial session state while individual operations remain complete, but SHALL NOT label an operation complete when intersecting/ambiguous loss affects it. Optional metadata SHALL evolve append-only under v1; stable-release breaking changes require an explicit compatibility-freeze change.

#### Scenario: Complete operation provenance

- **WHEN** operation is decoded from several WAL records
- **THEN** canonical data identifies recording, connection generation, and all source ranges with complete state

#### Scenario: Capture loss affects completeness

- **WHEN** recording reports loss overlapping or ambiguously affecting connection
- **THEN** intersecting/uncertain connection is degraded, provably outside operation remains eligible, and session reports partial completeness

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
