## Why

P2 acceptance still uses fixed delays before observing asynchronous WAL, ETL, quota, retention, and recorder transitions, so scheduler and filesystem timing can produce false failures. Destructive scenarios also share one runtime without explicit convergence boundaries, while a single nonzero profile result can currently classify every selected scenario as failed instead of preserving completed and never-started status.

## What Changes

- Replace fixed-delay correctness synchronization with reusable bounded eventual-condition waits that retain last-observed state and focused timeout diagnostics.
- Preserve elapsed-time waits only where configured age is behavior under test, then separately wait for resulting state convergence.
- Add explicit precondition and postcondition barriers for restart, checkpoint, quota, retention, corruption, reboot, and cleanup scenarios without resetting the VM between scenarios.
- Persist atomic machine-readable scenario execution state so reports classify each selected scenario as `passed`, `failed`, or `not_checked`, including scenario and gate timeouts.
- Keep existing recorder readiness polling, freshness, stale-owner, terminal-state, and bounded-diagnostic contract intact and compose it with broader convergence helpers.
- Add rootless acceptance-infrastructure tests for wait success/failure diagnostics, polling bounds, sleep classification, scenario barriers, and failure/timeout attribution.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `bounded-validation-execution`: Require condition-based convergence waits, explicit elapsed-time classification, compact last-observation evidence, and no unbounded acceptance polling.
- `layered-validation`: Require scenario-level durable execution state and deterministic `passed`/`failed`/`not_checked` attribution while preserving recorder readiness semantics.
- `p2-completion`: Require destructive P2 scenarios to enter and leave explicit converged state and prohibit passing while transitional scenario-owned state remains.

## Impact

Affected code is limited to acceptance shell helpers/scenarios, dispatcher, unified runner/report normalization, Multipass evidence transfer, acceptance tooling tests, and OpenSpec validation contracts. Recorder production lifecycle/status behavior, acceptance scenario coverage, supported environment, dependencies, and global timeout scale do not change.
