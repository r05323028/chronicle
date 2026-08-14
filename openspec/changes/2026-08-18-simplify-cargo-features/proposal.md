## Why

Building Chronicle from source currently requires Linux users to know an internal implementation detail: the `linux-ebpf` Cargo feature (`cargo build --release --features linux-ebpf --locked`). The feature propagates through three crates (`chronicle-cli` → `chronicle-application` → `chronicle-capture-ebpf`) purely to switch in Aya-based capture on Linux. This leaks implementation technology (eBPF) into the user-facing build contract, makes the crate that already IS the eBPF capture implementation carry a redundant `linux-ebpf` feature, and forces every Linux contributor to know Aya feature propagation before compiling.

Platform-specific behavior should be selected from the compilation target: Linux builds include live capture automatically; other supported targets build the portable surface without it. One canonical command — `cargo build --release --locked` — should work everywhere.

## What Changes

- Remove the public `linux-ebpf` feature from `chronicle-capture-ebpf`; its Aya dependencies become non-optional target-specific dependencies (`[target.'cfg(target_os = "linux")'.dependencies]`).
- Remove the `linux-ebpf` feature from `chronicle-application`; `chronicle-capture-ebpf` becomes a non-optional target-specific dependency, so the application always compiles live capture on Linux.
- Remove the user-facing feature from `chronicle-cli`; `tokio/signal` is enabled through a target-specific dependency entry so the signal watchers compile on Linux only.
- Rewrite every `#[cfg(all(target_os = "linux", feature = "linux-ebpf"))]` expression to a target-only `#[cfg(target_os = "linux")]` (and the negated forms to `#[cfg(not(target_os = "linux"))]`), keeping the existing big-endian guard as `all(target_os = "linux", target_endian = ...)`.
- Remove the now-dead `EbpfCaptureError::FeatureDisabled` variant; the portable fallback returns `UnsupportedPlatform`.
- Update README build instructions to the single canonical command, keep advanced eBPF object regeneration under a separate developer heading, and keep `chronicle doctor` as the runtime readiness mechanism.
- Update the release workflow, privileged acceptance scenarios, validation allowlists, validation tests, and OpenSpec normative specs to the new target-based contract.
- Adapt rootless record-failure tests: on Linux the live-capture binary fails preflight before touching the target (exit 3, no mutation); on non-Linux the portable build rejects live capture (exit 4).

## Capabilities

### Modified Capabilities

- `developer-onboarding-documentation`: canonical source build is `cargo build --release --locked`; Linux builds include live capture automatically; non-Linux builds provide the portable surface; `chronicle doctor` remains the runtime readiness check.
- `release-packaging`: release binaries are built with `--locked` on Linux targets, with live capture embedded automatically (no feature flag).
- `layered-validation`: rootless documentation-command contract and privileged quick-start scenario now describe failure/readiness in terms of live-capture availability rather than a `linux-ebpf` feature.

## Impact

Affected code: `crates/chronicle-{cli,application,capture-ebpf}/Cargo.toml`, `crates/chronicle-capture-ebpf/src/{error,preflight,source}.rs`, `crates/chronicle-application/src/{lib,record,command_record,etl,doctor}.rs`, `crates/chronicle-cli/src/main.rs`, `crates/chronicle-cli/tests/cli_contract.rs`, `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs`.

Affected non-code: `.github/workflows/release.yml`, `scripts/acceptance/lib/scenarios/{live-capture,recorder}/capture-basic.sh`, `scripts/acceptance/lib/scenarios/shared/documented-quickstart.sh`, `scripts/tests/validation/{test_release_workflow,test_validation_architecture}.py`, `validation/test-architecture/test-catalog.toml`, `tests/support/process.py`, `tests/smoke/test_documented_commands.py`, `docs/feasibility/README.md`, `README.md`, OpenSpec specs for `developer-onboarding-documentation`, `release-packaging`, and `layered-validation`.

No change to WAL, ETL, canonical, storage, replay, capture semantics, or the eBPF program itself. Linux binaries keep working live capture; non-Linux builds keep compiling the portable surface. `chronicle-wal` keeps `test-support` unchanged.
