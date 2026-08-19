## Why

The release workflow currently starts from a pushed `v*` tag, so the tag can exist before release qualification and artifact preparation have passed. Chronicle needs tag promotion semantics: one immutable release-candidate commit is qualified and packaged first, then promoted to a tag and published through a recoverable draft-release path.

## What Changes

- Replace the pushed-tag release entry point with `workflow_dispatch` inputs for `mode` (`prepare` or `release`) and required version.
- Restrict release runs to `main`, freeze `github.sha`, and use that SHA for every release checkout and the final tag.
- Resolve and validate the requested workspace version with strict SemVer support for stable and prerelease versions; reject build metadata.
- Run `./scripts/validate.sh release` as the single canonical release qualification gate before build and publication.
- Preserve x86_64/aarch64 builds, binary/eBPF validation, release notes, archive preparation, and checksum verification before tag creation.
- Add immutable, non-force tag promotion through the Git References API, with same-SHA retry recovery and different-SHA failure.
- Create or reuse a draft GitHub Release, repair/upload assets, verify every expected asset and prerelease flag, then publish; never overwrite a published release or implicitly create a tag.
- Add release concurrency without cancellation, semantic portable workflow tests, and a concise maintainer release procedure.

## Capabilities

### New Capabilities

### Modified Capabilities

- `release-packaging`: release tags become post-qualification promotions; dispatch/version/SHA validation, draft publication, immutable retry behavior, and stable/prerelease semantics become normative.
- `layered-validation`: portable release-workflow tests cover promotion ordering, frozen context, retry safety, and dry-run mutation boundaries.

## Impact

Affected files are the release workflow, release context/state scripts, portable release-workflow tests, release-packaging/layered-validation OpenSpec deltas, and a new `RELEASING.md`. No product, CLI, WAL, ETL, replay, capture, eBPF, or user documentation-content behavior changes. No real tag or GitHub Release is created by this change.
