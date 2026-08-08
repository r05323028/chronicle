## MODIFIED Requirements

### Requirement: Unified validation modes

Validation SHALL expose one entry point with `fast`, `targeted`, `gate p1`, `gate p2`, and `release` modes. `fast` SHALL run formatting, warnings-denied workspace Clippy, workspace unit tests, strict OpenSpec validation, and applicable changed-crate tests without privileged acceptance. `gate p1` and `gate p2` SHALL preserve complete named gate coverage. `release` SHALL execute or validly reuse every required P1/P2 check and fail when any obligation is neither executed nor reused. Every mode SHALL execute potentially long-running commands beneath configurable hard deadlines, and complete gate/release modes SHALL have an outer gate deadline that terminates their process trees and retains failed evidence. Contributor guidance SHALL recommend fast, then targeted recorder/acceptance validation, then complete privileged gates only when required; no development workflow SHALL rely on an unbounded foreground daemon or poll.

#### Scenario: Fast local validation

- **WHEN** developer runs `scripts/validate.sh fast`
- **THEN** lightweight checks run with command deadlines and no full evidence bundle is generated

#### Scenario: Explicit gate

- **WHEN** developer runs `scripts/validate.sh gate p1` or `gate p2`
- **THEN** corresponding existing complete privileged gate is run and reported beneath an outer gate deadline

#### Scenario: Release completeness

- **WHEN** release mode finishes
- **THEN** every required P1/P2 check is executed or matched by valid reusable evidence, otherwise command fails

#### Scenario: Agent recorder workflow

- **WHEN** recorder-related source or acceptance tooling changes during development
- **THEN** guidance selects fast and targeted bounded validation before requesting complete privileged gate evidence
