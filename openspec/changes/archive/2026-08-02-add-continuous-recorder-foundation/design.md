## Context

P1 established one bounded production recording:

```text
selected cgroup
  -> eBPF CaptureEvent/LossWindow
  -> bounded 4096-event ingest
  -> segmented WAL v1
  -> recovery-authoritative CommitMarker
  -> stopped-recording ETL
  -> deterministic Canonical Session v1
  -> FilesystemSessionStore
  -> inspect/replay
```

Critical P1 guarantees already exist and remain authoritative:

- fixed WAL v1 segment/envelope/commit-marker framing;
- one `fdatasync` group-commit boundary and no external durable watermark;
- strict physical per-recording limit/reservation;
- recovery authority derived only from complete validated marker/reference/digest chain;
- repair limited to verified incomplete final frame;
- bounded queues/reconstruction/output and typed loss;
- deterministic canonical IDs/content and atomic no-replace publication;
- complete provenance and fail-closed corruption;
- replay independent from capture/WAL and safe by default.

P1 intentionally stops at one recording, rejects live ETL, and uses `FinalSessionCheckpoint v1` as advisory post-publication final-session state with concrete local filesystem paths. Always-on operation creates new coordination problems: process ownership, planned epoch rollover, live-reader/writer concurrency, durable incremental parser state, publication/checkpoint crash windows, safe retention, global quota, readiness, and deployment privilege.

P2 extends existing boundaries instead of replacing them. Protocol decoding, canonical meaning, eBPF ABI, replay policy, and P1 one-shot commands remain unchanged.

### Current integration seams

- `chronicle-application::record_production` converges capture source, bounded ingest, WAL, stop polling, metadata, and finalization.
- `chronicle-wal::GroupCommitWalWriter` owns sequence, segment rotation, pending groups, commit authority, and writer lock.
- `scan_wal`/`RecoveryScan` own committed/uncommitted distinction and tail repair policy.
- `chronicle-etl::EtlPipeline` owns reconstruction/decode/canonicalization but currently starts fresh and emits one complete session.
- `FilesystemSessionStore` owns private staging, sync, manifest-last, atomic no-replace publication.
- CLI signal watcher already distinguishes first graceful and second forced signal.

## Goals / Non-Goals

### Goals

- Run one recorder continuously for one configured cgroup subtree/state root.
- Persist explicit lifecycle/readiness and recover before capture resumes.
- Roll bounded P1-compatible recording epochs without planned capture detach.
- Rotate WAL segments by size and age; persist crash-safe lifecycle manifest.
- Consume committed WAL incrementally and resume ETL without duplicates.
- Gate retention on validated commit, deterministic publication, durable checkpoint, and policy.
- Protect disk with global quota/minimum-free-space reservation and no unsafe deletion.
- Define immutable-artifact `RecordingStore` boundary with filesystem backend.
- Recommend and document supported systemd node-daemon placement.
- Preserve P1 durability, deterministic replay, provenance, crash recovery, bounds, and safety.
- Prove deterministic behavior with rootless tests and supported-runtime behavior with retained privileged evidence.

### Non-Goals

- New protocol support, protocol plugins, distributed tracing correlation, or canonical semantic redesign.
- Remote active WAL, S3 implementation, distributed/object-store lock, or multi-node recorder coordination.
- UI/dashboard, network control API/metrics server, Kubernetes operator/manifests, cloud control plane.
- Multi-scope manager inside one process, hot configuration reload, custom supervisor, or self-daemonization.
- Encryption at rest, comprehensive redaction, tenant isolation, or compliance guarantee.
- Carrying decoder state across recording IDs/epochs.
- Replacing P1 one-shot `record`/`etl`, filesystem session publication, or replay model.

## Decisions

### 1. Compose P1 as successive bounded recording epochs

Always-on service does not create one unbounded WAL/session. It keeps capture source attached and rolls finite epochs:

```text
Recorder instance R

  CaptureSource (attached once)
        |
        v
  bounded ingest/sequencer
        |
        +----> Epoch 17 / recording_id A / WAL seq 0..N
        |            seal + FinalSessionCheckpoint v1
        |
        +----> Epoch 18 / recording_id B / WAL seq 0..M
        |            ...
```

Each epoch keeps P1 recording ID, WAL v1 sequence space, commit-marker authority, 4 GiB hard maximum, and one-hour maximum age. Config may lower epoch duration/bytes but not exceed P1 maxima. Recorder-level epoch ordinal and predecessor ID provide continuous lineage without changing WAL bytes or merging canonical sessions.

Rollover keeps one capture source and bounded queue alive. Old epoch stops admission; writable queued prefix is committed; next epoch directory/header is durably opened; remaining/new observations route exactly once to next epoch. Capture timestamps preserve observation order. An **accepted observation** is one that has successfully entered the recorder-owned bounded ingest queue and received its ingest identity. At admission, it receives a stable ingest ordinal, payload length, capture timestamp, and source metadata required for loss reporting. During handoff, each admitted ordinal receives exactly one durable outcome range: old-epoch assignment only after its observation is covered by that epoch's authoritative marker, new-epoch assignment only after the successor commits it, or metadata-only `epoch_rollover_failure`. The bounded outcome ranges are persisted before rollover is reported successful; an outcome-journal persistence failure stops the recorder and leaves the handoff unresolved for deterministic recovery, which counts every admitted-but-unproven ordinal as rollover failure rather than guessing. If next epoch cannot open, recorder stops and fails; accepted observations that cannot reach either epoch become exact metadata-only `epoch_rollover_failure` loss counts/bytes with capture-time interval or explicit timestamp ambiguity. Recorder never detaches/re-attaches as planned rollover, silently discards accepted observations, or continues without WAL.

Connection open across epoch boundary is represented independently in each epoch. Old epoch finalizes incomplete unless trusted close/complete message existed before boundary. This is conservative but avoids cross-recording decoder identity and unbounded state.

**Alternatives considered:**

- One forever WAL: rejected because P1 recovery scans, hard cap, canonical session bounds, and retention become unbounded.
- Stop/start P1 recorder externally: rejected because capture detach gaps and lifecycle/ownership remain undefined.
- Carry connections across epochs: deferred; it couples independent recovery/checkpoint lineages and conflicts with bounded session model.

### 2. Use one OS lease and persistent lifecycle state

One foreground recorder owns one filesystem domain, one state root, and one cgroup scope. Preflight resolves configured WAL/store roots to a canonical filesystem-domain identity and deterministic configured domain-lock root. The recorder acquires `<domain-lock-root>/.chronicle-domain.lock` first; the state-root lock is subordinate. All Chronicle mutating commands resolve and attempt that same domain lock before mutation, while read-only commands acquire neither lock. Recorder lease ownership covers domain, not only process; multiple recorder instances sharing a WAL/store filesystem are unsupported. Standalone mutating commands reject a live-owned domain, while read-only commands may continue. OS advisory lock is ownership authority; PID file is not. State machine:

```text
                      unrecoverable error
                     ┌──────────────────────┐
                     v                      │
┌──────────┐   ┌────────────┐   ┌─────────┴┐
│ starting │-->| recovering │-->|  running │
└────┬─────┘   └─────┬──────┘   └────┬─────┘
     │               │               │ stop/signal
     │               │               v
     │               │         ┌──────────┐
     │               └-------->| draining │
     │ failure                  └────┬─────┘
     v                              │ success
┌────────┐                          v
│ failed │<-------------------┌─────────┐
└────────┘      failure       │ stopped │
                              └─────────┘

process crash: lock released; persisted starting/recovering/running/draining
               -> next attempt starts at recovering
```

Capture readiness becomes true only after:

1. filesystem-domain lease acquired;
2. configuration/state identity validated;
3. interrupted WAL/manifest/retention transactions reconciled;
4. quota headroom and filesystem reservations verified;
5. active epoch is durably appendable;
6. capture preflight and attachment succeed.

Processing readiness is separate and requires `IncrementalEtlCheckpoint v1` reconciliation, output-store availability, and incremental ETL progress. ETL lag/retry may make processing/overall health degraded while capture remains ready if durable WAL headroom is safe. Overall health is policy-derived; ETL lag is not capture failure by default.

Metadata is atomic private JSON and includes recorder ID, process-attempt ID, config digest, scope, lifecycle/capture-readiness/processing-readiness/overall-health, boot/clock identity, epoch/segment, authoritative marker summary, `IncrementalEtlCheckpoint v1` lineage/lag, quota, counters, last recovery, and failure/shutdown code. WAL markers remain authority for committed bytes; metadata is reconciled, never trusted ahead.

**Alternatives considered:**

- PID file: rejected; stale/reused PIDs do not prove ownership.
- SQLite state database: rejected; atomic files and current sync helpers cover single-owner state without new dependency.
- Multiple scopes in one process: deferred; systemd template instances give isolation with less state coupling.

### 3. Keep per-epoch WAL layout and add manifest as lifecycle journal

State layout:

```text
<state-root>/
  recorder.json
  recorder.lock
  epochs.json
  epochs/
    <20-digit-epoch>-<recording-uuid>/
      recording.json
      wal/
        manifest.json
        segments/
          <20-digit-first-sequence>.chwal
      etl/
        incremental-etl-checkpoint.json
        recovery-report.json
        publications/
          <deterministic-batch-key>.json
  trash/
```

Per-epoch `wal/manifest.json` v1 contains recording/epoch identity, policy, revision/predecessor, active segment, final authoritative-marker summary, and ordered segment entries. Sealed entries carry exact byte length/SHA-256/range. Segment states:

```text
active -> sealed -> processed -> retention_eligible -> deleting -> deleted
              \---------------------------------------------> quarantined
```

Manifest uses temp-write, file-sync, rename, directory-sync. It is authoritative for lifecycle/deletion intent only after reconciliation; it never overrides in-WAL commit authority. Missing/lagging manifest is rebuilt from validated segment bytes. Leading/contradictory manifest fails closed.

P1 byte-size segment rotation remains. P2 adds five-minute default age rotation, bounded by configuration. Monotonic elapsed time triggers rotation; wall time is observability only. Periodic tick rotates low-traffic non-empty segment. Empty segment does not rotate due age alone.

**Alternatives considered:**

- New WAL framing/segment seal record: rejected; existing final commit marker plus sealed-byte digest suffices and avoids format churn.
- One global manifest only: rejected; per-epoch recovery/retention isolation is simpler. `epochs.json` remains a bounded lineage index with deterministic compaction anchors, not an unbounded epoch catalog.
- Trust manifest watermark: rejected; would weaken P1 commit-marker proof.

### 4. Retain/delete only after proof chain; default active-prefix deletion deferred

Cleanup proof chain:

```text
validated committed marker
          |
          v
sealed segment digest/range
          |
          v
immutable canonical delta output durably published + verified
          |
          v
`IncrementalEtlCheckpoint v1` durably references exact input/output
          |
          v
final Canonical Session v1 + manifest verified for finalized epoch
          |
          v
retention policy age/bytes permits deletion
          |
          v
crash-safe delete transaction
```

P2 cleanup defaults to finalized epochs. Deleting processed segments from active epoch is not required; this avoids changing active recovery base while P2 foundations settle. Manifest still models segment lifecycle so future active-prefix cleanup can be added with explicit retained anchor.

Deletion transaction:

1. persist `deleting` intent with transaction ID and proof references;
2. rename segment to private same-filesystem trash name;
3. sync directory;
4. persist `deleted` tombstone/revision and sync;
5. unlink trash and sync.

Recovery completes or rolls back transaction from intent, checkpoint, digest, and file presence. Missing committed segment without tombstone is corruption, not assumed cleanup. Tombstone retains identity/range/digest/checkpoint/output/deletion reason without payload.

Lifecycle index compaction keeps long-running deployments bounded. `epochs.json`, per-epoch lineage history, deletion tombstones, and retention metadata each have configured maximum byte and entry limits. Startup recovery, capture readiness, and status queries MUST use the bounded current index and summaries; they MUST NOT scan unbounded historical epochs.

An epoch is eligible for compaction only after it is finalized, retained according to policy, cleanup is complete, and all deletion tombstones are durably persisted. Eligible history is replaced by a deterministic bounded lineage anchor that preserves first retained epoch identity, last compacted epoch identity, predecessor relationship, digest-chain root/tip, cleanup summary, and enough range/count information for missing committed evidence, digest mismatch, lineage fork, incomplete cleanup, and conflicting-compaction detection.

```text
epochs.json before

  epoch-1 ... epoch-100000
  epoch-100001
  epoch-100002

compacted epochs.json

  compaction:
    generation
    transaction_id
    source_index_revision
    source_index_digest
    candidate_digest
  compacted-lineage-anchor:
    first_retained_epoch
    last_compacted_epoch
    predecessor_epoch
    digest_chain_root
    digest_chain_tip
    cleanup_summary
  active_epochs:
    epoch-100001
    epoch-100002
```

Each compaction candidate records monotonic `compaction_generation`, unique `transaction_id`, source-index revision/digest, candidate identity/digest, and anchor data. The candidate digest covers anchor, active epochs, tombstone summary, and source metadata. Compaction is deterministic and uses private temp-write, file sync, atomic rename, and directory sync. Recovery validates old index and candidate independently: candidate installation is allowed only when its source revision/digest matches the old authoritative index and its digest/anchor recompute exactly; stale, incomplete, or conflicting candidates are rejected, with the old index preserved and a stable contradiction recorded. If both old and candidate files are complete, this same source-match rule deterministically selects the candidate; otherwise recovery fails closed rather than guessing. If protected failed, unprocessed, uncertain, or `retain` history cannot fit within hard bounds after legal compaction, recorder MUST stop new admission/epoch creation and enter failed/not-ready with stable metadata-limit evidence; it MUST not delete protected lineage or exceed the bound.

Configuration requires explicit `retain` or `delete_after` policy. `retain` never auto-deletes and eventually stops at quota. `delete_after` requires minimum age and may include retained-byte target, but never bypasses proof. Failed/quarantined/unprocessed/uncertain data remains protected.

**Alternatives considered:**

- Delete oldest at quota: rejected; age/order does not prove processing or integrity.
- Upload success alone permits deletion: rejected; upload is not ETL/checkpoint lineage.
- Delete active prefixes in P2: deferred to reduce recovery complexity.

### 5. Add global physical quota above existing per-epoch cap

Per-epoch 4 GiB remains. P2 supports exactly one active Chronicle mutating writer owner per filesystem domain. Recorder lease ownership covers the entire configured filesystem domain, not only the recorder process. A domain may have one live recorder owner; multiple recorder instances sharing the same WAL/store filesystem are unsupported. Standalone mutating commands MUST reject execution when a live recorder owns that domain; read-only commands may continue. Different cgroup scopes requiring independent recorder instances must use isolated filesystem domains.

Within that single owner, one synchronized reservation authority covers active/sealed WAL, manifest/checkpoint, RecordingStore objects, co-located final-session staging/objects, temporary/trash peak, next epoch header, and final marker. Separate filesystem domains keep independent reserves and cannot borrow headroom. This prevents ETL/session publication from consuming bytes already reserved for WAL finalization without implying safe cross-process quota sharing.

Pressure behavior:

```text
reservation fails
   |
   +--> run already-eligible cleanup
             |
             +--> enough headroom -> continue
             |
             +--> insufficient -> stop intake, finalize authoritative prefix,
                                  persist quota evidence, fail/not-ready
```

No policy deletes unprocessed/unverified/unexpired data. ETL lag may therefore stop capture. This is preferred to silent loss or corruption. Config preflight must ensure at least one segment/marker/manifest/checkpoint transaction fits.

**Alternative:** unbounded disk growth rejected as non-production. Best-effort capture after persistence failure rejected by P1 policy.

### 6. Incremental ETL reads validated marker snapshots, never mutable tails

Writer exposes/records a snapshot descriptor after successful marker sync. Descriptor contains epoch, marker segment/offset/sequence/digest, and exact input digest. It accelerates discovery but is not authority. ETL validates through marker independently.

Concurrency:

```text
WAL writer                         ETL reader
----------                         ----------
append frames
append marker
fdatasync
publish snapshot descriptor  ---> validate bytes/marker through boundary
continue append                    ignore later mutable suffix
                                  process delta from checkpoint
```

Recorder-owned incremental ETL worker/API never repairs/truncates. If it observes growing/partial suffix, it stops at prior authoritative marker or retries. Existing standalone `etl --wal-dir --output` retains P1 stopped-recording behavior and rejects a live writer; it may process a finalized P2 epoch. Exclusive startup recovery owns repair.

**Alternatives considered:**

- ETL waits for stopped epoch: rejected; not incremental.
- ETL scans/repairs active WAL: rejected; races writer and weakens ownership.
- External durable watermark: rejected; duplicates WAL authority and group-sync cycle.

### 7. Separate final-session and incremental ETL checkpoints

`FinalSessionCheckpoint v1` is the existing P1 stopped-recording checkpoint artifact. It remains owned by final-session ETL/publication and is not extended to represent live incremental state. `IncrementalEtlCheckpoint v1` is a distinct P2 artifact with a distinct discriminator, owner, and lifecycle. Unknown versions fail closed; no version has dual interpretation.

`IncrementalEtlCheckpoint v1` owns:

- WAL marker lineage;
- segment digest lineage;
- decoder reconstruction state;
- TCP direction state;
- HTTP correlation state;
- loss-window state;
- emitted delta batch references;
- deterministic ID counters;
- predecessor checkpoint digest;
- epoch/config/pipeline/canonical versions;
- exact input marker/WAL/segment digests;
- active connection generation and directional TCP ranges/chunks/provenance;
- lifecycle/loss-window state;
- protocol detector/decoder buffers and request/response correlation;
- deterministic ID derivation inputs/counters;
- emitted immutable batch keys/digests;
- existing bounds/counters/source status.

State obeys existing connection/chunk/byte/body/operation/issue/read-batch limits. Pending payload makes checkpoint sensitive; permissions remain private. Unknown version, lineage fork, checkpoint ahead of WAL, same boundary/different digest, or output mismatch fails closed.

State is not carried across recording epoch. At epoch close all connections finalize with honest completeness and final Canonical Session is produced.

**Alternatives considered:**

- Reread epoch from start on every poll: simpler but O(n²), prevents safe retention, and fails long-running goal.
- Serialize only byte offset: insufficient for fragmented TCP/HTTP state.
- Shared cross-epoch state: deferred due recovery/identity complexity.

### 8. Publish immutable canonical delta batches, then checkpoint

Incremental ETL transaction:

```text
validate input snapshot
       |
derive next state + completed canonical operations/evidence
       |
put-if-absent immutable CanonicalDeltaBatch v1
       |
verify every output key/length/digest/provenance
       |
atomic checkpoint write
       |
manifest segment -> processed
```

`CanonicalDeltaBatch` is an internal storage artifact, not replacement canonical protocol schema and not direct replay input. It wraps existing canonical operations/connection/provenance values plus exact epoch/WAL range. Deterministic key derives from epoch, pipeline version, source provenance, and content. Poll timing, worker scheduling, restart count, and publication wall time do not affect identity/order.

At epoch close ETL materializes existing Canonical Session v1. Final session must be byte-identical to one-shot ETL over same committed epoch. Existing FilesystemSessionStore remains sole owner of its multi-file manifest-last atomic no-replace publication. Finalized-epoch WAL cannot become retention eligible until that session/manifest/payload integrity is verified.

Crash handling:

| Crash point | Recovery |
| --- | --- |
| before output | old checkpoint; reprocess input |
| partial staging | remove/ignore private staging; reprocess |
| output published, checkpoint old | recompute deterministic key, verify, repair checkpoint |
| checkpoint new, manifest old | verify proof, mark segment processed |
| conflicting existing output | fail closed; no checkpoint advance |

Duplicate prevention uses exact WAL provenance/digests and deterministic keys, never HTTP equality/timestamp heuristics.

**Alternatives considered:**

- Mutable canonical session overwrite: rejected; conflicts with no-replace store and makes crash idempotency harder.
- Checkpoint before publication: rejected; can omit output permanently.
- Transactional database: rejected; no database requirement and filesystem ordering suffices.

### 9. RecordingStore owns immutable artifacts only

RecordingStore ownership is deliberately narrow:

- **RecordingStore owns:** immutable artifact storage semantics, artifact identity, integrity verification, artifact lifecycle metadata, and the filesystem backend implementation.
- **RecordingStore does not own:** active WAL append authority, WAL commit-marker authority, WAL repair, incremental checkpoint authority, or final Canonical Session publication authority.

Ownership flow is explicit:

```text
WAL
 | commit authority
 v
Incremental ETL
 | checkpoint authority
 v
RecordingStore
 | immutable artifact persistence
 v
FilesystemSessionStore
 | final Canonical Session publication
```

Active WAL requires local filesystem append/`fdatasync`/lock/repair and stays local. RecordingStore receives only immutable sealed artifacts:

```text
local durable spool                         RecordingStore
-------------------                         --------------
active WAL/lease/repair      X no upload
mutable recorder/checkpoint  X no upload
sealed segment --optional--> put_if_absent(key,len,sha,provenance)
canonical delta ------------> put_if_absent(...)
final session --------------> FilesystemSessionStore publication
```

WAL remains sole authority for committed bytes and repair. Incremental ETL remains sole authority for checkpoint advancement and must publish/verify immutable outputs before advancing its checkpoint. FilesystemSessionStore remains sole owner of final Canonical Session manifest/payload publication. RecordingStore stores and verifies artifacts and their lifecycle metadata but cannot bypass any upstream authority.

Minimum store semantics:

- put-if-absent with exact expected identity/length/SHA-256/schema/provenance;
- direct head/get with integrity verification;
- bounded list for diagnostics, never correctness;
- delete with exact digest precondition;
- typed created/matching/conflict/not-found/unsupported/transient/permanent/uncertain.

P2 implements filesystem backend only, preserving private staging, sync, path confinement, and atomic no-replace. It may store optional sealed WAL copies, canonical delta batches, recovery reports, and provenance indexes—not final Canonical Session transactions. Sealed WAL copies are future-compatible immutable artifacts, not mandatory P2 data flow; retention correctness uses local validated WAL/manifest/checkpoint/final-session proof and never requires duplicated WAL copies. S3-compatible behavior remains future design context only; no SDK/config/client or normative remote-backend contract lands in P2.

**Alternatives considered:**

- Generic CRUD trait for active WAL and objects: rejected; lowest-common-denominator API hides required durability semantics.
- Reuse `MetadataRepository`/`ArtifactStore` unchanged: rejected for recorder artifacts because existing contracts lack append/seal/provenance/precondition/lifecycle semantics; existing session store remains reused for sessions.
- Add S3 now: out of scope and would obscure local correctness.

### 10. Recommend systemd node recorder over sidecar

Comparison:

| Concern | Application sidecar | Node recorder (recommended) |
| --- | --- | --- |
| eBPF privilege | repeated per pod | centralized per service instance |
| lifecycle | tied to app pod churn | independent continuous recovery |
| local WAL | often ephemeral | persistent `StateDirectory` |
| capture self-inclusion | easier to misconfigure | explicit service cgroup outside target |
| failure impact | app rollout coupling | recorder fails independently |
| ownership | application team | platform + Chronicle operator |
| P2 complexity | orchestration/manifests | systemd foreground service |

Supported model: one systemd-managed foreground process per scope/state root and filesystem domain, optionally template unit. Different scopes needing independent recorder instances use isolated filesystem domains. Unit uses `Type=simple`: systemd active means liveness only, while automation polls local `recorder-status`/atomic state for capture and processing readiness. Dedicated service user, private state directory, least verified capabilities, restart backoff/start limit, SIGTERM stop, systemd timeout longer than Chronicle drain timeout, resource controls, and no custom daemonization or `sd_notify` integration.

Configuration is one private versioned file. No hot reload. Incompatible scope/state/durability change requires new root or explicit acknowledged migration. Status uses atomic local file plus `recorder-status`; no network control plane.

**Alternatives considered:**

- Sidecar default: rejected for P2 due privilege/storage/lifecycle coupling.
- One multi-scope node agent: deferred; adds scheduler/tenant isolation/control plane.
- Kubernetes DaemonSet/operator: later milestone.

### 11. Split liveness, capture readiness, processing readiness, and health

- **Liveness:** recorder process is executing.
- **Capture readiness:** recorder can safely capture: filesystem-domain lease ownership, recovered WAL, appendable epoch, quota reservation, and capture attachment are all valid.
- **Processing readiness:** incremental ETL can make progress: `IncrementalEtlCheckpoint v1` lineage is consistent, output store is available, and incremental processing is healthy.
- **Overall health:** policy-derived `healthy`, `degraded`, or `failed` status; ETL lag/retry may degrade processing/overall health without forcing capture failure while WAL headroom remains safe.

WAL unappendability, capture detachment, corruption, or unsafe quota makes capture not ready/failed. Checkpoint contradiction or output-store failure makes processing not ready; policy determines overall health. ETL lag MUST NOT be treated as capture failure by default.

Safe status fields: recorder/attempt/epoch/segment IDs; lifecycle/capture-readiness/processing-readiness/overall-health; config digest; commit/`IncrementalEtlCheckpoint v1` watermarks; lag records/bytes/age; retained/quota/free bytes; segment counts; capture/WAL loss; last recovery/cleanup/publication; stable code/remediation. No payload/header values, command line, or environment values. P2 adds no metrics server.

### 12. Preserve P1 signal and failure semantics at service scope

First SIGINT/SIGTERM initiates once-only drain:

```text
stop intake
 -> mandatory final capture-loss sample
 -> drain capacity-qualified queue
 -> final marker + fdatasync
 -> seal epoch/manifest
 -> finish in-flight ETL output/`IncrementalEtlCheckpoint v1` safe point
 -> detach maps/links
 -> persist recorder state as stopped
 -> release lease
```

Second signal may force exit after best-effort metadata. Next start treats stale active/draining state as crash, never graceful completion.

Failure/recovery matrix:

| Failure | Required response |
| --- | --- |
| capture attach/start | fail before readiness, clean partial links |
| capture runtime/final sample | stop, preserve uncertainty/committed prefix |
| WAL write/marker/sync | stop, preserve prior marker authority |
| manifest lag/missing | rebuild from validated WAL |
| manifest contradiction | fail closed, preserve files |
| ETL crash | capture continues while quota allows; resume `IncrementalEtlCheckpoint v1` |
| publish success/`IncrementalEtlCheckpoint v1` failure | verify deterministic output, repair incremental checkpoint |
| `IncrementalEtlCheckpoint v1` corruption/fork/ahead | fail ETL/retention; preserve WAL |
| store transient failure | bounded retry/degraded; no incremental checkpoint advance |
| quota with eligible data | validated cleanup then continue |
| quota without eligible data | stop/fail; no unsafe deletion |
| cleanup crash | complete/roll back transaction |
| segment corruption | quarantine, stop affected lineage |
| process/host crash | OS releases lease; full recovery before attach |

### 13. Security grows with retention and is explicit

Continuous plaintext capture increases exposure. Required P2 controls:

- service outside selected subtree;
- dedicated identity and minimum verified capabilities;
- `0700` directories / no broader than `0600` files / restrictive umask;
- private config, no CLI secrets;
- bounded explicit retention and quota;
- payload-safe logs/status/doctor;
- path confinement, symlink/traversal rejection, same-filesystem atomic operations;
- no remote upload implementation or implicit backup claim.

Documentation must state WAL/checkpoints/canonical artifacts may contain credentials and personal data and are not encrypted/redacted comprehensively.

### 14. Acceptance remains layered

OpenSpec strict validation proves artifact consistency only.

Rootless automated evidence covers deterministic state machines, manifests, fault injection, ETL state/transaction equivalence, store conformance, quota/retention, CLI/config/status, and docs/static unit behavior.

Privileged supported-Linux evidence covers only production-runtime claims: Ubuntu 24.04/kernel/cgroup v2/BTF/capabilities, real eBPF load/attach/capture, systemd placement, multi-epoch/segment operation, signals/crash/restart, WAL persistence, incremental ETL, disk pressure, cleanup, and process/cgroup/eBPF leak checks. Evidence is retained under exact commit/environment and labels untested combinations honestly.

## Implementation Gates

### Gate A: Recorder lifecycle foundation complete

Evidence required for domain lease ownership, lifecycle state, WAL manifest, epoch rollover and durable observation outcomes, ordered shutdown, and status contracts. These foundation tasks may use an ETL safe-point hook but MUST NOT require incremental ETL correctness. Do not begin incremental ETL implementation until Gate A passes.

### Gate B: Incremental ETL feasibility approved

Evidence required for a bounded decoder state inventory, `IncrementalEtlCheckpoint v1` serialization, and pause/resume equivalence. Do not enable live ETL until Gate B is independently reviewed and approved.

### Gate C: Incremental publication correctness proven

Evidence required for deterministic delta output, publication-before-checkpoint transaction, crash recovery, and one-shot equivalence. Do not enable retention transitions based on incremental output until Gate C passes.

### Gate D: Retention/quota enabled only after safety proof

Evidence required for the cleanup proof chain, one-owner-per-filesystem-domain quota model, bounded lifecycle-index compaction including protected-history exhaustion behavior, and contradiction/crash tests. Do not enable retention deletion or quota enforcement as production behavior until Gate D passes.

Implementation MUST NOT proceed to later gates without prior gate evidence. OpenSpec validation is artifact validation only and is not gate evidence.

## Data Flow Diagrams

### Continuous steady state

```text
┌──────────────────────── selected cgroup workload ────────────────────────┐
│ plaintext TCP/HTTP                                                       │
└───────────────────────────────┬──────────────────────────────────────────┘
                                v
                    ┌──────────────────────┐
                    │ eBPF CaptureSource   │
                    │ events + loss sample │
                    └──────────┬───────────┘
                               v
                    ┌──────────────────────┐
                    │ bounded ingest 4096  │
                    └──────────┬───────────┘
                               v
             ┌─────────────────────────────────┐
             │ active epoch WAL writer         │
             │ frame -> marker -> fdatasync    │
             └──────────┬──────────────┬───────┘
                        │              │ marker snapshot
                        │              v
                        │    ┌────────────────────────┐
                        │    │ incremental ETL        │
                        │    │ restore/process state  │
                        │    └──────────┬─────────────┘
                        │               v
                        │    ┌────────────────────────┐
                        │    │ immutable delta batch  │
                        │    │ then checkpoint        │
                        │    └──────────┬─────────────┘
                        v               v
                WAL manifest      RecordingStore
                lifecycle         filesystem backend
                        \               /
                         v             v
                         retention eligibility
```

### Startup recovery

```text
acquire lease
   -> validate config/state identity
   -> scan/recover active epoch WAL
   -> rebuild/reconcile manifest
   -> reconcile output/checkpoint
   -> finish/rollback cleanup transaction
   -> verify quota/reservations
   -> open appendable epoch
   -> capture preflight + attach
   -> running + ready
```

## Risks / Trade-offs

- **Serialized decoder state can become complex or sensitive** -> keep exact bounded v1 state, private permissions, explicit validation, one-shot equivalence vectors, no cross-epoch carry.
- **Manifest may be mistaken for durability authority** -> specs repeatedly subordinate it to in-WAL marker validation; tests corrupt/lead/lag manifest.
- **Epoch boundary may degrade long-lived operations** -> finalize honestly incomplete; later milestone may add cross-epoch correlation after proven need.
- **Five-minute segment age is operational guess** -> configurable bounded value, measured guidance separated from correctness; size rotation remains unchanged.
- **Explicit retention can delete raw evidence by policy** -> require checkpoint/output proof, age, tombstone, and clear unavailable-source provenance; `retain` mode available.
- **Retain mode eventually stops continuous capture** -> intentional fail-safe; true long-running operation requires explicit delete policy and sufficient quota.
- **ETL lag can exhaust disk** -> visible lag/headroom, cleanup only after proof, stop rather than silent loss.
- **Filesystem semantics may not hold on every mount** -> doctor probes and backend conformance; unsupported filesystems fail preflight.
- **Node daemon still needs powerful capabilities** -> dedicated identity, scope exclusion, minimum capability documentation, privileged acceptance.
- **No encryption/redaction magnifies production risk** -> explicit warning, private files, bounded retention; feature deferred rather than falsely claimed.
- **Incremental batch artifact adds format** -> keep internal v1 wrapper around existing canonical values; final public replay artifact remains Canonical Session v1.
- **Working tree contains concurrent P1 edits** -> implementation must rebase/reinspect current contracts and not overwrite unrelated changes.

## Decoder checkpoint implementation contract

`IncrementalEtlCheckpoint v1` binds each checkpoint to one recorder session, recording ID, epoch, normalized configuration, exact marker, and ordered sealed-segment lineage. Decoder state is a versioned `chronicle-reconstruction` snapshot containing active and finalized connection evidence, loss windows, terminal WAL-loss evidence, limits, and bounded payload bytes. The snapshot is non-empty even when no connection is pending; empty/default state is invalid. Snapshot restore checks kind, implementation version, schema version, session/recording/epoch/cursor bindings, configured bounds, duplicate identities, state consistency, and serialized length. Any mismatch, corruption, unsupported version, or lineage fork fails closed; no fresh-decoder fallback exists.

Incremental processing requires the next committed envelope sequence after a checkpoint cursor and verifies prior segment digests/ranges before accepting newer WAL. Immutable delta publication tracks stable operation fingerprints (canonical operation ID removed before hashing) in the checkpoint, so a restarted or retried worker does not republish prior operations. Publication failure restores the complete in-memory processor state, including reconstruction evidence, issues, segment lineage, operation fingerprints, and marker. Output publication remains before checkpoint persistence; final session publication still uses the existing one-shot canonical session authority.

## Migration Plan

1. Add new P2 artifacts/contracts beside P1; retain all one-shot commands/readers.
2. Implement lifecycle/lease/state with fake capture and no retention deletion.
3. Add age rotation/manifest/rebuild while preserving WAL bytes and per-epoch recovery.
4. Add incremental ETL checkpoint/output transaction and prove one-shot equivalence.
5. Add RecordingStore filesystem backend/conformance and route immutable batches through it.
6. Enable processed/retention transitions, cleanup transaction, quota pressure behavior.
7. Add foreground CLI/status, doctor probes, systemd/runbook docs.
8. Run rootless fault/equivalence/conformance suites.
9. Run privileged Ubuntu 24.04 acceptance from clean exact commit and retain evidence.
10. Enable production service only after strict OpenSpec validation, automated gates, and required privileged evidence.

Rollback: stop systemd service gracefully and preserve state root. P1 one-shot tools remain usable for stopped compatible epochs. If prior binary cannot read current mutable-v1 P2 control artifacts, do not force downgrade over same active root; preserve evidence and start separate root or restore matching binary. Rollback never deletes WAL automatically.

## Open Questions

No blocking product-scope question remains for proposal review. Implementation must still calibrate documented operational recommendations (segment-age bounds, restart backoff, ETL lag warning, shutdown timeout, and minimum free-space guidance) from tests without changing safety semantics. Any request for active-epoch prefix deletion, cross-epoch decoder carry, S3 backend, multi-scope process, Kubernetes deployment, or network control plane requires separate OpenSpec change.
