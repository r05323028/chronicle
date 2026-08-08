## 1. Timeout Primitive

- [x] 1.1 Add portable `scripts/run-with-timeout.sh` with validation, process-group TERM/KILL escalation, output preservation, original status, and timeout status 124.
- [x] 1.2 Add rootless wrapper tests for success, original failure, stdout/stderr, child cleanup, and TERM-ignoring child escalation.

## 2. Hierarchical Validation Boundaries

- [x] 2.1 Route shared validation command runners and complete gate/release execution through configurable command and gate deadlines.
- [x] 2.2 Add configurable scenario watchdogs to central dispatcher with scenario identity, phase, cleanup-preserving termination, and timeout artifacts.
- [x] 2.3 Add gate-level process-tree containment to unified acceptance runner and aggregate timeout records into central report.

## 3. Polling and External Operations

- [x] 3.1 Keep recorder readiness separately bounded, validate timeout inputs, bound status/service/diagnostic subprocesses, and retain transition/failure context.
- [x] 3.2 Bound systemd lifecycle commands and convert service convergence waits to deadline-based helpers with last-state diagnostics.
- [x] 3.3 Bound Multipass lifecycle, bootstrap, transfer, reboot-readiness, and remote acceptance commands with operation-specific budgets.
- [x] 3.4 Audit remaining acceptance polling and daemon/process waits; replace correctness-only fixed delays and unbounded waits with condition-or-deadline behavior.

## 4. Evidence, Tests, and Workflow

- [x] 4.1 Add scenario tests for pass, fail, timeout, cleanup, process cleanup, and report identity.
- [x] 4.2 Extend readiness and acceptance-runner tests for hanging subprocesses, stale status, terminal state, timeout evidence, and gate containment.
- [x] 4.3 Update `AGENTS.md` and validation/recorder documentation with timeout hierarchy, environment overrides, and fast → targeted → bounded scenario → full gate workflow.

## 5. Validation

- [x] 5.1 Run shell/Python timeout, scenario, readiness, and validation-tooling tests through bounded commands.
- [x] 5.2 Run `cargo fmt --all --check`, workspace tests, and warnings-denied Clippy through bounded commands.
- [x] 5.3 Run strict OpenSpec validation and refresh graphify output.
- [x] 5.4 Retain supported Ubuntu 24.04 Multipass P1/P2 evidence proving timeout cleanup and no leaked recorder/cgroup/eBPF resources; do not complete from macOS/rootless results alone.
