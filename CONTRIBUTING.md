# Contributing

Chronicle is early architecture-first software. Keep changes narrow, formats versioned, payloads redacted in logs, and capability claims honest. Rust toolchain and required components are pinned in `rust-toolchain.toml`; no external services are required for tests.

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Protocol work belongs behind `chronicle-protocol` interfaces. Add fixtures containing no real credentials or production data. Keep replay examples dry-run and default-deny; reference environment variable names instead of embedding connection credentials. Only fake protocol is currently functional; eBPF capture, real protocol codecs, PostgreSQL/S3 adapters, and real replay remain planned.
