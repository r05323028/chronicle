## ADDED Requirements

### Requirement: Installer validation wiring

The shell installer SHALL be covered by the portable validation layer, not privileged acceptance. `install.sh` SHALL own path selection in `validation/groups.toml` so installer changes select the installer tests in targeted mode without selecting privileged gates. The `cli_docs` targeted case and the CI checks job SHALL run the installer test suite, which SHALL exercise the installer against controlled local fixtures (no real GitHub API dependency) covering at minimum: Linux x86_64 asset selection, Linux aarch64 asset selection, unsupported OS, unsupported architecture, explicit version selection, latest release selection, failed download, checksum mismatch, missing checksum, custom install directory, install directory not on `PATH`, cleanup on failure, and replacement of an existing binary only after successful verification.

#### Scenario: Installer change selects installer tests

- **WHEN** `install.sh` (or installer fixtures/tests) changes
- **THEN** targeted validation runs the installer tests through the `cli_docs` group and selects no privileged gate

#### Scenario: Installer tests pass portable

- **WHEN** the installer test suite runs in CI or targeted mode
- **THEN** the full fixture-driven coverage above passes without network access to GitHub
