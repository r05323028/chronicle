## Context

The release workflow already resolves authoritative `VERSION`, `TAG`, and frozen `SHA`, then qualifies, builds, packages, and publishes from that candidate. Note generation must consume those outputs without checking out moving `main`; remote GitHub data may enrich presentation but never determines classification.

## Goals

- Make Conventional Commit classification and release-note grouping a deterministic function of Git history, `cliff.toml`, the explicit release range, and intended tag; allow GitHub metadata to enrich presentation only.
- Use one identical note path for `prepare` and `release`.
- Make retries identical when the intended same-SHA tag was created before a later publication failure.
- Fail closed on malformed or unsupported commits in the selected range.

## Non-goals

No change to release qualification, target builds, eBPF compilation, archive layout, checksum generation, prepared-release artifacts, tag promotion, draft-first publication, prerelease verification, concurrency, or permissions.

## Configuration

`cliff.toml` is the source of truth. git-cliff 2.13.1 enforces strict Conventional Commits and exposes only `feat`, `fix`, `perf`, breaking changes, and `revert` as product-facing groups; `docs`, `test`, `build`, `ci`, `chore`, and `refactor` are skipped. Hidden sortable group prefixes preserve Breaking Changes, Features, Bug Fixes, Performance, and Reverts order. Scopes and parsed messages render without raw type prefixes. The configured GitHub remote and read-only workflow token enrich included entries with `commit.remote.username`, `commit.remote.pr_number`, and optional first-contributor data; PR labels, fetched PR titles, and contributor identity never classify commits.

## Range algorithm

`scripts/release/resolve-release-range.py VERSION TAG SHA`:

1. Require a non-shallow repository and resolve `SHA` to its full commit ID.
2. Validate `TAG` as the workflow's `v<VERSION>` semantic release tag.
3. If the intended tag exists, require it to resolve to the frozen commit; exclude it from candidate selection.
4. List `v*` tags merged into the frozen commit. Reject invalid Chronicle-looking semantic tags and any reachable release tag whose SemVer is not lower than the intended release.
5. Select the greatest reachable lower SemVer tag, including prerelease ordering. Emit `range=<previous-tag>..<frozen-sha>` and the previous tag; emit the frozen SHA alone as the initial-release range when no previous tag exists.

The workflow provides the read-only GitHub token and repository, passes the emitted range to `git-cliff --github-repo "$GITHUB_REPO" --tag "$TAG"`, and never creates a tag for generation. Since both tag-present and tag-absent retries resolve the same previous boundary and frozen SHA, classification and grouping remain identical; remote metadata only enriches matching entries.

## Installation

The notes job downloads the official x86_64 Linux git-cliff 2.13.1 tarball from its immutable versioned release URL, verifies the checked-in SHA-512 digest, checks the reported version, and adds the binary to `GITHUB_PATH`. No new GitHub Action or moving `latest` dependency is introduced.

## Workflow path

The existing `release-notes` job keeps full-history checkout and artifact name/boundary. It installs git-cliff, resolves the range from `VERSION`/`TAG`/`SHA`, writes `release-notes.md`, validates it is non-empty, and uploads it. `prepare-release` and `publish` remain consumers of the same artifact; publication continues to use `--notes-file release-notes.md` and never `--generate-notes`.

## Validation

Portable tests assert the workflow contract, product-only config policy, range selection, current-tag exclusion, retry equivalence, malformed-history failure, breaking footers, metadata template conditionals, and actual git-cliff output in a temporary repository when pinned git-cliff is available. Local validation also runs representative product-history output without creating a release tag.
