# Chronicle release notes

You are writing the release notes for the Chronicle repository (a Rust toolchain that records real application traffic with eBPF and turns it into deterministic, replayable regression-test evidence).

Work from the checked-out repository and its git history. Do not rely on any precomputed list of commits: determine the release context yourself.

## What to determine

1. The release being prepared: determine the intended release version yourself.
   - If the checked-out repository points at a release tag (for example `git describe --tags --exact-match` or `git tag --points-at HEAD`), that tag identifies the release.
   - Otherwise (for example a manual dry run from a branch), there is no release tag: the intended release version is the workspace version in the root `Cargo.toml` and the intended release tag is `v<version>`.
2. The previous release: find the most recent semantic-version tag of the form `v<major>.<minor>.<patch>` that is not the current release. If no such tag exists, treat this release as the initial public release.
3. What actually shipped: inspect the repository documentation (README, docs/) and the implementation to understand the behavior delivered since the previous release. You may use git history (for example `git log --oneline <previous-tag>..HEAD`) to identify what changed, but do not dump raw commit history into the notes.

## Guidance

- Focus on user-facing functionality: installation; the record -> list/inspect -> replay workflow; `chronicle doctor`; Linux eBPF capture; durable WAL behavior; ETL/canonical session behavior where it matters to users; deterministic replay; replay safety; the currently supported protocol scope; and meaningful known limitations.
- Distinguish shipped functionality from planned functionality. Do not claim support for protocols, storage backends, deployment modes, or other features that are not implemented in this checkout.
- Ignore internal refactors unless they materially affect users.
- Write concise, developer-oriented release notes.

## Structure

Use exactly this structure:

## Highlights

## Installation

## What's included

## Known limitations

## Output rules

- Markdown only.
- No surrounding code fence.
- No introductory commentary before the first heading.
- No trailing commentary after the last section.
