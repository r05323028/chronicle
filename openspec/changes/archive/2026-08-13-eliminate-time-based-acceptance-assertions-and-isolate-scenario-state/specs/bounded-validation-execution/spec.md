## MODIFIED Requirements

### Requirement: Polling observes deadlines and terminal state

Acceptance polling SHALL use elapsed deadlines, return immediately on success, fail immediately on terminal state when appropriate, and report the expected condition, configured timeout, and bounded last observation on failure. A command within a poll SHALL remain bounded. Fixed sleeps SHALL NOT synchronize service readiness, recorder readiness, WAL/epoch growth, checkpoint/publication progress, quota transition, retention cleanup, process termination, or resource appearance/disappearance. Elapsed-time waiting is permitted only when configured time passage is behavior under test; after the trigger, resulting asynchronous state SHALL still converge through a bounded condition wait. Acceptance scenario polling SHALL NOT retry forever.

#### Scenario: Recorder transitions to ready

- **WHEN** status progresses starting to recovering to loading_ebpf to ready
- **THEN** transitions are logged and poll returns immediately at ready

#### Scenario: Recorder status command hangs

- **WHEN** one recorder-status invocation blocks
- **THEN** command deadline interrupts invocation and readiness remains bounded by configured readiness deadline

#### Scenario: Eventual condition converges

- **WHEN** an asynchronous WAL, checkpoint, publication, quota, cleanup, systemd, or resource condition becomes true before its deadline
- **THEN** acceptance returns immediately and evaluates the original semantic assertion without waiting a fixed correctness delay

#### Scenario: Eventual condition does not converge

- **WHEN** asynchronous condition remains transient through its deadline
- **THEN** acceptance fails with condition description, configured timeout, last observed state, active scenario, and available focused diagnostics

#### Scenario: Age threshold triggers asynchronous transition

- **WHEN** scenario verifies configured segment, epoch, or retention age
- **THEN** elapsed threshold wait is explicit, workload or cleanup is triggered, and separate bounded polling proves resulting transition

#### Scenario: Terminal observation

- **WHEN** condition command reports terminal failure before deadline
- **THEN** polling stops immediately, records last terminal observation, and does not hide failure behind generic retries
