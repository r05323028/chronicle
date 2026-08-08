## Context

Validation currently has bounded recorder-readiness and several bounded-attempt loops, but shared command runners, scenario dispatch, acceptance profile execution, Multipass operations, and service commands lack interruptible outer deadlines. A loop deadline is ineffective when one command inside it blocks. Existing profile EXIT traps own cleanup and report generation, so timeout handling must preserve those paths.

## Goals / Non-Goals

**Goals:**

- Enforce command → readiness/poll → scenario → gate deadlines with distinct configuration.
- Terminate spawned process trees with TERM, configurable grace, then KILL.
- Preserve stdout/stderr, cleanup traps, partial evidence, and exit code 124 for timeout.
- Make scenario/gate timeout identity and last-known runtime state explicit in reports.
- Keep targeted validation cheaper than full privileged gates.

**Non-Goals:**

- Change P1/P2 runtime assertions or evidence authority.
- Replace systemd, Multipass, or existing acceptance profiles.
- Add a daemon, dependency, generic job scheduler, or arbitrary retry framework.
- Claim privileged Linux behavior from macOS/rootless tests.

## Decisions

### Standard-library process-group wrapper

`scripts/run-with-timeout.sh` uses Python standard library process groups because macOS lacks GNU `timeout` by default while supported Ubuntu includes Python. It spawns command in a new session, preserves inherited stdio, returns original status, and on deadline sends TERM followed by KILL after `CHRONICLE_TIMEOUT_GRACE_SECONDS` (default 5). Linux enables child-subreaper ownership and continuously tracks descendants so double-fork/session-detached validation children remain contained; macOS uses process-table tracking for existing child trees. Invalid durations or missing commands exit 2.

Alternative: GNU `timeout`. Rejected because macOS developer workflow would require coreutils and nested child cleanup remains less explicit.

### Distinct hierarchical budgets

Defaults are command 900 seconds for validation/build steps, readiness command/readiness 10/180 seconds, service command 30 seconds, ordinary scenario 300 seconds, long scenario 600 seconds, cargo-heavy scenario 900 seconds, acceptance cleanup grace 180 seconds, validation acceptance profile 3300 seconds, and gate 3600 seconds. Multipass guest and remote deadlines derive below host profile deadline; status/VM-readiness, transfer, and bootstrap use 120, 300, and 900 seconds. Each environment variable validates as a positive integer. Shorter inner boundaries normally fail first; gate remains final containment.

Alternative: one global timeout. Rejected because it obscures failure phase and allows readiness or one scenario to consume the whole gate.

### Scenario watchdog preserves profile ownership

Scenario functions remain in the profile shell so existing shared variables and sequencing semantics are unchanged. Dispatcher starts a per-scenario watchdog in the profile process group. On expiry it atomically writes scenario timeout evidence, signals the profile, independently TERM/KILLs foreground descendants after command grace so deferred Bash traps can run, allows diagnostics and cleanup/report traps to run for separate 180-second cleanup grace, then KILLs only remaining profile descendants/process. Profiles execute beneath gate wrapper, ensuring a dedicated process group even for direct invocation.

Alternative: execute each scenario in a child shell. Rejected because scenario functions intentionally mutate profile state used by later scenarios and final reports.

### Bound commands inside polling loops

Recorder status, `systemctl`, and `journalctl` calls receive short command deadlines in addition to poll deadlines. Service transition helpers use elapsed deadlines and retain last observed state. Existing finite scenario loops may remain where already deterministic, but every command they call is bounded and scenario deadline remains outer containment.

### Timeout evidence is additive

Wrapper and dispatcher write small JSON timeout records before signaling. Central acceptance report aggregates these records into `timeouts` entries containing layer, scenario/phase, configured duration, and diagnostics paths. Existing readiness diagnostics remain authoritative for recorder/unit/process state and are collected on readiness or scenario failure where available. Timeout reports cannot be reused as passing evidence.

## Risks / Trade-offs

- [Risk] TERM cleanup exceeds cleanup grace and receives KILL. → Keep profile cleanup bounded, configure `CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS` separately from command grace, retain pre-signal timeout metadata.
- [Risk] Descendants double-fork or create sessions. → Linux subreaper ownership plus continuous tracking retains them; macOS process-table tracking covers existing validation trees without claiming arbitrary daemon containment.
- [Risk] Low timeout creates false failures on slow hosts. → Use conservative defaults and separate documented overrides; do not raise readiness to gate scale.
- [Risk] Timeout during artifact transfer leaves partial evidence. → Mark run failed/incomplete and never reuse it.
- [Risk] macOS cannot prove Linux systemd/eBPF cleanup. → Run rootless orchestration tests locally and retain supported Ubuntu Multipass evidence before completing privileged tasks.

## Migration Plan

Add wrapper and rootless tests first, then route validation, acceptance, scenarios, service commands, and Multipass through it. Update docs and targeted selection. Rollback removes wrapper calls and timeout fields; no production data migration exists.

## Open Questions

None blocking implementation.
