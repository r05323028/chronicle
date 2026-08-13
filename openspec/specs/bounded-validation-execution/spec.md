## Purpose

Hard deadline and timeout evidence requirements for repository validation and acceptance workflows.

## Requirements

### Requirement: Reusable process-tree timeout wrapper

Repository SHALL provide `scripts/run-with-timeout.sh <seconds> <command> [arguments...]`. Normal completion SHALL return command exit code and preserve stdout/stderr. Deadline expiry SHALL return 124 after sending SIGTERM to spawned process group, waiting configurable grace, then sending SIGKILL to remaining descendants. Invalid timeout, invalid grace, or missing command SHALL fail with clear usage error and exit 2.

#### Scenario: Command completes

- **WHEN** wrapped command exits before deadline
- **THEN** wrapper preserves output and original exit code

#### Scenario: Command tree exceeds deadline

- **WHEN** command or spawned child remains alive past deadline
- **THEN** wrapper sends TERM, escalates to KILL after grace, reaps command, leaves no descendant alive, and exits 124

### Requirement: Validation uses hierarchical deadlines

Validation SHALL enforce distinct configurable command, readiness/poll, scenario, and gate deadlines. Defaults SHALL keep readiness shorter than scenario and scenario shorter than gate. A command within a bounded poll SHALL have its own deadline so blocked command cannot defeat poll deadline. Gate timeout SHALL remain final safety boundary for `gate p1`, `gate p2`, and release acceptance.

#### Scenario: Inner readiness fails first

- **WHEN** recorder command responds but recorder never reaches ready
- **THEN** readiness deadline fails with diagnostics before scenario or gate deadline

#### Scenario: Scenario hangs outside readiness

- **WHEN** selected scenario stops making progress
- **THEN** scenario deadline identifies scenario, terminates its process tree, runs profile cleanup, and fails gate

#### Scenario: Gate exceeds aggregate budget

- **WHEN** inner boundaries do not end full validation before gate deadline
- **THEN** gate process tree is terminated and acceptance returns bounded failed evidence

### Requirement: Service and external operations are interruptible

Meaningful systemd lifecycle, Multipass, remote execution, build/test, Python acceptance-driver, and daemon execution layers SHALL have hard deadlines. Service transition polls SHALL report last observed state and fail explicitly for non-convergence or terminal states including failed, deactivating, activating, and auto-restarting.

#### Scenario: Service command blocks

- **WHEN** systemctl command does not return
- **THEN** command deadline interrupts it and scenario failure retains unit diagnostics

#### Scenario: Multipass operation blocks

- **WHEN** VM lifecycle, transfer, bootstrap, or remote command blocks
- **THEN** configured operation or gate deadline ends it with nonzero status and partial evidence is not reusable

### Requirement: Timeout failures are first-class evidence

Timeout evidence SHALL identify timeout layer, configured duration, command or scenario, current phase, and available recorder/unit/process diagnostics. Timeout SHALL produce failed status and deterministic nonzero exit; exit 124 SHALL identify direct deadline expiry. Existing readiness diagnostics SHALL remain bounded and be retained when relevant.

#### Scenario: Scenario timeout report

- **WHEN** `p2/checkpoint-kill-restart` exceeds scenario deadline
- **THEN** output names scenario and duration, report contains scenario timeout entry, and readiness/service/process evidence remains available

#### Scenario: Successful evidence reuse

- **WHEN** candidate report contains any timeout or incomplete artifact manifest
- **THEN** candidate is rejected for reuse

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
