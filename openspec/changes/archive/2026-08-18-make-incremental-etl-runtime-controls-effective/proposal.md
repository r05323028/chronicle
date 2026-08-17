## Why

Incremental ETL currently exposes retry, lag, and batch controls without making them authoritative at runtime: failed work can retry in one tight call, successful processing clears lag even when committed WAL remains unpublished, and each pass consumes the entire committed suffix. This can report processing as ready while capture continues to accumulate durable work and makes restart/retry behavior difficult to reason about.

## What Changes

- Make retry attempts and `backoff_millis` a non-blocking worker state machine with a real retry deadline; polling skips ETL before the deadline and capture continues.
- Plan incremental work only at complete WAL commit-marker boundaries, treating `batch_records` as a soft record bound and preserving publication-before-checkpoint ordering.
- Compute lag from committed WAL authority minus the last successfully published/checkpointed marker, including records, payload bytes, and a durable segment-based age; reconstruct it after restart.
- Keep capture readiness independent from processing readiness and make degraded ETL state visible in persisted recorder metadata/status.
- Add a narrow internal ETL batch/progress seam while retaining filesystem-backed implementations in 0.1.
- Add deterministic tests for batching, lag, retry deadlines, publication failure, restart, rollover, and continuation.
- Update architecture/operator documentation to describe actual co-located deployment and the explicit runtime seams without introducing future service infrastructure.

## Capabilities

### New Capabilities

- `incremental-etl-runtime-controls`: Durable, bounded incremental ETL batching, lag accounting, retry/backoff, readiness, and restart semantics.

### Modified Capabilities

<!-- Existing architecture capabilities already state the broad readiness and Recorder/ETL ownership invariants; this change adds a focused runtime contract without changing their requirements. -->

## Impact

- `chronicle-application`: worker state, continuous recorder polling, durable metadata/status projection, and focused tests.
- `chronicle-etl`: explicit committed-batch input/progress seam where needed; canonical publication remains ETL-owned.
- `chronicle-wal`: read-only committed-range/batch authority helpers only; no WAL v1 framing changes.
- Documentation and OpenSpec runtime contracts.
- No new workspace crates, no public CLI/schema change, no Capture Event v1 or Canonical Session v1 change, and no new storage/deployment backend.
