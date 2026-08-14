## Context

Chronicle main (commit 7283c39, 2026-08-14) provides a validated five-command intent-oriented CLI (record, replay, list, inspect, doctor) over a bounded plaintext HTTP/1.1 pipeline: eBPF Capture Event v1 -> CRC32C segmented WAL v1 -> restartable ETL -> Canonical Session v1 -> atomic filesystem publication -> safe loopback replay. Replay depends only on canonical sessions and protocol interfaces; it never depends on capture, WAL, eBPF, or ETL. Live capture is Linux-only (cgroup v2, kernel 6.1+, BTF, x86_64/aarch64, CAP_BPF/CAP_NET_ADMIN) and is compiled behind the non-default linux-ebpf feature that threads chronicle-cli -> chronicle-application -> chronicle-capture-ebpf (aya 0.14). The eBPF object is embedded at build time: chronicle-capture-ebpf/build.rs prefers ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture (built with nightly + -Z build-std=core + bpf-linker) and otherwise falls back to the tracked object crates/chronicle-capture-ebpf/objects/chronicle-ebpf-capture-bpfel.o, which makes a plain --features linux-ebpf build work without a nightly toolchain.

The README does not reflect any of this in onboarding order: it is a P1 implementation report that demonstrates only cargo run -p chronicle-cli -- ..., omits linux-ebpf from every live-recording example (so those commands fail the capture preflight on a default Linux build), and front-loads hidden 0.1.x compatibility entrypoints, WAL-format/schema-version policy, sidecar/catalog mechanics, and a fixture demo that routes through deprecated syntax. The repository has no release packaging (only .github/workflows/ci.yml, workspace publish = false, no tags) and no chronicle --version. docs/operations.md already documents the current public command surface and bounds; docs/architecture.md and docs/architecture/crate-boundaries.md hold the detailed architecture; docs/replay-safety.md holds replay safety semantics; CONTRIBUTING.md holds contributor workflow.

## Goals / Non-Goals

**Goals:**

- Restructure README.md into the primary developer onboarding document: What -> Install -> Quick Start -> Architecture -> Details, with no internal implementation detail required before the first working example.
- Define a truthful installation contract: released binary primary, build-from-source secondary with --features linux-ebpf explicit and prerequisites stated.
- Add the minimal release packaging needed to make the binary installation path real: release workflow, artifacts, checksums, version reporting. Nothing more.
- Preserve the replay boundary and the deny-by-default safety posture in the README presentation.
- Validate documented installation and Quick Start commands (rootless portable contract + privileged quick-start scenario + release-artifact checks), avoiding prose-grep tests.

**Non-Goals:**

- No new protocols, TLS decryption, HTTP/2/3, PostgreSQL/S3 runtime implementation, Kubernetes/Docker packaging, package managers, Homebrew, Docker images, or crates.io publishing (publish = false stays).
- No CLI redesign, no changes to WAL/ETL/canonical/storage/replay/capture/recorder behavior, no crate-boundary changes, no new Rust dependencies.
- No rewrite of docs/operations.md, docs/architecture.md, or docs/replay-safety.md semantics; README content that belongs there moves there, without re-deriving the operational or architectural contracts.

## Decisions

### D1. README becomes a product onboarding document

Reorder README.md to the target hierarchy: banner, positioning statement, What is Chronicle?, Current capabilities / status, Installation, Requirements, Quick Start, How It Works, Architecture, Supported Protocols, Safety, Current Limitations, CLI, Development, Documentation, License. The ordering principle is What -> Install -> Quick Start -> Architecture -> Details.

Positioning: "Chronicle records real application traffic with eBPF and turns it into deterministic, replayable regression-test evidence." Supporting principles, stated only as implemented: minimal application-side integration for capture (attach to a supervised command, a PID, or a cgroup; no app instrumentation); durable local persistence through a segmented WAL before interpretation; ETL into a protocol- and storage-independent canonical representation; replay decoupled from the production capture pipeline (consumes canonical sessions + protocol interfaces only); safe replay that never automatically targets the recorded production destination (dry-run default, deny-by-default effects). No claims of zero overhead, zero code changes, or arbitrary production safety — the implementation does not prove them.

Reclassify current README content: hidden compatibility entrypoints, deprecated syntax, WAL-format and schema-version policy, sidecar/catalog mechanics, recording bounds, and P1-specific machinery are operations/architecture detail -> docs/operations.md / docs/architecture.md (mostly already present; only move what is not). Keep in the README only what a new developer needs to evaluate, install, run, and extend Chronicle.

### D2. Installation contract

Primary: an installed chronicle binary from a GitHub Release (this change adds the release workflow; the README documents the binary path only after the workflow exists and produces artifacts — no invented download URLs).

Build from source (developer path): cargo build --release --features linux-ebpf on Linux for live capture, with explicit prerequisites: Rust toolchain from rust-toolchain.toml, Linux 6.1+, cgroup v2, BTF (/sys/kernel/btf/vmlinux), supported x86_64/aarch64, and CAP_BPF/CAP_NET_ADMIN for the recording process. The feature build works without a nightly toolchain because build.rs falls back to the tracked eBPF object; rebuilding the eBPF object from ebpf/ source requires nightly + rust-src + bpf-linker and cargo +nightly build -Z build-std=core --manifest-path ebpf/Cargo.toml --target bpfel-unknown-none --release before the workspace build. Non-Linux builds (macOS, etc.) compile the portable surface (list/inspect/replay-planning, doctor, fixture paths) without linux-ebpf; live capture is unavailable and record -- COMMAND... fails preflight with actionable remediation (exit 4, per the existing contract).

Every README live-recording command must enable the feature path explicitly — either installed binary (feature built in) or cargo run --features linux-ebpf -p chronicle-cli -- ... for contributor demos. The current bare cargo run -p chronicle-cli -- record ... examples are removed from the primary path.

chronicle doctor is the readiness check for live capture; it is non-destructive and reports platform, architecture, cgroup v2, BTF, embedded object/programs, attach, CAP_BPF, and CAP_NET_ADMIN probes with remediation. The README points at chronicle doctor instead of duplicating preflight internals.

### D3. Minimal release packaging

New .github/workflows/release.yml, triggered on version tags (v*):

- Matrix on native Linux runners: ubuntu-latest (x86_64) and ubuntu-24.04-arm (aarch64).
- Build the eBPF object from ebpf/ source (nightly + rust-src + bpf-linker, -Z build-std=core, --target bpfel-unknown-none --release, mirroring the existing unprivileged CI job), then build chronicle with cargo build --release --features linux-ebpf --locked so the object at ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture is embedded via the existing build.rs path.
- Archive as chronicle-<version>-<target>.tar.gz containing the chronicle binary, LICENSE, and a pointer to the README; emit SHA256SUMS.
- Create a GitHub Release with the archives and checksums attached.
- Add clap version to chronicle-cli so chronicle --version reports the workspace version (0.1.x) without adding a public subcommand.

Extend the existing unprivileged eBPF-compile CI job (or the release workflow) to assert the tracked fallback object matches a fresh build, keeping the no-nightly contributor path honest. Non-goals stay out: no package managers, Docker, Homebrew, crates.io.

### D4. Quick Start contract

Public Quick Start operates on an installed binary and stays short, covering the common command-mode workflow only:

chronicle doctor
chronicle record --name checkout -- ./my-app

# send representative traffic to the application

chronicle list
chronicle inspect checkout
chronicle replay checkout -- ./my-app

Every command is verified against the current CLI: doctor (non-destructive probes), record --name NAME -- COMMAND... (supervised command mode, duration default 600s/max 3600s, auto finalize/ETL/publish), list (catalog table, newest first), inspect RECORDING (recording = latest | rec_<uuid> | bare UUID | exact name), replay RECORDING -- COMMAND... (plan first, supervised cgroup target, inferred loopback listener, writes still explicit). Advanced forms — PID recording, explicit cgroup recording with shared-scope acknowledgement, explicit --target replay, --allow-host, explicit effect gates, allow-write/read/execute semantics, --retry, recovery behavior, WAL sizing, recorder internals, hidden compatibility commands, deprecated syntax — stay out of the initial Quick Start and link to docs/operations.md (they are already documented there).

### D5. Architecture presentation

The README gets a developer-friendly "How It Works" flow and "Architecture" responsibilities section, not the compile-time crate graph:

Application -> eBPF Capture -> WAL -> ETL -> Canonical Session
                                         |             |
                                         +--> Inspect  v
                                                     Storage
                                                       |
                                                       v
                                                      Replay

Explain responsibilities: eBPF capture (socket lifecycle + byte evidence, Linux-only, plaintext TCP); WAL (bounded, durable, crash-recoverable local persistence before interpretation); ETL/session reconstruction (bounded HTTP/1.1 session reconstruction and loss accounting); canonical session model (protocol- and storage-independent, deterministic); storage (atomic local publication); protocol adapters (detector/decoder/canonicalizer/replay/verifier SPI with built-in implementations); replay (planner/executor/verifier, dry-run default, loopback-only). The replay boundary is explicit and not simplified away: replay consumes canonical sessions and protocol interfaces and does not depend on capture, eBPF, WAL, or ETL. Link docs/architecture.md and docs/architecture/crate-boundaries.md; do not duplicate the full crate dependency graph.

### D6. Protocol support presentation

Compact capability matrix with honest statuses: supported (functional end to end) vs planned (registered/scaffolding, not support) vs research/unsupported. Only bounded plaintext HTTP/1.1 is supported end to end today. Internal test scaffolds such as Fake are excluded from the public table (they exist for boundary tests, not user value). PostgreSQL/MySQL/MariaDB/MongoDB/Kafka/NATS remain planned; Oracle Net/TNS remains research. S3-compatible artifact storage stays out of the support table. A registered protocol is scaffolding, not support — the README states this once.

### D7. Safety and limitations presentation

Keep prominent, concise safety guidance: captured data may contain sensitive production information (credentials, personal data); replay may have side effects and defaults to dry-run with every effect denied; replay must never automatically target the recorded production destination; explicit loopback/effect/policy gates remain visible to users; current security limitations are not hidden (no encryption at rest, no comprehensive redaction, no tenant isolation). Link docs/replay-safety.md and docs/operations.md for detail.

Compact public limitations: Linux-only live capture (with the requirements summarized in Requirements); plaintext HTTP/1.1 only (no TLS decryption, no HTTP/2+); bounded recording (duration + WAL ceiling); sensitive local artifacts; supported storage is local filesystem only; stateful/multiplexed protocols unsupported; preserve-timing scheduling modeled, not executed. Detailed semantics live in the docs, not the README.

### D8. Documentation separation

README = onboarding (what/install/quick start/overview/safety summary). docs/operations.md = operational detail and bounds (already current; referenced, not duplicated). docs/architecture.md + docs/architecture/crate-boundaries.md = architecture and crate contracts. docs/replay-safety.md = replay safety semantics. CONTRIBUTING.md = contributor workflow. docs/release-notes.md = version/deprecation history. The README links all of these; the change adds a link-existence check so referenced documents resolve.

### D9. Development section

Contributor instructions stay separate from installation: pinned toolchain (rust-toolchain.toml), cargo build --workspace, cargo test --workspace --all-features, gates (cargo fmt --all --check, clippy -D warnings), and the canonical layered validation entry point ./scripts/validate.sh fast (and targeted / gate p1|p2 / release when required). Privileged acceptance (./scripts/acceptance.sh --profile p1|p2 --executor local|multipass) is mentioned as opt-in Linux evidence, not part of the normal onboarding flow.

### D10. Validation of documented commands

- Rootless documentation-command contract (smoke-level test, tests/smoke/test_documented_commands.py): against the built binary assert --version, --help surface, doctor contract (non-destructive, actionable output; capture probes unsupported in a non-feature build), list empty contract, public record-syntax preflight behavior on a non-feature build (typed failure, no mutation), and fixture-recorded inspect/replay public forms where rootless-compatible. No prose matching of README text.
- The test runs in the cli_docs targeted case and in the CI checks job; tests/** already selects the portable group, so fast mode covers it too.
- Privileged quick-start scenario (P1): with a release-built linux-ebpf binary, execute the exact documented forms — chronicle doctor (capture probes supported), chronicle record --name checkout -- ./my-app with representative traffic, chronicle list, chronicle inspect checkout, chronicle replay checkout -- ./my-app — and assert the published recording, catalog entry, inspectable session, and replay verification. This is the truthfulness evidence for the live-capture Quick Start; documentation-only changes still select no privileged gate.
- Release-artifact validation: the release workflow runs chronicle --version, --help, doctor (embedded object probe), and verifies the checksums file against the archive contents.
- README relative-link existence is asserted by the documentation-command contract test.
