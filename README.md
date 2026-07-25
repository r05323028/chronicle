![Banner](./docs/branding/banner.png)

# Chronicle

Chronicle records versioned fixture socket-byte chunks through a local WAL, reconstructs bounded HTTP/1.1 sessions, persists them locally, inspects them safely, and replays only to an explicitly authorized loopback target. eBPF, PostgreSQL, S3, TLS, and protocols other than HTTP/1.1 remain planned.

> **Safety:** production capture can contain credentials and personal data. Replay can execute writes and publish messages. Current replay policy defaults to dry-run and denies every operation. Never point replay at a recorded production destination.

## Current implementation

Current HTTP/1.1 slice provides fixture capture, CRC32C WAL, bounded ordered socket-chunk assembly, canonical schema v2, atomic filesystem sessions, safe inspect output, strict loopback replay, fixed-header verification, and `record`, `inspect`, and `replay` CLI commands. Fake remains available for boundary tests.

HTTP support is deliberately narrow: plaintext HTTP/1.1, origin-form requests, fixed `Content-Length` framing, sequential exchanges, and immediate timing. Chunked/close-delimited messages, HTTP/2+, TLS, redirects, connection reuse, pipelined replay, and recorded credentials are unsupported.

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

A generated registration is not support. Fake and bounded HTTP/1.1 are functional.

| Protocol | Detection | Decode | Canonicalize | Replay | Verify |
|---|---:|---:|---:|---:|---:|
| Fake (functional scaffold) | Available | Available | Available | Available | Available |
| HTTP/1.1 | Available | Available | Available | Available | Available |
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
- Stateful authentication, prepared statements, multiplexing, and compression are not implemented. Optional Authorization replacement comes only from configured environment-variable names; captured credentials are stripped.
- WAL restart repair, retention, archival, and checkpoint persistence are deferred.
- Preserve-timing scheduling is modeled, not executed.

## Local HTTP demo

Fixtures are safe, credential-free test input. Record then inspect without exposing bodies:

```bash
cargo run -p chronicle-cli -- record --source fixture --input fixtures/http/basic-session.json --root /tmp/chronicle-demo
cargo run -p chronicle-cli -- inspect <session-id> --root /tmp/chronicle-demo
```

Replay stays dry-run unless `--execute` and matching loopback/effect gates are supplied. For manual verification, run `cargo run -p chronicle-application --example http_test_server -- 18080`, then replay the recorded session with `--target http://127.0.0.1:18080 --allow-host 127.0.0.1 --allow-read --execute`. Helper is test/demo-only; it is not production server.

## Development

Prerequisites: Rust 1.88.0, selected by `rust-toolchain.toml`. Linux is required only for future eBPF work; tests require no root access or external services.

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo run -p chronicle-cli -- doctor
```

`doctor` currently validates local configuration only; it does not probe external services.

Required gates:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

`chronicle-capture-ebpf` is a scaffold-only crate. No eBPF build or capture implementation is part of the current development gates.

No checked-in configuration should contain secrets. `postgres.connection_url_env` names an environment variable rather than storing credentials.

## MVP direction

Planned MVP: plaintext capture, crash-recoverable WAL operation, supported protocol codecs, canonical persistence in PostgreSQL/S3, replay-environment authentication, replay scheduling, protocol-specific verification, inspect/doctor behavior, and redaction before remote persistence.

Dynamic shared-library plugins, TLS decryption, HTTP/2, WAL replication, UI, and additional infrastructure are outside initialization scope.

Licensed under Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
