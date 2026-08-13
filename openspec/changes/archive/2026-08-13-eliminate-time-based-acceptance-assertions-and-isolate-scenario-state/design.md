## Context

P2 acceptance already has sound recorder readiness: bounded machine-readable polling, freshness relative to systemd activation, stale-owner handling, terminal failure, transition logging, and focused diagnostics. Remaining flakiness is elsewhere: scenario-local `sleep` calls precede WAL/epoch/checkpoint/quota/cleanup assertions; several bounded polling loops duplicate weak diagnostics; destructive scenarios return after immediate state checks; and central normalization converts an unexplained nonzero profile result into failure for every selected scenario.

Scenarios intentionally share one profile shell because later checks consume earlier real capture state. Whole-VM reset or per-scenario subprocess isolation would discard this useful continuity and make P2 slower. Isolation therefore means explicit convergence at scenario boundaries, not independent environments.

## Goals / Non-Goals

**Goals:**

- Express asynchronous correctness as bounded eventual conditions with last-observed evidence.
- Distinguish elapsed time required by age policy from convergence after its trigger.
- Ensure destructive scenarios leave stable recorder, systemd, WAL/checkpoint, mount/quota, retention, and cleanup state.
- Attribute profile failure and timeout to completed, active, and never-started scenarios.
- Preserve existing readiness and timeout containment architecture.

**Non-Goals:**

- Retry whole scenarios, loosen assertions, reset VM between scenarios, or increase global deadlines.
- Change recorder lifecycle/status production semantics.
- Replace privileged P1/P2 evidence with rootless simulation.
- Add dependencies or a general-purpose retry framework.

## Decisions

### One small shell convergence library

`scripts/acceptance/lib/wait.sh` provides one generic `wait_until` contract plus thin path/numeric/elapsed-time conveniences only where they remove repeated code. Each condition command returns 0 for success, 1 for transient observation, and 2 for terminal failure; stdout/stderr becomes bounded last-observed text. `wait_until` uses elapsed `SECONDS` deadlines, validates positive timeout/nonnegative interval, never retries past deadline, returns immediately on success/terminal state, and writes atomic compact current/timeout evidence under `ARTIFACT_ROOT`.

Alternative: one specialized helper per WAL/ETL/quota state. Rejected because it duplicates polling policy and hides scenario assertions. Alternative: generic external retry utility. Rejected because no dependency is needed and retries can obscure terminal failure.

### Elapsed time is explicit and separate

Age-policy scenarios call `wait_for_elapsed_time` with description and configured threshold. After workload triggers rotation/retention, `wait_until` observes epoch, segment, or cleanup convergence. Static tests reject bare correctness sleeps in scenario files; fixture processes may use documented long sleeps because sleeping is their workload, not synchronization.

Alternative: increase existing sleeps. Rejected because scheduler, filesystem, systemd, ring-drain, WAL-flush, and ETL latency remain implicit inputs.

### Boundary barriers are scenario-owned conditions

Restart/checkpoint scenarios begin from fresh recorder readiness and end only after stable unit/readiness plus checkpoint consistency. Quota owns isolated loop/mount state and ends after pressure removal, ready status, stopped unit, unmounted filesystem, detached loop, removed filler/control state, and cgroup cleanup. Retention ends after no pause marker, intent, or live trash remains and unit is stopped. Corruption uses immutable copies and verifies source digest. Reboot uses pre-reboot clean stop/handoff and post-reboot fresh readiness plus persisted continuity. Dispatcher records barrier description as active wait evidence.

Alternative: reboot/reset VM after each destructive scenario. Rejected due cost and because explicit boundaries prove cleanup behavior directly.

### Atomic scenario execution ledger is source of attribution

Dispatcher atomically writes `scenario-state.json` with ordered selection, current scenario, per-scenario status, completed list, and failed scenario. It marks current before invocation and passed only after function and postconditions return. Profile EXIT handling finalizes an active scenario as failed on nonzero exit. If gate termination prevents EXIT cleanup, runner combines last durable current scenario with timeout phase evidence.

Runner prefers valid ledger over legacy check inference. Completed scenarios remain passed; explicit/current failed scenario is failed; later untouched scenarios remain `not_checked`. Non-scenario infrastructure failure can fail profile while leaving all unexecuted scenarios `not_checked`. Legacy checks remain report payload and compatibility evidence, not coarse attribution authority.

Alternative: reconstruct from stdout/current phase alone. Rejected because timeout truncation and phase names are not durable scenario history.

### Recorder readiness remains specialized

`wait_for_recorder_ready` remains unchanged in authority: machine-readable status, lifecycle states, stale-owner option, status freshness versus latest systemd activation, bounded status commands, terminal failure, transition log, and diagnostics. Generic waits compose around it for broader unit/checkpoint/resource convergence; they do not reinterpret recorder state.

### Final in-process ETL must not re-acquire the live WAL lock

Strict stop convergence surfaced a production bug: the continuous recorder's final in-process ETL ran while the active epoch's flock was still held by the same process (the writer outlives `shutdown` until `drop(service)`). flock(2) treats a second fd for the same file in the same process as a conflicting owner, so the non-blocking re-acquire inside `process_recording_wal` failed with `WalError::RecordingLockHeld` and every graceful stop with committed segments errored (exit 3, "recording WAL lock is already held"). Fix: `GroupCommitWalWriter::release_recording_lock` / `RecordingIngest::release_wal_lock` invoked after graceful shutdown, before final publication. Verified A/B on Ubuntu 24.04: pre-fix stop failed with the lock error; post-fix stop completed and the active epoch's session was published.

### Capture liveness is an observable, not a sleep

A strict bounded wait replaced `sleep 1` before the legacy eBPF record workload, but `recording.json` appears before the eBPF ring is attached, so a workload racing that window can finish before capture is live (observed post-reboot: record `completed` with 0 received records). Legacy metadata stays `starting` for the whole run, and the legacy writer defers commits to record end (300s segment age; captured events are far below the 4MB group-commit threshold), so neither status nor mid-record commit is observable. Fix: drive bounded probes across the record window (duration 20s) so at least one lands after attach, then assert the final result committed capture evidence (`physical_wal_bytes > 0`). Verified in the failing post-reboot environment: the record captures and commits records, and the final assertion passes.

### Final-stop evidence must live in one sealed epoch

Graceful-stop convergence exposed two timing truths for one-second epochs. First, the first committed record lands while the ring still drains tail events, so an early stop drops the tail request; the stop therefore waits for a stable committed-record sample. Second, a workload straddling an epoch boundary splits across two recordings, so the published (last-with-segments) recording can carry a partial session; the final workload runs inside a freshly observed epoch and the stop waits for that epoch to roll (seal) before stopping. A primer workload keeps the epoch cadence live, because the recorder skips empty epoch rolls (no commit boundary) and a fresh-window wait would otherwise time out. The primer must itself be drained before the fresh window, or its ring-tail leaks an empty connection into the measured epoch and breaks daemon-vs-one-shot session equivalence. Verified end-to-end in the VM: the published recording carries the whole workload (operations >= 3, executable >= 2, integrity valid), daemon and one-shot sessions are identical, and replay completes.

### Whole-scenario retry is not recovery

Scenarios run once. Eventual waits retry only transient observations within explicit deadlines. Terminal states and deterministic assertion failures stop immediately. This preserves destructive-boundary evidence and avoids passing by erasing a first failure.

## Risks / Trade-offs

- [Condition output grows or leaks payload] → bound last observation, use metadata/counters/paths only, and retain full diagnostics only on failure.
- [Shell `set -e` bypasses ledger update] → profile EXIT finalizer atomically marks durable current scenario failed; runner also fails active scenario on nonzero/timeout.
- [A condition classifies terminal state as transient] → keep terminal return distinct and add focused tests; recorder readiness retains existing domain-specific terminal logic.
- [Shared runtime still permits undeclared coupling] → explicit scenario order remains, but destructive scenarios prove postconditions and static tests enforce migration convention.
- [Fresh privileged runs are expensive] → run rootless tooling first, then multiple `--no-reuse` P2 runs only after portable checks pass.

## Migration Plan

Add and test wait/ledger primitives, source them from profiles, migrate every async sleep/poll, add destructive barriers, then switch runner attribution to ledger with legacy fallback. Validate rootless and repository layers before fresh supported Multipass P1/P2 runs. Rollback restores prior scripts/report schema; no production or persisted user data migration exists.

## Open Questions

None blocking implementation.
