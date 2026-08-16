## 1. OpenSpec change

- [x] 1.1 Create change artifacts (proposal, design, tasks) and validate with `openspec validate --all --strict --no-interactive`.

## 2. chronicle-etl: one checkpoint model

- [x] 2.1 `checkpoint.rs`: remove `IncrementalEtlCheckpointV1`, `RecoveryAuthoritativeSnapshot`, `CheckpointError`, `write_checkpoint_atomic`, `read_checkpoint`, `CheckpointSummary`, `INCREMENTAL_CHECKPOINT_SCHEMA_VERSION` (v1) and V1 tests; keep shared lineage/decoder/output types.
- [x] 2.2 `continuation.rs`: rename `IncrementalEtlCheckpointV2` -> `IncrementalEtlCheckpoint`, `INCREMENTAL_CHECKPOINT_V2_SCHEMA_VERSION` -> `INCREMENTAL_CHECKPOINT_SCHEMA_VERSION`, `ContinuationDependencyV2` -> `ContinuationDependency`, `EpochContinuationCheckpointV1` -> `EpochContinuationCheckpoint`.
- [x] 2.3 `publication.rs`: merge `publish_delta_then_checkpoint_v2` + `publish_delta_then_checkpoint_with_fault` into one `publish_delta_then_checkpoint[_with_fault]` on `IncrementalEtlCheckpoint`; merge `recover_pending_checkpoint_v2` into `recover_pending_checkpoint`; remove `reconcile_delta_checkpoint`; rename `publish_continuation_then_checkpoint_v2`; update tests.
- [x] 2.4 `lib.rs` exports: drop removed names, export merged names.

## 3. chronicle-application: epoch catalog

- [x] 3.1 `epoch_catalog.rs`: remove `EpochCatalogV1`, `EpochCatalogEntry` (V1), V1 impl + tests, `load_compatible`, `from_legacy`, `to_legacy`, `LegacyOneEpochMapping`, `EpochEvidenceAuthority::LegacyOneEpochMapping`; rename V2 types to `EpochCatalog`/`EpochCatalogEntry`/`EpochCatalogSummary`; trim summary to `published` cache-only flag; keep checksum + lineage validation.
- [x] 3.2 Update `reconcile_epoch_evidence` and remaining callers to merged names.

## 4. chronicle-application: rollover transition

- [x] 4.1 `rollover_transition.rs`: remove `RolloverTransitionV1`, V1 phase enum, `load_transition` (V1), `write_transition_atomic` (V1), V1 checksum + tests; rename V2 to `RolloverTransition`/`RolloverTransitionPhase`, const to `ROLLOVER_TRANSITION_SCHEMA_VERSION` (=2); `load_transition_v2`/\`write_transition_v2_atomic` take short names; keep phase fault matrix.

## 5. chronicle-application: continuous recorder

- [x] 5.1 `continuous_recorder.rs`: drop V1 checkpoint field/read/write/validation; single `IncrementalEtlCheckpoint`; keep startup lineage validation for the surviving checkpoint file.
- [x] 5.2 Merge rollover entrypoints: `begin_rollover_transition_v2` -> `begin_rollover_transition`, `rollover_to_v2` -> `rollover_to`; remove V1 `ensure_/advance_` helpers; `complete_rollover_transition` advances the merged transition only.
- [x] 5.3 Update tests to the merged models (prepared-transition guard, quota denial, failure-boundary retention, continuation handoff).

## 6. chronicle-application: one-shot removal

- [x] 6.1 `record.rs`: remove `record_live_ebpf`, `record_live_ebpf_with_lifetime`, `recover_rollover_transition_for_root` (test helper) and V1 catalog load branch; `load_runtime_epoch_catalog` decodes the current model only.
- [x] 6.2 `etl.rs`: remove `record_production`, `record_production_with_lifetime` and their now-unused helpers; keep `process_and_publish_recording_wal` and the final-session checkpoint.
- [x] 6.3 `command_record.rs`: `finalize_and_publish` fails closed without epochs.json; remove one-shot catalog synthesis (parent UUID == epoch UUID); keep `finalize_continuous_and_publish` with minimal summary write.
- [x] 6.4 `doctor.rs`: checkpoint probe reads `RecordingEtlCheckpoint` (etl-checkpoint.json) instead of the removed V1 reader.
- [x] 6.5 `lib.rs`: exports updated; rewrite the two V1 rollover-recovery tests against `recover_rollover_transition_v2_for_root` (rollback unpublished successor, adopt published successor).

## 7. chronicle-cli

- [x] 7.1 `main.rs`: remove `Source::Ebpf`, `record_ebpf_legacy`, `LegacyInvocation::RecordEbpf`, `raw_legacy_record` ebpf branch, and related tests; keep `--source fixture`.

## 8. Static guard

- [x] 8.1 `scripts/validation.py`: add `legacy-names` check rejecting production references to removed identifiers.
- [x] 8.2 Wire into `scripts/validate.sh` fast gate; add `scripts/tests/validation/test_legacy_names.py` meta-test.

## 9. Specs and docs

- [x] 9.1 `openspec/specs/recording-identity/spec.md`: remove legacy one-epoch compatibility mapping requirement/scenario.
- [x] 9.2 `openspec/specs/mvp-schema-versioning/spec.md`: restate pre-0.1 policy (one active model; unreleased schemas replaceable).
- [x] 9.3 `AGENTS.md`: add durable invariant - before 0.1.0 prefer one current domain model over compatibility layers for unreleased internal formats.

## 10. Validation

- [x] 10.1 `cargo fmt --all --check`, clippy `-D warnings`, workspace tests.
- [x] 10.2 `openspec validate --all --strict --no-interactive`, architecture/ownership/catalog checks, tooling meta-tests.
- [x] 10.3 `validate.sh targeted --changed-since` when host permits; note Linux-only paths verified by build/tests only, not privileged acceptance.
