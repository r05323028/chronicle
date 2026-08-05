## 1. Lease-owned startup recovery

- [x] 1.1 Make `EpochCatalogV1::load` read-only and add lease-owned prepared-tail recovery.
- [ ] 1.2 Persist `Starting`/`Recovering` metadata before recovery and preserve truthful failure state.
- [ ] 1.3 Move normal WAL reopen/create and appendability checks behind `RecorderStartup` lease ownership.
- [ ] 1.4 Add startup crash-boundary, one-active-epoch, predecessor-immutability, and capture-source ordering tests.

## 2. Checkpoint and rollover safety

- [ ] 2.1 Validate checkpoint against authoritative WAL lineage, configuration, pipeline, and output artifacts before capture attach.
- [x] 2.2 Wire orphan delta/checkpoint reconciliation into lease-owned restart.
- [ ] 2.3 Add durable rollover transition/recovery with quota and successor rollback.
- [x] 2.4 Prevent empty time-based epochs and add semantic multi-epoch lineage tests.

## 3. Incremental ETL correctness

- [x] 3.1 Preserve epoch-boundary incomplete socket evidence without predecessor identity/state injection.
- [x] 3.2 Finalize authoritative sessions from verified incremental delta/checkpoint lineage.
- [x] 3.3 Add one-shot/incremental equivalence and cross-epoch endpoint/payload tests.

## 4. Quota, retention, and corruption

- [ ] 4.1 Reserve and release every production persistence peak and rebuild accounting after restart.
- [ ] 4.2 Implement recoverable versus terminal quota-pressure policy and status semantics.
- [ ] 4.3 Wire proof-gated retention cleanup and lifecycle-index compaction into application ownership.
- [ ] 4.4 Add production corruption quarantine/fail-closed and protected-cleanup tests.

## 5. Privileged acceptance and evidence

- [ ] 5.1 Add real checkpoint publication kill/restart scenarios and complete checkpoint checks.
- [ ] 5.2 Add real cleanup interruption/restart/protected-lineage scenarios and complete cleanup checks.
- [ ] 5.3 Strengthen reboot, three-epoch, quota, corruption, replay, and resource-cleanup assertions.
- [x] 5.4 Enforce exact start/end SHA/tree identity and remove stale-owner masking.
- [x] 5.5 Separate P1/P2 evidence namespaces and validate report task identity/check completeness.
- [ ] 5.6 Run targeted tests, Linux validation, OpenSpec validation, and independent review.
- [ ] 5.7 Generate fresh exact-SHA P1/P2 evidence and pass release validation before marking complete.
