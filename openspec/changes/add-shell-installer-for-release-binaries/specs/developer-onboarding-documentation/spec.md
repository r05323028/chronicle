## ADDED Requirements

### Requirement: Installer-primary installation contract

The README Installation section SHALL present the curl-pipe installer as the primary installation method:

```bash
curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/install.sh | sh
```

Manual tarball installation from GitHub Release artifacts (download, verify `SHA256SUMS`, extract, place on `PATH`) SHALL remain documented as the advanced/fallback option but SHALL NOT be the primary path. The README SHALL document the `CHRONICLE_VERSION` pin and the `CHRONICLE_INSTALL_DIR` override with examples, and SHALL NOT hardcode a specific Chronicle release version into general installation wording. Documented installer commands SHALL remain consistent with the release artifacts and checksums actually produced by the release workflow.

#### Scenario: Primary install command

- **WHEN** a developer opens the Installation section
- **THEN** the first installation command shown is the curl-pipe `install.sh` invocation, and manual tarball steps appear only as the advanced/fallback path

#### Scenario: Version pin and directory override documented

- **WHEN** a developer needs a pinned version or a custom destination
- **THEN** the README shows `CHRONICLE_VERSION=v0.1.0` and `CHRONICLE_INSTALL_DIR` examples without embedding a release version in the general wording

#### Scenario: No invented artifacts

- **WHEN** the documented installer commands are compared to the release workflow
- **THEN** every referenced URL, asset, and checksum contract exists and matches the workflow's naming convention
