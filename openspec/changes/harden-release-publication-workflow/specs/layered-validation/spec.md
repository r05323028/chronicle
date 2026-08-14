## Purpose

Layered validation: repository checks, unit, integration, smoke, acceptance, end-to-end, and privileged runtime validation selected by responsibility and runtime boundary. Portable assertions run in their lowest conclusive layer; privileged acceptance proves only behavior that requires the supported Linux/kernel environment.

## ADDED Requirements

### Requirement: Release workflow validation

Release-workflow behavior SHALL be covered by portable validation tests in `scripts/tests/validation/test_release_workflow.py` wired into the `tooling:validation-tests` command (fast and targeted validation). The tests SHALL protect, without duplicating the workflow YAML line-by-line: existence of `.github/prompts/release-notes.md`, absence of the release-note prompt inline in the workflow, no `--generate-notes` publication, publication consuming `release-notes.md`, explicit `gh` authentication/repository context, tag/workspace-version mismatch failure, environment-variable-driven OpenCode Go provider configuration, `workflow_dispatch` support, the publication gate (`github.event_name == 'push'` plus `startsWith(github.ref, 'refs/tags/v')`) and the absence of release-mutating `gh` commands outside the gated publish job, dry-run version resolution from the root `Cargo.toml`, `SHA256SUMS` generation and verification before publication, and the `prepared-release` bundle artifact.

#### Scenario: Workflow regression detected without privileged gates

- **WHEN** the release workflow, prompt file, or version-context script changes
- **THEN** the release-workflow validation tests run as part of portable tooling validation and fail on any protected regression without requiring a privileged environment

#### Scenario: Dry-run safety protected

- **WHEN** a regression would allow a manual dispatch to publish or would skip checksum verification before publication
- **THEN** the release-workflow validation tests fail
