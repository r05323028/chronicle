# Contributing

Chronicle is early architecture-first software. Keep changes narrow, formats versioned, payloads redacted in logs, and capability claims honest. Rust toolchain and required components are pinned in `rust-toolchain.toml`; no external services are required for tests.

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Protocol work belongs behind `chronicle-protocol` interfaces. Add fixtures containing no real credentials or production data. Keep replay examples dry-run and default-deny; reference environment variable names instead of embedding connection credentials. Bounded plaintext HTTP/1.1 fixture record/inspect/loopback replay is functional alongside fake; fixture capture is one configured WAL segment with no restart repair. eBPF capture, other real protocols, PostgreSQL/S3 adapters, TLS, chunked/close-delimited HTTP, and broad replay remain planned.
