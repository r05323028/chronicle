## Purpose

The README is the primary developer onboarding entry point for Chronicle: product positioning, truthful current capabilities, installation, platform requirements, a minimal record -> inspect -> replay quick start, a high-level architecture overview that preserves the replay boundary, honest protocol support status, concise safety and limitations, a CLI overview, contributor development guidance, and pointers to detailed documentation. Documented installation and Quick Start commands are validated rather than untested.

## Requirements

### Requirement: README role and information hierarchy

`README.md` SHALL be organized as a product/developer onboarding document with the ordering What -> Install -> Quick Start -> Architecture -> Details: banner, positioning statement, What is Chronicle?, Current capabilities / status, Installation, Requirements, Quick Start, How It Works, Architecture, Supported Protocols, Safety, Current Limitations, CLI, Development, Documentation, License (exact headings may be refined but the ordering principle and the "no internal implementation detail before the first working example" rule SHALL hold). The README SHALL NOT read like a P1 implementation report: hidden compatibility entrypoints, deprecated syntax as a primary interface, WAL-format and schema-version policy, sidecar/catalog mechanics, recording bounds, and P1-specific machinery SHALL live in the appropriate detailed documents and SHALL NOT appear in the primary onboarding path unless necessary for installation, architecture understanding, or safety.

#### Scenario: Ordering for a new developer

- **WHEN** a developer opens the README for the first time
- **THEN** positioning, capabilities, installation, and quick start appear before architecture and detail, and no hidden compatibility or WAL-format content appears in the primary path

#### Scenario: Reclassified implementation detail

- **WHEN** the README is reviewed against docs/operations.md, docs/architecture.md, and docs/replay-safety.md
- **THEN** operations, architecture, and safety semantics are referenced, not duplicated, and hidden compatibility commands are not presented as a product interface

### Requirement: Product positioning

The README SHALL position Chronicle in developer-oriented language, refined to the implementation: Chronicle records real application traffic with eBPF and turns it into deterministic, replayable regression-test evidence. Supporting principles SHALL be stated only as implemented: minimal application-side integration for capture; durable local persistence through a segmented WAL before interpretation; ETL into a protocol- and storage-independent canonical representation; replay decoupled from the production capture pipeline; and safe replay that never automatically targets the recorded production destination. The README SHALL NOT claim zero overhead, zero code changes, arbitrary production safety, or any guarantee the implementation does not prove.

#### Scenario: Claim traceability

- **WHEN** any capability or safety claim in the README is checked against the implementation
- **THEN** the claim is traceable to the CLI contract, doctor probes, replay-safety documentation, or operations documentation

### Requirement: Current capabilities and status

The README SHALL state current supported behavior truthfully: five public commands (`record`, `replay`, `list`, `inspect`, `doctor`), plaintext HTTP/1.1 live capture via eBPF on Linux, bounded WAL recording, canonical session publication to local storage, and safe loopback replay. Planned or scaffolded behavior (PostgreSQL/S3 persistence, TLS, protocols other than HTTP/1.1) SHALL be labeled as not implemented and SHALL NOT be advertised as currently supported.

#### Scenario: No planned feature as supported

- **WHEN** a reader inspects the capabilities section
- **THEN** only behavior implemented end to end is presented as supported, and planned items are explicitly marked not implemented

### Requirement: Installation contract

The README SHALL present installation through an installed `chronicle` binary as the primary user experience, referencing actual release artifacts and checksums produced by the repository's release workflow; the README SHALL NOT invent download URLs or artifacts that do not exist. A build-from-source path SHALL be documented for developers with every live-recording command enabling the required feature path explicitly: on Linux, `cargo build --release --features linux-ebpf` (or an equivalent contributor form) SHALL be the documented build, and the prerequisites SHALL be stated. The bare `cargo run -p chronicle-cli -- record ...` contributor examples without the feature SHALL NOT appear as the primary installation or live-recording experience.

#### Scenario: Binary installation path

- **WHEN** a developer follows the Installation section on a supported Linux host
- **THEN** they install the released `chronicle` binary (with the eBPF object embedded), verify checksums, and run `chronicle doctor` for readiness

#### Scenario: Build from source with capture

- **WHEN** a developer builds from source for live capture
- **THEN** the documented command enables `linux-ebpf` and states the Linux/kernel/cgroup/BTF/architecture/privilege prerequisites and the eBPF object embedding mechanism

### Requirement: Quick Start contract

The public Quick Start SHALL operate on an installed binary, remain short, and cover the common command-mode workflow: `chronicle doctor`; `chronicle record --name checkout -- ./my-app`; representative traffic to the application; `chronicle list`; `chronicle inspect checkout`; `chronicle replay checkout -- ./my-app`. Each command SHALL match the validated CLI contract. Advanced forms (PID recording, explicit cgroup recording, explicit replay target, `--allow-host`, explicit effect gates, `--retry`, recovery behavior, WAL sizing, continuous-recorder internals, hidden compatibility commands, deprecated syntax) SHALL NOT appear in the initial Quick Start and SHALL link to `docs/operations.md`.

#### Scenario: Quick Start executes

- **WHEN** a Linux user with a release-built binary follows the Quick Start
- **THEN** doctor reports capture readiness, record publishes a named recording, list shows it, inspect summarizes it, and replay verifies against the supervised target

#### Scenario: Advanced forms deferred

- **WHEN** the Quick Start is reviewed
- **THEN** no advanced selector, explicit-target, effect-gate, retry, recorder, or compatibility form appears in the primary Quick Start block

### Requirement: Platform requirements presentation

The README SHALL expose only the prerequisites a user needs before live capture: supported OS (Linux for live capture), minimum kernel, cgroup v2, BTF, supported architectures (x86_64/aarch64), and required eBPF capabilities or privileges, with `chronicle doctor` as the preferred readiness check. Low-level preflight implementation detail SHALL NOT be duplicated in the README. Non-Linux builds SHALL be described as supporting the portable surface without live capture.

#### Scenario: Requirements match doctor

- **WHEN** the Requirements section is compared to doctor probe codes and docs/operations.md
- **THEN** the stated prerequisites match the implemented probe set without duplicating its internals

### Requirement: Architecture overview preserves the replay boundary

The README SHALL present a developer-friendly flow and architecture responsibilities: eBPF capture, WAL, ETL/session reconstruction, the canonical session model, storage, protocol adapters, and replay. The presentation SHALL preserve the boundary that replay consumes canonical sessions and protocol interfaces and does not depend on capture, eBPF, WAL, or ETL; this boundary SHALL NOT be oversimplified away. The README SHALL link `docs/architecture.md` and `docs/architecture/crate-boundaries.md` and SHALL NOT duplicate the full compile-time crate dependency graph.

#### Scenario: Replay boundary explicit

- **WHEN** a developer reads the Architecture section
- **THEN** the statement that replay depends only on canonical sessions and protocol interfaces is present and unambiguous

### Requirement: Protocol support presentation

The README SHALL present a compact capability matrix with honest statuses: supported (functional end to end), planned (registered or scaffolded, not support), and research/unsupported. Only bounded plaintext HTTP/1.1 SHALL be listed as supported end to end today. Internal test scaffolds such as Fake SHALL NOT appear in the public-facing support table. The README SHALL state once that a registered protocol is scaffolding, not support.

#### Scenario: Honest protocol matrix

- **WHEN** the protocol matrix is compared to the implementation
- **THEN** only HTTP/1.1 (and no internal test scaffold) is listed as supported, planned protocols are marked planned, and research protocols are marked research

### Requirement: Safety and limitations presentation

The README SHALL keep prominent, concise safety guidance: captured data may contain sensitive production information; replay may have side effects and defaults to dry-run with effects denied; replay SHALL NOT automatically target the recorded production destination; explicit authorization and policy gates remain visible to users; and current security limitations (no encryption at rest, no comprehensive redaction, no tenant isolation) SHALL NOT be hidden. The README SHALL keep a compact public limitations list (Linux-only live capture; plaintext HTTP/1.1 only with no TLS decryption or HTTP/2+; bounded recording; sensitive local artifacts; local-filesystem storage; unsupported stateful/multiplexed protocols; preserve-timing modeled not executed) and SHALL link detailed semantics to `docs/replay-safety.md` and `docs/operations.md`.

#### Scenario: Safety summary complete

- **WHEN** the Safety section is reviewed
- **THEN** sensitive-data warning, replay side-effect warning, no-automatic-production-target guarantee, visible gates, and current security limitations are all present and link to detail

### Requirement: CLI overview

The README SHALL include a concise public command overview centered on `record`, `list`, `inspect`, `replay`, and `doctor`, explaining intent without replicating `--help` output, plus global options. Advanced operational forms SHALL link to `docs/operations.md`.

#### Scenario: CLI overview intent-only

- **WHEN** the CLI section is reviewed
- **THEN** it explains what each command does and where to find detail, without dumping full option lists

### Requirement: Development section and documentation links

The README SHALL keep contributor instructions separate from installation: pinned toolchain (`rust-toolchain.toml`), workspace build/test/gate commands, and the canonical layered validation entry point (`./scripts/validate.sh fast`, with targeted/gate/release when required); privileged acceptance SHALL be mentioned as opt-in Linux evidence, not part of normal onboarding. The README SHALL link the documentation that exists and remains accurate: `docs/architecture.md`, `docs/operations.md`, `docs/replay-safety.md`, `docs/release-notes.md`, and `CONTRIBUTING.md`; every README relative link SHALL resolve.

#### Scenario: Links resolve

- **WHEN** the documentation-command contract test runs
- **THEN** every relative link in the README resolves to an existing tracked file

### Requirement: Documented command validation

Documented installation and Quick Start commands SHALL be validated rather than untested. A rootless documentation-command contract test SHALL exercise the built binary: `--version`, `--help` (five public commands, internal mechanics hidden), `doctor` (non-destructive, actionable, capture probes unsupported on a non-feature build), `list` empty contract, public record-syntax preflight failure with no mutation on a non-feature build, and fixture-recorded public inspect/replay forms where rootless-compatible. The test SHALL NOT match README prose. Privileged P1/P2 acceptance SHALL include a quick-start scenario executing the exact documented command-mode forms against a release-built `linux-ebpf` binary and asserting the published recording, catalog entry, inspectable session, replay verification, and no recorded-destination contact.

#### Scenario: Rootless contract passes

- **WHEN** the documentation-command contract test runs on a non-Linux or non-feature build
- **THEN** version/help/doctor/list contracts pass, record-syntax preflight fails safely with typed error and no mutation, and README links resolve

#### Scenario: Privileged quick start passes

- **WHEN** the privileged quick-start scenario runs on supported Linux with a release-built binary
- **THEN** the exact documented record -> list -> inspect -> replay forms succeed and produce retained machine-readable evidence

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
