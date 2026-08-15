## ADDED Requirements

### Requirement: Semantic boundaries are enforced beyond Cargo edges

Crate dependency direction SHALL be complemented by a semantic/API boundary rule: `chronicle-cli` (and any future outer adapter) SHALL operate only on application-owned request/result/error/rendering contracts and MUST NOT name, construct, or pattern-match lower-layer vocabulary, even when that vocabulary is reachable through `chronicle-application` re-exports. `chronicle-application` MUST NOT publicly re-export lower-layer implementation vocabulary (replay policy/outcome/error types, protocol error taxonomy, WAL, ETL, capture, session, storage, canonical, or eBPF adapter types) as an escape hatch around the dependency policy; the sole standing exception is neutral primitives re-exported from `chronicle-common`. Any other lower-layer re-export SHALL be a reviewed, explicitly allowlisted contract with documented rationale added to machine policy and architecture documentation in the same change.

Replay policy (options, timing, target mapping, execution authorization) SHALL remain owned by replay/application composition; the CLI SHALL pass plain request data and receive application-owned results. Protocol error taxonomy (`ProtocolError`, `TransportErrorCategory`) and replay taxonomy (`ReplayError`, `ReplayOutcome`, `Replayability`, `OperationExecutionState`, `LoopbackReplayOptions`, `TimingMode`) MUST NOT leak into outer adapters; where an outer adapter needs a classification, application SHALL translate it into an application-facing classification. Application-owned view models exposed to outer adapters SHALL expose application-owned classification for CLI-visible decisions while preserving serialized JSON and exit-code contracts.

The semantic policy SHALL be machine-readable (`validation/architecture.toml` `[semantic]` table) and SHALL be enforced by the existing portable architecture check (extended `scripts/validation.py architecture`) wired into `validate.sh fast` and release validation. Enforcement SHALL use standard-library source scanning only, produce deterministic output, and report violations with invariant, location, rationale, and remediation. Violations MUST NOT require privileged execution to detect.

#### Scenario: Lower-layer vocabulary re-exported by application

- **WHEN** `chronicle-application` adds a public re-export such as `pub use chronicle_replay::ReplayOutcome;` without an allowlist entry
- **THEN** architecture validation fails and names the invariant, location, rationale, and remediation

#### Scenario: Outer adapter names lower-layer vocabulary

- **WHEN** `chronicle-cli` source names a forbidden symbol (for example `chronicle_application::ReplayOutcome` or `LoopbackReplayOptions`) even though the Cargo dependency graph is valid
- **THEN** architecture validation fails with file and line, even though `cli -> application` remains the only Cargo edge

#### Scenario: Application-owned contract passes

- **WHEN** `chronicle-cli` consumes an application-owned request/result/exit-code API and an application module internally composes lower-layer types without re-exporting them
- **THEN** architecture validation passes; internal orchestration is not constrained

#### Scenario: Allowlisted re-export

- **WHEN** a legitimate translated contract requires re-exporting a lower-layer type
- **THEN** the symbol is added to the reviewed allowlist with rationale and architecture documentation in the same change, and validation passes

#### Scenario: CLI-visible behavior preserved

- **WHEN** application-owned view models and exit classification replace lower-layer types in the CLI-facing contract
- **THEN** serialized JSON, human output, and process exit codes remain byte-identical and existing CLI integration, smoke, acceptance, and end-to-end suites pass unchanged
