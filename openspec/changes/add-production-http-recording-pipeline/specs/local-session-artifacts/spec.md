## MODIFIED Requirements

### Requirement: Versioned checksummed filesystem layout
Each session SHALL use existing `<root>/sessions/<session-id>/manifest.json`, `session.json`, and `payloads/<sha256-hex>` layout. Artifact refs SHALL remain session-qualified for direct lookup. Manifest v2 SHALL include session/canonical versions, session checksum, payload counts/sizes, ETL input high-water mark, pipeline version, WAL snapshot digest, source recording ID/status, capture time range, WAL format/provenance summary, bounded issue summary, completeness counts, integrity, and replayability reasons. Manifest SHALL NOT contain checkpoint-file checksum; post-publication checkpoint may contain manifest checksum. SHA-256 SHALL identify payload/session/WAL-snapshot bytes under documented scopes. Reader SHALL accept manifest v1/v2 and reject unknown newer version.

#### Scenario: Deterministic lookup
- **WHEN** session ID is known
- **THEN** store derives manifest/session/payload paths without scanning unrelated sessions

#### Scenario: Payload integrity
- **WHEN** payload file size or digest differs from manifest/ref
- **THEN** inspect or hydration reports corruption and replay does not execute

#### Scenario: Corrupt canonical session
- **WHEN** `session.json` checksum differs from manifest
- **THEN** load fails before exposing session or replaying

#### Scenario: Production provenance manifest
- **WHEN** P1 ETL publishes recovered recording
- **THEN** manifest v2 contains recording/checkpoint/provenance/completeness metadata without raw payload/header values

#### Scenario: Read P0 manifest
- **WHEN** valid manifest v1 session is loaded
- **THEN** store loads it with absent production provenance defaults

### Requirement: Inspect without original capture artifacts
Inspect SHALL load manifest and canonical session plus payload file metadata without reading or printing body contents. It SHALL verify manifest/session integrity and payload existence/size, expose recording/WAL provenance summaries when present, and succeed when fixture and WAL are absent after successful publication. Missing source WAL SHALL be reported as provenance availability, not session corruption.

#### Scenario: WAL removed after record
- **WHEN** valid published session is inspected after source WAL removal
- **THEN** inspect succeeds from canonical files and reports source recording ID with WAL unavailable

#### Scenario: Recovered WAL retained
- **WHEN** source recovered WAL still exists
- **THEN** inspect reports safe provenance/high-water/checksum summary without reading payload values

#### Scenario: Unknown session ID
- **WHEN** session ID has no final directory
- **THEN** inspect returns typed not-found error without scanning staging files

### Requirement: Safe human-readable inspection
Default inspect output SHALL show session ID, source recording ID/status, protocol, operation order/count, capture time range, source/destination metadata, methods/targets, response statuses, request/response payload sizes, typed completeness and malformed/unmatched counts, recovery/warning summary, artifact integrity, schema versions, replay readiness, and blocking reasons. It SHALL NOT print payload bodies, authorization/cookie values, or arbitrary captured header values.

#### Scenario: Complete replayable session
- **WHEN** complete valid P1 session is inspected
- **THEN** output reports complete, integrity valid, replay ready, operation order, and provenance summary without payload values

#### Scenario: Unsupported or truncated session
- **WHEN** session contains unsupported/truncated/malformed/unmatched evidence
- **THEN** output reports counts and stable blocking codes and never labels whole session complete

#### Scenario: Sensitive captured header
- **WHEN** persisted session contains Authorization, Cookie, or secret-like custom header
- **THEN** default human output shows no captured value

#### Scenario: Corrupt artifact
- **WHEN** manifest/session/payload metadata integrity check fails
- **THEN** output reports invalid integrity and replay not ready

### Requirement: Safe machine-readable inspection
JSON inspect output SHALL expose stable versioned structure for same safe summary fields as human output, including deterministic operation ordering, recording/provenance, completeness counts, integrity, schema versions, warnings, and replay readiness. It SHALL exclude payload and arbitrary/secret header values. JSON encoding failure SHALL return typed data error without partial invalid JSON.

#### Scenario: JSON inspect
- **WHEN** user requests JSON for valid session
- **THEN** stdout is one valid documented JSON object with deterministic field/array ordering and no sensitive values

#### Scenario: JSON serialization failure
- **WHEN** inspect result cannot be serialized
- **THEN** command emits safe typed JSON error on stderr, no partial stdout object, and non-success exit

## ADDED Requirements

### Requirement: Typed canonical provenance and completeness
Canonical Session v3 SHALL represent source recording, source connection generation, WAL segment/record/range provenance, and typed operation completeness (`complete`, `incomplete`, `truncated`, `malformed`, `unmatched`, `unsupported`). Session completeness SHALL aggregate operation/connection state and capture/recovery loss; it SHALL NOT be complete when any contributing source is untrustworthy.

#### Scenario: Complete operation provenance
- **WHEN** operation is decoded from several WAL records
- **THEN** canonical data identifies recording, connection generation, and all source ranges with complete state

#### Scenario: Capture loss affects completeness
- **WHEN** recording reports loss overlapping or ambiguously affecting connection
- **THEN** affected connection/session cannot be reported complete without explicit evidence

### Requirement: Sensitive production data boundary
WAL and session documentation/metadata SHALL state that stored headers and bodies can contain sensitive production data. Filesystem adapter SHALL use restrictive supported permissions and default logs/inspect SHALL remain value-safe. P1 SHALL NOT claim encryption at rest, secret detection, comprehensive redaction, or tenant isolation.

#### Scenario: Private publication
- **WHEN** production session and payloads are published on supported Unix filesystem
- **THEN** directories/files use existing private modes and no wider permissions are introduced

#### Scenario: Security documentation
- **WHEN** user follows recording documentation
- **THEN** sensitive-data warning and P1 non-guarantees appear before runnable production workflow

### Requirement: Atomic deterministic re-publication
Filesystem store SHALL retain staging, sync, manifest-last, and atomic no-replace rename behavior. When deterministic ETL session already exists, application SHALL verify matching manifest/provenance/checksum and report already processed; any mismatch SHALL fail without overwrite.

#### Scenario: Matching idempotent publication
- **WHEN** ETL reruns and final session matches deterministic output
- **THEN** no new session is created and existing session remains unchanged

#### Scenario: Conflicting deterministic publication
- **WHEN** same session ID path contains different content or provenance
- **THEN** publication fails closed and retains existing final directory unchanged
