## Purpose

Define deterministic runtime semantics for bounded incremental ETL so retry, batching, lag, readiness, publication ordering, restart, and epoch continuation remain truthful while capture continues independently.

## ADDED Requirements

### Requirement: Incremental ETL consumes complete committed batches

Incremental ETL SHALL process only recovery-authoritative WAL envelopes through complete commit-marker boundaries. `batch_records` SHALL be a soft upper bound on newly committed non-marker records per pass: a pass MAY exceed it to consume one indivisible commit batch, but SHALL NOT split a commit batch or process an uncommitted suffix. Checkpoint advancement SHALL be monotonic and SHALL identify the last successfully published commit marker.

#### Scenario: Multiple committed batches catch up incrementally

- **WHEN** committed WAL contains more complete marker-delimited batches than `batch_records` permits in one pass
- **THEN** each pass publishes at most the configured soft bound across complete batches, later passes continue from the checkpoint marker, and capture remains able to append

#### Scenario: One commit batch exceeds the soft bound

- **WHEN** one complete commit batch contains more non-marker records than `batch_records`
- **THEN** ETL processes that complete batch as one unit rather than splitting it or reading an uncommitted suffix

#### Scenario: Crash occurs after publication before checkpoint

- **WHEN** canonical delta publication succeeds but checkpoint advancement does not
- **THEN** restart reprocesses the same committed range idempotently and does not advance beyond the last durable checkpoint

### Requirement: Lag is derived from durable progress

Recorder lag SHALL be derived from validated committed WAL authority minus the last successfully published and checkpointed marker. `lag_records` SHALL count committed non-marker records after the checkpoint, `lag_bytes` SHALL count their committed payload bytes, and `lag_age_seconds` SHALL report the age of the oldest unprocessed committed WAL range using durable segment creation evidence and the polling clock. Lag SHALL be zero only after the checkpoint covers the latest committed marker; a successful processing attempt alone SHALL NOT clear lag.

#### Scenario: ETL falls behind while capture continues

- **WHEN** committed WAL advances while ETL is delayed, retrying, or processing an earlier bounded batch
- **THEN** lag records and bytes increase from durable committed progress while capture readiness remains independent and capture continues unless an existing quota or durability invariant stops admission

#### Scenario: Restart reconstructs lag

- **WHEN** recorder restarts with committed WAL newer than its incremental checkpoint
- **THEN** metadata reconstructs nonzero lag from WAL commit markers and the checkpoint before reporting processing readiness

#### Scenario: Catch-up clears lag

- **WHEN** ETL publishes and checkpoints every committed batch through the latest marker
- **THEN** lag records, bytes, and age return to zero and processing readiness can become ready

### Requirement: Retry and backoff are bounded and non-blocking

Incremental ETL retry attempts SHALL be bounded by `retry_attempts` after the initial attempt. A failed attempt that has retries remaining SHALL enter a backoff state with a retry-not-before deadline computed from `backoff_millis`; polling before that deadline SHALL skip ETL work without sleeping or blocking capture. Exhausted retries SHALL make processing not ready and overall health shall be degraded/failed according to existing policy. A recorder restart SHALL discard transient retry counters and deadlines, then deterministically reconstruct durable lag and begin a fresh bounded attempt sequence.

#### Scenario: Backoff prevents immediate retry

- **WHEN** an ETL attempt fails and `backoff_millis` is positive
- **THEN** polls before the retry deadline do not invoke ETL again, do not sleep, and continue capture polling

#### Scenario: Retry succeeds after deadline

- **WHEN** the retry deadline expires and the next attempt publishes and checkpoints successfully
- **THEN** the worker returns to running, resets its transient retry counter, and readiness reflects durable lag rather than the prior failure

#### Scenario: Retry budget is exhausted

- **WHEN** all configured retry attempts fail
- **THEN** ETL remains not ready, capture is not failed solely because ETL is lagging, and no unbounded retry loop occurs

#### Scenario: Restart occurs during backoff

- **WHEN** recorder restarts before a transient retry deadline
- **THEN** startup does not persist or infer the old monotonic deadline, reconstructs lag from durable state, and applies the configured bounded retry policy deterministically

### Requirement: Processing readiness is independent from capture readiness

ETL lag beyond `max_lag_records`, active retry/backoff, publication failure, checkpoint contradiction, or unresolved continuation SHALL make processing readiness not ready/degraded and overall health shall reflect existing policy. These conditions SHALL NOT make capture readiness false or stop capture unless WAL appendability, quota, recovery, or another existing explicit capture invariant requires it. Recorder status SHALL expose both readiness values and durable lag.

#### Scenario: Lag threshold is exceeded safely

- **WHEN** durable lag records exceed `max_lag_records` while WAL appendability and quota remain safe
- **THEN** capture readiness remains ready, processing readiness is not ready/degraded, overall health is degraded, and status reports the lag

#### Scenario: Publication failure leaves capture operating

- **WHEN** canonical delta publication fails after ETL reconstruction
- **THEN** checkpoint and processor progress remain at the prior durable point, retry/backoff is scheduled, processing readiness is not ready, and capture continues

### Requirement: Epoch continuation preserves bounded progress

Bounded incremental batches SHALL preserve existing publication-before-checkpoint ordering, decoder checkpoint state, explicit parent/epoch lineage, and continuation handoff semantics. Epoch rollover MAY proceed while predecessor ETL is lagging; successor ETL SHALL remain gated by durable continuation state and SHALL resume from the correct predecessor checkpoint without treating a WAL boundary as a protocol boundary.

#### Scenario: Rollover while predecessor ETL lags

- **WHEN** capture rolls to a successor epoch before predecessor ETL catches up
- **THEN** predecessor durable lag remains visible and protected, successor capture can operate under existing rollover rules, and continuation completion—not transient in-memory state—controls successor processing readiness

#### Scenario: Continuation catches up in bounded batches

- **WHEN** successor ETL processes several marker-delimited batches after restoring predecessor continuation
- **THEN** each checkpoint remains lineage-valid and monotonic, final canonical output remains equivalent to one-shot reconstruction, and no batch boundary becomes a protocol boundary
