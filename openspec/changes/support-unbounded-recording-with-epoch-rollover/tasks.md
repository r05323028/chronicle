## 1. Identity and authority foundations

- [ ] 1.1 Add neutral application `RecordingId` and `EpochId` newtypes in `chronicle-common`, with `rec_`/`epoch_` display forms and no WAL wire wrapper leakage.
- [ ] 1.2 Add explicit `WalV1RecordingIdentity` compatibility adapters in `chronicle-wal`; make new WAL APIs accept `EpochId` and prevent parent `RecordingId` from entering epoch-local writers.
- [ ] 1.3 Define checksummed atomic `EpochCatalogV2`/`epochs.json` as sole parent-to-epoch topology authority; keep `recording-run.json`, global catalog, status, and sessions derived.
- [ ] 1.4 Define typed conflict/recovery rules for catalog, transition journal, run summary, WAL markers, checkpoints, and legacy one-epoch mapping; reject mtime/newest-file selection.
- [ ] 1.5 Add identity, serialization, authority-conflict, lineage-fork, and legacy compatibility tests.

## 2. Run and epoch persistence model

- [ ] 2.1 Add parent run/epoch persistence DTOs, explicit predecessor/successor links, bounded lifecycle summaries, and derived `recording-run.json` regeneration.
- [ ] 2.2 Separate whole-recording lifetime/deadline policy from epoch and WAL segment bounds; remove public implicit 600-second and 3,600-second recording-wide limits.
- [ ] 2.3 Route command, PID, cgroup, and daemon modes through one application-owned continuous coordinator while preserving supervised-child cleanup and attached-target non-termination.
- [ ] 2.4 Implement explicit stop/source-completion/deadline/fatal-failure semantics and deterministic parent list/inspect/status views.
- [ ] 2.5 Add run/epoch persistence and lifecycle tests for omitted duration, explicit deadline, child completion, SIGTERM, attached target survival, and restart continuation.

## 3. Epoch rollover transaction and recovery

- [ ] 3.1 Keep WAL v1 bytes, commit-marker authority, final-tail repair, segment bounds, and epoch-local physical limits unchanged while making parent identity external.
- [ ] 3.2 Implement parent-aware capture/topology `RolloverTransitionV2` phases `prepared -> successor_created -> boundary_committed -> topology_activated -> complete` for successor allocation, predecessor final marker, admitted-observation outcome, catalog activation, and quota reservation; include no ETL-continuation completion phase.
- [ ] 3.3 Implement deterministic recovery hierarchy: WAL markers for bytes, `epochs.json` for topology, transition journal for one in-flight proof, and run summaries/checkpoints as derived evidence.
- [ ] 3.4 Keep capture source/queue alive across rollover, account admitted observations, and stop visibly only on reservation, WAL/outcome handoff, topology, or activation safety failure; continuation/ETL lag SHALL affect processing readiness, not ordinary capture rollover.
- [ ] 3.5 Add capture-only rollover crash/fault tests for every phase, including predecessor WAL seal before successor activation, successor activation while continuation is pending, conflicting summaries/catalog/journal/WAL evidence, quota pressure, retention, and no duplicate successor; task 3 tests SHALL not require ETL catch-up.

## 4. Cross-epoch continuation checkpoint model

- [ ] 4.1 Define bounded `EpochContinuationCheckpointV1` and parent-aware `IncrementalEtlCheckpointV2` schemas with checksums, version/discriminator, decoder/schema/pipeline identity, predecessor/successor IDs, marker/segment lineage, limits, and output references.
- [ ] 4.2 Add ETL-owned `ContinuationState` (`pending`, `ready`, `consumed`, `unavailable`, `failed`); create pending after capture seals predecessor, catch up from `IncrementalEtlCheckpointV2`, export exactly at predecessor final marker, and make successor restore/recovery idempotent without gating WAL activation.
- [ ] 4.3 Support explicitly bounded connection/generation, TCP reassembly, partial protocol frame, correlation, negotiation/session, loss, and issue state without shared mutable memory.
- [ ] 4.4 Define completion-owner operation semantics, cross-epoch provenance ranges, terminal incomplete fallback, and explicit unsupported/invalid/bound-exhaustion provenance.
- [ ] 4.5 Add continuation round-trip, checksum/version/identity/lineage mismatch, crash-during-catch-up, ready-after-successor-WAL, invalid-handoff, state-exhaustion, and multi-rollover boundedness tests.

## 5. Incremental ETL and deterministic publication

- [ ] 5.1 Keep per-epoch snapshot/publication/checkpoint/retention boundaries while allowing capture to accumulate successor WAL ahead of `IncrementalProcessor`, which restores one verified predecessor continuation seed only when `ready`.
- [ ] 5.2 Publish completed operations and immutable continuation-out references before advancing v2 checkpoint or processed/retention state; adopt matching outputs without overwrite.
- [ ] 5.3 Preserve bounded reconstruction, loss provenance, deterministic correlation, and fail-closed decoder restore for HTTP and future protocol adapters.
- [ ] 5.4 Prove one-shot/incremental equivalence for same parent/epoch input, including operation IDs, completion owner, cross-epoch provenance, completeness, and issue order.
- [ ] 5.5 Add ETL tests for HTTP request/response rollover, long-lived TCP, pipelining/correlation, crash handoff, invalid continuation, bound exhaustion, multiple rollovers, lag, independent per-epoch readiness, and fork/gap recovery.

## 6. Immutable storage and canonical sessions

- [ ] 6.1 Extend immutable artifact keys/manifests with parent/epoch identity, continuation-in/out references, WAL ranges, schema/pipeline digests, and retention proof.
- [ ] 6.2 Publish one immutable Canonical Session v1 per finalized epoch; never mutate predecessor output; make completion-owner operations appear exactly once.
- [ ] 6.3 Define parent aggregate rebuild from verified epoch manifests/checkpoints and continuation references, without merging mutable decoder state or promoting WAL bytes.
- [ ] 6.4 Implement independent source-WAL/derived-artifact retention, pending/ready/unconsumed continuation dependency protection, digest-preconditioned deletion, tombstones, and orphan handling; source deletion requires downstream proof.
- [ ] 6.5 Add storage/session tests for continuation references, missing/conflicting artifacts, immutable retry, raw-WAL deletion, and parent aggregate convergence.

## 7. Replay planning and safety

- [ ] 7.1 Build deterministic parent/epoch replay plans from verified terminal canonical operations; treat continuation-only evidence as non-executable completeness metadata.
- [ ] 7.2 Preserve loopback-only target/effect authorization, dry-run defaults, credential/header safety, bounded execution, and complete per-operation results across multi-epoch plans.
- [ ] 7.3 Define replay ordering from epoch ordinal plus canonical timeline/provenance, never publication time or raw continuation timing.
- [ ] 7.4 Add replay tests for explicit epoch selection, parent aggregation, same connection across epochs, completion-owned operation, missing/invalid continuation, partial classification, policy denial, and WAL-independent execution.

## 8. CLI, diagnostics, and operations

- [ ] 8.1 Add public duration parsing, optional-deadline validation, compatibility warnings, request/result/error/rendering changes, and explicit parent/epoch output fields.
- [ ] 8.2 Expose per-epoch continuation state, checkpoint v1/v2 status, lag, quota, retention, rollover, `capture_readiness`, `processing_readiness`, `overall_health`, and safe remediation without payload values.
- [ ] 8.3 Preserve liveness/capture-readiness/processing-readiness/overall-health semantics and update systemd long-lived recording/runbook guidance.
- [ ] 8.4 Add static/rootless tests for config, status schemas, unit safety, diagnostics redaction, and continuation/rollover policy.

## 9. Documentation, acceptance, and release readiness

- [ ] 9.1 Update canonical CLI, architecture, WAL, ETL, recorder, operations, replay, migration, and troubleshooting documentation with the invariant `WAL epoch boundary != protocol reconstruction boundary`.
- [ ] 9.2 Update affected website English pages and matching `zh-tw`/`ja` pages, terminology, and localization verification for changed user-facing behavior.
- [ ] 9.3 Plan a concise `AGENTS.md` invariant update for unbounded recording, bounded epochs/segments, and lineage-verified continuation; do not duplicate detailed rationale there.
- [ ] 9.4 Add privileged acceptance scenarios for live multi-epoch capture while predecessor ETL lags, crash before successor activation, crash with active successor/pending continuation, continuation ready after accumulated successor WAL, invalid continuation, multiple independent ETL lags, bounded state, replay, quota, retention, and cleanup evidence.
- [ ] 9.5 Run portable and required Ubuntu 24.04 validation with retained machine-readable evidence; review the final diff and confirm no production implementation is included in this planning-only change.
