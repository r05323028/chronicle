## MODIFIED Requirements

### Requirement: Privileged recorder readiness

P2 privileged acceptance SHALL consume an explicit machine-readable recorder state distinguishing `starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, and `failed`. Workload admission SHALL wait for `ready`, which SHALL imply recovery complete, writable WAL, reachable status control plane, initialized capture, required eBPF attachment, and no fatal startup error. Polling SHALL use configurable timeout and interval, print state transitions, tolerate temporary status unavailability, stop on terminal failure, reject status older than latest systemd activation, and retain bounded diagnostics on timeout or failure. General acceptance convergence helpers SHALL compose with this contract and SHALL NOT replace or weaken its freshness, stale-owner, lifecycle, or terminal-state semantics.

#### Scenario: Startup transition

- **WHEN** recorder status transitions from starting through recovery and eBPF loading
- **THEN** acceptance prints each transition and admits workload only at ready

#### Scenario: Terminal failure

- **WHEN** status reports failed or stale ownership
- **THEN** acceptance stops polling immediately and retains readiness diagnostics

#### Scenario: Recovery restart

- **WHEN** recorder restarts after crash or VM reboot with WAL/checkpoint state
- **THEN** polling waits for fresh ready state rather than trusting systemd active or stale metadata

#### Scenario: General convergence integration

- **WHEN** destructive scenario requires both stable systemd state and recorder readiness
- **THEN** general bounded barriers first observe unit convergence and then use existing recorder-readiness contract for fresh ready state

## ADDED Requirements

### Requirement: Acceptance reports preserve scenario execution state

Unified acceptance execution SHALL atomically persist ordered selected scenarios, current scenario, completed scenarios, failed scenario, and per-scenario `passed`, `failed`, or `not_checked` status under run artifacts. Central reporting SHALL prefer valid execution state over coarse profile exit inference and remain deterministic and machine-readable. A profile-level failure SHALL NOT classify scenarios never started as failed.

#### Scenario: Middle scenario fails

- **WHEN** scenario A passes, scenario B fails, and scenarios C and D are never started
- **THEN** central report records A `passed`, B `failed`, C and D `not_checked`, and profile status `failed`

#### Scenario: Active scenario times out

- **WHEN** scenario deadline or gate deadline expires while scenario B is current after A completed
- **THEN** timeout evidence and execution state attribute B `failed`, preserve A `passed`, and leave later scenarios `not_checked`

#### Scenario: Infrastructure fails before scenario start

- **WHEN** profile setup fails before dispatcher starts any selected scenario
- **THEN** profile status is `failed` while every selected scenario remains `not_checked`
