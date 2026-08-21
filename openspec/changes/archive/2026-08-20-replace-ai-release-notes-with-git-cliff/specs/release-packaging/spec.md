## REMOVED Requirements

### Requirement: Pi-generated release notes

**Reason**: Release notes are no longer model-generated; the active contract is deterministic Git-history classification rendered by git-cliff with optional GitHub presentation enrichment. The archived requirement remains historical record only.

**Migration**: Use the deterministic git-cliff release-note requirement below. Keep `release-notes.md` as the artifact consumed by preparation and publication.

## ADDED Requirements

### Requirement: Product-focused git-cliff release notes

The release workflow SHALL generate `release-notes.md` with repository-managed `cliff.toml` and git-cliff 2.13.1 from the frozen candidate's full Git history. The job SHALL use the authoritative `VERSION`, `TAG`, and frozen `SHA` resolved by the workflow, retain `fetch-depth: 0`, and fail closed if git history is shallow, git-cliff installation/version verification fails, malformed commits are selected, or generated output is empty. Conventional Commit parsing SHALL be enabled and strict. Only `feat`, `fix`, `perf`, breaking changes, and `revert` SHALL be exposed in stable order; `docs`, `test`, `build`, `ci`, `chore`, and `refactor` SHALL be skipped. Scopes and parsed messages SHALL render without raw type prefixes, breaking commits SHALL remain visible even when their types would otherwise be skipped, and empty groups SHALL be omitted. Configured GitHub metadata SHALL enrich included entries with contributor and pull-request presentation data only; it SHALL not classify commits using labels, PR titles, or contributor identity. Generation SHALL not create a tag solely for rendering.

#### Scenario: Explicit frozen release range

- **WHEN** the release-note job runs for intended `TAG` and frozen `SHA`
- **THEN** a repository-owned resolver selects the latest applicable semantic Chronicle release tag reachable from `SHA` and git-cliff renders exactly `previous-tag..SHA` (or the full-history initial-release range) with `--tag TAG`

#### Scenario: GitHub metadata enriches presentation

- **WHEN** the configured GitHub remote returns username or PR-number metadata for an included commit
- **THEN** the template appends `by @username` and/or `in #number` without changing the Conventional Commit-derived group, and missing metadata emits no dangling text

#### Scenario: Existing intended tag is excluded

- **WHEN** `TAG` already exists at `SHA`
- **THEN** the resolver verifies its target, excludes it from previous-tag selection, and returns the same range it would return before tag creation

#### Scenario: Inconsistent release history fails

- **WHEN** the repository is shallow, `TAG` points elsewhere, a reachable Chronicle release tag is malformed or not lower than the intended version, or multiple release boundaries are inconsistent
- **THEN** range resolution fails clearly before note generation

#### Scenario: Invalid commit fails closed

- **WHEN** a commit in the selected range is not a supported Conventional Commit
- **THEN** git-cliff exits nonzero and no successful `release-notes.md` artifact is uploaded

#### Scenario: Prepare and release use same deterministic path

- **WHEN** either `prepare` or `release` mode reaches note generation
- **THEN** both modes execute the same full-history checkout, pinned git-cliff, explicit-range, and artifact-upload path; `prepare` still cannot reach tag or GitHub Release mutation

#### Scenario: Retry is reproducible

- **WHEN** the intended same-SHA tag was created before a retry
- **THEN** identical previous tag, frozen SHA, intended tag, and `cliff.toml` produce identical range selection and `release-notes.md` bytes

#### Scenario: Publication consumes release-note artifact

- **WHEN** publication creates or updates a draft release
- **THEN** it consumes `release-notes.md` with `--notes-file` and never enables `--generate-notes`
