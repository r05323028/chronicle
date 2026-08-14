## Context

Chronicle main (commit f5c4b9f, 2026-08-15) publishes releases via `.github/workflows/release.yml`, triggered on `v*` tag pushes. Current flow: a two-target matrix build job (`release`) builds `x86_64`/`aarch64` Linux binaries with `--features linux-ebpf --locked`, packages `chronicle-<version>-<target>.tar.gz` archives, and uploads them as artifacts; `publish` downloads them and runs `gh release create "$GITHUB_REF_NAME" dist/chronicle-*.tar.gz --generate-notes`; `checksums` then downloads the archives from the release, composes a single `SHA256SUMS` over both targets, verifies with `sha256sum -c`, and uploads it with `gh release upload --clobber`. Workflow-level `permissions: contents: read`; `publish` and `checksums` override to `contents: write`. No job declares `GH_TOKEN`/`GH_REPO`; all `gh` calls rely on the runner's ambient `GITHUB_TOKEN` auto-detection. Version source: `version = "0.1.0"` at `Cargo.toml` line 7 (`[workspace.package]`), already parsed by the build job with `grep -m1 '^version' Cargo.toml`.

No release tags exist yet; the first `v0.1.0` release is imminent. The repo has no release-note tooling today; `docs/release-notes.md` is a hand-written developer-facing changelog.

Validation is layered (`scripts/validate.sh` fast/targeted/gate p1|p2/release); `.github/**` is owned by the `build_tooling` group (selects privileged gates). Portable tooling tests live in `scripts/tests/validation/` (stdlib unittest), enumerated explicitly in `scripts/validate.sh` (fast + gate-tooling-tests) and in the `tooling:validation-tests` catalog command in `validation/test-architecture/test-catalog.toml`.

## Goals / Non-Goals

**Goals:**

- Release body authored by the Pi coding agent from a repository-managed external prompt, used verbatim as the GitHub Release body.
- Pi configured through a custom `opencode-go` provider (`~/.pi/agent/models.json`) using the OpenAI-compatible Chat Completions API at `OPENCODE_GO_BASE_URL`, model `RELEASE_NOTES_MODEL` (`deepseek-v4-flash`), with the API key referenced by environment-variable name (`OPENCODE_GO_API_KEY`) and injected only into the provider-using step.
- Fail-closed release notes: any Pi/provider failure stops the pipeline; `--generate-notes` is removed entirely, never a fallback.
- Early tag/workspace-version consistency validation, failing before any build or publication.
- Explicit `GH_TOKEN`/`GH_REPO` on every `gh`-invoking job; least-privileged permissions preserved.
- Checksum guarantees preserved and completed: single deterministic `SHA256SUMS`, verification before upload, and an explicit assertion that the release carries the asset.
- Portable validation tests protecting the meaningful workflow behaviors.

**Non-Goals:**

- No changes to WAL, ETL, canonical session, storage, replay, capture, eBPF, recorder, or CLI product behavior.
- No new LLM SDK and no custom direct OpenCode Go API client; `shaftoe/pi-coding-agent-action` is the abstraction.
- No release tag created/pushed and no GitHub Release published by this change.
- No secrets in the repository; no duplication of literal `OPENCODE_GO_*` values across the workflow.
- No restructuring of the build matrix or archive layout; checksum flow keeps its post-publish position (release publication stays the source of truth for archives).

## Decisions

### D1: pi-coding-agent-action pinned to v2.27.0

Latest immutable stable release (2026-08-03, `immutable: true`, target `v2`) at the time of writing. Pinning an immutable `v2.x.x` release (never `develop`/`main`/`v2`) keeps behavior reproducible for a publish path that runs infrequently and must not break silently. Inputs used: `github_token`, `provider`, `model`, `thinking_level`, `load_builtin_extensions`, `prompt`; the `response` output carries the generated Markdown. `load_builtin_extensions: false` keeps the agent's tool surface minimal and read-only.

### D2: Custom provider configured in ~/.pi/agent/models.json, env-driven

The action's bundled Pi SDK reads `~/.pi/agent/models.json`; the workflow writes it in a preceding step. `baseUrl` and model `id` are populated from `OPENCODE_GO_BASE_URL`/`RELEASE_NOTES_MODEL` (non-secret workflow-level env) by `scripts/release/write-opencode-go-models-json.py`; `api` is `openai-completions`; `authHeader` is `true`; `apiKey` is the literal string `"$OPENCODE_GO_API_KEY"` — an environment-variable-name reference that Pi resolves at request time, so the secret value never appears in the file or the repository. The secret is injected only into the Pi action step's `env`. `scripts/release/write-opencode-go-models-json.py` fails if the required env vars are missing.

### D3: Dedicated release-notes job, fail closed, artifact handoff

`release-notes` runs after `validate-tag` and the `release` build matrix, checks out with `fetch-depth: 0` (full history so the agent can find the previous release tag itself), loads `.github/prompts/release-notes.md` into a step output, runs the Pi action, saves `steps.generate.outputs.response` to `release-notes.md`, sanity-checks the file is non-empty, and uploads it as a `release-notes` artifact. Any failure at any step aborts the job; `publish` depends on `release-notes`, so the release is never created without the agent's notes. No fallback path exists.

### D4: Publish consumes release-notes.md, verify-tag, no generate-notes

`publish` now `needs: [release, release-notes]`, downloads both the build artifacts and the `release-notes` artifact, and runs `gh release create "$GITHUB_REF_NAME" dist/chronicle-*.tar.gz --notes-file release-notes.md --verify-tag`. `--generate-notes` is deleted. `gh` stays the sole publisher; Pi only produces text.

### D5: Tag/workspace-version validation as an early job + testable script

`validate-tag` runs first (needed by `release` and `release-notes`), checks out, and runs `scripts/release/validate-tag.sh "$GITHUB_REF_NAME" Cargo.toml`. The script strips a leading `v`, derives the version from the manifest via the same grep/sed the build job uses, compares, and exits 1 with both values on mismatch. Extracting this into a script makes the mismatch-fails behavior functionally testable by the portable suite.

### D6: Explicit gh context and least privilege

All three `gh`-using jobs (`publish`, `checksums`) declare `env: GH_TOKEN: ${{ github.token }}, GH_REPO: ${{ github.repository }}`. `contents: write` stays limited to `publish` and `checksums` (release/assets mutation); `release-notes` and `validate-tag` stay read-only. The Pi action gets the default token via `github_token` input.

### D7: Checksums preserved with completion assertion

The `checksums` job keeps its post-publish position (release is the archive source of truth), composition, `sha256sum -c` verification, and `gh release upload --clobber`. It gains an explicit final assertion step that the release assets include `SHA256SUMS` (`gh release view --json assets`), so a release is not considered successfully completed without it, and the run fails otherwise.

### D8: Prompt file is the single prompt source

`.github/prompts/release-notes.md` holds the complete prompt (Markdown only, no GitHub Actions expressions, no precomputed commit list). The workflow loads the file verbatim into the action's `prompt` input; nothing about the prompt content lives in `release.yml`. The prompt directs the agent to determine current/previous tags and shipped scope itself from the checkout.

### D9: Portable workflow validation tests

`scripts/tests/validation/test_release_workflow.py` (stdlib unittest) asserts: prompt file exists and is non-empty; the prompt's structural headings are absent from `release.yml` (prompt not inlined); `release.yml` references the prompt path and never contains `--generate-notes`; publication uses `--notes-file release-notes.md` and `--verify-tag`; every `gh`-invoking job declares `GH_TOKEN`/`GH_REPO` and no read-only job requests `contents: write`; `validate-tag` uses the script and the script rejects a mismatched tag against a temp manifest and accepts a match; `write-opencode-go-models-json.py` writes `baseUrl`/model id from env, `api` = `openai-completions`, `authHeader` = true, `apiKey` = env-var-name reference, and fails on missing env. Registration: add to the `tooling-tests`/gate-tooling-tests lines in `scripts/validate.sh` and to the `tooling:validation-tests` catalog command. Structural checks are property-based (mini YAML-indentation scan), not a whole-file YAML duplicate.

## Risks / Mitigations

- **Pi/provider failure at release time** — fail closed by job dependency; run fails visibly; no silent fallback. First real release should be observed and the notes reviewed before use.
- **models.json apiKey resolution** — Pi resolves `$OPENCODE_GO_API_KEY` from the step environment at request time; the secret is injected only on the action step. If provider auth were ever required earlier, the symptom would be a Pi startup failure (fail closed), not a leaked secret.
- **Action version drift** — immutable pin v2.27.0; bump deliberately.
- **gh context regressions** — protected by the portable workflow tests (D9).

## Dry-run mode (extension)

### D10: workflow_dispatch = full preparation, no mutation

`workflow_dispatch` joins the trigger set. A manual run executes the identical preparation pipeline (resolve-context -> release matrix -> release-notes -> prepare-release) and stops there. All release-mutating `gh` commands live in the publish job, which carries a job-level condition `github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')` — an event/ref expression, not an environment variable, so nothing can accidentally enable publication. The previous post-publish `checksums` job is removed: checksum composition and verification move into `prepare-release` (before publication), and publish attaches `SHA256SUMS` at release creation and asserts the asset post-creation, preserving the completion gate.

### D11: Single version/tag context script

`scripts/release/resolve-release-context.sh` supersedes `validate-tag.sh`: it takes `<release|dry-run>` plus an optional tag, derives the version from the root `Cargo.toml` (semver sanity-checked), and prints `mode/version/tag/is-release` lines consumed by the `resolve-context` job outputs. Release mode fails on tag/version mismatch; dry-run mode requires no tag and reports the intended tag `v<version>`. Keeps version parsing in exactly one testable place.

### D12: Prepared bundle + summary

`prepare-release` downloads both archives and `release-notes.md`, asserts the expected `chronicle-<version>-<target>.tar.gz` set, composes and verifies `SHA256SUMS`, uploads the `prepared-release` artifact (in both modes; publish consumes it), and writes a `GITHUB_STEP_SUMMARY` block (mode, version, intended tag, archives, notes/checksum status, publication skipped/deferred). Dry runs end with the explicit "Dry run completed successfully. No GitHub Release was created." line. The summary is informational only; the publish condition is the security boundary.

### D13: Dry-run prompt context

The release-note prompt (single file, still external) instructs the agent to determine the release version itself: the checked-out release tag when present, otherwise the workspace version in the root `Cargo.toml` (tag form `v<version>`). No GitHub Actions expressions, no separate dry-run prompt, no inlining.

### D14: Dry-run validation

`test_release_workflow.py` gains dry-run protections: dispatch support, the publication gate expression, no release-mutating `gh` outside publish, checksums-before-publication ordering, prepared-release bundle presence, read-only preparation jobs (only `publish` holds `contents: write`), and functional `resolve-release-context.sh` coverage (match/mismatch/dry-run/malformed/missing).
