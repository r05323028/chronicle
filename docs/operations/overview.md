# Operations Guide

## Safety and scope

WAL and published sessions can contain production headers and bodies, including credentials and personal data. Chronicle uses private Unix modes and safe default output, but does **not** provide encryption at rest, secret discovery, comprehensive redaction, or tenant isolation. Inspect and reports omit bodies and arbitrary header values.

Live recording is bounded plaintext TCP capture, not always-on capture. Release binaries target Linux x86_64/aarch64; the 0.1 release-verified runtime environment is Ubuntu 24.04, Linux 6.8, aarch64, cgroup v2, readable BTF, and required production eBPF hooks/capabilities. x86_64, Linux 6.1+ minimum-boundary environments, other kernels, cloud-provider kernels, and offload characteristics require matching privileged acceptance; an artifact build is not runtime proof. macOS and ordinary CI use rootless fixtures, ETL, inspect, replay, and eBPF compile checks. Runtime evidence comes only from the opt-in privileged profile (see `validation/test-architecture/README.md`).

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

Hidden `chronicle internal etl --wal-dir DIR --output DIR` is the deployment mechanism for standalone WAL recovery, not normal user workflow. Rerunning unchanged input in same root reports `already_processed`; another output root is allowed. No `output-binding.json` is used.

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

## Doctor

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

Timeout hierarchies, acceptance runner internals, scenario dispatch, Multipass caching, evidence reuse, and tooling test commands are contributor-tooling concerns documented in `CONTRIBUTING.md` and `validation/test-architecture/README.md`; this operator guide keeps only what is needed to operate or diagnose Chronicle. Timeout evidence keeps layer, phase/scenario, duration, and retained diagnostics; failure artifacts retain a bounded reproducer and never upload `target/`, Cargo caches, or complete successful WAL directories.

`chronicle internal recorder-status` reports machine-readable readiness (`state` values `starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, `failed`) with liveness, capture readiness, processing readiness, and health; workload admission waits for `state=ready`, and systemd `active` alone is insufficient.

## Artifact and rollback compatibility

Authoritative WAL v1, canonical session v1, and session-manifest v1 formats are intentionally stable. `catalog.json` v1 and per-recording `recording-intent.json` v1 are additive advisory artifacts; older releases ignore them. Rollback requires coordinated Chronicle binaries/crates, not a CLI-only downgrade: a previous release can still inspect an explicit canonical root and process an explicit WAL directory, but does not understand recording names, `latest`, intent sidecars, catalog reconciliation, or `record --retry`.

Cross-version check:

```bash
cargo build -p chronicle-cli
CHRONICLE_PREVIOUS_RELEASE_BIN=/path/to/controlled-previous-chronicle \
CHRONICLE_CURRENT_WAL_DIR=/path/to/current-production-wal \
  ./scripts/tests/test-user-intent-cli-rollback.sh
```

The harness proves a controlled older binary reads current WAL/canonical artifacts. Supported 0.1.x readers and writers must preserve the published v1 contracts; repository-only or unsupported-version artifacts fail closed rather than receiving an implicit migration adapter.

Docker and Kubernetes packaging are future follow-up only; no container orchestrator integration is implemented.

TLS decryption, HTTP/2/3, upgrades, pipelining, remote storage, and arbitrary production replay are deferred.
