## Why

Installing Chronicle today means manually visiting the GitHub Release page, downloading the right `chronicle-<version>-<target>.tar.gz` for the host, downloading `SHA256SUMS`, verifying the checksum, extracting, and moving the `chronicle` binary onto `PATH`. That is a multi-step, error-prone chore for what should be a one-line first-run experience, and it is the primary path documented in the README. A small POSIX `install.sh` served over HTTPS turns this into `curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/install.sh | sh`, the same shape users already trust from rustup, deno, and countless other CLI tools. The prebuilt GitHub Release artifacts remain the underlying distribution mechanism — no `cargo install`, no package-manager wiring, no new build pipeline — so the change stays aligned with Chronicle's minimal-production-effort philosophy: one small consumer script over the immutable artifacts the release workflow already produces.

## What Changes

- Add a repository-level `install.sh` (POSIX `sh`, run through `sh`, no Rust/Cargo dependency) that selects, downloads, verifies, extracts, and installs the correct release binary for the host.
- Detect supported platform/architecture (Linux on `x86_64` or `aarch64`/`arm64`) and normalize to the release workflow's target names; fail with a clear explanation for anything else instead of guessing.
- Resolve the version: latest stable GitHub Release by default (`releases/latest`), explicit pin via `CHRONICLE_VERSION` (leading `v` tolerated), asset/version conventions taken from the existing release workflow.
- Verify integrity with the release pipeline's `SHA256SUMS` before extracting or installing; checksum failure or missing checksum aborts installation.
- Install to `$HOME/.local/bin` by default (`CHRONICLE_INSTALL_DIR` override), creating the directory when needed, atomically replacing an existing binary only after full verification, and printing a concise `PATH` instruction when the destination is not on `PATH` (no shell-config editing).
- Use a temporary directory with cleanup on success and failure via `trap`; never extract untrusted archives into the installation directory; download and verify first, install last.
- Keep dependencies to commonly available tools: `sh`, `curl`, `tar`, and a SHA-256 utility (`sha256sum` or `shasum -a 256`).
- Minimal release-workflow adjustment: compose one deterministic `SHA256SUMS` covering all target archives of a release so the installer's per-asset lookup is reliable (the current per-job file may contain only one target's line).
- Update the README so the curl-pipe installer is the primary installation method, manual tarball installation becomes an advanced/fallback option, and `CHRONICLE_VERSION` / install-directory overrides are documented without hardcoding a release version.
- Add installer tests (fixture/mocked HTTP, no real GitHub dependency) and wire them into the existing validation taxonomy and CI.

## Capabilities

### New Capabilities

- `shell-installer`: the `install.sh` consumer contract — platform detection and target normalization, version resolution (latest/pinned), asset resolution and download, checksum verification before install, installation destination and `PATH` handling, temporary-file safety and cleanup, dependency requirements, failure behavior, and controlled testability (configurable base URLs/repo).

### Modified Capabilities

- `release-packaging`: the checksum artifact contract gains a deterministic single `SHA256SUMS` over all target archives per release so a consumer can always find its own asset's line; asset naming and archive layout are unchanged.
- `developer-onboarding-documentation`: the installation contract changes primary installation to the curl-pipe installer, with manual tarball install as the advanced/fallback path and version-pin/install-dir override documentation.
- `layered-validation`: installer tests join the portable validation layer (`cli_docs` targeted group and CI checks job), and `install.sh` gains path ownership in `validation/groups.toml` so installer changes select the right targeted validation without privileged gates.

## Impact

Affected code: new `install.sh` at the repository root (the only new executable artifact).

Affected non-code: `.github/workflows/release.yml` (combined `SHA256SUMS` composition job), `README.md` (installation section), `validation/groups.toml` and `scripts/validate.sh` (installer test wiring), `tests/smoke/test_installer.py` (new portable test), `tests/fixtures/installer/` (controlled release fixtures).

No changes to WAL, ETL, canonical session, storage, replay, capture, eBPF, recorder, or CLI product behavior. No new Rust dependencies, no package-manager distribution (Homebrew/apt/rpm/deb/Nix), no Docker packaging, no automatic self-update behavior, no redesign of the release pipeline beyond the single checksum-manifest adjustment, and no release published by this change. `publish = false` stays.
