## MODIFIED Requirements

### Requirement: Unified validation modes

Validation SHALL expose one entry point with `fast`, `targeted`, `gate p1`, `gate p2`, and `release` modes. Validation modes SHALL select and orchestrate classified tests and repository checks; validation tooling SHALL NOT contain product correctness assertions or become a second test framework.

`fast` SHALL run formatting, warnings-denied workspace Clippy, strict OpenSpec validation, source/dependency architecture checks, unit tests, and explicitly cheap portable integration coverage without privileged acceptance. Existing broader workspace testing MAY remain during migration until classification and no-coverage-loss selection are proven. `targeted` SHALL select affected functional layers and environment-qualified tests from declarative ownership and SHALL print changed paths, selected tests/groups, functional layer, privilege requirement, gate reason, skipped tests/groups, and reason for every decision.

`gate p1` and `gate p2` SHALL preserve complete named milestone coverage by selecting required lower-layer prerequisites, smoke, acceptance, E2E, and privileged tests; P1/P2 SHALL NOT be test categories or dictate test storage. `release` SHALL execute or validly reuse every required portable and supported-platform privileged obligation and fail when any obligation is neither executed nor reused. Every mode SHALL execute potentially long-running commands beneath configurable hard deadlines, and complete gate/release modes SHALL have an outer gate deadline that terminates process trees and retains failed evidence. Contributor guidance SHALL recommend fast, then targeted recorder/acceptance validation, then complete privileged gates only when required; no development workflow SHALL rely on unbounded foreground daemon or poll.

#### Scenario: Fast local validation

- **WHEN** developer runs `scripts/validate.sh fast`
- **THEN** portable repository checks, unit tests, and cheap integration coverage run with command deadlines and no full privileged evidence bundle

#### Scenario: Targeted layer selection

- **WHEN** developer runs targeted validation for changed paths
- **THEN** output explains selected functional layers, environment requirements, milestone reasons, and every skip without treating P1/P2 as test locations

#### Scenario: Explicit gate

- **WHEN** developer runs `scripts/validate.sh gate p1` or `gate p2`
- **THEN** corresponding complete milestone obligation selects required classified tests and compatible evidence beneath outer gate deadline

#### Scenario: Release completeness

- **WHEN** release mode finishes
- **THEN** every required lower-layer, black-box, E2E, and supported-platform privileged check is executed or matched by valid reusable evidence, otherwise command fails

#### Scenario: Agent recorder workflow

- **WHEN** recorder-related source or test tooling changes during development
- **THEN** guidance selects fast and targeted bounded validation before requesting complete privileged gate evidence

## ADDED Requirements

### Requirement: Dependency-aware selection respects test responsibility

Declarative validation ownership SHALL distinguish functional test layer, environment requirement, crate/feature owner, and milestone gate membership. Acceptance or build-tooling changes SHALL select affected tooling self-tests and impacted classified product tests; they MUST NOT automatically force unrelated privileged product assertions solely because files live under former acceptance directory. eBPF changes SHALL select unprivileged build/decoder checks plus minimal real-kernel proof. WAL, ETL, replay, CLI, smoke, acceptance, and rootless E2E changes SHALL select their lowest conclusive portable coverage and privileged tests only when changed behavior or milestone requirements depend on real environment assertions.

#### Scenario: Acceptance helper change

- **WHEN** reusable process lifecycle helper changes
- **THEN** helper self-tests and every classified consumer are selected while unrelated privileged scenarios remain skipped with reason

#### Scenario: eBPF implementation change

- **WHEN** eBPF source or private ABI changes
- **THEN** portable build/decoder coverage and minimal relevant privileged capture proof are selected

#### Scenario: ETL-only change

- **WHEN** ETL transformation/checkpoint behavior changes without kernel capture impact
- **THEN** ETL unit/integration and affected rootless E2E run while real eBPF gate is selected only if milestone policy independently requires it

### Requirement: Gate evidence records classified coverage and environment outcomes

Acceptance evidence SHALL record selected test IDs, functional layers, privilege requirements, executed/reused status, scenario/check results, preflight result, infrastructure result, environment, fingerprint, source provenance, timeout/incomplete state, and artifact manifest. Unsupported or infrastructure-error runs SHALL remain distinguishable from product failures and SHALL never satisfy required gate coverage. Existing content-addressed compatibility, scenario attribution, current-release checkout integrity, and retention requirements SHALL remain.

#### Scenario: Compatible classified evidence reuse

- **WHEN** retained evidence has matching fingerprint, compatible environment/schema, unchanged required classified test set, complete manifest, and every required test passed
- **THEN** gate may reuse it and reports original provenance plus functional/environment coverage

#### Scenario: Unsupported evidence candidate

- **WHEN** retained report records unsupported environment, infrastructure error, timeout, `not_checked`, or incomplete test coverage
- **THEN** candidate is rejected for gate and release reuse
