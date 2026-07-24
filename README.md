# Chronicle

Chronicle is an early-stage, production-native network traffic capture and replay platform. Its target pipeline is `record → persist → transform → inspect → replay → verify`, with protocol-neutral eBPF capture, a local append-only WAL, canonical sessions, PostgreSQL metadata, and S3-compatible artifacts.

> **Safety:** production capture can contain credentials and personal data. Replay can execute writes and publish messages. Current replay policy defaults to dry-run and denies every operation. Never point replay at a recorded production destination.

## Current implementation

This repository is an architecture scaffold, not a usable capture product. It currently provides:

- transport-neutral capture types and in-memory source;
- a versioned, CRC32C-framed segmented local WAL with partial-tail detection;
- bounded ordered socket-chunk session assembly;
- compile-time protocol capability registry;
- versioned canonical session types and opaque payload preservation;
- in-memory metadata/artifact adapters;
- explicit target mapping, default-deny replay planning, and verification result types;
- CLI command hierarchy; and
- one functional fake-protocol end-to-end test.

PostgreSQL, S3, real protocol codecs, eBPF programs, replay scheduling, and external connectivity checks are planned MVP work.

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

Replay depends only on canonical sessions and protocol interfaces. It does not depend on capture, WAL, or eBPF. See [architecture](docs/architecture.md).

## Protocol target matrix

A generated registration is not support. Only fake scaffold protocol is functional.

| Protocol | Detection | Decode | Canonicalize | Replay | Verify |
|---|---:|---:|---:|---:|---:|
| Fake (scaffold only) | Available | Available | Available | Available | Available |
| HTTP/1.1 | Planned | Planned | Planned | Planned | Planned |
| PostgreSQL | Planned | Planned | Planned | Planned | Planned |
| MySQL | Planned | Planned | Planned | Planned | Planned |
| MariaDB | Planned | Planned | Planned | Planned | Planned |
| Oracle Net/TNS | Research | Research | Planned | Research | Research |
| MongoDB | Planned | Planned | Planned | Planned | Planned |
| Kafka | Planned | Planned | Planned | Planned | Planned |
| NATS | Planned | Planned | Planned | Planned | Planned |

MySQL and MariaDB share an explicit MySQL-family boundary. S3 API capture uses HTTP/1.1; S3-compatible artifact storage is a separate concern.

## Known limitations

- No eBPF program exists yet. Hook selection must determine whether Chronicle sees ordered socket chunks and where truncation can occur; current code never calls them packets or claims TCP reassembly.
- Only plaintext is decodable. Arbitrary TLS ciphertext captured from TCP cannot be decoded. Future pre-encryption/post-decryption uprobes may add visibility without exporting production certificates. Encrypted and unknown bytes must remain opaque.
- Oracle semantics are research-only because public protocol information and version behavior are incomplete.
- Stateful authentication, prepared statements, multiplexing, compression, and replay-environment credential replacement are not implemented.
- WAL restart repair, retention, archival, and checkpoint persistence are deferred.
- Preserve-timing scheduling is modeled, not executed.

## Development

Prerequisites: stable Rust. Linux is required only for future eBPF work; tests require no root access or external services.

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo run -p chronicle-cli -- doctor
```

Required gates:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Future Linux eBPF adapter build command:

```bash
cargo build -p chronicle-capture-ebpf --target x86_64-unknown-linux-gnu
```

No checked-in configuration should contain secrets. `postgres.connection_url_env` names an environment variable rather than storing credentials.

## MVP direction

Planned MVP: plaintext capture, crash-recoverable WAL operation, supported protocol codecs, canonical persistence in PostgreSQL/S3, replay-environment authentication, replay scheduling, protocol-specific verification, inspect/doctor behavior, and redaction before remote persistence.

Dynamic shared-library plugins, TLS decryption, HTTP/2, WAL replication, UI, and additional infrastructure are outside initialization scope.

Licensed under Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
