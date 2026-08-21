# Releasing Chronicle

## Release-note source

Chronicle release notes follow the canonical linear history:

```text
PR title → squash commit on main → git-cliff → release-notes.md
```

`cliff.toml` defines product-only grouping and strict Conventional Commit parsing. Its GitHub remote settings and the workflow's read-only token enrich included entries with PR and contributor metadata; remote metadata never controls classification. The workflow installs pinned git-cliff 2.13.1, verifies its SHA-512 digest, and renders commits after the previous reachable semantic `v<version>` tag through the frozen candidate SHA. It uses the same explicit range before and after an intended same-SHA tag exists. Release version remains authoritative in `Cargo.toml` and the workflow resolver; git-cliff does not bump versions.

## Normal flow

1. Prepare version and release documentation on `main`; commit and merge them.
2. Optionally run **Release → Run workflow** with `mode: prepare` and the workspace version. This runs qualification, builds, deterministic release notes, packaging, and checksum verification without changing GitHub release state.
3. Start the production workflow from `main` with `mode: release` and the matching workspace version.
4. The workflow freezes the selected commit, runs `./scripts/validate.sh release`, builds both Linux archives, generates `release-notes.md` from full Git history, and verifies `SHA256SUMS`.
5. Only then it creates `v<version>` at the frozen commit, creates or reuses a draft GitHub Release, verifies its assets, and publishes it.

Use `0.2.0` or `v0.2.0` for stable releases, and `0.2.0-rc.1` for prereleases. Build metadata is not accepted for release versions.

## Failure handling

- **Before tag creation:** fix `main` and start a new workflow run. No release state is expected.
- **After tag creation but before publication:** retry the same workflow run or rerun publication. The existing tag must point to the same frozen commit; the workflow reuses or repairs its draft release and regenerates identical notes.
- **After publication:** never move or replace the tag, and never overwrite the published release automatically. Prepare a new version for corrections.

## Repository configuration

Keep `main` ruleset protections unchanged. If a future `v*` tag ruleset restricts tag creation, allow the release workflow's repository `GITHUB_TOKEN`/automation actor to create tags. Do not weaken `main` protections or add a personal access token unless repository policy proves the token is required.
