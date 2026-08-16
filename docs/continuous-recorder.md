# Continuous recorder

## Runtime shape

```text
config -> domain lease -> parent intent/run + epochs.json recovery
       -> quota reservations -> appendable bounded epoch -> capture attach
       -> incremental ETL/continuation resume -> running

SIGTERM/SIGINT -> stop intake -> drain -> commit/sync -> seal active epoch
               -> configured ETL safe point -> stopped parent metadata -> unlock
```

A parent recording owns ordered bounded epochs. Epoch age/bytes and segment
limits trigger rollover, never ordinary parent termination. A whole-run
`--duration` is optional; omitted means no time deadline. Rollover capture and
ETL continuation are independent: successor capture may continue while a
predecessor checkpoint is pending.

Recorder owns one filesystem domain. WAL commit markers remain acknowledgement authority. Manifest, checkpoint, RecordingStore artifacts, and status metadata never promote uncommitted WAL bytes.

## Paths and ownership

- `state_root/recorder.json`: versioned, checksummed active metadata.
- `state_root/.chronicle-recorder.lock`: subordinate recorder-state lease.
- `<domain-lock-root>/.chronicle-domain.lock`: filesystem-domain lease.
- `recordings/<recording-id>/recording-intent.json`: one-time parent allocation/name.
- `recordings/<recording-id>/recording-run.json`: derived parent lifecycle summary.
- `recordings/<recording-id>/epochs.json`: checksummed parent-to-epoch topology authority.
- `recordings/<recording-id>/epochs/<ordinal>-<epoch-id>/`: bounded epoch WAL and ETL artifacts.
- `wal_dir/record.lock`: per-epoch WAL lock.
- `wal_dir/manifest.json`: reconciled segment metadata; never WAL authority.
- `wal_dir/incremental-etl-checkpoint-v2.json`: parent-aware ETL cursor when enabled.
- `wal_dir/continuation-in.json` / `continuation-out.json`: immutable bounded handoff evidence.
- `store_root/`: immutable delta, recovery, and provenance artifacts.

One mutating recorder per filesystem domain. Standalone ETL, cleanup, or store mutation must not bypass that ownership boundary.

## Readiness and health

`liveness` means process exists. `capture_readiness` becomes ready only after durable WAL appendability and capture attachment. `processing_readiness` is independent and can be unknown/degraded while capture remains ready. Stale ownership reports failed health with crash-recovery remediation.

Acceptance polls machine-readable status for at most `CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS` (default 180), while each status command defaults to 10s. Ordinary scenarios use 300s; quota/retention use 600s; cargo-heavy scenarios use 900s. Scenario cleanup gets an independent 180s grace before forced kill. Validation runs acceptance profile at 3300s beneath 3600s gate. Timeout retains readiness transitions, recorder status/journal, process, disk, WAL, and checkpoint diagnostics before bounded cleanup.

## Retention and quota

Retention transitions require sealed, processed, checkpointed, and verified finalized-epoch session evidence. Pending, ready-but-unconsumed, or retryable failed continuation protects predecessor WAL/checkpoints. Cleanup writes deleting intent, renames to same-filesystem trash, syncs, writes a tombstone, unlinks, and syncs again. Digest mismatch, missing intent, incomplete cleanup, and protected lineage fail closed.

Quota reserves WAL, manifest/checkpoint, RecordingStore, final-session, staging, and trash peaks while preserving configured minimum free bytes. Insufficient headroom stops admission; cleanup never authorizes deletion of unverified data.

## Security

Capture scope must be outside the recorder state/store subtree. Config files contain paths and bounds, not secrets. Human and JSON diagnostics omit payloads, credentials, and raw cgroup/process details. Replay remains explicit and host-allowlisted.

## Operations

Use `chronicle internal recorder --config /etc/chronicle/recorder.toml` in foreground. Use `chronicle internal recorder-status --state-root /var/lib/chronicle` for read-only readiness. See [`systemd/chronicle-recorder.service`](systemd/chronicle-recorder.service) and [`continuous-recorder-runbook.md`](./continuous-recorder-runbook.md).
