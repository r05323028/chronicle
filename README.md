# Chronicle

Chronicle is an early-stage, production-native network traffic capture and replay platform. Its target pipeline is:

```text
record → persist → transform → inspect → replay → verify
```

The project is designed around protocol-neutral socket-chunk capture, a local append-only WAL, versioned canonical sessions, PostgreSQL metadata, and S3-compatible artifacts.

The current implementation has a working fixture-driven HTTP/1.1 **record → WAL → ETL → canonicalize → filesystem persist → inspect** vertical slice. Real eBPF capture, HTTP replay, verification, and end-user CLI wiring are still in progress.

> **Safety:** production capture can contain credentials and personal data. Replay can execute writes and publish messages. Replay must remain default-deny, require explicit target remapping, and must never be pointed at a recorded production destination.

## Current status

The active HTTP vertical-slice change is implemented through task **11.2**. The repository currently provides:

- transport-neutral capture types, in-memory capture, and a validated fixture capture source;
- a versioned, CRC32C-framed segmented local WAL with ordered readback, checkpoints, corruption handling, and partial-tail detection;
- bounded ordered socket-chunk session assembly without inventing missing bytes;
- a compile-time protocol capability registry and protocol-neutral detector/decoder/canonicalizer interfaces;
- a stateful bounded HTTP/1.1 detector and decoder for the supported plaintext subset;
- deterministic handling of malformed, truncated, residual, and unsupported HTTP traffic while preserving opaque bytes;
- Canonical Session v2, backward-compatible v1 reading, warnings, and artifact-backed payload references;
- HTTP operation canonicalization with request/response correlation, effects, timing metadata, replay attributes, and verification metadata;
- an atomic filesystem session store with manifests, SHA-256 validation, safe permissions, collision refusal, and artifact hydration;
- a fixture-driven record application service that runs capture through WAL, ETL, canonicalization, and publication;
- an inspect application service with deterministic human and JSON v1 output that excludes bodies, arbitrary header values, and credentials;
- fake-protocol replay and verification scaffolding retained for architecture and compatibility tests.

The completed internal application flow is:

```text
HTTP fixture
    ↓
CaptureEvent
    ↓
segmented WAL
    ↓
WAL readback + checkpoint
    ↓
session assembly
    ↓
HTTP detection + decoding
    ↓
Canonical Session v2
    ↓
filesystem manifest + artifacts
    ↓
inspect-safe human / JSON summary
```

This flow is currently exposed as Rust application services. The public CLI still reports `record`, `inspect`, and `replay` as unimplemented until the CLI integration milestone is completed.

## Architecture

```text
capture -> WAL -> ETL -> session reconstruction -> protocol registry
                                    |                  |
                                    v                  v
                              canonical session -> storage
                                    |
                                    v
                        replay planner -> adapter -> verifier
```

Replay depends only on canonical sessions and protocol interfaces. It does not depend on fixture input, capture, WAL, or eBPF. Persisted canonical sessions remain inspectable after fixture and WAL data are removed.

See [architecture](docs/architecture.md).

## HTTP/1.1 supported subset

The current decoder supports a deliberately narrow plaintext HTTP/1.1 subset:

- fragmented and coalesced request/response heads;
- ordered duplicate headers;
- exact body framing with one unsigned decimal `Content-Length`;
- no-body requests and `HEAD` response semantics;
- sequential request/response correlation and multiple exchanges;
- bounded directional buffers and bounded header parsing;
- binary body preservation through inline or artifact payload references.

Unsupported or unsafe cases are retained deterministically as warnings and opaque data rather than guessed or silently discarded. These currently include, among others:

- TLS ciphertext and HTTP/2 prefaces;
- `Transfer-Encoding` and close-delimited response bodies;
- conflicting, duplicate, signed, list, or overflowing `Content-Length` values;
- informational responses, upgrades, `CONNECT`, unsupported versions, and unsupported request targets;
- incomplete, truncated, orphaned, or pipelined exchanges that cannot be replayed safely.

Detection, decoding, and canonicalization are implemented internally, but HTTP is not advertised as fully `Available` in the capability registry until replay and verification are also implemented.

## Protocol target matrix

A generated registration or partial implementation is not full protocol support.

| Protocol | Detection | Decode | Canonicalize | Persist / Inspect | Replay | Verify |
|---|---:|---:|---:|---:|---:|---:|
| Fake scaffold | Available | Available | Available | Test-only | Available | Available |
| HTTP/1.1 | Implemented subset | Implemented subset | Implemented subset | Implemented | In progress | In progress |
| PostgreSQL | Planned | Planned | Planned | Planned | Planned | Planned |
| MySQL | Planned | Planned | Planned | Planned | Planned | Planned |
| MariaDB | Planned | Planned | Planned | Planned | Planned | Planned |
| Oracle Net/TNS | Research | Research | Planned | Planned | Research | Research |
| MongoDB | Planned | Planned | Planned | Planned | Planned | Planned |
| Kafka | Planned | Planned | Planned | Planned | Planned | Planned |
| NATS | Planned | Planned | Planned | Planned | Planned | Planned |

MySQL and MariaDB share an explicit MySQL-family boundary. S3 API traffic uses the HTTP protocol path; S3-compatible artifact storage is a separate storage concern.

## Next milestones

The next implementation stages are:

1. replay planning with per-operation allowed, denied, and unsupported decisions;
2. strict loopback-only HTTP target mapping and execution safety gates;
3. a real local HTTP replay adapter with header sanitization and replay-time credential injection;
4. HTTP response verification and stable result categories;
5. CLI wiring for record, inspect, replay, output formats, and exit codes;
6. local demonstration server plus unit, integration, and CLI test coverage;
7. final documentation, capability-status updates, and repository validation gates.

A key replay invariant is that Chronicle must send **zero requests** when any preflight decision is not allowed.

## Known limitations

- No eBPF program exists yet. `chronicle-capture-ebpf` remains a scaffold-only crate.
- Current recording demonstrations use validated fixtures, not live production containers or processes.
- Only plaintext traffic is decodable. Arbitrary TLS ciphertext captured from TCP cannot be decoded.
- HTTP replay and verification are not implemented yet.
- `record`, `inspect`, and `replay` are not wired into the public CLI yet; only the underlying record and inspect application services are implemented.
- PostgreSQL metadata and S3-compatible artifact adapters are not implemented; the working vertical slice uses the filesystem session store.
- Stateful authentication, prepared statements, multiplexing, compression, and general replay-environment credential replacement are not implemented.
- Oracle semantics remain research-only because public protocol information and version behavior are incomplete.
- WAL restart repair, retention, archival, and persistent restart checkpoints are deferred.
- Preserve-timing scheduling is modeled but not executed.

Future visibility for encrypted application traffic may require pre-encryption or post-decryption uprobes. Chronicle must not claim TCP packet capture or full TCP reassembly when the selected hook only exposes ordered socket chunks.

## Development

Prerequisite: Rust 1.88.0, selected by `rust-toolchain.toml`.

Linux is required only for future eBPF work. The current fixture-driven tests require no root access or external services.

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo run -p chronicle-cli -- doctor
```

`doctor` currently validates local configuration only; it does not probe external services.

Required repository gates:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The HTTP record and inspect services currently have focused unit and application tests, but the final full-workspace and OpenSpec validation milestone has not yet been completed.

No checked-in configuration should contain secrets. `postgres.connection_url_env` names an environment variable rather than storing credentials.

## MVP direction

The planned MVP includes:

- plaintext production capture through eBPF/socket hooks;
- crash-recoverable WAL operation;
- supported protocol detection, decoding, and canonicalization;
- canonical persistence in PostgreSQL and S3-compatible storage;
- safe inspection and redaction before remote persistence;
- explicit replay target mapping and replay-environment authentication;
- replay scheduling, protocol-specific execution, and verification;
- stable human and machine-readable CLI output.

Dynamic shared-library plugins, arbitrary TLS decryption, HTTP/2, WAL replication, a graphical UI, and additional infrastructure dependencies are outside the initial MVP scope.

Licensed under Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
