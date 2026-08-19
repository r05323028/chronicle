## Purpose

Minimal release packaging so a public developer-facing installation path is real: a GitHub Actions release workflow that produces supported Linux binaries with the eBPF capture object embedded, versioned archives, SHA-256 checksums, GitHub Release artifacts, and `chronicle --version` reporting. Distribution channels beyond raw release artifacts (package managers, Docker, Homebrew, crates.io) are explicit non-goals.

## Requirements

### Requirement: Release workflow and targets

The repository SHALL provide a release workflow started only by `workflow_dispatch`, with required `version` and explicit `mode` inputs (`prepare` or `release`). A `release` mode run SHALL accept only `github.ref == refs/heads/main`; a non-main dispatch SHALL fail before qualification, builds, tag creation, or publication. The workflow SHALL freeze `github.sha` at its first job, verify that checkout resolves to that SHA, and pass the frozen SHA explicitly to every release-related checkout and artifact/publication job. The release workflow SHALL run the canonical `./scripts/validate.sh release` gate before release builds and SHALL produce supported x86_64 Linux and aarch64 Linux archives with the eBPF capture object embedded.

#### Scenario: Dispatch from main qualifies one commit

- **WHEN** a `release` or `prepare` dispatch starts from `main`
- **THEN** the workflow freezes `github.sha`, all release checkouts use that SHA, canonical release qualification passes before build, and both supported target archives are prepared from that commit

#### Scenario: Dispatch from another ref fails closed

- **WHEN** a maintainer dispatches the workflow from a branch or ref other than `main`
- **THEN** context resolution fails before qualification, build, tag creation, or GitHub Release mutation

#### Scenario: Tagged release builds

- **WHEN** a release dispatch from `main` has a version matching the workspace
- **THEN** the workflow builds x86_64 and aarch64 Linux binaries with live capture embedded, and prepares release artifacts

### Requirement: Artifact format and checksums

Release artifacts SHALL be versioned and target-qualified archives named `chronicle-<version>-<target>.tar.gz` containing the `chronicle` binary and license, accompanied by a `SHA256SUMS` file over the archives. Artifact verification SHALL be documented in the README installation section.

#### Scenario: Archive and checksum generation

- **WHEN** the release workflow completes
- **THEN** each target produces its versioned tar.gz archive plus a checksums file whose hashes match the archives

### Requirement: Version reporting

`chronicle --version` SHALL report the workspace version without introducing a new public subcommand or changing existing command behavior.

#### Scenario: Version flag

- **WHEN** a user runs `chronicle --version`
- **THEN** the command exits 0 and prints `chronicle <workspace-version>`

### Requirement: eBPF object provenance

The tracked fallback eBPF object used by no-nightly contributor builds SHALL match a fresh build from `ebpf/` source; CI SHALL detect drift. Release builds SHALL embed the object built from source.

#### Scenario: Object drift detection

- **WHEN** the tracked fallback object differs from a fresh build of `ebpf/` source
- **THEN** CI fails and the documentation states the regeneration command

### Requirement: Release artifact validation

The release workflow SHALL validate artifacts before publishing: `chronicle --version` and `--help` on the release binary, `chronicle doctor` asserting the embedded capture object probe, and checksum verification against archive contents. Artifacts SHALL fail closed if any check fails.

#### Scenario: Release binary health

- **WHEN** the release workflow validates an artifact
- **THEN** version/help/embedded-object checks pass and checksums verify, otherwise the artifact is not published

### Requirement: Distribution non-goals

The release scope SHALL NOT introduce package managers, Docker images, Homebrew formulae, or crates.io publishing; the workspace `publish = false` SHALL remain. Docker and Kubernetes packaging remain future follow-up only.

#### Scenario: No extra channels

- **WHEN** the release scope is reviewed
- **THEN** only raw GitHub Release archives and checksums are produced

### Requirement: Single deterministic checksum manifest

Each release SHALL publish one `SHA256SUMS` file covering every target archive of that release, so any consumer can deterministically find the checksum line for a specific asset. The release workflow SHALL compose this combined manifest after all target archives are built and SHALL publish it with the release (replacing any per-job partial manifest). Asset naming, archive layout, and archive contents SHALL remain unchanged.

#### Scenario: Combined manifest over all targets

- **WHEN** a release is published
- **THEN** its `SHA256SUMS` contains a verified line for both `chronicle-<version>-x86_64-unknown-linux-gnu.tar.gz` and `chronicle-<version>-aarch64-unknown-linux-gnu.tar.gz`

#### Scenario: Installer consumes the manifest

- **WHEN** the shell installer looks up its asset in the release `SHA256SUMS`
- **THEN** the line for that asset is present and verifies, regardless of which target was built first

### Requirement: Pi-generated release notes

The release body SHALL be generated by the Pi coding agent from the repository-managed `.github/prompts/release-notes.md` and used verbatim as the GitHub Release body. The prompt SHALL instruct the agent to inspect the checked-out frozen release candidate and git history itself, including tagless pre-promotion and prepare contexts; previous semantic-version release tag or initial-public-release handling; shipped versus planned functionality; user-facing focus; and exact section structure. It SHALL be Markdown-only, contain no GitHub Actions expressions or precomputed commit lists, and fail closed on agent/provider failure without GitHub-generated-note fallback. The same prompt and generation job SHALL serve prepare and release modes.

#### Scenario: Tagged release gets Pi-written notes

- **WHEN** a release dispatch reaches the release-notes job after qualification and build preparation
- **THEN** the agent receives the full prompt file contents from the frozen checkout, its response is saved to `release-notes.md`, and publication uses `--notes-file release-notes.md --verify-tag` rather than `--generate-notes`

#### Scenario: Pi generation fails closed

- **WHEN** the Pi action, provider, or release-notes job fails
- **THEN** the tag and publication jobs do not proceed and no release body falls back to auto-generated notes

#### Scenario: Dry run generates the same notes

- **WHEN** a `prepare` dispatch reaches the release-notes job before tag promotion
- **THEN** the same prompt and generation path produce `release-notes.md` for the intended workspace version without pretending a Git tag exists

### Requirement: Release version context validation

The release workflow SHALL resolve the workspace version from the root `Cargo.toml` through one testable source of truth. The required version input SHALL match that workspace version after an optional single leading `v` is normalized. The resolver SHALL implement SemVer 2.0.0 validation, including stable versions such as `0.2.0` and prereleases such as `0.2.0-rc.1`, and SHALL reject build metadata such as `0.2.0+build.1`, malformed identifiers, and numeric identifiers with leading zeroes. The intended tag SHALL be `v<workspace-version>`. Before expensive work, the workflow SHALL reject a tag pointing at a different SHA and an already-published GitHub Release; a same-SHA existing tag and an existing draft release SHALL be resumable.

#### Scenario: Requested stable version matches workspace

- **WHEN** input `0.2.0` matches workspace version `0.2.0`
- **THEN** context resolution emits tag `v0.2.0` and `is-prerelease=false`

#### Scenario: Leading-v input is deterministic

- **WHEN** input `v0.2.0` matches workspace version `0.2.0`
- **THEN** the resolver emits the same normalized version and tag as input `0.2.0`

#### Scenario: Prerelease version is supported

- **WHEN** input `0.2.0-rc.1` matches workspace version `0.2.0-rc.1`
- **THEN** context resolution succeeds and emits `is-prerelease=true`

#### Scenario: Build metadata is rejected

- **WHEN** input or workspace version contains `+build.1`
- **THEN** context resolution fails before qualification or build

#### Scenario: Existing release state is safe

- **WHEN** the intended tag exists at the frozen SHA and its GitHub Release is absent or draft
- **THEN** the workflow may continue and resume publication; when the tag points elsewhere or the release is already published, the workflow fails without moving the tag or overwriting the release

#### Scenario: Matching tag

- **WHEN** the requested release version matches the workspace version in `Cargo.toml`
- **THEN** context resolution passes and the release pipeline proceeds

#### Scenario: Mismatched tag

- **WHEN** the requested release version differs from the workspace version in `Cargo.toml`
- **THEN** context resolution fails with a diagnostic naming both values and the pipeline stops before publication

#### Scenario: Dry-run version context

- **WHEN** a `prepare` dispatch resolves a matching workspace version without a Git tag
- **THEN** the intended release version and tag are derived from the workspace, no tag is created, and malformed versions fail

### Requirement: Release dry-run mode

`prepare` mode SHALL reuse the production qualification, build, notes, and preparation jobs, SHALL upload the prepared bundle for inspection, and SHALL stop before every repository release-state mutation. It SHALL emit a summary containing mode, version, tag, frozen SHA, archives, notes/checksum status, and publication skipped status.

#### Scenario: Prepare mode is useful and safe

- **WHEN** a maintainer dispatches `prepare` from `main`
- **THEN** qualification and artifact preparation run fully, but no tag, GitHub Release, or release asset mutation occurs

#### Scenario: Manual dry run completes without publication

- **WHEN** a maintainer triggers `workflow_dispatch` in `prepare` mode
- **THEN** the full preparation path runs, the `prepared-release` artifact contains the expected archives plus `SHA256SUMS` and `release-notes.md`, the run summary reports success, and no GitHub Release, tag, or release state is created or modified

#### Scenario: Dry run cannot publish

- **WHEN** a `prepare` run reaches the end of preparation
- **THEN** no tag-creation command or release-mutating `gh` command executes

### Requirement: Explicit GitHub CLI context and least privilege

Every job invoking `gh` SHALL declare `GH_TOKEN` and `GH_REPO`. Only jobs that create a tag or mutate a draft/final GitHub Release SHALL receive `contents: write`; context inspection, qualification, builds, notes, preparation, and checksum verification SHALL remain read-only.

#### Scenario: Mutation permissions are narrow

- **WHEN** the workflow permissions are inspected
- **THEN** only `create-tag` and `publish` have `contents: write`, and no other job has write permission

#### Scenario: gh invocations are authenticated explicitly

- **WHEN** a release job runs any `gh` release or release-state command
- **THEN** that job declares `GH_TOKEN` and `GH_REPO`, and only mutation jobs hold write permission

### Requirement: Checksum completion gate

The workflow SHALL generate and verify one deterministic `SHA256SUMS` over all target archives in `prepare-release`, before `create-tag` is reachable. Publication SHALL attach/retain `SHA256SUMS`, verify both archives and the checksum asset through GitHub, and SHALL publish only after the expected asset set is present.

#### Scenario: Missing asset blocks publication

- **WHEN** any expected archive or `SHA256SUMS` is missing or verification fails
- **THEN** the draft is not published and the workflow fails

#### Scenario: Checksums generated and verified before publication

- **WHEN** the preparation stage runs
- **THEN** `SHA256SUMS` is composed from the collected archives and verified before tag creation or publication can run

#### Scenario: Checksums missing or failing

- **WHEN** checksum composition, verification, or post-creation asset verification fails
- **THEN** the workflow fails and the release pipeline is not considered successfully completed

### Requirement: Third-party release actions use immutable commit references

The release workflow SHALL reference the third-party `shaftoe/pi-coding-agent-action` at the verified full 40-character commit SHA corresponding to the intended stable `v2.27.0` release. A semantic-version tag, branch, abbreviated SHA, or other moving reference SHALL fail workflow validation. The action step SHALL retain least privilege and receive `OPENCODE_GO_API_KEY` only where release-note generation requires it.

#### Scenario: Verified action pin

- **WHEN** the release workflow is checked before publication
- **THEN** the Pi action reference is a full commit SHA and resolves to the intended `v2.27.0` release commit

#### Scenario: Version tag is rejected

- **WHEN** a maintainer changes the Pi action reference to `v2.27.0`, `v2`, a branch, or an abbreviated SHA
- **THEN** the release workflow validation fails before release publication

### Requirement: Release qualification and artifact preparation

The canonical `./scripts/validate.sh release` command SHALL be the release qualification boundary and SHALL be a hard dependency of release build and tag-promotion jobs. Release preparation SHALL fail closed unless both target archives, `release-notes.md`, and a single generated `SHA256SUMS` exist; `SHA256SUMS` SHALL be verified with `sha256sum -c` before tag creation. Release binary version/help checks and embedded eBPF capture-object checks SHALL remain mandatory. Release notes SHALL continue to come from the repository-managed `.github/prompts/release-notes.md` and SHALL never fall back to GitHub-generated notes.

#### Scenario: Qualification or preparation fails

- **WHEN** canonical release qualification, release-note generation, archive validation, checksum generation, or checksum verification fails
- **THEN** no tag or GitHub Release is created

#### Scenario: Prepared artifacts are complete

- **WHEN** qualification and preparation succeed
- **THEN** the prepared bundle contains `chronicle-<version>-x86_64-unknown-linux-gnu.tar.gz`, `chronicle-<version>-aarch64-unknown-linux-gnu.tar.gz`, `SHA256SUMS`, and `release-notes.md`, with verified checksums

### Requirement: Release promotion and publication

A production run SHALL create `refs/tags/v<version>` only after qualification and preparation succeed, targeting exactly the frozen release SHA through an explicit GitHub References API call. Tag creation SHALL never force-update, delete, or recreate an existing tag. After tag promotion, publication SHALL create or reuse a draft GitHub Release with `--verify-tag` or equivalent explicit tag verification, upload/repair every required asset, verify the complete asset set and prerelease flag, and only then publish the draft. A published release SHALL never be overwritten automatically.

#### Scenario: Tag is promoted after preparation

- **WHEN** all qualification and prepared-artifact jobs pass in `release` mode
- **THEN** `refs/tags/v<version>` points to the frozen SHA and publication is reachable

#### Scenario: Dry preparation cannot mutate release state

- **WHEN** `prepare` mode completes
- **THEN** it exercises qualification, builds, notes, packaging, and checksum verification but cannot reach tag creation, draft creation, asset upload, or draft publication

#### Scenario: Draft publication is verified before public release

- **WHEN** a tag exists and the draft release is created or reused
- **THEN** both archives and `SHA256SUMS` are present and verified before the draft is made public

#### Scenario: Stable and prerelease publication flags

- **WHEN** version has no prerelease component
- **THEN** the published release is not a prerelease

- **WHEN** version contains a prerelease component such as `-rc.1`
- **THEN** the published release is marked as a prerelease

### Requirement: Release retry and concurrency safety

The workflow SHALL use a dedicated `chronicle-release` concurrency group without `cancel-in-progress`. If infrastructure fails after tag creation, a retry with the same frozen SHA SHALL reuse the immutable tag, reuse or repair a draft release, and resume asset verification/publication. It SHALL fail loudly when the existing tag points elsewhere or when the release is already published, and SHALL never solve retry by moving the tag.

#### Scenario: Retry after tag creation

- **WHEN** tag creation succeeds but draft creation, upload, or verification fails
- **THEN** a retry may continue from the same tag and repair the draft without changing the tag

#### Scenario: Concurrent release requests

- **WHEN** two release requests are started
- **THEN** the dedicated release concurrency group serializes their release-state mutations and does not cancel the in-progress run
