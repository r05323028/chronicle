## MODIFIED Requirements

### Requirement: Versioned command JSON contracts

Documented public `record`, `list`, `inspect`, `replay`, and `doctor` machine-readable outputs that are declared stable SHALL use one deterministic version-1 schema per command contract and SHALL remain compatible within 0.1.x. Public JSON SHALL contain no human prose or captured sensitive values. Hidden `internal` output, advisory diagnostics, and other non-public reports are not compatibility-sensitive unless an active specification explicitly declares them public. Any public schema version increase, field-meaning change, or incompatible output change SHALL follow `public-compatibility-boundary` and an explicit OpenSpec change; no ad hoc v2 dispatch or fallback is permitted.

#### Scenario: New public reports

- **WHEN** a public command emits JSON under a documented stable contract
- **THEN** stdout is one valid deterministic version-1 object matching that command's documented schema

#### Scenario: No ad hoc v2

- **WHEN** implementation adds a public field or changes a public report
- **THEN** it does not introduce v2 or a v1/v2 reader/renderer dispatch without the explicit public compatibility process

#### Scenario: Automation parses outputs

- **WHEN** user or automation runs a public command with JSON format
- **THEN** stdout matches one documented deterministic v1 schema, contains no human prose or sensitive values, and hidden/advisory output is not misrepresented as public
