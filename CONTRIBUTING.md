# Contributing

Chronicle is early architecture-first software. Keep changes narrow, formats versioned, payloads redacted in logs, and capability claims honest. Rust toolchain and required components are pinned in `rust-toolchain.toml`; no external services are required for tests.

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

The canonical local validation entry point is `./scripts/validate.sh fast` (formatting, warnings-denied Clippy, workspace tests, strict OpenSpec validation, and repository consistency checks); use `./scripts/validate.sh targeted --changed-since origin/main` for focused changed-path validation and `gate p1|p2` / `release` for complete or release evidence. Real eBPF runtime coverage is opt-in privileged acceptance (`./scripts/acceptance.sh --profile p1|p2 --executor local|multipass`) on supported Linux; see the [operations guide](docs/operations.md) and [architecture](docs/architecture.md) for details.

Protocol work belongs behind `chronicle-protocol` interfaces. Add fixtures containing no real credentials or production data. Keep replay examples dry-run and default-deny; reference environment variable names instead of embedding connection credentials. Bounded plaintext HTTP/1.1 fixture record/inspect/loopback replay is functional alongside fake; fixture capture is one configured WAL segment with no restart repair. eBPF capture, other real protocols, PostgreSQL/S3 adapters, TLS, chunked/close-delimited HTTP, and broad replay remain planned.
