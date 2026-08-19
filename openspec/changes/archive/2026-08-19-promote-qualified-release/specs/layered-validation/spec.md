## Purpose

Layered Chronicle validation includes portable workflow-contract tests that protect release promotion without requiring GitHub mutation or privileged kernel execution.

## MODIFIED Requirements

### Requirement: Release workflow validation

Release-workflow behavior SHALL be covered by portable tests in `scripts/tests/validation/test_release_workflow.py`, wired into fast and release qualification tooling validation. Tests SHALL verify semantic properties rather than snapshotting the complete YAML, including: no pushed `v*` production trigger; dispatch inputs and `main` restriction; strict stable/prerelease version behavior and workspace matching; frozen SHA propagation and explicit checkout refs; canonical `./scripts/validate.sh release` dependency before build/tag; tag target and immutable retry state handling; preparation/checksum ordering; no tag or GitHub Release mutation path for `prepare`; draft creation/reuse, expected-asset verification, `--verify-tag`/equivalent behavior, no generated-notes fallback, prerelease flags, least-privilege permissions, non-cancelling concurrency, and repository-managed release-note prompt behavior.

#### Scenario: Workflow contract is portable

- **WHEN** release workflow or context/state scripts regress
- **THEN** portable validation fails without requiring a real tag, GitHub Release, or privileged environment

#### Scenario: Dry preparation mutation boundary is protected

- **WHEN** a change makes a `prepare` path able to create a tag, create/upload/edit a GitHub Release, or publish a draft
- **THEN** portable validation fails

#### Scenario: Workflow regression detected without privileged gates

- **WHEN** the release workflow, prompt file, or version-context/state script changes
- **THEN** release-workflow validation tests run as part of portable tooling validation and fail on a protected regression without requiring a privileged environment

#### Scenario: Dry-run safety protected

- **WHEN** a regression would allow a manual `prepare` dispatch to publish or would skip checksum verification before tag promotion
- **THEN** release-workflow validation tests fail
