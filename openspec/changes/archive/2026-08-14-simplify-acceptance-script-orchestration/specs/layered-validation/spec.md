## ADDED Requirements

### Requirement: Canonical acceptance entrypoint

Acceptance SHALL expose exactly one user-facing entrypoint, `scripts/acceptance.sh`, accepting `--profile p1|p2|all` and `--executor local|multipass`. Compatibility wrappers SHALL NOT exist without a concrete external caller or documented compatibility obligation; a self-referential test asserting that a wrapper delegates to canonical tooling does not constitute such an obligation. Development-only deprecated aliases and compatibility-named test runners that only forward to canonical tooling SHALL be removed, and documentation and CI SHALL reference only the canonical entrypoint.

#### Scenario: Single entrypoint

- **WHEN** a contributor or CI step runs acceptance
- **THEN** they invoke `scripts/acceptance.sh` and no deprecated alias or forwarding test-name runner exists in the repository

#### Scenario: Wrapper removal is regression-tested

- **WHEN** the repository is scanned after removal
- **THEN** no tracked file references a removed wrapper path, and tooling tests fail if such a reference reappears

### Requirement: Scenario ownership and dispatch convention

Every acceptance scenario SHALL have exactly one implementation owner resolved by an explicit convention: a `shared/<scenario>.sh` providing `scenario_<name>()` when the behavior is identical across profiles, otherwise a `<profile>/<scenario>.sh` providing `scenario_<profile>_<name>()`. The dispatcher SHALL source the shared implementation when present and otherwise the profile implementation, and SHALL NOT require profile-specific extension modules that merely forward to the shared implementation. The shell dispatcher and the Python runner SHALL validate the same convention so no dynamic scenario implementation can become undiscoverable.

#### Scenario: Shared behavior scenario

- **WHEN** a scenario executes identical behavior under P1 and P2
- **THEN** a single shared implementation runs in both profiles with no per-profile extension files

#### Scenario: Profile-specific scenario

- **WHEN** P1 and P2 runtimes require different scenario assertions
- **THEN** the profile directory provides the implementation and the dispatcher resolves it without a shared stub

#### Scenario: Missing implementation fails loudly

- **WHEN** a selected scenario has neither a shared nor a profile implementation
- **THEN** both dispatcher and runner reject the run before execution with an explicit error naming the scenario and profile
