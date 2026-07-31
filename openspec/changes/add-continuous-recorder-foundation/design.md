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

P1 intentionally stops at one recording, rejects live ETL, keeps only advisory post-publication checkpoint, and uses concrete local filesystem paths. Always-on operation creates new coordination problems: process ownership, planned epoch rollover, live-reader/writer concurrency, durable incremental parser state, publication/checkpoint crash windows, safe retention, global quota, readiness, and deployment privilege.

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
        |            seal + final checkpoint
        |
        +----> Epoch 18 / recording_id B / WAL seq 0..M
        |            ...
```

Each epoch keeps P1 recording ID, WAL v1 sequence space, commit-marker authority, 4 GiB hard maximum, and one-hour maximum age. Config may lower epoch duration/bytes but not exceed P1 maxima. Recorder-level epoch ordinal and predecessor ID provide continuous lineage without changing WAL bytes or merging canonical sessions.

Rollover keeps one capture source and bounded queue alive. Old epoch stops admission; writable queued prefix is committed; next epoch directory/header is durably opened; remaining/new observations route exactly once to next epoch. Capture timestamps preserve observation order. If next epoch cannot open, recorder stops and fails; accepted queued entries that cannot reach either epoch become exact metadata-only `epoch_rollover_failure` loss counts/bytes with capture-time interval or explicit timestamp ambiguity. Recorder never detaches/re-attaches as planned rollover, silently discards accepted entries, or continues without WAL.

Connection open across epoch boundary is represented independently in each epoch. Old epoch finalizes incomplete unless trusted close/complete message existed before boundary. This is conservative but avoids cross-recording decoder identity and unbounded state.

**Alternatives considered:**

- One forever WAL: rejected because P1 recovery scans, hard cap, canonical session bounds, and retention become unbounded.
- Stop/start P1 recorder externally: rejected because capture detach gaps and lifecycle/ownership remain undefined.
- Carry connections across epochs: deferred; it couples independent recovery/checkpoint lineages and conflicts with bounded session model.

### 2. Use one OS lease and persistent lifecycle state

One foreground recorder owns one state root and one cgroup scope. OS advisory lock is ownership authority; PID file is not. State machine:

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

Readiness becomes true only after:

1. lease acquired;
2. configuration/state identity validated;
3. interrupted WAL/manifest/retention transactions reconciled;
4. ETL output/checkpoint reconciled;
5. quota headroom and filesystem reservations verified;
6. active epoch is durably appendable;
7. capture preflight and attachment succeed.

Metadata is atomic private JSON and includes recorder ID, process-attempt ID, config digest, scope, state/readiness, boot/clock identity, epoch/segment, authoritative marker summary, ETL checkpoint/lag, quota, counters, last recovery, and failure/shutdown code. WAL markers remain authority for committed bytes; metadata is reconciled, never trusted ahead.

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
        checkpoint.json
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
- One global manifest only: rejected; per-epoch recovery/retention isolation is simpler. `epochs.json` remains small lineage index.
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
ETL checkpoint durably references exact input/output
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

Configuration requires explicit `retain` or `delete_after` policy. `retain` never auto-deletes and eventually stops at quota. `delete_after` requires minimum age and may include retained-byte target, but never bypasses proof. Failed/quarantined/unprocessed/uncertain data remains protected.

**Alternatives considered:**

- Delete oldest at quota: rejected; age/order does not prove processing or integrity.
- Upload success alone permits deletion: rejected; upload is not ETL/checkpoint lineage.
- Delete active prefixes in P2: deferred to reduce recovery complexity.

### 5. Add global physical quota above existing per-epoch cap

Per-epoch 4 GiB remains. Recorder config must supply quota and minimum free bytes for each filesystem domain. One synchronized reservation authority covers all Chronicle writers sharing a filesystem: active/sealed WAL, manifest/checkpoint, RecordingStore objects, co-located final-session staging/objects, temporary/trash peak, next epoch header, and final marker. Separate state/store filesystems keep independent reserves and cannot borrow headroom. This prevents ETL/session publication from consuming bytes already reserved for WAL finalization.

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

### 7. Persist bounded reconstruction/decoder state

P1 checkpoint is advisory after final session publication and does not contain intermediate state. P2 checkpoint v1 adds:

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

### 9. RecordingStore boundary starts at immutable artifacts

Active WAL requires local filesystem append/`fdatasync`/lock/repair and stays local. `RecordingStore` receives only immutable sealed artifacts:

```text
local durable spool                         RecordingStore
-------------------                         --------------
active WAL/lease/repair      X no upload
mutable recorder/checkpoint  X no upload
sealed segment -------------> put_if_absent(key,len,sha,provenance)
canonical delta ------------> put_if_absent(...)
final session --------------> existing atomic filesystem publication
```

Minimum store semantics:

- put-if-absent with exact expected identity/length/SHA-256/schema/provenance;
- direct head/get with integrity verification;
- bounded list for diagnostics, never correctness;
- delete with exact digest precondition;
- typed created/matching/conflict/not-found/unsupported/transient/permanent/uncertain.

P2 implements filesystem backend only, preserving private staging, sync, path confinement, and atomic no-replace. It stores sealed WAL copies, canonical delta batches, recovery reports, and provenance indexes—not final Canonical Session transactions. S3-compatible behavior remains future design context only; no SDK/config/client or normative remote-backend contract lands in P2.

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

Supported model: one systemd-managed foreground process per scope/state root, optionally template unit. Unit uses `Type=simple`: systemd active means liveness only, while automation polls local `recorder-status`/atomic state for readiness. Dedicated service user, private state directory, least verified capabilities, restart backoff/start limit, SIGTERM stop, systemd timeout longer than Chronicle drain timeout, resource controls, and no custom daemonization or `sd_notify` integration.

Configuration is one private versioned file. No hot reload. Incompatible scope/state/durability change requires new root or explicit acknowledged migration. Status uses atomic local file plus `recorder-status`; no network control plane.

**Alternatives considered:**

- Sidecar default: rejected for P2 due privilege/storage/lifecycle coupling.
- One multi-scope node agent: deferred; adds scheduler/tenant isolation/control plane.
- Kubernetes DaemonSet/operator: later milestone.

### 11. Separate liveness, readiness, and health

- **Liveness:** process executing.
- **Readiness:** lifecycle `running` after recovery, WAL appendability, capture attachment, ETL reconciliation, and quota checks.
- **Health:** `healthy`, `degraded`, or `failed`.

ETL lag/store retry may be degraded while durable WAL has headroom. WAL unappendable, capture detached, corruption, unsafe quota, or checkpoint contradiction is failed/not ready.

Safe status fields: recorder/attempt/epoch/segment IDs; lifecycle/readiness/health; config digest; commit/checkpoint watermarks; lag records/bytes/age; retained/quota/free bytes; segment counts; capture/WAL loss; last recovery/cleanup/publication; stable code/remediation. No payload/header values, command line, or environment values. P2 adds no metrics server.

### 12. Preserve P1 signal and failure semantics at service scope

First SIGINT/SIGTERM initiates once-only drain:

```text
stop intake
 -> mandatory final capture-loss sample
 -> drain capacity-qualified queue
 -> final marker + fdatasync
 -> seal epoch/manifest
 -> finish in-flight ETL output/checkpoint safe point
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
| ETL crash | capture continues while quota allows; resume checkpoint |
| publish success/checkpoint failure | verify deterministic output, repair checkpoint |
| checkpoint corruption/fork/ahead | fail ETL/retention; preserve WAL |
| store transient failure | bounded retry/degraded; no checkpoint advance |
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
