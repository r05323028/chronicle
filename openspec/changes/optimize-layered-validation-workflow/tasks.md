# 1. Validation Selection

- [x] 1.1 Add declarative `validation/groups.toml` mapping source/build/acceptance paths to portable, eBPF, WAL, ETL, replay, CLI/docs, P1, and P2 groups.
- [x] 1.2 Add `scripts/validate.sh` mode/option parsing and targeted changed-path reporting with conservative unknown-path behavior.
- [x] 1.3 Implement fast checks, changed-crate detection, and strict OpenSpec validation without privileged evidence.
- [x] 1.4 Add focused targeted command groups for eBPF, WAL/recovery/retention/quota, ETL/checkpoint, replay, CLI/docs, and acceptance tooling.

## 2. Evidence and Fingerprints

- [x] 2.1 Implement canonical per-gate fingerprint generation from mapped inputs, toolchain, architecture, kernel capabilities, and validation configuration.
- [x] 2.2 Implement compact manifest/environment/summary/checksum artifacts and explicit executed-versus-reused reporting.
- [x] 2.3 Implement failure-first extraction, zstd compression, `--no-artifact`, `--artifact-on-failure`, and `--keep-workdir` behavior without copying caches or successful WAL.
- [x] 2.4 Implement `--reuse-evidence` validation, checksum verification, invalidation, and release completeness enforcement.

## 3. Privileged Runtime and CI

- [x] 3.1 Update Multipass wrappers to reuse existing Ubuntu VM and VM-local Cargo target/cache paths with one-time dependency bootstrap.
- [x] 3.2 Route explicit P1/P2 gates and release through existing authoritative acceptance scripts without dropping checks or exact-SHA/environment guards.
- [x] 3.3 Update CI/configuration and operations documentation with cache separation, evidence retention, failure collection, and runtime/artifact measurement guidance.

## 4. Verification

- [x] 4.1 Add shell tests for path selection, skipped/selected reasons, fingerprint invalidation, and evidence reuse.
- [x] 4.2 Add tests proving successful artifacts exclude target/cache/full WAL and failed artifacts contain a bounded reproducer set.
- [x] 4.3 Run fast, targeted representative cases, OpenSpec strict validation, existing acceptance-runner tests, and record before/after runtime and artifact-size measurements. Readiness contract and diagnostics tests pass; fresh P2 and release evidence retained and unchanged P2 evidence reuse verified.
