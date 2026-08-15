## Context

See `proposal.md` for motivation. Inspection of main at `b1dd505` found the following current seams:

- `chronicle-application::record_production` creates one `GroupCommitWalWriter`, enforces `ProductionRecordingBounds.duration_seconds`, stops on `DurationLimit` or `WalSizeLimit`, then runs final ETL. Public command, PID, and cgroup paths still use this one-shot flow.
- `chronicle-application::record_continuous_ebpf` and `ContinuousRecorderService` already keep one capture source/orchestrator alive, persist `EpochCatalogV1` and `RolloverTransitionV1`, reserve successor quota, publish incremental ETL, and roll at `config.epoch.max_age_seconds`/`max_bytes`. Its epoch UUIDs currently occupy the `RecordingId` field and are presented as unrelated recordings by the public catalog.
- `EpochCatalogEntry` currently has ordinal, recording ID, predecessor, path, and state. `RolloverTransitionV1` has old/new epoch ordinals and old/new recording UUIDs. `IncrementalEtlCheckpointV1` already binds recording ID, epoch ordinal, marker lineage, segment lineage, decoder state, and immutable output references.
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
- Cross-epoch decoder/connection state. A connection observed across a boundary remains separately and honestly represented in each epoch.
- New protocols, eBPF ABI changes, TLS decryption, replay authorization broadening, remote active WAL, distributed coordination, a network control API, or a Kubernetes operator.
- A second recorder/rollover implementation beside the existing continuous-recorder service.
- Silent deletion, best-effort lineage reconstruction, or a large replacement value for the removed recording-wide duration cap.

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

### 2. Separate public run identity from legacy WAL epoch identity

`RecordingId` remains the stable public parent ID and is allocated once before capture attachment. Introduce a distinct `EpochId` in application/common identity contracts. Each epoch receives one `EpochId`, one ordinal, one predecessor, and one WAL directory. The successor is the unique next catalog entry after predecessor validation; the transition journal carries both sides, so two independent successor fields cannot disagree.

Do not rewrite existing WAL v1 bytes. The UUID currently serialized as `recording_id` in a segment header/envelope is treated by the continuous path as the epoch's WAL identity (`EpochId`) and remains validated exactly as today. The parent `RecordingId` is carried by the parent run manifest, epoch catalog, per-epoch metadata, ETL checkpoint, immutable artifact provenance, and canonical session provenance. WAL remains independent of application-level run aggregation.

Persist a parent run manifest beside the recording intent/catalog. It contains stable run ID, source mode/scope identity, created/ended times, optional deadline, lifecycle/stop reason, configuration digest, and aggregate lineage summary. `EpochCatalogEntry` contains parent `RecordingId`, `EpochId`, ordinal, predecessor, relative path, state, and bounded commit/ETL/retention summary. The first persisted ordinal remains `0` for compatibility with current epoch catalogs; public rendering is one-based (`Epoch 0001`).

Use a canonical layout for new public runs:

```text
<data-dir>/recordings/<recording-id>/
  recording-intent.json          # allocation/name, parent identity
  recording-run.json             # durable run lifecycle/deadline summary
  epochs.json                    # bounded ordered parent -> epoch lineage
  epochs/<ordinal>-<epoch-id>/
    recording.json               # epoch/WAL identity and epoch metadata
    segments/*.chwal
    incremental-etl-checkpoint.json
    epoch-outcomes.json
```

The daemon may use its configured WAL root as the parent run root; it must not create a second competing run. A legacy directory without `epochs/` is a one-epoch run whose parent ID and epoch/WAL ID are the same. A legacy continuous catalog is grouped into one parent only when its predecessor/ordinal/path evidence proves one chain; unrelated UUID directories are never guessed into one run.

Canonical session IDs remain independent. Each finalized epoch publishes one deterministic session derived from parent ID, epoch ID/ordinal, pipeline version, and that epoch's verified snapshot. `SourceProvenance.recording_id` becomes the stable parent ID for new output and gains optional epoch ID/ordinal provenance under the existing append-only v1 policy. `SessionSummary` and artifact references expose both identities. Legacy one-epoch sessions keep their existing ID/provenance fallback.

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

Rollover is initiated by the coordinator when the active epoch's fake-clock age or physical bytes reaches its configured threshold, reserving enough capacity for the next header, first/final marker, manifest/checkpoint staging, and concurrent ETL peak. It does not detach the capture source.

The durable sequence is:

1. Stop admission to the old epoch at the coordinator boundary while retaining the source and bounded queue; record accepted-observation ordinals, payload bytes, timestamps, and source metadata.
2. Persist a checksummed transition journal containing parent ID, old/new epoch IDs and ordinals, old/new paths, reservation identity, and phase `prepared`.
3. Create and sync the successor directory and WAL header; advance to `successor_created` only after identity and quota reservation validate.
4. Drain capacity-qualified admitted observations to the old writer, append/flush/sync its final marker, persist exact outcome ranges for every admitted ordinal, and advance to `boundary_committed`. An ordinal is assigned to the old epoch only after its authoritative marker covers it; it is assigned to the successor only after the successor marker covers it; otherwise it becomes metadata-only `epoch_rollover_failure` evidence.
5. Persist old-epoch final metadata and the prepared catalog entry, publish successor epoch metadata, and atomically activate the successor. Advance to `metadata_published` only when catalog, metadata, and lineage identities agree.
6. Switch the coordinator's active writer/epoch state, reset only the per-epoch ETL decoder/checkpoint state, enqueue the sealed old epoch for incremental/final ETL, and remove the transition journal after directory sync.

The source remains attached throughout. The only intentional pause is bounded admission handoff; no planned detach/reattach gap is introduced. If reservation, successor creation, outcome persistence, or activation fails, the coordinator stops accepting new observations and fails visibly rather than looping empty epochs or continuing without an owner.

### 6. Define recovery as a deterministic transition decision

Recovery owns the lease and quota authorities before mutating catalog, transition, WAL, checkpoint, or retention state. It validates the parent manifest, catalog, transition checksum/phase, old and successor identities/paths, WAL marker authority, metadata, and reservations. It never chooses an epoch by newest mtime or PID existence.

| Crash point | Recovery result |
| --- | --- |
| writing active epoch | Recover the final segment with existing verified-tail-only repair; keep its committed prefix; resume same epoch if appendable, otherwise finalize it as failed/aborted and create one linked successor. ETL retries from exact checkpoint lineage. |
| before rollover journal/transition | Old active epoch remains authoritative; threshold is retried. No empty successor is catalogued. |
| after successor allocation/header, before old finalization | If successor has no metadata/committed evidence, release reservation and roll back prepared directory. If evidence exists, do not delete it; complete or fail closed from the journal/catalog proof. |
| after predecessor finalization, before successor active | Validate boundary/outcome proof and successor metadata. Complete the one prepared-to-active catalog transition, or preserve both artifacts and fail closed if proof is contradictory. Never create a second successor. |
| ETL processing previous epoch | Capture continues in the active successor while the old epoch remains finalized but retention-ineligible. Retry/adopt matching immutable output, then advance that epoch checkpoint once. Quota pressure stops capture if lag consumes protected headroom. |
| recorder process restart | A persisted non-terminal run with matching scope/config/domain identity is recovered under the same parent ID. Active epoch resumes or is linked to exactly one successor; no new public recording is allocated merely because process-attempt ID changed. |
| host/pod restart | Persistent state/WAL/catalog are treated like process crash. A restart without durable end intent resumes lineage; a durable explicit end intent prevents continuation and leaves the run stopped. |

The parent run becomes terminal only after all required active-epoch finalization and downstream publication/retention decisions meet shutdown policy. A crash must not be relabeled as graceful completion.

### 7. Keep incremental ETL per epoch and aggregate at the run boundary

Each epoch has an independent incremental processor/checkpoint because the existing ETL contract deliberately does not carry decoder state across recording IDs. The checkpoint binds stable parent ID, epoch ID/ordinal, epoch WAL identity, marker/segment lineage, config/pipeline/schema, decoder snapshot, output keys, and source lifecycle status. Delta/output keys include parent ID plus epoch identity and marker range, preventing identical marker ranges in different epochs from colliding.

After rollover, the old epoch is a finalized ETL input and the successor is the only active capture input. ETL may snapshot the successor concurrently. Publication remains immutable-output first, checkpoint second, processed transition third. Final epoch session publication uses existing `FilesystemSessionStore`; a bounded parent aggregation/index records the ordered epoch sessions and aggregate counters. It does not merge unbounded decoder state or make a parent manifest a WAL authority.

On run shutdown, the coordinator stops intake, finalizes the active epoch, lets in-flight epoch publications reach a safe boundary, and then persists parent terminal metadata. If an epoch cannot publish within the configured drain policy, the run is `recoverable`/`failed` with protected WAL and a retry path; it is not reported as fully published.

### 8. Preserve quota, retention, and bounded lineage

Quota reservation covers current epoch, reserved successor, segment/manifest/checkpoint peaks, in-flight ETL/store staging, final session publication, and same-filesystem cleanup trash. Every epoch rollover must reserve successor capacity before mutation. Reservations are released/consumed only from durable transaction evidence and rebuilt from filesystem state after restart.

`retain` mode keeps finalized raw epochs and eventually stops/fails capture when no protected data can be deleted. `delete_after` can delete only finalized epochs whose committed WAL, ETL checkpoint, immutable outputs, final epoch session, retention age, and cleanup tombstone satisfy policy. Parent lineage, aggregate session references, and deletion proof remain after raw segment deletion. Unprocessed, corrupt, uncertain, failed, or protected epochs are never deleted to make room.

Epoch catalog/lifecycle index compaction is deterministic and bounded. It preserves first/last retained IDs, ordinal range, predecessor/digest-chain proof, cleanup/tombstone summary, aggregate counts, and active tail. If protected lineage cannot fit after legal compaction, stop creating epochs with typed metadata-limit failure; never silently drop lineage.

### 9. Public views and replay use the parent aggregate

`list` returns one row per stable parent recording, not one row per epoch. Existing ID/name/created/status fields remain; `sessions` and `operations` aggregate published epoch sessions, and additive `epoch_count`, `active_epoch`, and `published_epoch_count` fields expose bounded lifecycle state. In-progress duration is reported as nullable or explicitly elapsed according to the output contract, never as a false final duration. Ordering remains parent creation time then stable ID.

`inspect RECORDING` prints parent lifecycle/deadline/stop reason, aggregate counts, and an ordered bounded epoch summary: public epoch number, epoch ID, state, committed range/bytes, ETL/checkpoint/publication state, retention state, completeness, and warnings. It does not scan unbounded raw history or print payloads. Read-only list/inspect may reconcile in memory while a live owner holds the domain lock.

`replay RECORDING` resolves the parent, verifies all selected epoch session manifests, and builds an application-owned ordered multi-session plan. A stopped/published run selects every epoch in ordinal order. An active run may select only the contiguous immutable finalized prefix and must report the active tail as not selected; it must never silently skip a failed or missing middle epoch. Any gap, contradictory lineage, or non-replayable selected epoch causes a no-traffic denial unless a future explicit epoch-selection contract is added. The replay executor runs each session's operations in order without carrying decoder state across epochs. Command-mode target supervision and explicit-target non-termination/authorization rules remain unchanged.

### 10. Keep crate boundaries explicit

- `chronicle-common` owns neutral `RecordingId`/`EpochId` primitives and formatting only.
- `chronicle-wal` owns epoch-local framing, marker authority, segment rotation, recovery, and manifests; it does not know parent run policy.
- `chronicle-etl` owns complete extraction, transformation, per-epoch checkpoint/publication ordering, and storage publication; it does not own CLI or run stop policy.
- `chronicle-storage` owns immutable artifact/session persistence and read-only summaries; it does not decide WAL authority or rollover.
- `chronicle-replay` remains independent of WAL/ETL/capture; application supplies ordered canonical sessions through application-owned replay requests/results.
- `chronicle-application` composes parent lifecycle, source modes, transition, quota, ETL, catalog, inspect, and replay selection.
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
- **[Canonical compatibility]** Per-epoch sessions add provenance fields. → Use append-only v1 optional metadata, preserve existing validation and session-store publication, and keep legacy readers valid.

## Migration Plan

1. Add identity/run/epoch DTOs and read-only compatibility mapping for current single-epoch directories and valid legacy epoch catalogs.
2. Add deadline/epoch/segment policy separation and tests without changing the legacy hidden one-shot adapter.
3. Move public command/PID/cgroup modes onto the existing continuous coordinator behind deterministic fake sources and supervised/attached lifecycle adapters.
4. Extend the existing epoch catalog/transition/recovery implementation with stable parent identity, per-epoch output/checkpoint lineage, and the complete crash matrix.
5. Route finalized epochs through incremental ETL/storage while successor capture remains active; add parent aggregation and bounded compaction.
6. Update list/inspect/replay/status application contracts and CLI rendering; retain hidden 0.1.x syntax with warnings until the documented 0.2 removal boundary.
7. Update systemd and persistent-volume/DaemonSet deployment guidance, CLI/concepts/architecture/ETL/troubleshooting docs, localized website pages, acceptance scenarios, and `AGENTS.md` if required.
8. Run deterministic rootless tests, then changed-path validation and supported Ubuntu 24.04 privileged recorder evidence. Do not claim Linux/eBPF completion from macOS.

Rollback stops the run gracefully when possible, preserves parent/epoch/WAL/checkpoint/store artifacts, and restores the prior binary only if it can validate the same mutable-v1 state. If not, use a separate state root or matching reader; never delete or relabel lineage during rollback.

## Open Questions

No product-scope question is left unresolved. Implementation may tune non-semantic operational recommendations (segment-age defaults, lag warnings, restart backoff, and shutdown grace) through tests and documentation without changing the specified identity, stop, rollover, recovery, quota, or replay rules.
