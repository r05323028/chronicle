## Context

Current `.github/workflows/release.yml` starts from `push.tags: v*`, qualifies through a workflow-specific build and acceptance path, and publishes directly. `scripts/validate.sh release` is already Chronicle's canonical release qualification boundary, including portable checks and fresh privileged acceptance. The change must preserve existing release assets and repository-managed Pi notes while moving tag creation after qualification and preparation.

## Goals

- Make `workflow_dispatch` the only entry point, with explicit `prepare` and `release` modes.
- Make `github.sha` immutable release context and prevent a later `main` merge from changing any checkout.
- Reuse `./scripts/validate.sh release` rather than maintaining a second release qualification definition.
- Keep preparation identical for dry runs and production until the mutation boundary.
- Make tag promotion immutable and publication resumable after infrastructure failure.

## Non-goals

- No product, CLI, WAL, ETL, replay, capture, eBPF, installer, or release-artifact format changes.
- No tag creation, release publication, personal access token, ruleset weakening, or new third-party action.
- No automatic overwrite of published releases or movement of tags.

## Workflow contract

Inputs:

- `mode`: required choice, `prepare` or `release`; default `prepare`.
- `version`: required string; accepts `0.2.0` and one leading `v`, rejects malformed SemVer and `+build` metadata.

Top-level workflow uses `workflow_dispatch` only and has `concurrency: group: chronicle-release` with `cancel-in-progress: false`. Default permissions remain `contents: read`.

`resolve-context` is first job. It explicitly checks `github.event_name == workflow_dispatch`, `github.ref == refs/heads/main`, and that the checked-out `git rev-parse HEAD` equals `github.sha`. It runs the single version resolver against root `Cargo.toml`, emits `mode`, normalized `version`, intended `tag`, `is-prerelease`, and `sha`, and calls a read-only GitHub API state check. Existing tag handling is fail-closed: different SHA fails; same SHA is marked recoverable. Existing published release fails; an existing draft is marked resumable. First-run absence passes.

All subsequent source checkouts use `ref: ${{ needs.resolve-context.outputs.sha }}`. No job checks out `main` by name. The build, release-note, and qualification jobs therefore observe one commit even if `main` advances.

## Job graph

1. `resolve-context`: branch/event/version/semver/frozen-SHA/state preflight; read-only GitHub API.
2. `release-qualification`: checkout frozen SHA, install OpenSpec and Multipass prerequisites, run `./scripts/validate.sh release --vm chronicle-release --force`. This is the sole release qualification gate.
3. `release`: matrix-build eBPF and Chronicle binaries for x86_64 and aarch64 from the frozen SHA; validate binary and package archives.
4. `release-notes`: after build, checkout frozen SHA with full history and generate `release-notes.md` from `.github/prompts/release-notes.md` using the existing pinned Pi action/provider.
5. `prepare-release`: collect both archives and notes, assert exact expected filenames, generate/verify `SHA256SUMS`, upload `prepared-release`, and emit summary. It needs qualification explicitly as well as build/note jobs.
6. `create-tag`: job condition `needs.resolve-context.outputs.mode == 'release'`; needs qualification and preparation; uses only `contents: write` and `gh api` to create `refs/tags/<tag>` with `<sha>`. It reuses same-SHA tag, verifies final ref, and has no force/delete path.
7. `publish`: job condition `needs.resolve-context.outputs.mode == 'release'`; needs `create-tag` and `prepare-release`; downloads the prepared bundle; creates or reuses a draft release; uploads required assets with repair-safe clobbering; verifies exact asset names, checksums, tag, draft state, and prerelease flag; then patches the draft to public.

Build and notes jobs remain read-only. `create-tag` and `publish` are the only write jobs. The release state helper distinguishes missing, same-SHA, and different-SHA tags and rejects published releases during preflight. Publication independently rejects a published release before any edit/upload.

## Publication retry model

- Before tag creation: any failure leaves no production tag; fix `main`, then start a new dispatch with matching workspace version.
- After tag creation: the tag is immutable. A same-SHA rerun skips tag creation, finds an absent/draft release, and creates or repairs the draft. A different-SHA run fails rather than moving the tag.
- Existing draft: update its notes if needed, upload required assets with `--clobber` to repair partial uploads, verify assets, and publish only after verification.
- Existing published release: fail before mutation. A release correction requires a new version.
- API errors other than an explicit not-found response fail closed; no missing-state assumption on transient errors.

## Version and release flags

The resolver reads `[workspace.package].version` with Python standard-library TOML parsing, validates SemVer 2.0.0 without build metadata, normalizes one leading `v`, and emits `is-prerelease` based on a non-empty prerelease component. The publish job compares GitHub's `isPrerelease` to that output before making the draft public.

## Rulesets and permissions

The workflow uses repository `GITHUB_TOKEN` and GitHub REST refs API. No PAT is introduced. Repository administration must ensure the workflow actor/token is allowed to create `v*` refs if a future tag ruleset restricts tag creation; the `main` ruleset remains unchanged. If a tag ruleset rejects the repository token, maintainers must configure an actor/app exception or approved token policy rather than weakening `main`; code does not bypass rulesets.

## Validation strategy

Extend the existing standard-library workflow tests with semantic assertions and functional temp-manifest/fake-`gh` tests for version, prerelease, frozen SHA, state recovery, ordering, dry-run mutation boundaries, draft publication, asset verification, permissions, concurrency, and prompt preservation. Run the release-workflow suite, resolver/state tests, installer smoke if asset names change (expected unchanged), website release-doc verification, strict OpenSpec validation, `./scripts/validate.sh fast`, and deeper release qualification where Multipass supports it.
