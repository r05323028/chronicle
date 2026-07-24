# Contributing

Chronicle is early architecture-first software. Keep changes narrow, formats versioned, payloads redacted in logs, and capability claims honest.

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

Protocol work belongs behind `chronicle-protocol` interfaces. Add fixtures containing no real credentials or production data.
