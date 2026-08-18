## MODIFIED Requirements

### Requirement: Canonical Session v1

Writer SHALL emit sole Canonical Session v1 with backend-neutral Artifact payload refs, structured warnings, typed completeness, endpoint evidence, and typed recording/WAL provenance. Reader SHALL accept only `schema_version == 1` and reject every other value. Canonical Session v1 is a frozen public contract within 0.1.x under `public-compatibility-boundary`; its existing field meanings, integrity/provenance semantics, and replay-relevant completeness behavior SHALL NOT be silently changed. Additive optional metadata may remain version 1 only when it preserves existing field meaning, remains readable when absent, and is covered by the public compatibility review. `PayloadRef::Object` remains supported. WAL, protocol payload, and Session Manifest v1 remain separate contracts.

`CanonicalSession` SHALL own source provenance, connection/operation completeness maps, and replay attributes directly. Completeness maps SHALL be authoritative; connection/operation flags SHALL not duplicate them. Timeline SHALL be sole authoritative operation order.

#### Scenario: Write production HTTP session

- **WHEN** the recording ETL publishes a production session
- **THEN** session uses Canonical Session v1 and typed HTTP/provenance structures

#### Scenario: Non-v1 canonical version

- **WHEN** reader sees schema version zero, two, or newer
- **THEN** it rejects explicitly, performs no replay, and reports unsupported version without mutating the session

#### Scenario: Append compatible optional metadata

- **WHEN** Chronicle adds optional canonical metadata without changing existing field meaning
- **THEN** writer and reader remain Canonical Session v1, existing readers continue to handle the absent field, and the compatibility review records the additive change
