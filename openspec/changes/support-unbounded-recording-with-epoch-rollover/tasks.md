## 1. Identity and compatibility foundations

- [ ] 1.1 Add neutral parent `RecordingId`/`EpochId` contracts and deterministic formatting without exposing WAL-layer types across crate boundaries.
- [ ] 1.2 Add versioned parent run manifest, epoch metadata, and bounded lifecycle-index models with checksums, predecessor/successor lineage, and explicit lifecycle states.
- [ ] 1.3 Map legacy one-epoch directories and provable legacy epoch chains into parent/epoch views; reject ambiguous grouping and preserve existing artifact readers.
- [ ] 1.4 Add deterministic identity, lineage, fork, gap, digest-conflict, and compatibility tests.

## 2. Public lifecycle and orchestration

- [ ] 2.1 Separate whole-recording lifetime/deadline policy from epoch and WAL segment bounds; remove public implicit 600-second and 3,600-second recording-wide limits.
- [ ] 2.2 Route command, PID, cgroup, and daemon recording modes through one application-owned continuous coordinator while preserving supervised-child cleanup and attached-target non-termination.
- [ ] 2.3 Implement explicit stop/source-completion/deadline/fatal-failure semantics and stable terminal reasons for parent recordings.
- [ ] 2.4 Add public duration parsing, optional-deadline validation, compatibility warnings, request/result/error/rendering changes, and deterministic list/inspect aggregation.
- [ ] 2.5 Add lifecycle tests for omitted duration, explicit deadline, child completion, SIGTERM, attached target survival, fatal failure, and restart continuation.

## 3. Epoch WAL rollover and recovery

- [ ] 3.1 Extend epoch-local WAL configuration and manifest APIs so size/age limits apply per epoch while preserving WAL v1 framing, commit-marker authority, final-tail repair, and segment bounds.
- [ ] 3.2 Implement crash-safe rollover transition journal, successor allocation, quota reservation, predecessor final marker, metadata publication, and successor activation with no overlapping writers.
- [ ] 3.3 Keep capture source and bounded ingest ownership alive across planned rollover; account for admitted observations and explicit rollover-boundary loss evidence.
- [ ] 3.4 Implement deterministic startup recovery for every rollover phase, process crash, host restart, transition fork, missing successor, and contradictory catalog/marker case.
- [ ] 3.5 Add quota, retention, lifecycle-index compaction, stale-state, and rollover fault-injection tests proving no accepted committed data is silently dropped or deleted.

## 4. Per-epoch incremental ETL

- [ ] 4.1 Make incremental ETL consume immutable recovery-authoritative epoch snapshots while standalone ETL retains stopped-recording/live-writer behavior.
- [ ] 4.2 Bind `IncrementalEtlCheckpoint v1`, decoder snapshots, immutable output keys, and checkpoint lineage to parent ID plus epoch ID/ordinal and exact marker/segment digests.
- [ ] 4.3 Preserve bounded TCP/HTTP reconstruction, loss provenance, source lifecycle status, and fail-closed restore behavior independently for each epoch; never carry decoder state across epochs.
- [ ] 4.4 Implement publication-before-checkpoint ordering, matching-output adoption, conflict detection, checkpoint repair, processed transitions, and epoch retention proofs.
- [ ] 4.5 Implement deterministic parent aggregation from verified epoch outputs without changing epoch Canonical Session v1 bytes or one-shot semantics.
- [ ] 4.6 Add snapshot concurrency, checkpoint round-trip, WAL fork/gap, crash-boundary, lag/backpressure, multi-epoch, and incremental/one-shot byte-equivalence tests.

## 5. Immutable storage and retention

- [ ] 5.1 Extend immutable artifact identity and manifests with parent/epoch/WAL provenance, schema/pipeline digests, retention class, and exact integrity metadata.
- [ ] 5.2 Preserve `RecordingStore`/`FilesystemSessionStore` ownership boundaries and add deterministic parent aggregate/index publication and rebuild from verified epoch references.
- [ ] 5.3 Implement independent source-WAL and derived-artifact retention eligibility, digest-preconditioned deletion, tombstones, orphan handling, and successor-safe cleanup.
- [ ] 5.4 Add filesystem RecordingStore conformance and fault-injection tests for epoch keys, idempotent retry, conflicting retry, uncertain durability, lifecycle, and forbidden authority bypass.

## 6. Canonical sessions and replay

- [ ] 6.1 Publish one deterministic Canonical Session v1 per finalized epoch with append-only parent/epoch provenance and verified manifest/checksum metadata.
- [ ] 6.2 Extend inspect and parent/session selection to distinguish parent recording, epoch, and session identities and report missing, pending, failed, or expired epochs honestly.
- [ ] 6.3 Build deterministic parent replay plans from verified ordered epoch sessions through existing application-owned replay interfaces; reject gaps and never synthesize cross-epoch protocol state.
- [ ] 6.4 Preserve loopback-only target/effect authorization, dry-run defaults, credential/header safety, bounded execution, and complete per-operation results across multi-epoch plans.
- [ ] 6.5 Add replay tests for explicit epoch selection, parent aggregation, repeated operations across epochs, missing/invalid epochs, rollover-boundary incompleteness, policy denial, and WAL-independent execution.

## 7. Diagnostics and operations

- [ ] 7.1 Extend recorder status, doctor probes, ETL summaries, and JSON/human rendering with parent/current/prior epoch identity, marker watermarks, per-epoch lag, rollover state, quota, retention, and safe remediation codes.
- [ ] 7.2 Preserve liveness/capture-readiness/processing-readiness/overall-health semantics and expose stale, recovering, lagging, quota-blocked, and failed states without payload values.
- [ ] 7.3 Update supported systemd operation, restart/stop, resource, quota, retention, crash recovery, and long-lived recording runbooks; keep unsupported deployment models explicit.
- [ ] 7.4 Add static/rootless tests for config, status schemas, unit safety, rollover policy, diagnostics redaction, and runbook command contracts.

## 8. Privileged acceptance and evidence

- [ ] 8.1 Add acceptance scenarios for multi-epoch live capture, concurrent finalized-epoch ETL, recorder/ETL crash restart, forced transition failure, quota pressure, retention, and leak cleanup.
- [ ] 8.2 Run portable validation for deterministic lifecycle, WAL, ETL, storage, session, replay, CLI, and diagnostic contracts with retained machine-readable evidence.
- [ ] 8.3 Run required Ubuntu 24.04 privileged recorder/live-capture acceptance through the documented entrypoint and retain fingerprint, source provenance, environment, lifecycle, checksums, and checked/not-checked claims.

## 9. Documentation and release readiness

- [ ] 9.1 Update canonical CLI, architecture, WAL, ETL, recorder, operations, replay, migration, and troubleshooting documentation for parent/run versus epoch versus segment semantics.
- [ ] 9.2 Update affected website English pages and matching `zh-tw`/`ja` pages, terminology, and localization verification for changed user-facing behavior.
- [ ] 9.3 Review `AGENTS.md`, crate-boundary policy, validation architecture/catalogs, and OpenSpec specs; update only durable repository-wide invariants and run strict validation.
- [ ] 9.4 Review implementation diff and acceptance evidence, confirm no production implementation is included in this planning-only change, and archive only after implementation verification.
