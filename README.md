![Banner](./docs/branding/banner.png)

# Chronicle

Chronicle records real application traffic with eBPF and turns it into deterministic, replayable regression-test evidence.

Capture attaches to a supervised command, a running process, or a cgroup with no application instrumentation. Evidence is persisted durably to a local write-ahead log before any interpretation, reconstructed into a protocol-independent canonical session, and later replayed against an explicitly authorized loopback target — never automatically against the recorded production destination.

> **Safety:** captured traffic can contain credentials and personal data. Replay can have side effects and defaults to dry-run with every effect denied. Never point replay at a recorded production destination.

## What is Chronicle?

Chronicle turns real application behavior into durable, replayable evidence for regression testing. Instead of hand-writing fixtures or coupling tests to a service's internals, you record what an application actually does in production or staging, store it as a durable local recording, and replay that behavior later to verify the application still behaves the same way.

The design follows four principles:

- **Minimal integration.** Recording attaches around a command, a process, or a cgroup. The application is not instrumented, patched, or restarted into a special mode.
- **Durable before interpreted.** Raw evidence lands in a crash-recoverable WAL before any decoding, so a crash mid-recording does not lose what was captured.
- **Protocol- and storage-independent core.** ETL reconstructs evidence into a canonical session representation that does not depend on how it was captured or where it is stored, so recordings remain portable and stable.
- **Replay decoupled from capture.** Replay consumes canonical sessions through protocol interfaces. It has no dependency on capture, eBPF, WAL, or ETL, and it never automatically targets the original production destination.

Chronicle is early but runnable software. See [Current capabilities](#current-capabilities--status) and [Current limitations](#current-limitations) for exactly what is and is not supported today.

## Current capabilities / status

Chronicle 0.1.x exposes five intent-oriented commands: `record`, `replay`, `list`, `inspect`, and `doctor`. Today it supports:

- **Live capture of plaintext HTTP/1.1 traffic with eBPF on Linux** — attach to a supervised command (`record -- ./my-app`), a running process (`record --pid PID`), or a cgroup (`record --cgroup PATH`). Recording has an optional whole-recording `--duration`; when omitted it runs until the source completes, an explicit stop, or a fatal failure. WAL storage is bounded by segments and epochs (4 GiB physical ceiling per epoch by default).
- **Bounded, crash-recoverable recording** — a segmented WAL with in-WAL commit markers, recovery, and restartable ETL. Failures during finalization can be retried without recapturing (`record --retry RECORDING`).
- **Deterministic canonical sessions** — ETL reconstructs bounded HTTP/1.1 sessions and publishes them atomically to local storage with stable recording identities (`rec_<uuid>`, names, `latest`).
- **Safe replay** — dry-run by default with every effect denied. Command mode replays into a supervised copy of the application and infers its loopback listener; explicit-target mode requires a loopback target, matching `--allow-host`, effect authorization, and `--execute`. Writes always require `--allow-write`. Recorded production destinations are never contacted.
- **`chronicle doctor`** — non-destructive readiness checks with actionable remediation for platform, kernel, cgroup, BTF, embedded capture programs, and privileges.
- **Fixture recording** — deterministic, credential-free test input for development and CI on any platform.

Not implemented (planned or out of scope, not silently supported): PostgreSQL/S3 persistence, TLS decryption, HTTP/2+, protocols other than bounded plaintext HTTP/1.1, multi-host distributed capture, encryption at rest, redaction, and container-orchestration packaging.

## Installation

### Install a released binary (recommended)

On a supported Linux host (`x86_64` or `aarch64`), install the latest release with a single command:

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

The installer detects the platform, resolves the latest stable release, downloads the matching `chronicle-<version>-<target>.tar.gz` archive, verifies it against the release's `SHA256SUMS`, and installs the `chronicle` binary into `$HOME/.local/bin` (created if missing). If that directory is not on your `PATH`, the installer prints the `export PATH` line to add. It requires only `sh`, `curl`, `tar`, and a SHA-256 utility; no Rust toolchain.

Verify the install:

```bash
chronicle --version
chronicle doctor
```

**Pin a specific release version:**

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

**Install to a custom directory:**

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

Release binaries are built on Linux with live capture included automatically, so it works out of the box on a supported Linux host. Prefer releases over building from source unless you are developing Chronicle itself.

### Manual installation (advanced/fallback)

For environments where the installer is not usable, Linux binaries are also published as GitHub Releases on version tags. Each release provides `chronicle-<version>-<target>.tar.gz` archives for `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` together with a `SHA256SUMS` file:

1. Download the archive for your platform and its `SHA256SUMS` from the latest GitHub Release.
2. Verify the archive before extracting: `sha256sum -c SHA256SUMS` (or the equivalent on your system).
3. Extract and put the `chronicle` binary on your `PATH`.
4. Sanity-check the install: `chronicle --version` and `chronicle doctor`.

### Build from source

Prerequisites: the Rust toolchain pinned in [`rust-toolchain.toml`](rust-toolchain.toml). Live capture additionally requires the [platform prerequisites](#requirements) below.

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

The same command works on every supported platform. On Linux the build automatically includes live capture, embedding the capture programs from the checked-in object under `crates/chronicle-capture-ebpf/objects/` — no nightly Rust install is required. On other platforms the build produces the portable surface: `list`, `inspect`, replay planning and verification, `doctor`, and fixture recording, without live capture.

Run `chronicle doctor` to check whether live capture can actually operate on your host: it probes platform, kernel, cgroup v2, BTF, the embedded capture programs, and the required privileges, and reports remediation for anything missing.

#### Advanced: rebuilding the eBPF capture programs

Normal builds embed the checked-in capture object with the pinned stable toolchain. Only touch `ebpf/` if you are developing the capture pipeline itself; rebuild the object and commit it together with the source (CI enforces this):

```bash
rustup toolchain install nightly --profile minimal --component rust-src
cargo +nightly install bpf-linker --locked
cargo +nightly build -Z build-std=core \
  --manifest-path ebpf/Cargo.toml \
  --target bpfel-unknown-none --release --locked
cp ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture \
  crates/chronicle-capture-ebpf/objects/chronicle-ebpf-capture-bpfel.o
```

Contributor demos on Linux can run the CLI directly: `cargo run -p chronicle-cli -- ...`.

## Requirements

Live capture on Linux requires:

- a supported OS and kernel: Linux 6.1+ with cgroup v2 enabled and BTF available (`/sys/kernel/btf/vmlinux`);
- a supported architecture: little-endian x86_64 or aarch64;
- eBPF capabilities for the recording process: `CAP_BPF` and `CAP_NET_ADMIN`;
- the capture programs embedded in the binary (every Linux build has them; non-Linux builds omit live capture).

Run `chronicle doctor` to check all of this before recording. `doctor` is non-destructive: it probes platform, architecture, cgroup v2, BTF, the embedded capture object and programs, attachment, and capabilities, and reports remediation for anything missing. Non-Linux hosts support the portable surface without live capture.

## Quick Start

This walks through the common command-mode workflow with an installed `chronicle` binary on a supported Linux host. Replace `./my-app` with a real HTTP/1.1 application (plaintext, no TLS).

```bash
# 1. Confirm the host is ready for live capture.
chronicle doctor

# 2. Record the application. Chronicle attaches capture first, then starts
#    the app; recording stops on exit, Ctrl+C, an explicit `--duration`, or a fatal failure.
chronicle record --name checkout -- ./my-app

# 3. While recording runs, send representative traffic to the application
#    from another terminal, for example curl requests against its endpoint.

# 4. See the recording in the catalog.
chronicle list

# 5. Summarize what was captured.
chronicle inspect checkout

# 6. Replay the recording into a fresh, supervised copy of the application
#    and verify it still behaves the same way.
chronicle replay checkout -- ./my-app
```

The application must be reachable on a **non-loopback address** while recording: command-mode replay starts a supervised copy that listens on loopback and refuses to target the exact recorded destination (replay never contacts recorded production destinations). If your application only listens on `127.0.0.1`, record traffic to it through the machine's network address instead, or use explicit `--target` replay mode (see the [operations guide](docs/operations.md)).

`record -- COMMAND...` finalizes the WAL, runs ETL, and publishes a canonical session for each finalized epoch automatically when the application exits, an explicit `--duration` expires, or a fatal failure occurs. `latest` resolves to the newest published recording; recordings can also be addressed by `rec_<uuid>`, bare UUID, or exact name. `replay RECORDING -- COMMAND...` plans first, starts the application in a supervised cgroup, discovers its unique loopback listener, and replays — dry-run by default, with writes still requiring `--allow-write`.

Advanced workflows — recording an already-running workload with `--pid`/`--cgroup`, explicit `--target` replay, `--allow-host` and effect gates, `--retry`, recovery and WAL-sizing details, the continuous recorder, and the hidden `internal` namespace — are documented in the [operations guide](docs/operations.md).

## How It Works

```text
Application
    |
    v
eBPF Capture
    |
    v
WAL
    |
    v
ETL
    |
    v
Canonical Session
    |
    +------> Inspect
    |
    v
Storage
    |
    v
Replay
```

- **eBPF Capture** observes socket lifecycles and byte flows at the kernel level for the recorded scope — a supervised command, a process, or a cgroup. Only plaintext TCP payloads are capturable; TLS ciphertext stays opaque.
- **WAL** persists the captured evidence to a bounded, segmented, crash-recoverable write-ahead log before anything is interpreted. Commit markers and recovery make the durable prefix authoritative.
- **ETL** reads the recovery-authoritative WAL prefix, reconstructs bounded HTTP/1.1 sessions with explicit loss accounting, and produces one deterministic canonical session per finalized epoch.
- **Canonical Session** is the protocol- and storage-independent model of the recording: connections, operations, integrity, and replayability, with no capture or WAL mechanics inside.
- **Storage** publishes canonical sessions atomically to local filesystem artifacts.
- **Inspect** reads the canonical session to summarize endpoints, operations, loss warnings, and replay eligibility.
- **Replay** plans, executes, and verifies. It consumes only canonical sessions and protocol interfaces — it does not depend on capture, eBPF, WAL, or ETL — and it replays only to an authorized loopback target.

## Architecture

- **Capture (`chronicle-capture-ebpf`)** — Linux eBPF socket lifecycle and payload evidence. Kernel ABI details stay inside this crate; only normalized capture events cross outward.
- **WAL (`chronicle-wal`)** — append-only durable framing with commit authority, recovery, and retention. Local durability boundary.
- **ETL (`chronicle-etl`)** — complete extract-transform-load from evidence through canonical publication, owning storage publication and checkpoint ordering.
- **Canonical model (`chronicle-canonical`)** — protocol-independent recording/replay model plus validation.
- **Storage (`chronicle-storage`)** — persistence abstractions with filesystem and in-memory implementations; atomic publication of canonical sessions.
- **Protocol adapters (`chronicle-protocol` + `chronicle-protocol-builtins`)** — the detector/decoder/canonicalizer/replay/verifier SPI and its concrete implementations. A registered protocol is scaffolding, not support.
- **Replay (`chronicle-replay`)** — replay planning, execution, and verification. Depends only on canonical sessions and protocol interfaces; it has no capture, WAL, ETL, or eBPF knowledge.
- **Application and CLI (`chronicle-application`, `chronicle-cli`)** — user-facing use-case composition and argument/rendering/exit mapping. The CLI is a thin outer adapter.

Replay consumes canonical sessions and protocol interfaces, not the production capture pipeline — this boundary is deliberate and preserved. For the full dependency graph, one-responsibility rules, and forbidden edges, see [docs/architecture.md](docs/architecture.md) and [docs/architecture/crate-boundaries.md](docs/architecture/crate-boundaries.md).

## Supported Protocols

Only bounded plaintext HTTP/1.1 is functional end to end today. Everything else below is planned or research — registration in the protocol registry is scaffolding, not support.

| Protocol | Status |
| --- | --- |
| HTTP/1.1 (plaintext) | Supported |
| PostgreSQL | Planned |
| MySQL / MariaDB | Planned |
| MongoDB | Planned |
| Kafka | Planned |
| NATS | Planned |
| Oracle Net/TNS | Research |

HTTP/1.1 support is deliberately narrow: origin-form requests, exact `Content-Length`, bounded chunked responses, trusted close-delimited responses, and sequential keep-alive exchanges. HTTP/2+, TLS decryption, redirects, pipelining, upgrades, chunked requests, and recorded credentials are not supported. S3-compatible artifact storage is a separate, future concern.

## Safety

- **Captured data is sensitive.** WAL and session files can contain production headers, bodies, credentials, and personal data. Chronicle uses private file modes and safe default output, but does not promise encryption at rest, secret discovery, comprehensive redaction, or tenant isolation.
- **Replay can have side effects.** Replay defaults to dry-run and denies every operation — reads, writes, authentication, publication. Explicit authorization is required: loopback target, matching host, effect gates, and `--execute`. Writes require `--allow-write`.
- **Replay never targets the production destination.** Recorded destinations are never a fallback, and mapping back to a recorded endpoint is blocked by default.
- **Gates stay visible.** Authorization and policy gates are explicit user inputs, never hidden defaults.

See [docs/replay-safety.md](docs/replay-safety.md) for the complete safety model and [docs/operations.md](docs/operations.md) for operational bounds.

## Current Limitations

- **Live capture is Linux-only** — cgroup v2, kernel 6.1+, BTF, x86_64/aarch64, and eBPF privileges are required. Other platforms get the portable surface only.
- **Plaintext HTTP/1.1 only** — TLS ciphertext is opaque and cannot be decoded; HTTP/2+ is not supported.
- **Bounded, epoch-rolled recording** — an optional whole-recording `--duration`; when omitted, recording runs until the source completes, an explicit stop, or a fatal failure. WAL storage is bounded by segments and epochs (4 GiB physical ceiling per epoch by default), and epochs roll over without terminating the parent recording. Not a multi-host distributed capture platform.
- **Sensitive local artifacts** — no encryption at rest, no comprehensive redaction, no tenant isolation.
- **Local filesystem storage only** — PostgreSQL/S3 persistence is not implemented.
- **Loss windows are temporal and unattributed** — capture or WAL-limit loss can make overlapping operations incomplete.
- **Stateful and multiplexed protocols are not implemented**, and preserve-timing scheduling is modeled, not executed.

## CLI

| Command | Intent |
| --- | --- |
| `record` | Capture application behavior into a published recording: `record -- COMMAND...`, `record --pid PID`, `record --cgroup PATH`, `record --retry RECORDING`. |
| `list` | Show the recording catalog, newest first. |
| `inspect` | Summarize a recording by `latest`, ID, or name. |
| `replay` | Replay a recording into an authorized loopback target: `replay RECORDING -- COMMAND...` or explicit `--target`. |
| `doctor` | Non-destructive platform, capture, WAL/output, protocol, and replay-policy readiness probes with remediation. |

Global options: `--format human|json` and `--data-dir DIR` (also `--config FILE` for configured deployments). Advanced operational forms — shared-cgroup acknowledgement, explicit target/host/effect gates, retry and recovery, WAL sizing, the continuous recorder, and the hidden `internal` namespace — are documented in the [operations guide](docs/operations.md).

## Development

Chronicle is a Rust workspace; the toolchain and components are pinned in [`rust-toolchain.toml`](rust-toolchain.toml). No external services are required for tests.

```bash
cargo build --workspace
cargo test --workspace --all-features
```

Canonical local validation:

```bash
./scripts/validate.sh fast
```

`fast` runs formatting, warnings-denied Clippy, workspace tests, strict OpenSpec validation, and the repository consistency checks. The full gate set (including manual gate commands):

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Targeted and release validation: `./scripts/validate.sh targeted --changed-since origin/main`, `live-capture|recorder`, and `release`. Real eBPF runtime coverage is opt-in privileged acceptance on supported Linux: `./scripts/acceptance.sh --profile live-capture|recorder --executor local|multipass`. Live-capture development on Linux requires the [platform prerequisites](#requirements); change the capture programs in `ebpf/` only by regenerating and committing the object as described in [Advanced: rebuilding the eBPF capture programs](#advanced-rebuilding-the-ebpf-capture-programs).

No checked-in configuration contains secrets: `postgres.connection_url_env` names an environment variable rather than storing credentials. See [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidance.

## Documentation

- [Architecture](docs/architecture.md) — components, boundaries, versioning policy, and deferred implementations.
- [Crate boundaries](docs/architecture/crate-boundaries.md) — per-crate responsibility and dependency rules.
- [Operations](docs/operations.md) — quick start, bounds, cgroup selection, record/recover/publish, inspect/replay, doctor, and validation.
- [Replay safety](docs/replay-safety.md) — the replay safety model and authorization semantics.
- [Contributing](CONTRIBUTING.md) — how to contribute.
- [Website product brief](docs/PRODUCT.md) — website scope and product truth.
- [Website design brief](docs/DESIGN.md) — website visual system and responsive rules.
- [Website maintainers](website/MAINTAINERS.md) — deployment and documentation version workflow.

## License

Licensed under Apache-2.0. See [LICENSE](LICENSE).
