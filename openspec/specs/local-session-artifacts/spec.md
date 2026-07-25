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
Each session SHALL use `<root>/sessions/<session-id>/manifest.json`, `session.json`, and `payloads/<sha256-hex>`. Artifact refs SHALL use session-qualified keys `sessions/<session-id>/payloads/<sha256-hex>` for direct key lookup. Manifest v1 SHALL include session/canonical versions, session checksum, payload counts/sizes, processing checkpoint, bounded issue summary, completeness, and replayability reasons. SHA-256 SHALL identify payload and session bytes.

#### Scenario: Deterministic lookup
- **WHEN** valid session ID is requested
- **THEN** store resolves exact UUID path without scanning or path traversal

#### Scenario: Payload integrity
- **WHEN** payload is loaded
- **THEN** store verifies expected size and SHA-256 before returning bytes

#### Scenario: Corrupt canonical session
- **WHEN** session bytes do not match manifest checksum
- **THEN** inspect and replay fail typed integrity error and do not interpret session

#### Scenario: Corrupt payload
- **WHEN** payload bytes do not match Artifact checksum
- **THEN** replay hydration fails typed integrity error; inspect checks reference existence and filesystem size only and does not claim full payload checksum verification

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
Inspect SHALL load manifest and canonical session plus payload file metadata (existence and size) without reading body contents. It SHALL succeed when fixture and WAL are absent after successful publication.

#### Scenario: WAL removed after record
- **WHEN** persisted session remains but original fixture and `<root>/wal/<session-id>` are removed
- **THEN** inspect returns same canonical summary

#### Scenario: Unknown session ID
- **WHEN** requested session directory does not exist
- **THEN** inspect returns stable not-found error without creating files

### Requirement: Safe human-readable inspection
Default inspect output SHALL show session ID; protocol; recorded source/destination; operation count; methods/targets; response statuses; request/response payload sizes; warnings/truncation/incompleteness; replayable state; and blocking reasons. It SHALL NOT print payload bodies, authorization/cookie values, or arbitrary captured header values.

#### Scenario: Complete replayable session
- **WHEN** complete supported HTTP session is inspected
- **THEN** output says replayable and lists operation summaries without body bytes

#### Scenario: Unsupported or truncated session
- **WHEN** session includes opaque, unsupported, truncated, or incomplete evidence
- **THEN** output says non-replayable and lists stable reasons/warning codes

#### Scenario: Sensitive captured header
- **WHEN** canonical HTTP data contains captured Authorization or Cookie
- **THEN** inspect output identifies sensitive-header presence only if relevant and never value

### Requirement: Safe machine-readable inspection
JSON inspect output SHALL expose same summary fields and stable warning/blocking codes as human output, excluding payload and secret values. Output ordering SHALL be deterministic.

#### Scenario: JSON inspect
- **WHEN** caller selects global JSON format
- **THEN** stdout is valid deterministic JSON summary suitable for tests and contains no payload bytes
