## Why

Chronicle's PR-title-to-squash-commit workflow makes linear `main` a deterministic release-history source. Model-generated release notes add an unnecessary provider, network, and secret dependency to release publication and can vary for identical history.

## What Changes

- Add repository-managed `cliff.toml` with strict Conventional Commit parsing, product-only groups, breaking-change protection, stable ordering, and GitHub contributor/PR presentation enrichment.
- Add a small repository-owned release-range resolver that selects the latest reachable semantic Chronicle release tag before the frozen candidate SHA, explicitly excluding the intended tag.
- Replace the release workflow's Pi/OpenCode Go note job with pinned git-cliff 2.13.1 installation, explicit range generation, read-only GitHub metadata enrichment, and `release-notes.md` artifact upload for both `prepare` and `release` modes.
- Remove release-note-only prompt, provider configuration, model configuration, and API-key plumbing.
- Update the active release-packaging specification, release workflow tests, focused git-cliff fixtures, and `RELEASING.md`.

## Capabilities

### New Capabilities

- Deterministic release-note generation from frozen Git history.

### Modified Capabilities

- `release-packaging`: release notes are product-focused git-cliff output over an explicit semantic-tag range, with GitHub metadata used only for entry attribution; qualification, packaging, immutable promotion, and publication contracts remain unchanged.

## Non-goals

- No version bumping, Cargo manifest modification, GoReleaser, CHANGELOG.md, product behavior, qualification, native build, packaging, checksum, tag, or publication redesign.

## Impact

Only release tooling, active release specifications, release documentation, and portable validation are affected. Historical archived OpenSpec records remain unchanged. No real tag or GitHub Release is created.
