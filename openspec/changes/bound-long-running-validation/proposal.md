## Why

Acceptance and validation contain bounded recorder-readiness polling, but scenario dispatch, service lifecycle commands, Multipass operations, and whole gates can still block indefinitely. Coding agents need layered deadlines that fail with deterministic status, cleanup, and retained diagnostics instead of hanging.

## What Changes

- Add a shell-native process-group timeout wrapper with TERM/KILL escalation and focused tests.
- Add distinct configurable command, readiness, scenario, and gate timeout boundaries.
- Make scenario timeouts first-class acceptance evidence while preserving cleanup and diagnostics.
- Bound service lifecycle, Multipass, privileged driver, and meaningful validation execution layers.
- Replace weak polling/fixed-delay waits with deadline-based waits where acceptance correctness depends on convergence.
- Document fast-to-privileged agent workflow and bounded command usage.

## Capabilities

### New Capabilities

- `bounded-validation-execution`: Defines hierarchical deadlines, process-group termination, timeout evidence, cleanup, and deterministic timeout status.

### Modified Capabilities

- `layered-validation`: Requires validation modes and privileged gates to use bounded execution while preserving targeted-first agent workflow and existing P1/P2 semantics.

## Impact

Affected areas include `scripts/run-with-timeout.sh`, validation and acceptance dispatch, recorder readiness, service lifecycle scenarios, Multipass wrappers, acceptance reports/evidence, shell/Python tests, `AGENTS.md`, and validation documentation. No production data format, capture semantics, or new dependency is introduced.
