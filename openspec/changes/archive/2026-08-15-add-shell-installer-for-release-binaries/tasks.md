## 1. Installer script

- [x] 1.1 Add repository-level `install.sh` (POSIX `sh`, run through `sh`, no Rust/Cargo) implementing the full pipeline: preflight required tools (`curl`, `tar`, `sha256sum` or `shasum -a 256`); detect OS/arch via `uname`; normalize to `x86_64-unknown-linux-gnu` / `aarch64-unknown-linux-gnu`; fail clearly on non-Linux or unsupported architecture; resolve version (`CHRONICLE_VERSION` pin, leading `v` tolerated, else GitHub `releases/latest`); build asset name `chronicle-<version>-<target>.tar.gz`; download archive and `SHA256SUMS` from the release; verify SHA-256 (missing checksum line, mismatch, or missing utility aborts); extract into `mktemp -d`; install atomically to `CHRONICLE_INSTALL_DIR` or `$HOME/.local/bin` (create if missing, clear error when not writable, temp-file + `mv` replace so an existing binary is replaced only after a fully verified install); `chmod +x`; print concise `export PATH="<dir>:$PATH"` instruction when the destination is not on `PATH` (no shell-config edits). Support test seams: `CHRONICLE_REPO` (owner/repo), `CHRONICLE_API_URL`, `CHRONICLE_BASE_URL` (downloads), and a uname seam for detection tests; unset = real GitHub URLs. Trap cleanup of the temp dir on success and failure. Acceptance: shellcheck-clean POSIX; manual fixture run installs a fake `chronicle` binary; every failure path exits nonzero with a stderr diagnostic and no partial install.

## 2. Release workflow: deterministic checksum manifest

- [x] 2.1 Adjust `.github/workflows/release.yml` minimally: remove the per-job `SHA256SUMS` write/attach from the matrix jobs; add a checksum job that runs after the matrix, downloads both target archives from the release, composes one `SHA256SUMS` over both, verifies it, and uploads it to the release (`gh release upload --clobber`). Asset names, archive layout, and the `sha256sum` file format SHALL remain unchanged. Acceptance: a dry-run produces a single `SHA256SUMS` containing verified lines for both targets; no other workflow behavior changes.

## 3. Documentation

- [x] 3.1 Update the README Installation section: primary method becomes `curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/install.sh | sh`; document `CHRONICLE_VERSION=v0.1.0` pin and `CHRONICLE_INSTALL_DIR=/some/path` override with examples; state Linux x86_64/aarch64 scope; manual tarball installation becomes the advanced/fallback option; keep `chronicle doctor` as the readiness check; do not hardcode a release version into general wording. Acceptance: installation section reads installer-first; no invented URLs/artifacts; the documentation-command contract still passes.

## 4. Installer tests

- [x] 4.1 Add `tests/smoke/test_installer.py` (portable, Python stdlib only) that serves fixture release trees (fixtures under `tests/fixtures/installer/`) over a local HTTP server and drives `install.sh` with `CHRONICLE_API_URL`/`CHRONICLE_BASE_URL`/uname seams. Cover at minimum: Linux x86_64 asset selection; Linux aarch64 asset selection; unsupported OS; unsupported architecture; explicit version selection (with and without leading `v`); latest release selection; failed download; checksum mismatch; missing checksum line; custom install directory; install directory not on `PATH` (instruction printed); cleanup on failure (no temp residue); replacement of an existing binary only after successful verification (old binary intact on failed verification, replaced on success). Acceptance: suite passes rootless on macOS/Linux without network access to GitHub.

## 5. Validation wiring

- [x] 5.1 Add `install.sh` and installer test/fixture paths to the `cli_docs` group `paths` in `validation/groups.toml`; extend the `cli_docs` case in `scripts/validate.sh` to run `python3 tests/smoke/test_installer.py`; add the same invocation to the CI checks job. Acceptance: `./scripts/validate.sh targeted --changed-since origin/main` with an `install.sh` change runs the installer tests through `cli_docs` and selects no privileged gate; CI runs the suite on every push/PR; `python3 scripts/validation.py catalog --root .` and the layered-validation tooling tests still pass.

## 6. Final validation

- [x] 6.1 Run strict OpenSpec validation and bounded fast/targeted validation. Acceptance: `openspec validate --all --strict --no-interactive` passes; `./scripts/validate.sh fast` and targeted pass with the new installer test.
- [x] 6.2 Confirm no product behavior changed: WAL/ETL/canonical/storage/replay/capture/recorder/CLI suites pass unchanged; diff review shows only the installer, checksum-manifest job, README installation section, and validation wiring. Acceptance: workspace tests green; `git diff` scope review passes.
