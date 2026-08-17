## Context

The current worker performs all retries inside one `process_once` call, `poll_incremental` reads and processes the complete committed snapshot, and the service clears metadata lag after a successful callback. The WAL already exposes authoritative commit markers, cumulative durable record/payload counts, marker digests, segment lineage, and marker-capped snapshots. Existing ETL publication helpers already implement output-before-checkpoint ordering.

## Goals / Non-Goals

**Goals:**

- Make existing retry, batch, lag, and readiness controls truthful without blocking the capture poll.
- Use commit-marker ranges and durable checkpoint evidence as the sole progress coordinates.
- Preserve restart, publication failure, rollover, continuation, WAL, Capture Event v1, Canonical Session v1, and CLI contracts.
- Keep the current local filesystem implementation while making the ETL input/publication seams explicit in code and documentation.

**Non-Goals:**

- No WAL framing or persisted checkpoint schema change.
- No retry-deadline persistence, distributed leases, queues, remote stores, new crates, or service binaries.
- No direct arbitrary record slicing and no change to replay behavior.

## Decisions

### 1. Use caller-supplied polling time as the deadline clock

`ContinuousRecorderService::poll(now_millis)` is the existing deterministic scheduler boundary. The worker will store `retry_not_before` and compare it with this value; production callers provide the existing wall-clock millisecond value and tests advance it directly. No `sleep` occurs. Restart creates a fresh worker, so transient retry counters/deadlines are intentionally not persisted; durable WAL/checkpoint state remains authoritative.

Alternative rejected: sleeping in `process_once`, because it blocks capture and makes lifecycle tests timing-dependent. Persisting a monotonic deadline was rejected because it is not portable across boots and would require a new persisted state contract.

### 2. Plan work at commit-marker boundaries

The application will read the recovery-authoritative committed snapshot, locate the checkpoint marker, and select the shortest prefix ending at a commit marker whose newly committed non-marker count reaches `batch_records`. If the first complete commit batch exceeds the bound, that complete batch is the unit of progress. The ETL processor receives the selected prefix, retaining its existing cursor logic and decoder state. The selected marker's digest, segment ordinal, active-segment prefix digest, and decoded cumulative counts become the candidate checkpoint/progress coordinate.

This is a soft bound: it avoids invalid partial commits while still guaranteeing progress. The plan is recomputed from durable WAL/checkpoint state after every pass and restart.

### 3. Compute lag from marker counters, not attempts

For the latest committed snapshot and the checkpoint marker, decode the corresponding WAL commit markers and subtract `durable_record_count` and `durable_payload_bytes`. Age uses the creation timestamp of the oldest WAL segment containing the unprocessed committed range and the caller's polling time; this is durable, conservative evidence rather than an in-memory timer. Missing/no-commit state reports zero lag. Metadata is refreshed from this calculation on every poll, including backoff and publication-failure paths.

No new JSON fields are needed: existing `LagSummary` fields gain their intended durable meanings. Small in-memory WAL snapshot additions carry segment creation evidence without changing WAL bytes or checkpoint schemas.

### 4. Keep ETL ownership at existing input/publication seams

`chronicle-etl::CommittedWalSnapshot` remains the application-to-ETL input port, now supplied with one marker-capped batch prefix at a time. `IncrementalProcessor` owns reconstruction/decoding and its resumable state. `chronicle-etl` publication functions remain the output-before-checkpoint seam; application owns capture lifecycle, WAL append/rollover, quota, and metadata composition. Filesystem WAL/store implementations remain the only 0.1 adapters. Documentation will state this current co-location explicitly instead of claiming process/filesystem independence today.

Alternative rejected: adding a trait hierarchy, RPC, or storage adapter for one filesystem implementation; it would broaden visibility and dependency surface without proving deployment independence.

### 5. Stage publication failure as worker failure

The service will treat reconstruction plus canonical delta publication/checkpoint as one worker attempt. Processor state is restored to the pre-attempt snapshot on publication failure; checkpoint remains authoritative. The worker then schedules retry/backoff through the same non-blocking transition used for read/reconstruction failures. Success updates worker lag only from the post-publication durable checkpoint calculation.

## Risks / Trade-offs

- **Repeated full snapshot scans** → retain the existing recovery-authoritative scan path and bound ETL work by marker ranges; optimize incremental indexing only if profiling proves this path material.
- **Segment creation time is a conservative age signal** → document it as oldest unprocessed segment age, not exact per-record commit time; zero remains reserved for caught-up durable progress.
- **Transient retry state resets on restart** → deterministic and safe; durable lag/checkpoint state is never reset, and each boot receives a fresh bounded retry budget.
- **Existing metadata validation rejects nonzero lag without a checkpoint** → preserve that invariant; before a first checkpoint, committed work is represented as not-ready with lag fields remaining schema-valid through the checkpoint transition.

## Migration Plan

No migration is required. Existing WAL v1, checkpoint files, metadata JSON, and canonical artifacts remain readable. Deploying the new binary recomputes lag from existing WAL/checkpoint evidence and resets only transient in-memory retry state.
