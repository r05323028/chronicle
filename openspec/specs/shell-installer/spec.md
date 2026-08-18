## Purpose

A repository-level POSIX `install.sh` that automates selecting, downloading, verifying, extracting, and installing the correct Chronicle release binary over the existing GitHub Release artifacts, giving users a reliable `curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/install.sh | sh` installation path without manual archive handling or a Rust toolchain.

## Requirements

### Requirement: Installer script contract

The repository SHALL provide a single `install.sh` at the repository root that runs under `sh` (POSIX-friendly), requires no Rust or Cargo, requires no manual release-archive download, and fails clearly and safely on errors: every failure SHALL print a diagnostic to stderr and exit nonzero, and SHALL NOT leave a partially installed or partially replaced binary behind.

#### Scenario: Pipe-to-sh install

- **WHEN** a user runs `curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/install.sh | sh` on a supported Linux host
- **THEN** the correct release binary is downloaded, verified, extracted, and installed without further user action and without a Rust toolchain

#### Scenario: Clear failure

- **WHEN** any step fails (download, checksum, extraction, install)
- **THEN** the installer prints a concise diagnostic to stderr, exits nonzero, and leaves no partial installation

### Requirement: Platform detection

The installer SHALL detect the operating system via `uname` and support only Linux for the initial scope. SHALL support the `x86_64` and `aarch64`/`arm64` architectures, normalizing them to the release workflow's target names `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`. Any other OS or architecture SHALL fail with a clear explanation naming the supported set rather than guessing.

#### Scenario: Linux x86_64 detection

- **WHEN** the host reports Linux on `x86_64`
- **THEN** the installer selects target `x86_64-unknown-linux-gnu`

#### Scenario: Linux aarch64 detection

- **WHEN** the host reports Linux on `aarch64` (or `arm64`)
- **THEN** the installer selects target `aarch64-unknown-linux-gnu`

#### Scenario: Unsupported OS

- **WHEN** the host OS is not Linux
- **THEN** installation aborts with an explicit unsupported-OS message

#### Scenario: Unsupported architecture

- **WHEN** the host architecture is not x86_64/aarch64/arm64
- **THEN** installation aborts with an explicit unsupported-architecture message

### Requirement: Release selection

By default the installer SHALL resolve the latest stable GitHub Release (via the GitHub `releases/latest` API). `CHRONICLE_VERSION` SHALL pin an explicit release version, accepting a value with or without a leading `v`, and SHALL be normalized to the release workflow's version convention (version without leading `v` in the asset name, tag with leading `v`). A missing release or missing asset SHALL fail with a clear message. Prereleases SHALL NOT be selected by `latest`; pinning a prerelease requires an explicit `CHRONICLE_VERSION`.

#### Scenario: Latest release selected

- **WHEN** `CHRONICLE_VERSION` is unset
- **THEN** the installer resolves the latest stable release tag and installs that version

#### Scenario: Explicit version pin

- **WHEN** `CHRONICLE_VERSION=v0.1.0` (or `0.1.0`) is set
- **THEN** the installer resolves the matching release and its assets

#### Scenario: Missing release or asset

- **WHEN** the pinned version or its asset does not exist on the release
- **THEN** installation aborts with a clear message naming the version and asset

### Requirement: Release asset download

The installer SHALL determine the expected asset `chronicle-<version>-<target>.tar.gz` for the detected target using the release workflow's existing naming convention and packaging format, and download it from the release's download URL. No redesign of the archive format is permitted unless the current format makes reliable installation impossible (it does not: the archive contains the `chronicle` binary at its top level).

#### Scenario: Correct asset requested

- **WHEN** the installer runs on Linux x86_64 for version `v0.1.0`
- **THEN** it downloads `chronicle-0.1.0-x86_64-unknown-linux-gnu.tar.gz` from the release

#### Scenario: Failed download

- **WHEN** the archive download fails or returns an error status
- **THEN** installation aborts with a clear message and no partial install

### Requirement: Integrity verification

Downloaded archives SHALL be verified against the release's `SHA256SUMS` before extraction or installation, using `sha256sum` when available and `shasum -a 256` otherwise. The installer SHALL locate the checksum line for its exact asset filename; a mismatch, a missing checksum line, a missing checksums file, or an unavailable SHA-256 utility SHALL abort installation. Verification SHALL NOT be skippable or silently degradable.

#### Scenario: Checksum mismatch

- **WHEN** the downloaded archive's hash differs from the release's `SHA256SUMS` line
- **THEN** installation aborts before any extraction or install

#### Scenario: Missing checksum

- **WHEN** the release has no `SHA256SUMS` line for the requested asset (or no `SHA256SUMS` at all)
- **THEN** installation aborts rather than continuing unverified

### Requirement: Installation destination

The default installation directory SHALL be `$HOME/.local/bin`. `CHRONICLE_INSTALL_DIR` SHALL override the destination. The installer SHALL create the directory when missing, fail clearly when it cannot (for example when not writable), install the binary with an atomic replace so a pre-existing working binary is only replaced after a fully verified install, and SHALL NOT edit shell configuration files. When the installation directory is not on `PATH`, the installer SHALL print a concise instruction (for example `export PATH="$HOME/.local/bin:$PATH"`) and continue.

#### Scenario: Custom install directory

- **WHEN** `CHRONICLE_INSTALL_DIR=/some/path` is set
- **THEN** the binary installs there and the directory is created if missing

#### Scenario: Directory not writable

- **WHEN** the destination directory cannot be created or written
- **THEN** installation aborts with a clear permission/error message

#### Scenario: Install directory not on PATH

- **WHEN** the resulting binary location is not on `PATH`
- **THEN** a concise `PATH` export instruction is printed and no shell config is modified

#### Scenario: Existing binary replaced only after verification

- **WHEN** a previous `chronicle` binary exists and a new verified install completes
- **THEN** the new binary atomically replaces the old one; a failed install leaves the old binary intact

### Requirement: Safe temporary files and cleanup

The installer SHALL download and extract inside a temporary directory (for example `mktemp -d`), clean up the temporary directory on both success and failure via a trap, never extract untrusted archive paths directly into the installation directory, and only perform the final installation step after download, verification, and extraction all succeed.

#### Scenario: Cleanup on failure

- **WHEN** any step fails after the temporary directory is created
- **THEN** the temporary directory and its contents are removed

#### Scenario: No direct extraction into install dir

- **WHEN** the installer extracts the archive
- **THEN** extraction happens in the temporary directory, never into the installation directory

### Requirement: Installer dependencies

The installer SHALL require only commonly available tools used by its implementation: POSIX `sh`, `curl`, `tar`, `uname`, `grep`, `head`, `cut`, `awk`, `mktemp`, and one SHA-256 utility (`sha256sum` or `shasum -a 256`), plus standard POSIX file utilities. Missing commands that are explicitly required by preflight SHALL fail with a clear diagnostic. The installer SHALL NOT require package managers, Rust/Cargo, or any heavyweight runtime.

#### Scenario: Missing dependency

- **WHEN** a required command such as `curl`, `awk`, `grep`, `head`, `cut`, or the SHA-256 utility is absent
- **THEN** installation aborts before download with a message naming the missing command

### Requirement: Installer testability

Release resolution and download SHALL be testable against controlled fixtures without hitting the real GitHub API: the installer SHALL support configuration hooks (for example a base download URL and a releases API URL, and/or an explicit repository override) so tests can serve fixture releases locally. Behavior SHALL remain identical when the hooks are unset (real GitHub URLs used).

#### Scenario: Fixture-driven tests

- **WHEN** installer tests run against a local fixture server
- **THEN** asset selection, checksum verification, and installation are exercised without network access to GitHub

#### Scenario: Production defaults

- **WHEN** no configuration hooks are set
- **THEN** the installer uses the real GitHub repository and release URLs
