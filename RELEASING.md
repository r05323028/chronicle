# Releasing Chronicle

## Normal flow

1. Prepare version and release documentation on `main`; commit and merge them.
2. Optionally run **Release → Run workflow** with `mode: prepare` and the workspace version. This runs qualification, builds, release notes, packaging, and checksum verification without changing GitHub release state.
3. Start the production workflow from `main` with `mode: release` and the matching workspace version.
4. The workflow freezes the selected commit, runs `./scripts/validate.sh release`, builds both Linux archives, generates notes, and verifies `SHA256SUMS`.
5. Only then it creates `v<version>` at the frozen commit, creates or reuses a draft GitHub Release, verifies its assets, and publishes it.

Use `0.2.0` or `v0.2.0` for stable releases, and `0.2.0-rc.1` for prereleases. Build metadata is not accepted for release versions.

## Failure handling

- **Before tag creation:** fix `main` and start a new workflow run. No release state is expected.
- **After tag creation but before publication:** retry the same workflow run or rerun publication. The existing tag must point to the same frozen commit; the workflow reuses or repairs its draft release.
- **After publication:** never move or replace the tag, and never overwrite the published release automatically. Prepare a new version for corrections.

## Repository configuration

Keep `main` ruleset protections unchanged. If a future `v*` tag ruleset restricts tag creation, allow the release workflow's repository `GITHUB_TOKEN`/automation actor to create tags. Do not weaken `main` protections or add a personal access token unless repository policy proves the token is required.
