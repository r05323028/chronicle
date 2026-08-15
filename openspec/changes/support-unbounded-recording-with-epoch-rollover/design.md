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
4. Valid `IncrementalEtlCheckpointV2` and `EpochContinuationCheckpointV1` decide only processing cursor/decoder lineage and publication readiness for their verified epoch edge; they cannot promote WAL bytes, activate topology, or replace a predecessor.
5. Run summaries, status, sessions, and global indexes are rebuilt or marked inconsistent from the stronger domain authorities.

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

### 5. Split capture/WAL rollover from ETL continuation

Capture/WAL rollover and protocol continuation are separate bounded processes. Capture/WAL rollover SHALL NOT wait for `IncrementalProcessor` progress, ETL publication, checkpoint advancement, or a decoder continuation artifact. Cross-epoch continuation gates successor ETL decoding only; it never gates whether successor WAL may be created, activated, or written.

The capture/WAL transaction owns only parent/predecessor/successor identities, reservation, the authoritative predecessor boundary, admitted-observation outcome ranges, successor creation, `epochs.json` topology activation, and crash-safe writer handoff. It has no `continuation_checkpointed` phase and no ETL completion predicate. Its phases are:

1. **`prepared`** — reserve bounded successor/safety capacity and persist checksummed `RolloverTransitionV2` with parent ID, predecessor/successor IDs and ordinals, paths, reservation identity, and intended boundary. Reservation proves capture safety, not ETL readiness.
2. **`successor_created`** — create and sync successor WAL directory/header under the reservation. Successor is not capture-active until topology activation; no continuation artifact is required.
3. **`boundary_committed`** — stop predecessor admission for one bounded handoff while keeping source attached and queue bounded; drain admitted observations, append/flush/sync predecessor authoritative final marker, and persist exact admitted-observation outcome ranges. Unresolved outcome is metadata-only `epoch_rollover_failure` evidence and prevents activation.
4. **`topology_activated`** — atomically update checksummed `epochs.json` to finalize predecessor and make exactly one successor active, with marker/outcome/path/reservation digests matching transition.
5. **`complete`** — switch coordinator to successor writer, resume capture, sync transition cleanup, and remove journal only after directory durability. Source remains attached throughout; no planned detach/reattach gap exists.

Capture may resume with successor while predecessor is sealed and its ETL cursor is behind final marker. Successor WAL may accumulate committed bytes before successor ETL begins. If reservation, boundary outcome, successor creation, topology activation, or writer handoff fails, capture stops visibly at last authoritative boundary; it does not wait for, invent, or silently bypass decoder continuation.

### 6. Define recovery as two coordinated but independent decisions

Recovery owns lease and quota authorities before mutating `epochs.json`, transition, WAL, checkpoint, continuation, or retention state. It validates WAL commit-marker authority, then authoritative epoch catalog, then capture transition and derived summaries. Continuation state is evaluated separately by ETL recovery. Recovery never chooses an epoch by newest mtime, PID existence, or filesystem order.

| Crash point | Recovery result |
| --- | --- |
| writing active epoch | Recover verified committed prefix with existing tail repair; resume same epoch if appendable, otherwise finalize failed/aborted and create one linked successor. ETL retries its checkpoint/continuation independently. |
| before rollover transition | Old active epoch remains authoritative; retry threshold; no empty successor is catalogued. |
| after successor allocation/header, before predecessor boundary | Roll back only evidence-free prepared resource. Preserve any evidence and reconcile from exact transition/catalog/WAL proof; ETL is not consulted. |
| after predecessor WAL seal, before successor activation | Predecessor marker and admitted-observation outcome remain authoritative. Complete same successor creation/topology activation or fail closed with protected evidence; predecessor ETL need not reach final marker. |
| after successor topology/WAL activation while continuation is pending | Recover same parent and active successor. Capture readiness depends only on appendability, lease, attachment, quota, and WAL proof; successor processing remains `blocked_on_predecessor_continuation`. |
| predecessor ETL catches up or crashes during catch-up | Retry from `IncrementalEtlCheckpointV2`; successful retry emits same continuation artifact for same predecessor final marker; failure preserves WAL/topology and records processing failure/incomplete provenance. |
| continuation becomes ready after successor WAL accumulated | Verify parent/epoch IDs, predecessor final marker, segment/marker lineage, versions, bounds, and checksum; consume once, then process committed successor WAL from its own checkpoint. |
| invalid predecessor continuation | Preserve valid successor WAL/topology. Successor ETL fails closed or emits explicit incomplete/loss provenance; invalid decoder state never rolls back capture evidence. |
| run summary disagrees with `epochs.json` | Rebuild derived summary from authoritative catalog under lease; stable parent-ID conflict is recovery failure. |
| transition disagrees with `epochs.json` or WAL | Use catalog topology and WAL markers. Complete/rollback only with exact capture phase/digest proof; otherwise preserve evidence and remain capture-not-ready. |
| recorder/host restart | Recover same parent; capture epoch and ETL dependency states resume independently. Durable end intent prevents new capture; process-attempt change never allocates a new public recording. |

Parent becomes terminal only after active-epoch finalization and configured downstream publication/retention shutdown policy. Crash is never relabeled graceful completion. Invalid decoder state is never replaced with an empty decoder, but decoder failure never rewrites valid WAL/topology authority.

### 7. Keep incremental ETL per epoch while preserving asynchronous protocol continuation

Each epoch remains an independent WAL snapshot, immutable publication, checkpoint, and retention unit, but not an independent protocol reconstruction universe. `IncrementalProcessor` may initialize from exactly one verified predecessor `EpochContinuationCheckpointV1`; successor ETL may not decode cross-epoch state until that dependency is ready. This gate is downstream of capture and does not prevent successor WAL accumulation.

ETL owns a separate bounded, checksummed continuation lifecycle, not `epochs.json` or `RolloverTransitionV2`:

`pending -> ready -> consumed`

or

`pending -> unavailable` / `pending -> failed`

`pending` is recorded once predecessor final marker and successor identity are known, even if predecessor ETL cursor is behind that marker. ETL catches up from `IncrementalEtlCheckpointV2`, then emits one deterministic continuation-out artifact at exactly the predecessor final authoritative marker. The artifact binds stable parent `RecordingId`, predecessor/successor `EpochId`, final marker, exact segment/marker lineage, decoder/schema/pipeline version, bounded decoder state, configured limits, and checksum/digest. `ready` permits successor ETL to verify and consume once; successor ETL then processes already-recorded committed WAL from its own checkpoint. `consumed` records input digest and downstream checkpoint proof. `unavailable`/`failed` records unsupported, corrupt, over-limit, or retry-exhausted semantics and never claims clean protocol reset.

`IncrementalEtlCheckpointV1` remains read-only compatibility for legacy one-epoch state. New processing uses `IncrementalEtlCheckpointV2` with parent/epoch IDs, continuation dependency state/digests, marker/segment lineage, decoder/schema/pipeline versions, bounded state, and publication keys. `EpochContinuationCheckpointV1` contains only bounded connection/generation, TCP/reassembly, partial frames, correlation/negotiation, loss evidence, limits/counters, and predecessor/successor marker proofs. Unknown versions, unrelated predecessor, digest/marker gaps, checksum failure, schema mismatch, and bound overflow fail closed in ETL; none can change WAL existence or topology.

Completion-owner semantics remain unchanged: terminal operation belongs to epoch where decoding completes; N+1 emits one operation with both epoch ranges; predecessor sessions never mutate; unresolved final state emits one incomplete operation; continuation-only manifests are inspectable and non-replayable.

After capture rollover, sealed-predecessor ETL may catch up while successor capture writes. Successor ETL starts only after verified `ready` continuation or explicit terminal unsupported/incomplete decision. Invalid/unsupported/exhausted continuation leaves successor WAL valid and emits explicit processing loss/incomplete provenance; it does not claim a clean connection or operation start. Parent aggregation orders verified sessions by epoch ordinal/capture provenance without merging mutable decoder state.

On shutdown, coordinator stops intake and finalizes active epoch through capture transaction first. Downstream ETL may then drain, emit terminal incomplete operations, and persist parent terminal metadata. Failure to publish within shutdown policy yields `recoverable`/`failed` with protected WAL, not fully published status.

### 8. Preserve quota, retention, and bounded lineage

Quota covers current epoch, reserved successor, segment/manifest/checkpoint peaks, protected ETL backlog, continuation dependencies, ETL/store staging, final publication, and cleanup trash. Ordinary ETL lag does not block rollover while successor and safety margin can be reserved. Reservations rebuild from durable capture/processing evidence after restart.

Retention protects predecessor while its committed marker is ahead of ETL checkpoint, continuation is `pending`, `ready` is not consumed/proven by successor checkpoint, or retryable `failed` state needs source evidence. `delete_after` source-WAL deletion requires final marker/checkpoint, immutable outputs/session, continuation dependency proof, retention policy, and cleanup tombstone. Terminal `unavailable` satisfies dependency only after incomplete/loss output and retry/retention decision are durable. Continuation artifacts, checkpoints, lineage, aggregate references, and deletion proof remain separately inspectable. Unprocessed, corrupt, uncertain, failed, or protected epochs are never deleted to make room.

As protected backlog grows, capture continues while appendability, successor reservation, and safety headroom remain valid. Status degrades with per-epoch lag/protected bytes. When no safe successor reservation or minimum-free-space margin remains, recorder stops/fails before accepting unaccountable observations; it never drops WAL, deletes pending dependencies, or forks decoder lineage. Compaction may include only epochs with resolved continuation/processing and cleanup proofs. If protected lineage cannot fit, stop with typed metadata/quota failure.

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
4. Extend capture/WAL rollover transition and recovery with authoritative catalog conflict handling, successor reservation, admitted-observation outcomes, and deterministic handoff recovery; do not require ETL or continuation export.
5. Add ETL-owned continuation state, bounded continuation/checkpoint v2, completion-owner operation provenance, per-epoch immutable session publication, and one-shot equivalence; successor ETL may consume already-recorded WAL only after its dependency is ready.
6. Route finalized epochs through incremental ETL/storage while successor capture remains active; add parent aggregation and bounded retention/compaction.
7. Update list/inspect/replay/status application contracts and CLI rendering; retain hidden 0.1.x syntax with warnings until the documented 0.2 removal boundary.
8. Update systemd, CLI/concepts/architecture/ETL/troubleshooting docs, localized website pages, acceptance scenarios, and plan the concise `AGENTS.md` invariant update.
9. Run deterministic rootless validation, then changed-path validation and supported Ubuntu 24.04 privileged recorder evidence. Do not claim Linux/eBPF completion from macOS.

Rollback stops the run gracefully when possible, preserves parent/epoch/WAL/checkpoint/store artifacts, and restores the prior binary only if it can validate the same mutable-v1 state. If not, use a separate state root or matching reader; never delete or relabel lineage during rollback.

## Open Questions

No product-scope question is left unresolved. This change chooses completion-owner semantics for cross-epoch operations: the epoch where decoding reaches terminal completion owns the immutable canonical operation; unresolved state is emitted once as terminal incomplete evidence. Implementation may tune non-semantic operational recommendations (segment-age defaults, lag warnings, restart backoff, and shutdown grace) through tests and documentation without changing identity, authority, continuation, stop, rollover, recovery, quota, or replay rules.
