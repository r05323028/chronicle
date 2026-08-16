# Operations Guide

## Safety and scope

WAL and published sessions can contain production headers and bodies, including credentials and personal data. Chronicle uses private Unix modes and safe default output, but does **not** provide encryption at rest, secret discovery, comprehensive redaction, or tenant isolation. Inspect and reports omit bodies and arbitrary header values.

Live recording is bounded plaintext TCP capture, not always-on capture. It supports Linux 6.1+, cgroup v2, readable BTF, x86_64/aarch64, and required eBPF hooks/capabilities. macOS and ordinary CI use rootless fixtures, ETL, inspect, replay, and eBPF compile checks. Runtime evidence comes only from the opt-in privileged profile.

## Quick start

```bash
chronicle record --name checkout --duration 5m -- ./bin/my-app
chronicle list
chronicle inspect latest
chronicle replay latest -- ./bin/my-app
```

`record -- COMMAND...` starts application only after capture attachment. Send representative traffic while recording runs; Chronicle rolls over bounded epochs without detaching capture, then stops on application exit, signal, explicit deadline, or fatal failure. Finalization publishes immutable epoch sessions and one parent summary under one data-domain lock. `replay RECORDING -- COMMAND...` plans first, starts application in supervised cgroup, discovers unique owned loopback listener, then reuses existing replay executor. Writes still require `--allow-write`.

Recording references are `latest`, `rec_<uuid>`, bare UUID, or exact name. Global `--data-dir` overrides configured `data_dir`, then `CHRONICLE_DATA_DIR`, then platform default. Human output suggests next intent command; `--format json` emits one versioned object.

## Flow and bounds

```text
selected cgroup subtree -> eBPF capture -> 4096-event queue -> segmented WAL
  -> recovery-authoritative commit boundary -> ETL -> HTTP/1.1 session
  -> atomic filesystem publication -> inspect or authorized loopback replay
```

Defaults and hard limits:

- whole-recording deadline: omitted by default; explicit positive durations such as `10m` and `24h` are checked for runtime-clock overflow;
- epoch WAL: 4 GiB physical ceiling by default, including segment headers, frames, and temporary segment files; lower values are allowed down to segment size; each epoch is bounded independently; the parent recording has no total-WAL cap;
- segment: 256 MiB default, `16 MiB..=4 GiB`;
- group commit: 4 MiB unsynced data or 10 ms, plus rotation, flush, and shutdown;
- loss sampling: complete per-CPU sample at least every 100 ms, using actual monotonic timestamps, plus mandatory final sample;
- eBPF ring: 8 MiB; payload: 16 KiB per event, at most four continuations;
- HTTP/reconstruction limits remain bounded at 8 MiB per connection/body and 10,000 canonical operations.

A delayed poll records its actual expanded interval. Zero counter delta advances the boundary without WAL growth. Positive, reset, regression, clock-mismatch, and final-sample evidence remains typed and unattributed.

A group writes data frames, one in-WAL `CommitMarker`, flushes, and calls one `fdatasync`. Runtime acknowledgement exists only after sync returns. Recovery trusts a marker only after validating frame references, contiguous sequences, same-segment membership, cumulative counts, identities, CRCs, and exact batch digest. Recovery can therefore recognize persisted durability without claiming any caller observed its acknowledgement. There is no external watermark file or second metadata-watermark sync cycle.

Recording status describes finalization: `starting`, `recording`, `completed`, `failed`, or recovery-produced `aborted`. Shutdown reason is separate: `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`; legacy `wal_size_limit` remains readable but new epoch thresholds request rollover rather than parent termination.

Only a verified incomplete final WAL frame may be repaired. Middle corruption, invalid marker references/digests, and unsupported versions fail closed. Complete frames after the final authoritative marker remain uncertainty and are excluded from ETL.

## Cgroup selection

Use exactly one advanced selector for already-running workload:

```bash
chronicle --data-dir /var/lib/chronicle record --duration 5m --pid 1234
chronicle --data-dir /var/lib/chronicle record --duration 5m \
  --cgroup /sys/fs/cgroup/my-workload --allow-shared-cgroup
```

Selector mode never terminates selected workload. It uses the same stable parent ID, bounded epoch WAL → ETL/publication flow, parent catalog, and exact-lock transaction as command mode. `capture_readiness` may remain ready while predecessor ETL reports lag or waits for continuation.

The selected scope is the exact cgroup node plus descendants, called the **selected subtree**. `cgroup.procs` is read directly from the selected node. Numeric PIDs are resolved to host-visible thread-group IDs and deduplicated into the **direct cgroup TGID set**; its cardinality is **direct TGID count**. This is not POSIX PGID, session ID, thread count, container ID, or descendant membership. Descendant directories are counted separately as **descendant cgroup count**.

A dedicated leaf with direct TGID count zero or one and no descendants needs no acknowledgement. Explicit cgroups with more direct TGIDs or descendants require `--allow-shared-cgroup`:

```bash
chronicle --data-dir /var/lib/chronicle record --duration 5m \
  --cgroup /sys/fs/cgroup/team --allow-shared-cgroup
```

The flag is invalid with `--pid` and cannot override `/`, `/system.slice`, `/user.slice`, `/machine.slice`, Chronicle containment anywhere in the subtree, namespace ambiguity, movement, or unreadable enumeration. PID selection rejects unrelated direct TGIDs and never widens to an ancestor. Output includes only canonical path/ID, direct TGID count, descendant cgroup count, selected-subtree scope, and acknowledgement state.

## Record, recover, and publish

Public `record` writes private parent intent/run state and bounded epoch WAL under `recordings/<recording-id>/`. Epoch age/bytes trigger a checksummed successor transition; they do not terminate the parent. A missing deadline means capture continues until source completion, explicit stop, or fatal safety failure. If successor reservation, WAL handoff, outcome, or topology activation cannot be proven, capture stops visibly at the last authoritative boundary; it never drops or deletes protected evidence.

After capture, public command/PID/cgroup modes process only recovery-authoritative committed prefix, publish one immutable Canonical Session v1 per finalized epoch with parent/epoch provenance, write advisory ETL checkpoint, and complete parent catalog entry before releasing domain lock. Recoverable failures retry without recapture:

```bash
chronicle --data-dir /var/lib/chronicle record --retry RECORDING
```

Hidden `chronicle internal etl --wal-dir DIR --output DIR` remains deployment/compatibility mechanism, not normal user workflow. Rerunning unchanged input in same root reports `already_processed`; another output root is allowed. No `output-binding.json` is used.

## Inspect and replay

```bash
chronicle --data-dir /var/lib/chronicle inspect latest
chronicle --data-dir /var/lib/chronicle replay latest -- ./bin/my-app
chronicle --data-dir /var/lib/chronicle replay latest --epoch 0 \
  --target http://127.0.0.1:18080 --allow-host 127.0.0.1 \
  --allow-read --execute
```

Canonical v1 preserves connection/operation completeness, loss-window provenance, WAL ranges, integrity, and replayability. Complete supported operations outside loss windows may execute while degraded siblings remain visible as not attempted. Command mode infers only unique owned loopback listener and grants execution/read/host/target needed for that spawned application; writes remain explicit. Explicit `--target` mode is dry-run by default, requires exact `--allow-host`, effect authorization, and `--execute`, and never supervises or terminates target. Neither mode contacts recorded destinations, follows redirects, expands DNS, retries, uses proxy/TLS, or replays captured credentials.

Replay outcomes are `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`. Exit codes: 0 for successful record/ETL/inspect, dry-run, and completed replay; 3 for data/persistence/finalization errors; 4 for policy, invalid-session, unsupported live preflight, or required doctor probe failure; 5 for transport stop; 6 for executed verification failure.

## Doctor and test profiles

```bash
chronicle --format json --data-dir /var/lib/chronicle doctor
```

Doctor is diagnostic only. It performs independent non-destructive probes and never starts capture, creates public data directory, repairs WAL, publishes sessions, or sends replay traffic. It reports resolved data-directory path/source and existing or prospective access without destructive write test. Aggregate status is `unsupported` for required unsupported probes, `not_checked` for required undecidable probes, `supported_with_warnings` for warnings/optional gaps, otherwise `supported`.

Validation entry point:

```bash
./scripts/validate.sh fast
./scripts/validate.sh targeted --changed-since origin/main
./scripts/acceptance.sh --profile live-capture --executor multipass
./scripts/acceptance.sh --profile recorder --executor multipass
./scripts/acceptance.sh --profile all --executor multipass
./scripts/acceptance.sh --profile recorder --executor multipass --release
./scripts/acceptance.sh --profile recorder --executor multipass --no-reuse
./scripts/validate.sh release --reuse-evidence
```

`validation/groups.toml` maps paths to focused groups. Targeted output lists every selected/skipped group and reason. Documentation-only changes select no privileged gate; ETL changes select ETL/checkpoint checks; WAL/recovery changes select focused WAL/recorder checks; eBPF changes select build/decoder checks and a minimal privileged smoke. Explicit gates and release retain all existing live-capture/recorder checks.

Fast and targeted modes use local checks and default to failure-only artifacts. Recorder changes should use `fast`, targeted changed-path/recorder tests, then a bounded lifecycle scenario before a complete privileged gate. Agents must not start an unbounded foreground recorder.

Timeout hierarchy is independent: validation/build command 900s, readiness command/readiness 10s/180s, systemd command 30s, ordinary scenario 300s (600s quota/retention, 900s cargo-heavy), cleanup grace 180s, acceptance profile 3300s under `validate.sh`, complete gate 3600s. `scripts/run-with-timeout.sh` returns 124 after process-tree TERM/grace/KILL. Configure `CHRONICLE_TIMEOUT_GRACE_SECONDS`, `CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS`, `CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS`, and gate timeout variables without collapsing inner and outer budgets. Multipass exposes guest, status, VM-readiness, transfer, bootstrap, and remote timeout variables named in `AGENTS.md`; remote deadline exceeds guest deadline plus cleanup grace while remaining below host profile deadline. Reboot recovery retains differing pre/post boot IDs as proof restart occurred. Timeout evidence names layer, phase/scenario, duration, and retained diagnostics.

Successful gate artifacts contain only `summary.json`, `environment.json`, `manifest.json`, and `checksums.txt`; failure artifacts contain a bounded reproducer and failure-listed WAL/session data. Never upload `target/`, Cargo caches, VM filesystems, or complete successful WAL directories. Retention guidance: no upload or 1–3 days for fast/targeted successes, about 7 days for gate successes, about 14 days for gate failures, and long-term for release evidence.

Privileged caches stay in reused Multipass VM paths `/home/ubuntu/chronicle-target`, `/home/ubuntu/chronicle-ebpf-target`, and `/home/ubuntu/.cargo`; the unified executor transfers an immutable source snapshot so dirty development changes are tested, not discarded. Evidence lives under `target/validation-evidence/acceptance/<profile>/<fingerprint>/<run-id>/`. Reuse matches acceptance-sensitive source fingerprint, scenario coverage, report schema, manifest integrity, and compatible executor/environment. Commit/tree SHA is provenance only, never evidence identity; release mode additionally requires a clean, identifiable current checkout that remains unchanged through validation.

`scripts/acceptance.sh` is the only user-facing acceptance entrypoint; no compatibility wrappers are retained. `scripts/acceptance/scenarios.toml` defines scenario/profile coverage; `--profile all` runs the recorder superset once because it includes the live-capture scenarios. `runner.py` owns selection/fingerprint/report/reuse, and `lib/multipass.sh` owns VM lifecycle, snapshot transfer, reboot, and artifacts. `lib/scenario-dispatch.sh` loads ordered scenario modules from `lib/scenarios/`; profile scripts provide runtime setup/cleanup only. Scenario implementations have one owner: `lib/scenarios/shared/<scenario>.sh` provides `scenario_<name>()` when behavior is identical across profiles, otherwise `lib/scenarios/live-capture/<scenario>.sh` or `lib/scenarios/recorder/<scenario>.sh` provides `scenario_live_capture_<name>()`/`scenario_recorder_<name>()`. Run `python3 scripts/tests/acceptance/test_runner.py`, `python3 scripts/tests/validation/test_layered_validation.py`, and `bash scripts/tests/acceptance/test-recorder-readiness.sh` for tooling coverage.

Recorder readiness contract: `chronicle internal recorder-status` reports machine-readable `state` values (`starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, `failed`) alongside liveness, lifecycle, capture readiness, processing readiness, and health. Workload admission waits for `state=ready`; systemd `active` alone is insufficient. Polling prints state transitions, tolerates temporary status-command failures, stops immediately on terminal failure, and uses configurable timeout/interval values (`CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_INTERVAL_SECONDS`).

Previous blocker root cause: the recorder treated `processing_readiness=not_ready` as capture-not-ready and waited forever even though capture was safe to admit workload; status-command errors were also suppressed and startup/recovery, stale-owner, and terminal failure collapsed into one generic timeout. Recovery can spend longer than the old 60-second loop validating a large incremental checkpoint. Failure diagnostics now retain status/service/journal, kernel/BTF/cgroup/eBPF, WAL listing, checkpoint metadata, process, and disk evidence without Cargo caches or VM data. Multipass stops stale recorder units and fixes artifact ownership before transfer.

The profile uses local upstream/replay targets and dedicated/shared cgroups. It verifies capture, sampling, one-marker/one-sync WAL behavior, crash/recovery authority, hard-cap queue discard, ETL idempotency across roots, cgroup safety, signal/limit finalization, mixed replay, and original-destination isolation. Unsupported hosts must report skipped/not-checked rather than passing runtime coverage.

## Legacy 0.1.x syntax migration

Hidden aliases remain through 0.1.x and emit deprecation warnings; removal occurs at 0.2:

| Deprecated form | Replacement |
| --- | --- |
| `chronicle recorder --config FILE` | `chronicle internal recorder --config FILE` |
| `chronicle recorder-status --state-root DIR` | `chronicle internal recorder-status --state-root DIR` |
| `chronicle etl --wal-dir WAL --output ROOT` | `chronicle internal etl --wal-dir WAL --output ROOT` |
| `chronicle record --source fixture ...` | `chronicle internal record-fixture --input FILE --root ROOT` |
| `chronicle record --source ebpf ...` | `chronicle record -- COMMAND...`, `--pid`, or `--cgroup` |
| `chronicle inspect SESSION --root ROOT` | `chronicle inspect RECORDING` |
| `chronicle replay SESSION --root ROOT ...` | `chronicle replay RECORDING -- COMMAND...` or explicit `--target` mode |

Docker and Kubernetes packaging are future follow-up only; no container orchestrator integration is implemented.

Unbounded public capture, TLS decryption, HTTP/2/3, upgrades, pipelining, remote storage, and arbitrary production replay are deferred.
