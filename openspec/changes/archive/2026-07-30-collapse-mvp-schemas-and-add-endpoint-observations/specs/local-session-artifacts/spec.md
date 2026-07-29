## MODIFIED Requirements

### Requirement: Versioned checksummed filesystem layout
Each session SHALL use `<root>/sessions/<session-id>/manifest.json`, `session.json`, and `payloads/<sha256-hex>`. Artifact refs SHALL use session-qualified keys `sessions/<session-id>/payloads/<sha256-hex>` for direct key lookup. Sole current manifest v1 SHALL include current Canonical v1 identifier, session checksum, payload counts/sizes, processing checkpoint, bounded issue summary, completeness, replayability reasons, and source endpoint provenance summary where applicable. SHA-256 SHALL identify payload and session bytes. Store SHALL read only current manifest v1 and Canonical v1; obsolete repository layouts SHALL NOT have compatibility readers.

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

#### Scenario: Obsolete layout version
- **WHEN** manifest or canonical session declares version other than 1
- **THEN** store rejects artifact with typed unsupported-version error
- **AND** no compatibility dispatch, migration, or defaulting occurs
