## 1. Manifests

- [x] 1.1 `crates/chronicle-capture-ebpf/Cargo.toml`: remove `[features]`; make `aya`/`aya-obj`/`libc` non-optional under `[target.'cfg(target_os = "linux")'.dependencies]`.
- [x] 1.2 `crates/chronicle-application/Cargo.toml`: remove `[features]`; make `chronicle-capture-ebpf` a non-optional target-specific dependency.
- [x] 1.3 `crates/chronicle-cli/Cargo.toml`: remove `[features]`; add `tokio` with `signal` under `[target.'cfg(target_os = "linux")'.dependencies]`.

## 2. cfg cleanup

- [x] 2.1 `chronicle-capture-ebpf/src/{preflight,source}.rs`: rewrite cfg expressions per design Decision 2; drop the `FeatureDisabled` branch in the non-Linux stub `load`.
- [x] 2.2 `chronicle-capture-ebpf/src/error.rs`: remove the `FeatureDisabled` variant.
- [x] 2.3 `chronicle-capture-ebpf/tests/privileged_feasibility.rs`: `#![cfg(target_os = "linux")]`.
- [x] 2.4 `chronicle-application/src/{lib,record,command_record,etl,doctor}.rs`: rewrite cfg expressions; update doctor remediation wording to target/capability language.
- [x] 2.5 `chronicle-cli/src/main.rs` and `tests/cli_contract.rs`: rewrite cfg expressions; adjust fallback error wording.

## 3. Rootless tests

- [x] 3.1 `tests/support/process.py`: `require_rootless_record_failure` accepts exit 3 on Linux (live-capture preflight) and exit 4 elsewhere; keep no-mutation/no-target assertions.
- [x] 3.2 `tests/smoke/test_documented_commands.py`: record-syntax preflight test accepts 3 (Linux rootless) / 4 (non-Linux); no-mutation assertions unchanged.

## 4. Scripts and workflow

- [x] 4.1 `scripts/acceptance/lib/scenarios/{live-capture,recorder}/capture-basic.sh`: drop `--features linux-ebpf` from the CLI build; update phase wording.
- [x] 4.2 `scripts/acceptance/lib/scenarios/shared/documented-quickstart.sh`: wording only.
- [x] 4.3 `scripts/tests/validation/test_release_workflow.py`: assert the canonical build command and no `--features` in the release build step.
- [x] 4.4 `scripts/tests/validation/test_validation_architecture.py`: update the privileged embedded-cargo allowlist to the feature-free commands.
- [x] 4.5 `validation/test-architecture/test-catalog.toml`: update scenario descriptions mentioning `linux-ebpf`.
- [x] 4.6 `.github/workflows/release.yml`: drop `--features linux-ebpf`; rename step.
- [x] 4.7 `docs/feasibility/README.md`: drop `--features linux-ebpf`.

## 5. Documentation

- [x] 5.1 `README.md`: canonical `cargo build --release --locked` build; Linux live capture automatic; non-Linux portable surface; doctor as runtime check; eBPF object regeneration under a separate advanced heading.

## 6. OpenSpec specs

- [x] 6.1 `openspec/specs/developer-onboarding-documentation/spec.md`: source-build contract without `linux-ebpf`.
- [x] 6.2 `openspec/specs/release-packaging/spec.md`: release build without `linux-ebpf`.
- [x] 6.3 `openspec/specs/layered-validation/spec.md`: rootless/privileged contract wording by capability.

## 7. Validation

- [x] 7.1 `cargo build --release --locked` works and produces a live-capture binary on Linux (verified via `cargo check -p chronicle-cli --target x86_64-unknown-linux-gnu --locked`; full Linux release build covered by CI/release workflow and privileged acceptance on Linux hosts).
- [x] 7.2 Formatting, clippy, workspace tests, strict OpenSpec validation, and repository validation pass (fmt check, clippy -D warnings, 35/35 test binaries, OpenSpec 27/27, architecture check, tooling tests, rootless suites).
- [x] 7.3 Privileged Linux validation per policy (requires a privileged Linux host/Multipass VM; covered by fresh x86_64 Multipass release acceptance run `32175100105`).
