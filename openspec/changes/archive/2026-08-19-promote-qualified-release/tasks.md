## 1. OpenSpec and scope

- [x] 1.1 Create proposal, release-packaging delta, layered-validation delta, design, and this task list; keep change name version-independent.
- [x] 1.2 Validate OpenSpec artifacts strictly before implementation and keep product/content scope limited to release process.

## 2. Context and state

- [x] 2.1 Update `scripts/release/resolve-release-context.sh` to read `[workspace.package].version`, normalize one leading `v`, validate SemVer stable/prerelease forms, reject build metadata, and emit `mode`, `version`, `tag`, and `is-prerelease`.
- [x] 2.2 Add release-state/tag helper logic that checks existing tag SHA and release draft/public state through read-only `gh api`, fails closed on non-404 API errors, and supports same-SHA recovery.
- [x] 2.3 Extend portable tests for stable, prerelease, leading-v, malformed, build-metadata, workspace mismatch, existing tag same/different SHA, draft reuse, and published-release refusal.

## 3. Workflow promotion path

- [x] 3.1 Replace `push.tags` with `workflow_dispatch` inputs `mode` and required `version`; add `chronicle-release` concurrency without cancellation and enforce `refs/heads/main`.
- [x] 3.2 Freeze and verify `github.sha` in `resolve-context`; propagate it through job outputs and explicit checkout `ref` values.
- [x] 3.3 Add one `release-qualification` job that runs `./scripts/validate.sh release` with required OpenSpec/Multipass setup and fresh-evidence `--force`; remove the duplicate workflow-specific privileged acceptance path.
- [x] 3.4 Make build, release notes, and `prepare-release` depend on canonical qualification; preserve target archives, binary/eBPF validation, notes, prepared bundle, and checksum verification.
- [x] 3.5 Add `create-tag` after qualification/preparation; use `gh api` refs creation against frozen SHA, no force/delete, and same-SHA retry behavior; gate job to `release` mode.
- [x] 3.6 Replace direct publication with draft create/reuse, asset repair/upload, exact asset and prerelease verification, then draft publication; retain `--verify-tag`/explicit tag checks and no generated notes.

## 4. Tests and documentation

- [x] 4.1 Extend `scripts/tests/validation/test_release_workflow.py` with semantic/property checks for triggers, main restriction, SHA freeze/checkout propagation, job ordering, dry-run mutation reachability, permissions, concurrency, draft publication, retry safety, assets, prerelease flags, and prompt behavior.
- [x] 4.2 Keep release-workflow tests wired into `scripts/validate.sh` and validation catalog without changing product validation layers.
- [x] 4.3 Add concise `RELEASING.md` covering preparation, production flow, pre/post-tag failures, immutable tags, and new-version corrections.
- [x] 4.4 Review website release-doc/version verification and installer smoke paths; run them without changing docs content or asset names.

## 5. Validation and review

- [x] 5.1 Run release workflow portable/context/state tests and any installer smoke required by unchanged asset names.
- [x] 5.2 Run website version/release-doc verification, strict OpenSpec validation, and `./scripts/validate.sh fast`.
- [x] 5.3 Run deeper `./scripts/validate.sh release` only where Multipass/Linux environment supports it; record blockers without creating tags or releases.
- [x] 5.4 Review final diff for unrelated product, CLI, WAL, ETL, replay, capture, eBPF, and documentation-content changes; run `graphify update .` after code edits.
