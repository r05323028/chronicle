## Context

See `proposal.md` for motivation. Inspection of main at `b1dd505` found the following current seams:

- `chronicle-application::record_production` creates one `GroupCommitWalWriter`, enforces `ProductionRecordingBounds.duration_seconds`, stops on `DurationLimit` or `WalSizeLimit`, then runs final ETL. Public command, PID, and cgroup paths still use this one-shot flow.
- `chronicle-application::record_continuous_ebpf` and `ContinuousRecorderService` already keep one capture source/orchestrator alive, persist `EpochCatalogV1` and `RolloverTransitionV1`, reserve successor quota, publish incremental ETL, and roll at `config.epoch.max_age_seconds`/`max_bytes`. Its epoch UUIDs currently occupy the `RecordingId` field and are presented as unrelated recordings by the public catalog.
- `EpochCatalogEntry` currently has ordinal, recording ID, predecessor, path, and state. `RolloverTransitionV1` has old/new epoch ordinals and old/new recording UUIDs. `IncrementalEtlCheckpointV1` currently binds a raw UUID, epoch ordinal, marker lineage, segment lineage, decoder state, and immutable output references; that raw UUID is an epoch/WAL identity seam, so new parent-aware continuation requires typed `IncrementalEtlCheckpointV2` rather than reusing V1 semantics.
- The public CLI parser applies a 600-second default and rejects `--duration` above 3,600 seconds. `ProductionRecordingBounds` mixes recording duration, segment size, and total WAL bytes. `recording_catalog` assumes one recording maps to at most one published session.
- Existing WAL v1 headers/envelopes carry one UUID called `recording_id`, and recovery validates that identity. Commit markers remain the only durability authority. Existing docs state that the public one-shot path stops on duration/WAL limits and that Kubernetes packaging is future work.

The design must preserve current crate ownership and the repository rule that WAL markers, not metadata/catalog/checkpoints, decide committed bytes. It must also preserve the existing bounded queue, segment rotation, final-tail repair, deterministic ETL, no-replace publication, deny-by-default replay, and exact-domain lease.

## Goals / Non-Goals

**Goals:**

- Give users one stable recording/capture-run identity that can own an ordered, potentially unbounded epoch lineage.
- Keep every epoch and segment operationally bounded and independently recoverable.
- Reuse `ContinuousRecorderService`/`RecorderOrchestrator` as the sole long-lived capture/rollover owner for command, PID, cgroup, and daemon modes.
- Make age/byte thresholds rollover causes, never ordinary recording terminal reasons.
- Let finalized epochs progress through incremental ETL, immutable storage, checkpointing, and retention while the successor captures.
- Define deterministic list/inspect/replay views, restart continuation, quota pressure, retention, and every rollover crash point.
- Preserve 0.1.x artifacts and hidden compatibility paths without preserving confusing public defaults.

**Non-Goals:**

- One forever-growing WAL, unbounded in-memory reconstruction, or one canonical session spanning unlimited epochs.
- Unbounded cross-epoch decoder memory. A WAL epoch is not inherently a protocol reconstruction boundary; supported state crosses it only through bounded continuation artifacts.
- New protocols, eBPF ABI changes, TLS decryption, replay authorization broadening, remote active WAL, distributed coordination, a network control API, or a Kubernetes operator.
- A second recorder/rollover implementation beside the existing continuous-recorder service.
- Silent deletion, best-effort lineage reconstruction, or a large replacement value for the removed recording-wide duration cap.

## Architectural invariants

- Recording lifetime may be unbounded; epoch and segment lifetimes remain bounded.
- A WAL epoch boundary is a durability, recovery, quota, retention, and publication boundary, not inherently a protocol reconstruction boundary.
- Cross-epoch protocol state may continue only through bounded, versioned, checksummed, lineage-verified continuation checkpoints. No protocol state is inferred from wall-clock order, filesystem mtime, or an unrelated epoch.
- Finalized epoch outputs remain immutable. A continuation-only handoff is immutable evidence; it never permits mutation of an already-authoritative session.

## Decisions

### 1. Model three explicit lifetimes

The application owns this hierarchy:

```text
Recording / Capture Run (stable RecordingId; user-visible)
  lifecycle: starting -> recovering -> running -> draining -> stopped|failed
  optional whole-run deadline; explicit stop/failure conditions
  |
  +-- Epoch (unique EpochId; monotonically ordered ordinal)
  |     bounded age and bytes; one WAL identity and ETL lineage
  |     +-- Segment (ordinal/path; existing bounded WAL unit)
  |     +-- Incremental checkpoint and immutable epoch outputs
  |
  +-- Epoch ...
```

Responsibilities are deliberately non-overlapping:

| Level | Owns | Must not own |
| --- | --- | --- |
| Recording/run | user lifecycle, source/supervision policy, stable identity, whole-run deadline, aggregate counters/status, ordered epoch index | WAL commit authority, segment repair, protocol decoding |
| Epoch | bounded WAL quota/age, epoch transition/recovery, per-epoch ETL/checkpoint/session, retention eligibility, predecessor lineage | whole-run stop merely because a threshold is reached |
| Segment | append/commit/recovery unit, size/age rotation, fixed WAL v1 framing and marker authority | recording/run identity or ETL publication |

An epoch threshold requests rollover. A source-completion, user-stop, optional recording deadline, unrecoverable capture/runtime error, or explicit end intent requests recording shutdown. `epoch_rollover`, `segment_rotation`, and `WalSizeLimit` are not public terminal reasons for the new public lifecycle; a capacity failure that prevents a safe successor is a typed recording failure.

### 2. Separate public run identity, epoch identity, and authority

`chronicle-common` owns application-facing `RecordingId` and `EpochId` newtypes. `RecordingId` is the stable public parent/capture-run identity. `EpochId` is the unique bounded durability/recovery identity with ordinal and predecessor. They are never aliases, even when a legacy one-epoch mapping contains equal UUID bytes. New application, catalog, session, checkpoint, and diagnostic APIs use the typed fields explicitly.

`chronicle-wal` owns a private/internal `WalV1RecordingIdentity` adapter for the legacy v1 `recording_id` wire field. The adapter serializes an `EpochId` into unchanged v1 bytes and maps a recovered v1 UUID back to an epoch identity only through an explicit application-supplied epoch descriptor. New code SHALL NOT pass a parent `RecordingId` to a WAL writer or interpret a WAL `recording_id` as a parent. WAL-owned wire wrappers do not cross into CLI, canonical, replay, or user-facing APIs.

The current `RecordingId`-typed WAL/application seams are compatibility seams, not permission to reuse the type. New APIs accept `EpochId` for epoch-local WAL operations and `RecordingId` plus `EpochId` for parent-aware operations. `SourceProvenance.recording_id` means parent recording for new artifacts; additive `epoch_id`, `epoch_ordinal`, and epoch/WAL-range fields identify the source epoch. New multi-epoch incremental checkpoints use `IncrementalEtlCheckpointV2`; `IncrementalEtlCheckpointV1` remains a read-only legacy artifact whose serialized `recording_id` is explicitly interpreted as legacy WAL/epoch identity, never as parent identity.

Serialization/display is explicit: public parent references render `rec_<full-uuid>`; operational epoch references render `epoch_<full-uuid>`; legacy WAL v1 fields remain bare UUID bytes/strings only inside compatibility readers and WAL manifests. Public parent resolution never accepts an epoch reference. A legacy one-epoch recording maps `parent_recording_id == epoch_id` by an explicit `LegacyOneEpochMapping`, while preserving distinct types and conversion points.

The application-level parent-to-epoch topology has exactly one authority: checksummed atomic `epochs.json` (`EpochCatalogV2` for new runs, with a read-only V1 compatibility reader). It owns parent ID, epoch IDs/ordinals, predecessor links, paths, lifecycle topology, and topology digests. `recording-intent.json` owns one-time parent allocation only. `recording-run.json` is a derived lifecycle/status/aggregate summary and MUST NOT select active epochs, repair topology, or participate in WAL recovery. Global `catalog.json`, status metadata, session manifests, and checkpoints are derived indexes/evidence for their own domains, not alternate topology authorities.

`RolloverTransitionV2` is an in-flight transaction journal, not a second lineage source. It may complete or roll back one prepared catalog transition only when its parent/epoch IDs, phase, checksums, and WAL evidence match `epochs.json` and the commit-marker authority. It cannot introduce an unproven topology. Recovery applies this hierarchy:

1. WAL commit markers decide which WAL bytes are committed.
2. Valid `epochs.json` decides parent/epoch topology and active/prepared relationships.
3. The transition journal proves or rolls back an in-flight topology change under the lease.
4. Run summaries, status, checkpoints, sessions, and global indexes are rebuilt or marked inconsistent from those stronger authorities.

A run-summary disagreement is repaired by regenerating the summary from `epochs.json`, unless its stable parent ID conflicts, in which case recovery fails closed. A catalog/journal disagreement is completed or rolled back only from exact transition phase plus WAL/metadata evidence; conflicting IDs, digests, ordinals, or paths fail closed. A journal/WAL disagreement never promotes the journal: a claimed marker absent from WAL fails closed, while a WAL marker ahead of stale derived journal state may advance derived transition/catalog metadata only after exact identity and digest validation. No case chooses by mtime, newest file, or process existence.

Use a canonical layout for new public runs:

```text
<data-dir>/recordings/<recording-id>/
  recording-intent.json          # one-time parent allocation/name
  recording-run.json             # derived lifecycle/deadline/status summary
  epochs.json                    # sole authoritative parent -> epoch topology
  epochs/<ordinal>-<epoch-id>/
    recording.json               # epoch/WAL identity and epoch metadata
    segments/*.chwal
    incremental-etl-checkpoint-v2.json
    continuation-in.json         # immutable seed from unique predecessor, if any
    continuation-out.json        # immutable seed for unique successor, if any
    epoch-outcomes.json
```

The daemon may use its configured WAL root as the parent run root; it must not create a second competing run. A legacy directory without `epochs/` is a one-epoch run whose parent and epoch IDs are linked by the explicit compatibility mapping. A legacy continuous catalog is grouped into one parent only when its predecessor/ordinal/path evidence proves one chain; unrelated UUID directories are never guessed into one run.

Canonical session IDs remain independent. Each finalized epoch publishes one deterministic session derived from parent ID, epoch ID/ordinal, pipeline version, and that epoch's verified output. A session manifest may reference immutable continuation-in/out artifacts. Legacy one-epoch sessions keep their existing ID/provenance fallback.

### 3. Replace mixed recording bounds with deadline plus epoch/segment bounds

Define application-owned inputs equivalent to:

```text
RecordingLifetime { deadline: Option<Duration>, stop_policy }
EpochBounds       { max_age_seconds, max_bytes }
SegmentBounds     { max_age_seconds, max_bytes }
```

`RecordingLifetime.deadline = None` means no time-based run stop. An explicit deadline is checked against a monotonic clock for the whole run and is not reset at rollover. The CLI parser accepts checked positive durations such as `10m` and `24h`; it rejects zero, arithmetic overflow, and values that cannot be represented by the runtime clock. It does not impose a substitute arbitrary multi-day recording cap.

`EpochBounds` retain the existing safety ceilings: default epoch age may remain one hour and epoch bytes may remain the 4 GiB physical hard maximum. Configured lower values remain valid. `SegmentBounds` retain existing segment minimum/maximum and age behavior. The WAL writer's total limit applies to the active epoch only; the parent run has no WAL-size limit. Global filesystem quota, retention, ETL progress, and lifecycle-index bounds remain the resource ceiling.

Remove `DEFAULT_RECORDING_DURATION_SECONDS` and `MAX_RECORDING_DURATION_SECONDS` from the public lifecycle path. A deprecated hidden legacy adapter may retain the old 600/3,600 behavior for the 0.1.x one-shot syntax, but it must not feed the new run coordinator or be described as the public default.

### 4. Make one continuous coordinator serve every capture mode

`ContinuousRecorderService` becomes the parent-run coordinator; `RecorderOrchestrator` remains the source plus bounded ingest owner. Public command mode supplies a supervised-scope source and child lifecycle callbacks. PID/cgroup mode supplies an attached source and a no-termination detach callback. Daemon mode supplies its configured selector. All modes use the same domain lease, run allocation, epoch catalog, rollover transition, incremental ETL coordinator, quota authority, finalization, and catalog publication.

The old `record_production` one-shot loop remains only as a hidden compatibility implementation until its deprecation boundary, or becomes a thin adapter over the same coordinator if that is smaller. New public paths must not maintain a separate duration/WAL-stop loop.

Source behavior is explicit:

| Condition | Command mode | PID/cgroup mode | Daemon mode |
| --- | --- | --- | --- |
| child/source completes | end run with `source_completed`; bounded scope cleanup | not applicable; continue until stop/failure | not applicable; continue until stop/failure |
| first SIGINT/SIGTERM/user stop | drain run; clean owned child scope with TERM/KILL policy | drain and detach only; target remains alive | drain/end or persist continuation intent per service policy |
| whole-run deadline | drain; clean owned child scope | drain/detach; target remains alive | drain/end |
| fatal capture/runtime/storage failure | fail run; clean owned scope | fail run; never kill target | fail run; supervisor may restart/recover |
| epoch age/bytes | rollover only | rollover only | rollover only |

A command child is never released before capture attachment. Existing bootstrap hardening, child output isolation, and cleanup deadlines remain. Attached selectors never send TERM/KILL to the selected process or cgroup. The domain lock remains held through run finalization, all epoch ETL/publication required by the shutdown policy, and parent catalog update.

### 5. Use one durable rollover transaction

Rollover is initiated by the coordinator when the active epoch's fake-clock age or physical bytes reaches its configured threshold, reserving enough capacity for the next header, first/final marker, continuation handoff, manifest/checkpoint staging, and concurrent ETL peak. It does not detach the capture source and does not reset protocol reconstruction by default.

The durable sequence is:

1. Stop admission to the old epoch at the coordinator boundary while retaining the source and bounded queue; record accepted-observation ordinals, payload bytes, timestamps, and source metadata.
2. Persist a checksummed `RolloverTransitionV2` containing parent ID, old/new epoch IDs and ordinals, old/new paths, reservation identity, and phase `prepared`.
3. Create and sync the successor directory and WAL header; advance to `successor_created` only after identity and quota reservation validate.
4. Drain capacity-qualified admitted observations to the old writer, append/flush/sync its final marker, persist exact outcome ranges for every admitted ordinal, and advance to `boundary_committed`. An ordinal is assigned to an epoch only after that epoch's authoritative marker covers it; otherwise it becomes metadata-only `epoch_rollover_failure` evidence.
5. Export a bounded `EpochContinuationCheckpointV1` from the predecessor's decoder at the exact committed marker. It binds the parent, predecessor, allocated successor, decoder/schema/pipeline versions, segment/marker lineage, and a checksum. Persisting it advances the transition to `continuation_checkpointed`; inability to serialize, bound, or verify it is an explicit rollover failure or typed continuation loss, never an implicit clean reset.
6. Persist old-epoch final metadata and the authoritative catalog transition, publish successor metadata including the continuation seed reference, and atomically activate the successor. Advance to `metadata_published` only when catalog, metadata, continuation identity, and lineage agree.
7. Switch the coordinator's active writer/epoch state, enqueue the sealed old epoch for incremental/final ETL, seed successor ETL only from the verified predecessor continuation artifact, and remove the transition journal after directory sync.

The source remains attached throughout. The only intentional pause is bounded admission handoff; no planned detach/reattach gap is introduced. A continuation seed is a serialized handoff, not shared mutable decoder memory. If reservation, successor creation, outcome persistence, continuation export, or activation fails, the coordinator stops accepting new observations and fails visibly rather than looping empty epochs, silently resetting a connection, or continuing without an owner.

### 6. Define recovery as a deterministic transition decision

Recovery owns the lease and quota authorities before mutating `epochs.json`, transition, WAL, checkpoint, continuation, or retention state. It first validates WAL commit-marker authority, then the authoritative epoch catalog, then the in-flight transition and derived summaries. It never chooses an epoch by newest mtime, PID existence, or filesystem order.

| Crash point | Recovery result |
| --- | --- |
| writing active epoch | Recover the final segment with existing verified-tail-only repair; keep its committed prefix; resume the same epoch if appendable, otherwise finalize it as failed/aborted and create one linked successor. ETL retries from exact checkpoint/continuation lineage. |
| before rollover journal/transition | Old active epoch remains authoritative; threshold is retried. No empty successor is catalogued. |
| after successor allocation/header, before old finalization | If successor has no metadata/committed evidence, release reservation and roll back the prepared directory. If evidence exists, do not delete it; complete or fail closed from exact transition/catalog/WAL proof. |
| after predecessor finalization, before continuation checkpoint | Predecessor marker remains authoritative. Re-export the same bounded continuation from the same decoder cursor, or fail closed with explicit continuation-loss evidence; never activate a clean successor that hides pending state. |
| after continuation checkpoint, before successor active | Verify checksum, parent/predecessor/successor IDs, marker/segment lineage, and transition phase. Complete exactly one catalog activation and seed, or preserve both artifacts and fail closed on contradiction. |
| ETL processing previous epoch | Capture continues in the active successor while the old epoch remains finalized but retention-ineligible. ETL consumes the unique continuation seed, publishes completed operations, emits the next seed, and advances checkpoints only after immutable outputs are verified. |
| run summary disagrees with `epochs.json` | Rebuild the derived summary from the authoritative catalog under the lease; stable parent-ID conflict is a recovery failure. |
| transition disagrees with `epochs.json` or WAL | Use catalog topology and WAL markers as authority. Complete/rollback only with exact phase and digest proof; otherwise preserve evidence and remain not ready. |
| recorder process restart | A persisted non-terminal run with matching scope/config/domain identity is recovered under the same parent ID. Active epoch and continuation seed resume or link to exactly one successor; no new public recording is allocated merely because process-attempt ID changed. |
| host/pod restart | Persistent state/WAL/catalog are treated like process crash. A restart without durable end intent resumes lineage; a durable explicit end intent prevents continuation and leaves the run stopped. |

The parent run becomes terminal only after all required active-epoch finalization and downstream publication/retention decisions meet shutdown policy. A crash must not be relabeled as graceful completion. Recovery never silently replaces an invalid continuation checkpoint with an empty decoder; it records typed incomplete/loss provenance and keeps the affected evidence protected.

### 7. Keep incremental ETL per epoch while preserving protocol continuation

Each epoch remains an independent WAL snapshot, immutable publication, checkpoint, and retention unit, but it is not an independent protocol reconstruction universe. `IncrementalProcessor` may initialize from exactly one verified predecessor `EpochContinuationCheckpointV1` and MUST emit a bounded successor continuation checkpoint at its own committed boundary. Continuation is serialized, versioned, checksummed, bounded, and lineage-verified; it never shares mutable decoder memory across workers or epochs.

`IncrementalEtlCheckpointV1` remains a read-only compatibility artifact for legacy one-epoch/epoch-local state. New parent-aware processing uses `IncrementalEtlCheckpointV2`, whose typed fields bind parent `RecordingId`, current `EpochId`/ordinal, optional unique predecessor/successor IDs, input/output continuation digests, exact marker/segment lineage, decoder/schema/pipeline versions, bounded decoder state, and publication keys. `EpochContinuationCheckpointV1` contains only explicitly supported bounded connection identity/generation, TCP/reassembly state, partial protocol frames, correlation/negotiation state, loss evidence, limits/counters, and its predecessor/successor marker proofs. Unknown versions, unrelated predecessor, digest/marker gaps, checksum failure, schema/pipeline mismatch, and bound overflow fail closed.

Operation ownership is completion-based: a logical operation belongs to the epoch in which its protocol decoder reaches terminal completion. The additive canonical provenance contract SHALL expose `completion_owner_epoch_id`, typed contributing `epoch_wal_ranges` (each retaining the existing segment/sequence/byte range fields), and immutable continuation references; legacy single-epoch `wal_ranges` remain readable. If it completes in N+1, N+1 emits exactly one operation with provenance ranges in both epochs and a deterministic ID derived from parent, connection/generation, correlation lineage, and completion owner. If the run ends before completion, the final epoch that owns the last trusted evidence emits exactly one incomplete operation. Already-published predecessor sessions are never mutated. Continuation-only manifests remain inspectable immutable evidence and are excluded from replay operation plans.

After rollover, the old epoch is finalized and may publish completed operations plus its immutable continuation-out reference while the successor captures. Successor ETL seeds only from the unique verified predecessor; if the protocol adapter does not support continuation, or state is unavailable/corrupt/over limit, it records explicit `cross_epoch_continuation_unavailable`/incomplete provenance and does not claim a clean connection or operation start. Parent aggregation orders verified epoch sessions and terminal operations by epoch ordinal/capture provenance without merging mutable decoder state.

On run shutdown, the coordinator stops intake, finalizes the active epoch, lets in-flight epoch publications reach a safe boundary, emits terminal incomplete operations for unresolved continuation, and then persists parent terminal metadata. If an epoch cannot publish within the configured drain policy, the run is `recoverable`/`failed` with protected WAL and a retry path; it is not reported as fully published.

### 8. Preserve quota, retention, and bounded lineage

Quota reservation covers current epoch, reserved successor, segment/manifest/checkpoint peaks, in-flight ETL/store staging, final session publication, and same-filesystem cleanup trash. Every epoch rollover must reserve successor capacity before mutation. Reservations are released/consumed only from durable transaction evidence and rebuilt from filesystem state after restart.

`retain` mode keeps finalized raw epochs and eventually stops/fails capture when no protected data can be deleted. `delete_after` can delete only finalized epochs whose committed WAL, ETL checkpoint, immutable outputs, final epoch session, retention age, and cleanup tombstone satisfy policy. Parent lineage, aggregate session references, and deletion proof remain after raw segment deletion. Unprocessed, corrupt, uncertain, failed, or protected epochs are never deleted to make room.

Epoch catalog/lifecycle index compaction is deterministic and bounded. It preserves first/last retained IDs, ordinal range, predecessor/digest-chain proof, cleanup/tombstone summary, aggregate counts, and active tail. If protected lineage cannot fit after legal compaction, stop creating epochs with typed metadata-limit failure; never silently drop lineage.

### 9. Public views and replay use the parent aggregate

`list` returns one row per stable parent recording, not one row per epoch. Existing ID/name/created/status fields remain; `sessions` and `operations` aggregate verified terminal operations from published epoch sessions, and additive `epoch_count`, `active_epoch`, and `published_epoch_count` fields expose bounded lifecycle state. Continuation-only evidence is counted in completeness/warnings, never as an executable operation. In-progress duration is reported as nullable or explicitly elapsed according to the output contract, never as a false final duration. Ordering remains parent creation time then stable ID.

`inspect RECORDING` prints parent lifecycle/deadline/stop reason, aggregate counts, and an ordered bounded epoch summary: public epoch number, epoch ID, state, committed range/bytes, ETL/checkpoint/publication state, continuation in/out state, retention state, completeness, and warnings. It does not scan unbounded raw history or print payloads. Read-only list/inspect may reconcile in memory while a live owner holds the domain lock.

`replay RECORDING` resolves the parent, verifies all selected epoch session manifests and continuation references, and builds an application-owned ordered multi-session plan from terminal canonical operations. A stopped/published run selects every epoch in ordinal order. An active run may select only the contiguous immutable finalized prefix and must report the active tail as not selected; it must never silently skip a failed or missing middle epoch. Any gap, contradictory lineage, invalid continuation reference, or non-replayable selected operation causes a no-traffic denial unless the user explicitly selects a valid epoch/session. The replay executor runs terminal operations in deterministic order; it does not decode raw continuation fragments, recreate cross-epoch timing, or infer a new request/response. A predecessor session with continuation-only evidence is independently inspectable but contributes no duplicate replay operation. Command-mode target supervision and explicit-target non-termination/authorization rules remain unchanged.

### 10. Keep crate boundaries explicit

- `chronicle-common` owns neutral application `RecordingId`/`EpochId` primitives, display/parsing, and no WAL wire types.
- `chronicle-wal` owns epoch-local framing, marker authority, segment rotation, recovery, manifests, and private `WalV1RecordingIdentity` compatibility adapters; it does not know parent run policy.
- `chronicle-etl` owns bounded continuation checkpoint encoding/restore, complete extraction/transformation, per-epoch publication/checkpoint ordering, and storage publication; it does not own CLI or run stop policy.
- `chronicle-storage` owns immutable epoch artifacts, continuation references, session persistence, and read-only summaries; it does not decide WAL authority or rollover.
- `chronicle-replay` remains independent of WAL/ETL/capture; application supplies verified terminal canonical operations and parent/epoch provenance through application-owned replay requests/results.
- `chronicle-application` composes parent lifecycle, source modes, transition, quota, continuation handoff, ETL, authoritative epoch catalog, inspect, and replay selection.
- `chronicle-cli` parses optional duration/selectors and renders application-owned results only.

No new crate, dependency edge, eBPF exposure, or CLI-to-lower-layer vocabulary is needed.

## Risks / Trade-offs

- **[Stable identity migration]** Existing epoch UUIDs occupy `RecordingId` fields and public catalog rows. → Keep WAL v1 bytes untouched, map provable legacy chains under a parent manifest, preserve one-epoch ID equality, and fail closed on ambiguous grouping.
- **[Long-lived run metadata]** A parent index can grow for years. → Bound entries/bytes, compact only protected-proof-safe history, and stop before losing lineage.
- **[ETL lag]** Finalized epochs can accumulate while capture continues. → Expose per-epoch lag, reserve ETL peaks, retain inputs, and stop at quota pressure instead of dropping observations.
- **[Boundary loss]** Admission can race a writer handoff. → Stable admitted ordinals plus durable outcome ranges; unresolved ranges are exact metadata-only rollover loss and fail the run.
- **[Partial replay]** A live run has an immutable prefix and a mutable tail. → Default replay selects only a contiguous published prefix, labels omission, and rejects gaps; no implicit skip.
- **[Supervised child behavior]** Removing the default deadline can leave command mode running until its child exits. → Keep explicit `--duration`, Ctrl-C handling, bounded TERM/KILL cleanup, and safe JSON behavior.
- **[Quota policy]** Retain mode cannot support infinite physical capture. → State clearly that logical recording is unbounded but storage is not; controlled stop/failure is correct behavior.
- **[Service restart intent]** A clean supervisor restart can look like an explicit stop. → Persist an explicit end/continue intent and document the supervisor contract; crash/host loss without end intent resumes, explicit end does not.
- **[Canonical compatibility]** Per-epoch sessions add provenance and continuation references. → Use append-only v1 optional metadata where safe, introduce parent-aware checkpoint v2 rather than overloading v1, preserve existing validation/session publication, and keep legacy readers valid.
- **[Cross-epoch continuation]** A protocol operation may straddle immutable epoch publications. → Use completion-owner semantics, immutable continuation-in/out artifacts, bounded v2 checkpoints, and explicit incomplete provenance on unsupported/invalid handoff; never mutate predecessor sessions.

## Migration Plan

1. Add typed parent/epoch identities, explicit WAL v1 compatibility adapters, authoritative `EpochCatalogV2`, derived run summaries, and legacy one-epoch readers.
2. Add run/epoch persistence and deadline/epoch/segment policy separation without changing the legacy hidden one-shot adapter.
3. Move public command/PID/cgroup modes onto the existing continuous coordinator behind deterministic fake sources and supervised/attached lifecycle adapters.
4. Extend rollover transition/recovery with authoritative catalog conflict handling, successor reservation, predecessor continuation export, and deterministic handoff recovery.
5. Add bounded continuation/checkpoint v2, completion-owner operation provenance, per-epoch immutable session publication, and one-shot equivalence before broad ETL/CLI aggregation changes.
6. Route finalized epochs through incremental ETL/storage while successor capture remains active; add parent aggregation and bounded retention/compaction.
7. Update list/inspect/replay/status application contracts and CLI rendering; retain hidden 0.1.x syntax with warnings until the documented 0.2 removal boundary.
8. Update systemd, CLI/concepts/architecture/ETL/troubleshooting docs, localized website pages, acceptance scenarios, and plan the concise `AGENTS.md` invariant update.
9. Run deterministic rootless validation, then changed-path validation and supported Ubuntu 24.04 privileged recorder evidence. Do not claim Linux/eBPF completion from macOS.

Rollback stops the run gracefully when possible, preserves parent/epoch/WAL/checkpoint/store artifacts, and restores the prior binary only if it can validate the same mutable-v1 state. If not, use a separate state root or matching reader; never delete or relabel lineage during rollback.

## Open Questions

No product-scope question is left unresolved. This change chooses completion-owner semantics for cross-epoch operations: the epoch where decoding reaches terminal completion owns the immutable canonical operation; unresolved state is emitted once as terminal incomplete evidence. Implementation may tune non-semantic operational recommendations (segment-age defaults, lag warnings, restart backoff, and shutdown grace) through tests and documentation without changing identity, authority, continuation, stop, rollover, recovery, quota, or replay rules.
