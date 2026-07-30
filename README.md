![Banner](./docs/branding/banner.png)

# Chronicle

Chronicle records deterministic fixtures or Linux eBPF socket evidence through a bounded, segmented WAL, reconstructs bounded HTTP/1.1 sessions, persists them locally, inspects them safely, and replays only to an explicitly authorized loopback target. PostgreSQL, S3, TLS, and protocols other than HTTP/1.1 remain outside P1.

> **Safety:** production capture can contain credentials and personal data. Replay can execute writes and publish messages. Current replay policy defaults to dry-run and denies every operation. Never point replay at a recorded production destination.

## Current implementation

P1 provides fixture and Linux eBPF Capture Event v1, endpoint/role provenance, CRC32C segmented WAL v1 with in-WAL commit markers, bounded ingest and loss windows, crash recovery, restartable ETL, Canonical Session v1, atomic filesystem publication, safe inspect, strict loopback replay, and `record`, `etl`, `inspect`, `replay`, and `doctor` commands. Fake remains available for rootless tests.

All internal MVP artifacts use one mutable v1 until explicit compatibility freeze. Readers reject other versions; repository fixtures, tests, and docs change together rather than adding compatibility dispatch.

HTTP support is deliberately narrow: plaintext HTTP/1.1, origin-form requests, exact `Content-Length`, bounded chunked responses, trusted close-delimited responses, and sequential keep-alive exchanges. HTTP/2+, TLS decryption, redirects, pipelining, upgrades, chunked requests, and recorded credentials remain unsupported.

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

- Live capture is Linux-only: cgroup v2, kernel 6.1+, BTF, supported x86_64/aarch64, and required eBPF privileges/hooks are required. macOS and rootless CI use fixture capture, ETL, inspect, replay, and eBPF compile checks only.
- Recording is bounded to 10 minutes by default (maximum 60 minutes) and a 4 GiB physical WAL ceiling. It is not always-on, rotating, incremental, or distributed capture.
- Loss windows are temporal and unattributed. Capture and WAL-limit loss can make overlapping operations incomplete; operations proven outside windows may remain replayable.
- Only plaintext is decodable. Arbitrary TLS ciphertext captured from TCP remains opaque.
- Stateful authentication, prepared statements, multiplexing, compression, and future protocols are not implemented. Captured credentials are stripped; optional Authorization replacement names an environment variable but never stores its value.
- WAL/session files can contain sensitive headers and bodies. P1 does not promise encryption at rest, secret discovery, comprehensive redaction, or tenant isolation.
- Preserve-timing scheduling is modeled, not executed.

## Local HTTP demo

Fixtures are safe, credential-free test input. Record, ETL, inspect, then replay only to an explicit loopback target:

```bash
cargo run -p chronicle-cli -- record --source fixture --input fixtures/http/basic-session.json --root /tmp/chronicle-demo
# fixture output prints session_id; production ETL uses --wal-dir instead of --root
cargo run -p chronicle-cli -- inspect <session-id> --root /tmp/chronicle-demo
```

Replay stays dry-run unless `--execute` and matching loopback/effect gates are supplied. For manual verification, run `cargo run -p chronicle-application --example http_test_server -- 18080`, then replay with `--target http://127.0.0.1:18080 --allow-host 127.0.0.1 --allow-read --execute`. Never use recorded destinations.

## Development

Prerequisites: Rust version selected by `rust-toolchain.toml`. Linux with cgroup v2/BTF and documented privileges is required only for live eBPF recording; rootless tests require no external services.

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo run -p chronicle-cli -- doctor
```

`doctor` runs non-destructive platform, eBPF, WAL/output, protocol, and replay-policy probes. Omitted paths remain optional `not_checked`; it never starts capture, repairs files, or sends replay traffic.

Required gates:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Rootless CI runs workspace tests and isolated eBPF compilation. Real eBPF runtime coverage is opt-in and retained by `./scripts/p1-privileged-acceptance.sh`; skipped privileged runs are not passed off as runtime coverage.

No checked-in configuration should contain secrets. `postgres.connection_url_env` names an environment variable rather than storing credentials.

## P1 commands

```text
record --source fixture --input FILE --root ROOT
record --source ebpf (--pid PID | --cgroup PATH [--allow-shared-cgroup]) --wal-dir DIR
etl --wal-dir DIR --output ROOT
doctor [--wal-dir DIR] [--output ROOT]
inspect SESSION_ID --root ROOT
replay SESSION_ID --root ROOT --target http://LOOPBACK:PORT --allow-host LOOPBACK [--execute]
```

Production record defaults to 600 seconds and 4 GiB WAL, with `--duration-seconds 1..=3600` and a lower `--max-wal-bytes` allowed down to segment size. It never runs ETL implicitly. See [P1 operations](docs/operations.md).

## MVP direction

Future work: PostgreSQL/S3 persistence, replay scheduling, redaction before remote persistence, retention, and additional protocol codecs.

Dynamic shared-library plugins, TLS decryption, HTTP/2, WAL replication, UI, and additional infrastructure are outside initialization scope.

Licensed under Apache-2.0. See [CONTRIBUTING.md](CONTRIBUTING.md).
