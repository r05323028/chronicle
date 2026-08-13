# Process/lifecycle pattern inventory (version 1)

Inventory date: 2026-08-13. Purpose: task 6.1 — find repeated process
orchestration across Rust, Python, and shell tests so shared helpers are
introduced only where at least two concrete consumers exist; one-off support
stays local.

## Measured occurrences (heuristic scan, current checkout)

| Pattern | Language | Count | Distinct files |
| --- | --- | --- | --- |
| `Command::new` / `spawn` | Rust | 37 | 5 |
| thread/tokio sleep | Rust | 29 | 12 |
| TcpListener/TcpStream / bind :0 | Rust | 53 | 7 |
| kill / signal / ExitStatusExt | Rust | 17 | 5 |
| `Popen` | Python | 1 | 1 |
| poll / wait / wait_for | Python | 7 | 2 |
| terminate / kill | Python | 2 | 2 |
| port-file handoff | Python | 12 | 2 |
| background `&` | Shell | 16 | 7 |
| `sleep` | Shell | 25 | 10 |
| kill / terminate | Shell | 31 | 10 |
| `trap` cleanup | Shell | 23 | 9 |
| wait_until / wait_for_* | Shell | 129 | 20 |
| cleanup / rm -rf | Shell | 27 | 12 |

## Candidate shared helpers (>= 2 consumers)

### ChronicleProcess / RecorderProcess ownership

Consumers: `cli_contract.rs` + `privileged_signal.rs` (Rust binary spawn),
acceptance scenarios (shell background recorder/upstream), `runner.py`
(Popen). Justified (3 language surfaces). Responsibility: spawn, bounded
readiness handoff, signal, deterministic reap on pass/fail/timeout.

### WorkloadServer lifecycle

Consumers: `http_test_server.rs` (Rust loopback), `http_acceptance_driver.py`
(Python serve), acceptance scenarios (upstream/replay targets). Justified
(3 surfaces). Responsibility: bind ephemeral port, readiness via port-file or
condition, terminate deterministically.

### Readiness/WaitCondition

Consumers: `wait.sh` (shell), `recorder-readiness.sh` (shell), Python driver
self-test port-file poll, Rust listener bind waits. Justified. Existing
`wait_until` + `wait_for_*` are the shell implementation; recorder readiness
remains specialized (freshness/stale-owner/terminal semantics, task 6.5).

### Timeout

Consumers: every bounded path via `scripts/run-with-timeout.sh` (already
shared). Reused, not reimplemented.

### Port allocation

Consumers: Rust `bind(127.0.0.1:0)` in cli_contract/application tests, Python
port-file in driver. Two consumers; may stay language-local (Rust uses
ephemeral bind; Python uses port-file). No cross-language helper needed yet.

### EvidenceCollector / Cleanup

Consumers: acceptance profiles/scenarios (trap + artifact root), runner.py
(compact/failure evidence), report.py (manifest). Already centralized in
acceptance tooling; stays there. Rust test cleanup stays crate-local
(`CgroupGuard` Drop, temp dirs).

## One-off / local (no shared helper)

- `crates/chronicle-application/tests/support/mod.rs` — single crate support.
- `CgroupGuard` in `privileged_adapter.rs` / `privileged_signal.rs` —
  privileged crate tests; stays local (Aya/cgroup ownership, task 6.2
  dependency-direction rule).
- `test_runner.py` Popen — single runner; reuse shell helpers instead.

## Migration guidance for task 6.2/6.3

- Implement language-local helpers in the new black-box suites (tests/smoke,
  tests/acceptance, tests/e2e) with the responsibilities above; reuse
  `run-with-timeout.sh` and acceptance `wait_until` where practical.
- Replace arbitrary synchronization sleeps with observable bounded conditions;
  only elapsed-age behavior keeps explicit waits followed by convergence.
- No new workspace crate or third-party framework is justified by this
  inventory; every helper above has a named owner and allowed dependencies.
