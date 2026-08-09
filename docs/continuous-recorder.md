# Continuous recorder

## Runtime shape

```text
config -> domain lease -> state/WAL/manifest/checkpoint recovery
       -> quota reservations -> appendable epoch -> capture attach
       -> incremental ETL resume -> running

SIGTERM/SIGINT -> stop intake -> drain -> commit/sync -> seal
               -> ETL safe point -> detach -> stopped metadata -> unlock
```

Recorder owns one filesystem domain. WAL commit markers remain acknowledgement authority. Manifest, checkpoint, RecordingStore artifacts, and status metadata never promote uncommitted WAL bytes.

## Paths and ownership

- `state_root/recorder.json`: versioned, checksummed active metadata.
- `state_root/.chronicle-recorder.lock`: subordinate recorder-state lease.
- `<domain-lock-root>/.chronicle-domain.lock`: filesystem-domain lease.
- `wal_dir/record.lock`: per-recording WAL lock.
- `wal_dir/manifest.json`: reconciled segment metadata; never WAL authority.
- `wal_dir/etl-checkpoint.json`: recorder-owned incremental ETL checkpoint.
- `store_root/`: immutable delta, recovery, and provenance artifacts.

One mutating recorder per filesystem domain. Standalone ETL, cleanup, or store mutation must not bypass that ownership boundary.

## Readiness and health

`liveness` means process exists. `capture_readiness` becomes ready only after durable WAL appendability and capture attachment. `processing_readiness` is independent and can be unknown/degraded while capture remains ready. Stale ownership reports failed health with crash-recovery remediation.

Acceptance polls machine-readable status for at most `CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS` (default 180), while each status command defaults to 10s. Ordinary scenarios use 300s; quota/retention use 600s; cargo-heavy scenarios use 900s. Scenario cleanup gets an independent 180s grace before forced kill. Validation runs acceptance profile at 3300s beneath 3600s gate. Timeout retains readiness transitions, recorder status/journal, process, disk, WAL, and checkpoint diagnostics before bounded cleanup.

## Retention and quota

Retention transitions require sealed, processed, checkpointed, and verified finalized-session evidence. Cleanup writes deleting intent, renames to same-filesystem trash, syncs, writes a tombstone, unlinks, and syncs again. Digest mismatch, missing intent, incomplete cleanup, and protected lineage fail closed.

Quota reserves WAL, manifest/checkpoint, RecordingStore, final-session, staging, and trash peaks while preserving configured minimum free bytes. Insufficient headroom stops admission; cleanup never authorizes deletion of unverified data.

## Security

Capture scope must be outside the recorder state/store subtree. Config files contain paths and bounds, not secrets. Human and JSON diagnostics omit payloads, credentials, and raw cgroup/process details. Replay remains explicit and host-allowlisted.

## Operations

Use `chronicle internal recorder --config /etc/chronicle/recorder.toml` in foreground. Use `chronicle internal recorder-status --state-root /var/lib/chronicle` for read-only readiness. Top-level `recorder` and `recorder-status` remain hidden deprecated aliases through 0.1.x and are removed at the 0.2 boundary. See [`systemd/chronicle-recorder.service`](systemd/chronicle-recorder.service) and [`continuous-recorder-runbook.md`](./continuous-recorder-runbook.md).
