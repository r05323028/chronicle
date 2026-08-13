## 1. Convergence primitives

- [x] 1.1 Add shared bounded condition and explicit elapsed-time helpers with validated deadlines, terminal/transient status, compact last-observation evidence, and timeout diagnostics.
- [x] 1.2 Source shared convergence helpers from P1/P2 profiles and replace duplicated path/unit/process polling where practical without changing recorder readiness authority.
- [x] 1.3 Add rootless helper tests for immediate success, eventual success, deterministic timeout, terminal failure, last-observed state, bounded loops, and elapsed-time classification.

## 2. Scenario synchronization and isolation

- [x] 2.1 Migrate segment/epoch/WAL growth and incremental publication assertions from fixed sleeps to bounded observable conditions while preserving explicit age-threshold behavior.
- [x] 2.2 Migrate recorder processing, checkpoint kill/restart, quota entry/recovery, retention interruption/recovery, replay target, and graceful stop synchronization to bounded conditions.
- [x] 2.3 Add explicit destructive-scenario preconditions/postconditions for fresh recorder readiness, stable restart ownership, checkpoint consistency, removed control files, quota resource cleanup, retention convergence, corruption immutability, and reboot handoff/recovery.
- [x] 2.4 Add maintainable static/meta-tests rejecting undocumented bare synchronization sleeps and unbounded acceptance polling while allowing fixture sleeper processes and explicit elapsed-time behavior.
- [x] 2.5 Fix final in-process ETL re-acquiring the active epoch's WAL flock (self-deadlock surfaced by strict stop convergence), with same-process re-acquire regression test and VM A/B evidence.

## 3. Scenario-level attribution

- [x] 3.1 Persist atomic scenario execution state from dispatcher, marking selected/current/completed/failed/per-scenario status around each scenario and timeout.
- [x] 3.2 Load and validate execution state in central report/runner, using legacy checks only as fallback and preserving completed/failed/not_checked classification on nonzero exit.
- [x] 3.3 Integrate scenario state with profile/gate timeout evidence and Multipass pre/post-reboot transfer so active scenario is failed and never-started scenarios remain not_checked.
- [x] 3.4 Add rootless tests for A-pass/B-fail/remaining-not_checked, active-scenario timeout, infrastructure failure before dispatch, malformed ledger fail-closed behavior, and recorder readiness integration.

## 4. Validation and retained evidence

- [x] 4.1 Run acceptance tooling tests, timeout/readiness tests, normal rootless layered validation, and strict OpenSpec validation.
- [x] 4.2 Run bounded `cargo fmt --all --check`, workspace all-feature check, warnings-denied Clippy, and workspace all-feature tests.
- [x] 4.3 Refresh graphify graph after code changes and run focused independent review of wait semantics, barriers, attribution, diagnostics, and coverage preservation.
- [x] 4.4 Run fresh supported Ubuntu 24.04 Multipass P1 and multiple P2 executions with reuse disabled; inspect and fix every failure instead of pass-seeking retries.
- [x] 4.5 Reconcile final implementation with proposal/design/spec deltas, record exact validation/evidence results, and mark only evidence-proven tasks complete.
