## 1. OpenSpec and durable input contract

- [x] 1.1 Add `incremental-etl-runtime-controls` delta spec and validate its commit-boundary batching, durable lag, non-blocking retry, readiness, and continuation scenarios.
- [x] 1.2 Extend WAL read-only snapshot authority with the minimum in-memory segment timestamp/progress evidence needed for durable lag and marker-bounded planning; preserve WAL v1 framing and recovery semantics.
- [x] 1.3 Add an explicit marker-capped ETL batch/progress input seam using existing `chronicle-etl` types; keep filesystem-backed adapters as the only 0.1 implementation.

## 2. Worker retry/backoff

- [x] 2.1 Replace tight-loop retry with deadline-based worker transitions driven by caller-supplied poll time; track retry attempts, retry deadline, skipped-backoff polls, and terminal failure without sleeping.
- [x] 2.2 Define restart behavior: transient retry state resets while durable checkpoint/lag state is reconstructed; keep capture polling independent.
- [x] 2.3 Add deterministic worker tests for retry scheduling, pre-deadline skips, post-deadline retry, bounded exhaustion, and reset on restart/start.

## 3. Marker-bounded incremental processing

- [x] 3.1 Plan the next complete committed marker range from the checkpoint cursor using `batch_records` as a soft non-marker-record bound; never split a commit batch or process a mutable suffix.
- [x] 3.2 Advance processor/checkpoint state only after ETL publication verification; restore processor state and schedule retry when publication fails.
- [x] 3.3 Add focused tests for multiple bounded batches, oversized indivisible batches, monotonic checkpoints, idempotent retry, and publication-before-checkpoint ordering.

## 4. Durable lag and readiness

- [x] 4.1 Compute lag records/bytes from latest committed marker counters minus the checkpoint marker counters; compute oldest-unprocessed segment age from durable WAL evidence.
- [x] 4.2 Refresh lag on normal, delayed, backoff, failure, restart, and catch-up paths; remove unconditional lag zeroing after an attempt.
- [x] 4.3 Keep capture readiness independent; set processing readiness/overall health from durable lag, worker state, publication/checkpoint state, and continuation state. Add metadata/status assertions.
- [x] 4.4 Add tests for capture continuing while ETL lags, lag reconstruction after restart, lag zero only after catch-up, and epoch rollover/continuation while lagging.

## 5. Documentation and targeted validation

- [x] 5.1 Update architecture/operations documentation to distinguish logical Recorder/ETL responsibilities, current co-located filesystem deployment, and future replaceable adapters; document batch, lag, retry, and restart semantics.
- [x] 5.2 Run targeted application/ETL worker, WAL, checkpoint/publication, restart, rollover, continuation, command-recording, and readiness tests; inspect failures and iterate.
- [x] 5.3 Run `openspec validate --all --strict --no-interactive` and `./scripts/validate.sh fast`; run the existing recorder/privileged gate if changed behavior requires it.
