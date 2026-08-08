# P1 Operations Guide

## Safety and scope

WAL and published sessions can contain production headers and bodies, including credentials and personal data. Chronicle uses private Unix modes and safe default output, but P1 does **not** provide encryption at rest, secret discovery, comprehensive redaction, or tenant isolation. Inspect and reports omit bodies and arbitrary header values.

P1 live recording is bounded plaintext TCP capture, not always-on capture. It supports Linux 6.1+, cgroup v2, readable BTF, x86_64/aarch64, and required eBPF hooks/capabilities. macOS and ordinary CI use rootless fixtures, ETL, inspect, replay, and eBPF compile checks. Runtime evidence comes only from the opt-in privileged profile.

## Flow and bounds

```text
selected cgroup subtree -> eBPF capture -> 4096-event queue -> segmented WAL
  -> recovery-authoritative commit boundary -> ETL -> HTTP/1.1 session
  -> atomic filesystem publication -> inspect or authorized loopback replay
```

Defaults and hard limits:

- duration: 600 seconds; accepted range `1..=3600` seconds;
- total WAL: 4 GiB physical ceiling by default, including segment headers, frames, and temporary segment files; lower values are allowed down to segment size;
- segment: 256 MiB default, `16 MiB..=4 GiB`;
- group commit: 4 MiB unsynced data or 10 ms, plus rotation, flush, and shutdown;
- loss sampling: complete per-CPU sample at least every 100 ms, using actual monotonic timestamps, plus mandatory final sample;
- eBPF ring: 8 MiB; payload: 16 KiB per event, at most four continuations;
- HTTP/reconstruction limits remain bounded at 8 MiB per connection/body and 10,000 canonical operations.

A delayed poll records its actual expanded interval. Zero counter delta advances the boundary without WAL growth. Positive, reset, regression, clock-mismatch, and final-sample evidence remains typed and unattributed.

A group writes data frames, one in-WAL `CommitMarker`, flushes, and calls one `fdatasync`. Runtime acknowledgement exists only after sync returns. Recovery trusts a marker only after validating frame references, contiguous sequences, same-segment membership, cumulative counts, identities, CRCs, and exact batch digest. Recovery can therefore recognize persisted durability without claiming any caller observed its acknowledgement. There is no external watermark file or second metadata-watermark sync cycle.

Recording status describes finalization: `starting`, `recording`, `completed`, `failed`, or recovery-produced `aborted`. Shutdown reason is separate: `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `wal_size_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`.

Only a verified incomplete final WAL frame may be repaired. Middle corruption, invalid marker references/digests, and unsupported versions fail closed. Complete frames after the final authoritative marker remain uncertainty and are excluded from ETL.

## Cgroup selection

Use exactly one selector:

```bash
chronicle --format json record --source ebpf --cgroup /sys/fs/cgroup/my-workload --wal-dir /var/lib/chronicle/wal
chronicle --format json record --source ebpf --pid 1234 --wal-dir /var/lib/chronicle/wal
```

The selected scope is the exact cgroup node plus descendants, called the **selected subtree**. `cgroup.procs` is read directly from the selected node. Numeric PIDs are resolved to host-visible thread-group IDs and deduplicated into the **direct cgroup TGID set**; its cardinality is **direct TGID count**. This is not POSIX PGID, session ID, thread count, container ID, or descendant membership. Descendant directories are counted separately as **descendant cgroup count**.

A dedicated leaf with direct TGID count zero or one and no descendants needs no acknowledgement. Explicit cgroups with more direct TGIDs or descendants require `--allow-shared-cgroup`:

```bash
chronicle --format json record --source ebpf \
  --cgroup /sys/fs/cgroup/team --allow-shared-cgroup \
  --wal-dir /var/lib/chronicle/wal
```

The flag is invalid with `--pid` and cannot override `/`, `/system.slice`, `/user.slice`, `/machine.slice`, Chronicle containment anywhere in the subtree, namespace ambiguity, movement, or unreadable enumeration. PID selection rejects unrelated direct TGIDs and never widens to an ancestor. Output includes only canonical path/ID, direct TGID count, descendant cgroup count, selected-subtree scope, and acknowledgement state.

## Record, recover, and ETL

`record` writes `recording.json`, a lock, and `segments/*.chwal`. It stops on signal, duration, or physical WAL limit. WAL-limit shutdown writes only the capacity-safe queued prefix, counts the remainder as `discarded_from_queue_due_to_wal_limit`, and emits typed terminal loss evidence when space permits. Kernel/backend drops, post-stop arrivals, and written-not-committed bytes remain separate counters. Record never runs ETL.

```bash
chronicle --format json etl --wal-dir /var/lib/chronicle/wal --output /var/lib/chronicle/sessions
```

ETL processes only the recovered committed prefix, publishes one deterministic Canonical Session v1, and writes its advisory checkpoint after publication. Rerunning unchanged input in the same root reports `already_processed`; another output root is allowed. No `output-binding.json` is used.

## Inspect and replay

```bash
chronicle --format json inspect SESSION_ID --root /var/lib/chronicle/sessions
chronicle --format json replay SESSION_ID --root /var/lib/chronicle/sessions \
  --target http://127.0.0.1:18080 --allow-host 127.0.0.1 \
  --allow-read --allow-write --execute
```

Canonical v1 preserves connection/operation completeness, loss-window provenance, WAL ranges, integrity, and replayability. Complete supported operations outside loss windows may execute while degraded siblings remain visible as not attempted. Replay is dry-run by default, requires explicit loopback target, exact `--allow-host`, effect authorization, and `--execute`; it never contacts recorded destinations, follows redirects, expands DNS, retries, uses a proxy/TLS, or replays captured credentials.

Replay outcomes are `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`. Exit codes: 0 for successful record/ETL/inspect, dry-run, and completed replay; 3 for data/persistence/finalization errors; 4 for policy, invalid-session, unsupported live preflight, or required doctor probe failure; 5 for transport stop; 6 for executed verification failure.

## Doctor and test profiles

```bash
chronicle --format json doctor --wal-dir /var/lib/chronicle/wal --output /var/lib/chronicle/sessions
```

Doctor is diagnostic only. It performs independent non-destructive probes, removes temporary artifacts, and never starts capture, repairs WAL, publishes sessions, or sends replay traffic. Aggregate status is `unsupported` for required unsupported probes, `not_checked` for required undecidable probes, `supported_with_warnings` for warnings/optional gaps, otherwise `supported`. Omitted paths are optional `not_checked`; no default path is guessed.

Validation entry point:

```bash
./scripts/validate.sh fast
./scripts/validate.sh targeted --changed-since origin/main
./scripts/acceptance.sh --profile p1 --executor multipass
./scripts/acceptance.sh --profile p2 --executor multipass
./scripts/acceptance.sh --profile all --executor multipass
./scripts/acceptance.sh --profile p2 --executor multipass --release
./scripts/acceptance.sh --profile p2 --executor multipass --no-reuse
./scripts/validate.sh release --reuse-evidence
```

`validation/groups.toml` maps paths to focused groups. Targeted output lists every selected/skipped group and reason. Documentation-only changes select no privileged gate; ETL changes select ETL/checkpoint checks; WAL/recovery changes select focused WAL/P2 checks; eBPF changes select build/decoder checks and a minimal privileged smoke. Explicit gates and release retain all existing P1/P2 checks.

Fast and targeted modes use local checks and default to failure-only artifacts. Recorder changes should use `fast`, targeted changed-path/recorder tests, then a bounded lifecycle scenario before a complete privileged gate. Agents must not start an unbounded foreground recorder.

Timeout hierarchy is independent: validation/build command 900s, readiness command/readiness 10s/180s, systemd command 30s, ordinary scenario 300s (600s quota/retention, 900s cargo-heavy), cleanup grace 180s, acceptance profile 3300s under `validate.sh`, complete gate 3600s. `scripts/run-with-timeout.sh` returns 124 after process-tree TERM/grace/KILL. Configure `CHRONICLE_TIMEOUT_GRACE_SECONDS`, `CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS`, `CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS`, and gate timeout variables without collapsing inner and outer budgets. Multipass exposes guest, status, VM-readiness, transfer, bootstrap, and remote timeout variables named in `AGENTS.md`; remote deadline exceeds guest deadline plus cleanup grace while remaining below host profile deadline. Reboot recovery retains differing pre/post boot IDs as proof restart occurred. Timeout evidence names layer, phase/scenario, duration, and retained diagnostics.

Successful gate artifacts contain only `summary.json`, `environment.json`, `manifest.json`, and `checksums.txt`; failure artifacts contain a bounded reproducer and failure-listed WAL/session data. Never upload `target/`, Cargo caches, VM filesystems, or complete successful WAL directories. Retention guidance: no upload or 1–3 days for fast/targeted successes, about 7 days for gate successes, about 14 days for gate failures, and long-term for release evidence.

Privileged caches stay in reused Multipass VM paths `/home/ubuntu/chronicle-target`, `/home/ubuntu/chronicle-ebpf-target`, and `/home/ubuntu/.cargo`; the unified executor transfers an immutable source snapshot so dirty development changes are tested, not discarded. Evidence lives under `target/validation-evidence/acceptance/<profile>/<fingerprint>/<run-id>/`. Reuse matches acceptance-sensitive source fingerprint, scenario coverage, report schema, manifest integrity, and compatible executor/environment. Commit/tree SHA is provenance only, never evidence identity; release mode additionally requires a clean, identifiable current checkout that remains unchanged through validation.

`scripts/acceptance.sh` is the only user-facing acceptance entrypoint. `scripts/acceptance/scenarios.toml` defines scenario/profile coverage; `--profile all` runs the P2 superset once because it includes P1 scenarios. `runner.py` owns selection/fingerprint/report/reuse, and `lib/multipass.sh` owns VM lifecycle, snapshot transfer, reboot, and artifacts. `lib/scenario-dispatch.sh` loads ordered scenario modules; profile scripts provide runtime setup/cleanup only. `scripts/acceptance/p1-*.sh` and `p2-*.sh` remain thin deprecated compatibility wrappers. Run `python3 scripts/tests/acceptance/test_runner.py`, `python3 scripts/tests/validation/test_layered_validation.py`, and `bash scripts/tests/acceptance/test-p2-readiness.sh` for tooling coverage.

P2 readiness contract: `recorder-status` now reports machine-readable `state` values (`starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, `failed`) alongside liveness, lifecycle, capture readiness, processing readiness, and health. Workload admission waits for `state=ready`; systemd `active` alone is insufficient. Polling prints state transitions, tolerates temporary status-command failures, stops immediately on terminal failure, and uses configurable timeout/interval values (`CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_INTERVAL_SECONDS`).

Previous blocker root cause: P2 treated `processing_readiness=not_ready` as capture-not-ready and waited forever even though capture was safe to admit workload; status-command errors were also suppressed and startup/recovery, stale-owner, and terminal failure collapsed into one generic timeout. Recovery can spend longer than the old 60-second loop validating a large incremental checkpoint. Failure diagnostics now retain status/service/journal, kernel/BTF/cgroup/eBPF, WAL listing, checkpoint metadata, process, and disk evidence without Cargo caches or VM data. Multipass stops stale P2 units and fixes artifact ownership before transfer.

The profile uses local upstream/replay targets and dedicated/shared cgroups. It verifies capture, sampling, one-marker/one-sync WAL behavior, crash/recovery authority, hard-cap queue discard, ETL idempotency across roots, cgroup safety, signal/limit finalization, mixed replay, and original-destination isolation. Unsupported hosts must report skipped/not-checked rather than passing runtime coverage.

Always-on capture, rotating sessions, incremental ETL, TLS decryption, HTTP/2/3, upgrades, pipelining, remote storage, and arbitrary production replay are deferred.
