# Deliberate decisions for the test architecture (version 1)

Every decision names a responsibility or reliability reason; no
directory-aesthetic-only move is made.

## Rust colocated unit tests (42 files)

**Decision: stay.** All colocated `#[cfg(test)]` modules prove local logic and
stay colocated; no `tests/unit` tree is created. Heuristic signal flags
(subprocess/network/sleep/privilege) are fixture or production-code text
matches, not daemon or full-system lifecycle.

## Crate integration suites (8 files)

| Suite | Decision | Reason |
| --- | --- | --- |
| `application/tests/fake_vertical.rs` | stay (integration) | rootless synthetic composition contract via public APIs; binary E2E adds the composed path |
| `application/tests/fixtures_http_vertical.rs` | stay (integration) | rootless deterministic fixture pipeline |
| `application/tests/http_test_server.rs` + `support/mod.rs` | stay (integration) | crate-local test support, one consumer |
| `capture-ebpf/tests/privileged_adapter.rs` | remain privileged (integration) | real eBPF attach/capture requires supported kernel |
| `capture-ebpf/tests/privileged_kernel.rs` | split | state-machine proof is rootless; production-object kernel acceptance stays privileged |
| `cli/tests/cli_contract.rs` | split | arg/exit/rendering stays CLI integration; liveness -> smoke; features -> acceptance; composition -> E2E; no new lower dev deps |
| `cli/tests/privileged_signal.rs` | remain privileged (integration) | real cgroup/signal behavior |

## Validation/acceptance tooling self-tests

Stay as portable tooling tests; they prove selector/orchestrator contracts,
never product correctness.

## Privileged acceptance scenarios (13)

- `remain_privileged`: recorder-readiness, quota-pressure, reboot-recovery.
- `make_rootless`: wal-recovery, incremental-etl, corruption-quarantine.
- `split`: capture-basic, replay, user-intent-lifecycle, resource-cleanup,
  checkpoint-kill-restart, retention-interruption.

Per-scenario classification details are in `path-classification.md`.

## Embedded cargo in privileged scenarios

Every portable WAL/ETL/replay/CLI/fmt/workspace cargo invocation was removed
from scenario bodies into portable prerequisites and repository checks. The
only embedded cargo that remains is genuinely privileged: building release and
eBPF artifacts and executing the `--ignored` privileged test suites (kernel
kernel acceptance, signal handling). `scripts/tests/validation/test_validation_architecture.py`
enforces this deny-by-default via a documented allowlist.

## New root black-box suites (tasks 4.1/4.2/5.1/6.2)

- `tests/support/process.py` — shared bounded process/JSON/fixture/driver
  support (3 consumers: smoke, acceptance, e2e). No new dependency or
  workspace crate.
- `tests/smoke/test_smoke.py` — help, usage error, empty list,
  missing-fixture error, unsupported-build record failure, minimal fixture
  liveness per public surface.
- `tests/acceptance/test_record.py`, `test_inspect.py`, `test_replay.py` —
  per-feature black-box contracts.
- `tests/e2e/test_rootless_pipeline.py` — synthetic capture -> WAL ->
  canonical -> replay -> verification through the binary (invariants only).

Decisions:

- **Internal ETL CLI contract.** ETL has no documented public user surface, so
  the internal ETL seam is not Smoke or Acceptance. Its fail-closed contracts
  (metadata-less WAL rejected, identity mismatch rejected) live in
  `crates/chronicle-cli/tests/cli_contract.rs` under the CLI-owned
  `integration:cli-contract`; application ETL correctness stays separately
  owned by `integration:etl-contract` (chronicle-application); the two are
  never merged. Public ETL acceptance will be added separately only if ETL
  gains a documented public command.
- **Smoke version probe.** The binary exposes no `--version` flag; `--help`
  is the liveness probe.
- **Internal forms retained intentionally.** `internal record-fixture`
  remains exercised as deterministic rootless setup for public
  smoke/acceptance/E2E surfaces; the `internal etl` seam is covered only by
  the CLI-owned Integration contract, never as Smoke/Acceptance proof. Public
  `record`, `inspect`, `replay` drive user acceptance. Real
  capture/catalog flows remain privileged on supported Linux.
- **Catalog status.** All 47 classified catalog tests are `existing`; live-capture
  requires 42 and recorder/release require 47.

## Selection reporting + evidence reuse policy

- `scripts/validation.py select` annotates each selected group with its
  gate-relevant catalog tests (id/layer/environment/owner) plus aggregate
  layers/environments/owners and a no-coverage-loss marker. Unknown changed
  paths conservatively select full validation.
- Timeout/unsupported/infrastructure/failed/not_checked evidence is never
  reused; passed manifests report executed=true and are checksum-verified
  against the fingerprint and environment.
- CI hosted runner builds the CLI binary and runs portable
  smoke/acceptance/rootless-E2E suites; privileged runtime remains a
  supported-platform concern.

## Residual risk: portable loss-window fake vs real sampler

`loss_window_model.rs` (rootless) mirrors the real production
`LossWindowSampler` (chronicle-capture-ebpf/src/adapter.rs) plus
source.rs `sample_loss_at`; the fake and the Linux-only real sampler are not
linked, so drift would go undetected by the portable test. Promotion of the
real sampler into chronicle-capture is a candidate follow-up (moves
production code).
