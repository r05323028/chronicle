## 1. Rename validation machinery

- [x] 1.1 Rename profile/gate/selector/evidence identifiers `p1`/`p2` to `live-capture`/`recorder` in `scripts/validate.sh` (CLI modes `live-capture|recorder`, drop the `gate` wrapper), `scripts/validation.py`, `validation/groups.toml`, `validation/test-architecture/test-catalog.toml`, `scripts/acceptance/scenarios.toml`, `runner.py`, `report.py`, `lib/multipass.sh`, and the acceptance scenario/profile files.
- [x] 1.2 Rename files: `profile-{p1,p2}.sh` → `profile-{live-capture,recorder}.sh`; scenario dirs `p1/`/`p2/` → `live-capture/`/`recorder/`; `test-p2-readiness.sh` → `test-recorder-readiness.sh`; scenario functions `scenario_{p1,p2}_*` → `scenario_{live_capture,recorder}_*`; `p2-completion` spec → `recorder-durability`.
- [x] 1.3 Update CI (`.github/workflows/ci.yml`) and all validation tests to the new names.

## 2. Docs and specs

- [x] 2.1 Update README, CONTRIBUTING, AGENTS.md (one-sentence scope descriptions), docs/operations.md, docs/architecture.md, docs/feasibility, and validation/test-architecture docs.
- [x] 2.2 Rename milestone-qualified terminology in the normative OpenSpec specs (layered-validation, developer-onboarding-documentation, production-recorder-operation, recording-diagnostics, recording-store, recoverable-recording-wal, restartable-recording-etl, runnable-http-cli, bounded-validation-execution, continuous-recorder-lifecycle, http11-operations, local-session-artifacts, production-http-capture, safe-local-http-replay, workspace-dependency-boundaries, recorder-durability).

## 3. Code-level terminology

- [x] 3.1 `ETL_PIPELINE_VERSION` value `"p1"` → `"etl-v1"` (self-describing format version) and update test literals.
- [x] 3.2 `Gate A` → `capture-feasibility` in the feasibility test strings/report path and docs; crate comments updated.

## 4. Validation

- [x] 4.1 `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --workspace --all-features` (35/35 binaries), strict OpenSpec validation (28/28), architecture/catalog/select/ownership checks.
- [x] 4.2 Validation python suites, acceptance runner/readiness tests, and rootless suites pass; `validate.sh fast`, help, and dry runs show the new modes.
- [ ] 4.3 Privileged Linux acceptance (live-capture/recorder) requires a privileged Linux host/VM per policy; covered by CI/acceptance on supported Linux.
