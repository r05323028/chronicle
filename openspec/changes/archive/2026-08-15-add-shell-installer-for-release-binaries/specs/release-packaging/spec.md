## ADDED Requirements

### Requirement: Single deterministic checksum manifest

Each release SHALL publish one `SHA256SUMS` file covering every target archive of that release, so any consumer can deterministically find the checksum line for a specific asset. The release workflow SHALL compose this combined manifest after all target archives are built and SHALL publish it with the release (replacing any per-job partial manifest). Asset naming, archive layout, and archive contents SHALL remain unchanged.

#### Scenario: Combined manifest over all targets

- **WHEN** a release is published
- **THEN** its `SHA256SUMS` contains a verified line for both `chronicle-<version>-x86_64-unknown-linux-gnu.tar.gz` and `chronicle-<version>-aarch64-unknown-linux-gnu.tar.gz`

#### Scenario: Installer consumes the manifest

- **WHEN** the shell installer looks up its asset in the release `SHA256SUMS`
- **THEN** the line for that asset is present and verifies, regardless of which target was built first
