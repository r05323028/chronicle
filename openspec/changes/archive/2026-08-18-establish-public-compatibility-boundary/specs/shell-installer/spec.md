## MODIFIED Requirements

### Requirement: Installer dependencies

The installer SHALL require only commonly available tools used by its implementation: POSIX `sh`, `curl`, `tar`, `uname`, `grep`, `head`, `cut`, `awk`, `mktemp`, and one SHA-256 utility (`sha256sum` or `shasum -a 256`), plus standard POSIX file utilities. Missing commands that are explicitly required by preflight SHALL fail with a clear diagnostic. The installer SHALL NOT require package managers, Rust/Cargo, or any heavyweight runtime.

#### Scenario: Missing dependency

- **WHEN** a required command such as `curl`, `awk`, `grep`, `head`, `cut`, or the SHA-256 utility is absent
- **THEN** installation aborts before download with a message naming the missing command
