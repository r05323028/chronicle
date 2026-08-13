# Gate coverage comparison (pre/post migration)

Baseline: `validation/test-architecture/baseline.json` (pre-change snapshot).
Post: `validation/test-architecture/migration-ledger.toml` +
`test-catalog.toml` + `gate-coverage.toml`.

## Displaced privileged-run commands

14 `embedded_cargo_repo_commands` ran cargo suites inside P1/P2 scenario
bodies before the migration. Every one has a classified ledger entry
(uncovered = 0, extra = 0).

| Command (scenario) | Environment | Post home | Disposition |
| --- | --- | --- | --- |
| cargo build --release (p1 capture-basic:14) | portable | ebpf group / CI ebpf-compile | move |
| privileged_feasibility kernel test (p1 :162) | privileged | selected privileged test | remain_privileged |
| cargo test -p chronicle-wal (p1 :164) | portable | chronicle-wal unit + integration:chronicle-wal-matrix | move (3.2) |
| ingest-limit (p1 :174) | portable | chronicle-application unit | move (3.2) |
| ingest-bounds (p1 :175) | portable | chronicle-application unit | move (3.2) |
| replay matrix (p1 replay:27) | portable | chronicle-replay unit (3.4) | move |
| cgroup decision x4 (p1 replay:36-39) | portable | cgroup_selection unit (3.2) | move |
| privileged_signal (p1 replay:48) | privileged | selected privileged test | remain_privileged |
| cargo fmt --check (p1 resource-cleanup:23) | portable | validate.sh fast/release | move |
| cargo check --workspace (p1 :25) | portable | validate.sh fast/release | move |
| eBPF nightly build (p2 capture-basic:14) | portable | ebpf group / CI ebpf-compile | move |

Environment split: 12 portable (now rootless) / 2 privileged (kernel-only).

## Duplicate reduction

`cargo test -p chronicle-wal` ran in TWO scenario bodies (p1 capture-basic:164,
p2 wal-recovery:199); the replay matrix ran in p1 and p2 replay.sh. Post:
scenario obligations reference single classified tests
(`integration:chronicle-wal-matrix`, `unit:chronicle-replay`, plus unit
prerequisites) instead of re-running whole crate suites per scenario.

## Coverage-loss check

- Every baseline embedded command has a ledger entry: covered (14/14); the blind-spot audit below accounts for every current-tree cargo invocation.
- Required gate selectors reference classified tests: catalog_check issues = 0
  (47 tests, 8 planned privileged).
- Portable checks now run outside the privileged executor: validate.sh fast
  (fmt/clippy/workspace/catalog/tooling) + CI hosted runner smoke/rootless
  suites + crate matrices; privileged scenario bodies keep only
  kernel/environment-dependent proof (removal of remaining duplicate steps is
task 8.x, blocked on the concurrent change).
- Replacement proof is equal or stronger: rootless crate matrices add
  deterministic real-file coverage (wal_matrix 8, etl_contract 4,
  loss_window_model 1) beyond the re-invoked unit tests.

## Blind-spot audit (upgraded scanner)

The baseline scanner matched only lines starting with `cargo`. The upgraded
scanner (inventory.py) also matches env-prefixed (`VAR=... cargo ...`) and
`run_compat_command ID cargo ...` invocations. Re-inventory of the current
tree finds 28 command-position cargo invocations: the 14 baseline commands
plus 14 duplicate occurrences, every one a second execution of a command
already dispositioned under its baseline ledger entry:

| Current-tree occurrence | Duplicate of ledger entry |
| --- | --- |
| p1 capture-basic.sh:19 (EBPF_TARGET_DIR build) | cargo build --release (move, ebpf compile) |
| p2 capture-basic.sh:15 (EBPF_TARGET_DIR build) | cargo build --release (move, ebpf compile) |
| p2 wal-recovery.sh:198 (wal-feasibility) | privileged_feasibility kernel test (remain_privileged) |
| p2 wal-recovery.sh:199 (wal-tests) | cargo test -p chronicle-wal (move 3.2) |
| p2 wal-recovery.sh:203 (ingest-limit) | ingest-limit (move 3.2) |
| p2 wal-recovery.sh:204 (ingest-bounds) | ingest-bounds (move 3.2) |
| p2 replay.sh:29 (replay-matrix) | replay matrix (move 3.4) |
| p2 resource-cleanup.sh:5-8 (cgroup x4) | cgroup decision x4 (move 3.2) |
| p2 resource-cleanup.sh:10 (signal) | privileged_signal (remain_privileged) |
| p2 resource-cleanup.sh:15 (format) | cargo fmt --check (move) |
| p2 resource-cleanup.sh:20 (workspace) | cargo check --workspace (move) |

Ledger entries stay baseline-scoped (14/14, uncovered = 0); the duplicate
occurrences are removed by the scenario-body reduction in task 8.x (blocked
on the concurrent change).
