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

Acceptance polling SHALL use elapsed deadlines, return immediately on success, fail immediately on terminal state when appropriate, and report last observation on failure. Fixed sleeps SHALL not be sole correctness condition for service readiness, recorder readiness, checkpoint availability, or process termination.

#### Scenario: Recorder transitions to ready

- **WHEN** status progresses starting to recovering to loading_ebpf to ready
- **THEN** transitions are logged and poll returns immediately at ready

#### Scenario: Recorder status command hangs

- **WHEN** one recorder-status invocation blocks
- **THEN** command deadline interrupts invocation and readiness remains bounded by configured readiness deadline
