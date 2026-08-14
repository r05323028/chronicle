## Purpose

Minimal release packaging so a public developer-facing installation path is real: a GitHub Actions release workflow that produces supported Linux binaries with the eBPF capture object embedded, versioned archives, SHA-256 checksums, GitHub Release artifacts, and `chronicle --version` reporting. Distribution channels beyond raw release artifacts (package managers, Docker, Homebrew, crates.io) are explicit non-goals.

## ADDED Requirements

### Requirement: Release workflow and targets

The repository SHALL provide a release workflow triggered on version tags that builds release binaries for supported targets: x86_64 Linux and aarch64 Linux on native runners. Each release binary SHALL be built with `--features linux-ebpf` and `--locked` so live eBPF capture is available in the shipped artifact, with the eBPF object built from `ebpf/` source (nightly + rust-src + bpf-linker, `-Z build-std=core`, `--target bpfel-unknown-none --release`) and embedded through the existing `chronicle-capture-ebpf` build path.

#### Scenario: Tagged release builds

- **WHEN** a `v*` tag is pushed
- **THEN** the workflow builds x86_64 and aarch64 Linux binaries with `linux-ebpf`, embeds the eBPF object, and publishes release artifacts

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
