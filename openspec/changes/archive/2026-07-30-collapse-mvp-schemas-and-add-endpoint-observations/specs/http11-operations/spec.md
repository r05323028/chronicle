## MODIFIED Requirements

### Requirement: Canonical schema compatibility
Writer and reader SHALL use sole mutable Canonical Session v1 with backend-neutral Artifact payload refs, structured canonical warnings, and current HTTP operation data. Reader SHALL reject any canonical schema value other than 1 and MUST NOT dispatch to, migrate, or default fields from obsolete canonical models.

#### Scenario: Read current v1 session
- **WHEN** valid current schema v1 session is loaded
- **THEN** all required warning, payload-reference, protocol-data, endpoint, and replay metadata fields are interpreted directly

#### Scenario: Write HTTP session
- **WHEN** HTTP session is persisted
- **THEN** canonical schema version is 1
- **AND** HTTP protocol data uses sole active v1 representation

#### Scenario: Unsupported canonical version
- **WHEN** filesystem manifest or session declares canonical schema other than 1
- **THEN** inspect and replay fail typed compatibility error without partial interpretation
- **AND** no old-version fallback or migration path runs
