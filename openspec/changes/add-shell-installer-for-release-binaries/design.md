## Context

Chronicle main (commit 61e1ee2, 2026-08-15) ships release binaries via `.github/workflows/release.yml`, triggered on `v*` tags. The workflow builds two targets on native runners — `x86_64-unknown-linux-gnu` (ubuntu-latest) and `aarch64-unknown-linux-gnu` (ubuntu-24.04-arm) — each with `--features linux-ebpf --locked`, and packages `chronicle-${VERSION}-${TARGET}.tar.gz` containing the `chronicle` binary, `LICENSE`, and `README.md` at the archive top level (no directory prefix). Version comes from `Cargo.toml` (currently 0.1.0, no leading `v`); release tags carry a leading `v`. Each matrix job writes a per-job `SHA256SUMS` (`sha256sum` format) and both jobs attach archives and checksums to the same release with `gh release create ... --generate-notes`. The README currently documents a manual four-step tarball install as the primary path; `docs/operations.md`, `docs/architecture.md`, `docs/replay-safety.md`, and `docs/release-notes.md` hold the detailed documentation.

Validation is layered (`scripts/validate.sh`): `fast`, `targeted`, `gate p1|p2`, `release`. Group ownership lives in `validation/groups.toml`; root `tests/**` is portable, `scripts/**` + `validation/**` + `.github/**` is `build_tooling` (selects privileged gates), and the `cli_docs` group (paths: `crates/chronicle-cli/**`, `docs/**`, `README.md`, `CONTRIBUTING.md`, `AGENTS.md`, `openspec/**`) runs strict OpenSpec validation plus the documentation-command contract (`tests/smoke/test_documented_commands.py`) and selects no privileged gate. CI runs the rootless Python smoke/acceptance/e2e tests explicitly in the checks job.

## Goals / Non-Goals

**Goals:**

- One reliable `curl -fsSL .../install.sh | sh` installation path over the existing release binaries, with version pinning and install-directory override.
- Integrity-gated installation: verify against the release `SHA256SUMS` before any extraction or install, abort on mismatch or missing checksum.
- Safe failure behavior: temp-dir cleanup, atomic replacement of an existing binary only after a fully verified install, no shell-config edits.
- Minimal release-workflow adjustment only where the current artifact contract makes reliable installation impossible (deterministic combined `SHA256SUMS`).
- Installer covered by the existing portable validation taxonomy with fixture-driven tests that never hit the real GitHub API.

**Non-Goals:**

- No Homebrew, apt, rpm, deb, Cargo, Nix, Docker, or other package-manager distribution.
- No redesign of the release pipeline or archive format; no new checksum algorithm; no automatic self-update behavior.
- No changes to WAL, ETL, canonical session, storage, replay, capture, eBPF, recorder, or CLI product behavior; `publish = false` stays.
- No release published by this change; the installer consumes artifacts the workflow already produces.

## Decisions

### D1. One root-level `install.sh`, POSIX sh

A single `install.sh` at the repository root, executed as `sh install.sh` via the curl pipe. POSIX subset (`set -eu`, `trap`, no bashisms such as `pipefail` or arrays) so it runs on any `/bin/sh`. Every failure prints a diagnostic to stderr and exits nonzero. The script is the only new executable artifact.

### D2. Platform detection and target normalization

Use `uname -s` and `uname -m`. Map to the release workflow's exact target names:

- Linux + `x86_64` (or `amd64`) → `x86_64-unknown-linux-gnu`
- Linux + `aarch64` (or `arm64`) → `aarch64-unknown-linux-gnu`
- non-Linux → error naming the supported set (Linux only)
- any other architecture → error naming supported architectures

No invented targets: the mapping is exactly the two targets the release matrix produces.

### D3. Version resolution

- `CHRONICLE_VERSION` unset → query the GitHub `releases/latest` API and read the release tag. `latest` excludes prereleases by GitHub's own semantics; the current release workflow publishes only stable `v*` tags with `--generate-notes` (no prerelease flag), so this matches the release process. Pinning a prerelease requires an explicit `CHRONICLE_VERSION`.
- `CHRONICLE_VERSION` set → normalize: strip one leading `v` for the asset name (asset uses `chronicle-<version-without-v>-<target>.tar.gz`), keep the `v` for the tag (tags are `v*`). Both `v0.1.0` and `0.1.0` are accepted.
- Missing release (404) or missing asset → clear error naming version/asset.

### D4. Asset and URL contract

Asset name: `chronicle-<version>-<target>.tar.gz` — exactly the release workflow's convention (verified against `release.yml`: `dist/chronicle-${VERSION}-${TARGET}.tar.gz`). Archive layout: `chronicle` binary at top level (with `LICENSE` and `README.md`) — no strip-prefix logic needed beyond `tar -xzf` in a temp dir. Download URLs:

- archive: `https://github.com/<owner>/<repo>/releases/download/<tag>/<asset>`
- checksums: same download root, `SHA256SUMS`
- latest API: `https://api.github.com/repos/<owner>/<repo>/releases/latest`

Repository defaults to the README's `r05323028/chronicle`; `CHRONICLE_REPO=owner/repo` overrides for forks. For fixture-driven tests, `CHRONICLE_BASE_URL` (downloads) and `CHRONICLE_API_URL` (latest resolution) override the GitHub roots; unset = real GitHub. These hooks are the only test seams.

### D5. Integrity verification

Download `SHA256SUMS` from the same release, extract the line for the exact asset filename, compute the archive's SHA-256, and compare. Use `sha256sum` when present, else `shasum -a 256`. Mismatch, missing line, missing file, or no SHA-256 utility → abort before extraction. Verification is not skippable.

### D6. Install destination, atomic replace, PATH guidance

Default `$HOME/.local/bin`; `CHRONICLE_INSTALL_DIR` overrides. Missing directory → `mkdir -p`; failure (e.g. not writable) → clear error. Install sequence: download + verify + extract into the temp dir, then copy the binary into the destination as `chronicle.tmp.<pid>` and `mv` over the target (same filesystem → atomic), then `chmod +x`. A pre-existing working binary is therefore never partially replaced. `PATH` check after install: if the destination is not on `PATH`, print `export PATH="<dir>:$PATH"` (a concise instruction) and exit success — no shell config editing.

### D7. Temp files and cleanup

`mktemp -d` for downloads/extraction; `trap 'rm -rf "$TMPDIR"' EXIT HUP INT TERM` guarantees cleanup on success and failure. The install directory is touched only by the final atomic move.

### D8. Dependencies

Required: `sh`, `curl`, `tar`, and a SHA-256 utility. Each is preflighted with a clear missing-tool error. No package managers, no Rust/Cargo, no heavyweight runtime.

### D9. Release workflow adjustment: combined SHA256SUMS

Today each matrix job writes its own `SHA256SUMS` containing only that job's archive line, and both jobs upload a file with the same name to the same release — so the published manifest may contain either target's line depending on upload order. The installer greps its own asset's line; if that line lost the race, installation fails on a healthy release. Minimal, deterministic fix: a small checksum job (runs after the matrix) downloads both archives, composes one `SHA256SUMS` over both, and uploads it to the release (`gh release upload --clobber`). Asset names, archive layout, and the `sha256sum` file format are unchanged — only completeness becomes deterministic. The per-job `SHA256SUMS` write/attach is removed from the matrix jobs to avoid the name collision.

### D10. Documentation

README Installation section: primary = curl-pipe installer; version pin and install-dir override examples (`CHRONICLE_VERSION=v0.1.0`, `CHRONICLE_INSTALL_DIR=/some/path`); no release version hardcoded into general wording; manual tarball install stays as the advanced/fallback subsection; `chronicle doctor` remains the readiness check. The installer is documented as Linux/x86_64/aarch64 only, mirroring the release matrix.

### D11. Testing strategy and validation wiring

New `tests/smoke/test_installer.py` (portable, Python stdlib): spins a local HTTP server over fixture release trees (fixtures under `tests/fixtures/installer/`), runs `install.sh` with `CHRONICLE_API_URL`/`CHRONICLE_BASE_URL` pointing at it, and asserts the full matrix of behaviors: x86_64/aarch64 asset selection, unsupported OS/arch, explicit version pin, latest resolution, failed download, checksum mismatch, missing checksum line, custom install dir, install dir not on PATH (instruction printed), cleanup on failure (no temp residue), and atomic replacement (existing binary intact when verification fails, replaced when it succeeds). An OS is simulated by passing an overridable uname seam (env hook, e.g. `CHRONICLE_UNAME`) or by running the detection function in isolation — the design keeps detection pure so tests can exercise unsupported-OS/arch branches without a fake OS.

Wiring: add `install.sh` (plus installer test/fixture paths) to the `cli_docs` group's `paths` in `validation/groups.toml`; extend the `cli_docs` case in `scripts/validate.sh` to run `python3 tests/smoke/test_installer.py`; add the same invocation to the CI checks job. This keeps installer coverage in the portable layer (no privileged gates), matching the existing `cli_docs` precedent. No new validation system. No real release-level smoke is required for the installer itself; the release workflow's existing artifact validation already proves the archive/checksum contract, and a manual/optional smoke of the curl-pipe flow against a real release remains available as a release-mode sanity check only if maintainers choose to run it (not a gate obligation — the fixture suite is the deterministic proof).
