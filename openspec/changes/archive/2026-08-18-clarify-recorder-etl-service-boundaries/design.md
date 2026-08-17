# Recorder / Evidence Store / ETL / Canonical Store Boundaries

## Target logical architecture

```text
Application
    |
    v
Recorder
    |
    v
Local WAL
    |
    v
Evidence Publisher / Shipper (future)
    |
    v
Durable Evidence Store
    |
    v
Independent ETL Worker
    |
    v
Canonical Store
```

A future Kubernetes deployment may look like:

```text
Application Pods
    |
    v
Recorder DaemonSet
    |
    v
Local WAL
    |
    v
S3 / MinIO
    |
    v
ETL Deployment
    |
    v
Canonical artifacts
```

The current implementation co-locates Recorder and incremental ETL in the same process (`ContinuousRecorderService` owns `IncrementalWorker`, `IncrementalProcessor`, the protocol registry, incremental checkpoints, and continuation state) and uses filesystem-backed storage. This is acceptable for the current local implementation and is explicitly an implementation/deployment choice, not a permanent correctness dependency.

## Component boundaries

### 1. Recorder

Responsibilities:

- eBPF capture.
- Protocol-neutral capture evidence.
- Local WAL append/commit.
- WAL recovery.
- Segment rollover.
- Epoch rollover.
- Capture-side loss accounting.
- Future durable evidence shipping.

Recorder must not own: HTTP decoding, database protocol decoding, canonical operation semantics, causal request correlation, replay semantics, or final canonical storage layout.

### 2. Local WAL

The local WAL is the capture durability and recovery authority. Durable evidence publication moves evidence downstream; it does not replace local WAL durability in the capture hot path. `chronicle-wal` remains independent of S3 or any other remote storage implementation.

### 3. Durable Evidence Store

Responsibilities:

- Durable handoff between Recorder and ETL.
- Immutable evidence.
- Checksum verification.
- Lineage preservation (explicit parent/epoch lineage).
- Idempotent publication/discovery.
- Independent Recorder and ETL lifecycles.

Current backend may be filesystem; future backend may be S3-compatible. The Durable Evidence Store is NOT: the local WAL durability authority, a protocol decoder, a canonical model owner, or a replay component.

### 4. ETL

Responsibilities (all remain ETL-owned):

- Consume recovery-authoritative evidence.
- Reconstruct connections; preserve cross-segment reconstruction.
- Preserve cross-epoch continuation.
- Detect and decode protocols.
- Produce canonical operations.
- Produce incremental deltas.
- Publish canonical results (incremental and final).
- Verify publication.
- Advance checkpoints safely (output-before-checkpoint ordering).

ETL correctness must not require: eBPF attachment, capture ownership, Recorder process memory, Recorder process lifecycle, sharing a process with Recorder, or sharing a local filesystem namespace with Recorder.

### 5. Canonical Store

Responsibilities:

- Persist canonical sessions.
- Persist canonical payload artifacts.
- Expose canonical evidence for inspect/replay.
- Preserve immutable publication semantics.

Current implementation is `FilesystemSessionStore`; it is not a permanent requirement. Replay consumes canonical evidence and does not require Recorder, WAL, ETL, or Evidence Store internals.

## Durable invariants

The following become normative architecture language:

1. **Recorder / ETL separation.** Recorder and ETL MAY be co-located in the current local deployment, but their correctness MUST NOT depend on sharing a process, memory, or capture ownership.
2. **WAL authority.** Local WAL is the capture durability and recovery authority. Remote evidence storage is a durable handoff/distribution boundary and MUST NOT replace local WAL durability in the capture hot path.
3. **ETL independence.** ETL consumes recovery-authoritative evidence through a durable evidence contract and MUST NOT require ownership of the capture runtime.
4. **Storage boundaries.** WAL segment, WAL epoch, object-store object, and ETL batch boundaries MUST NOT define protocol or logical interaction boundaries.
5. **ETL ownership.** ETL owns canonical publication, publication verification, and checkpoint advancement ordering.
6. **Replay independence.** Replay consumes canonical evidence and remains independent from capture, WAL, ETL, and evidence-store implementation details.

No causal interaction guarantees are introduced beyond what is implemented.

## Authority domains

The current `.chronicle-domain.lock` is a local deployment coordination mechanism: one exact normalized path protects name claim, capture, ETL, publication, and catalog update as one transaction within one filesystem domain. Existing behavior is not weakened. It is not a universal distributed architecture invariant; a future system may have independent authority domains:

- **Recorder local authority**: local lease/lock, capture ownership, local WAL, WAL recovery, local shipping state.
- **Evidence Store authority**: immutable evidence publication, checksums, lineage, object visibility.
- **ETL authority**: worker ownership/lease, incremental processing checkpoint, continuation processing, output-before-checkpoint ordering, canonical publication.

A single filesystem lock is not required to span future processes, nodes, object stores, or ETL workers.

## ETL input boundary

Current ETL APIs operate on local WAL paths (`process_recording_wal(wal_directory, ...)`, `process_recording_wal_with_parent(...)`) and the hidden operational surface uses `chronicle internal etl --wal-dir ... --output ...`. The WAL-path entries are application orchestration seams over the `chronicle-etl` pipeline; `chronicle-etl` owns the pipeline, publication, verification, and checkpoint semantics. These remain for the current filesystem implementation and are not replaced. Architecture documentation expresses the durable contract conceptually: ETL consumes recovery-authoritative recording evidence. A future ETL worker may obtain that evidence from another durable source without owning the Recorder runtime. The future distributed transport API is not designed here.

## Storage seam

`chronicle-storage` already contains recording-artifact abstractions (`RecordingStore` trait, `InMemoryRecordingStore`, `FilesystemRecordingStore`, immutable artifact lifecycle with `PutIfAbsent`/`DeleteIfDigest` and lineage) for concepts such as sealed WAL copies, continuation checkpoints, canonical delta batches, recovery reports, and provenance indexes. This is a useful future seam for durable evidence publication and is documented as such; it is not redesigned or replaced, and no new adapter is added. `MetadataRepository`/`ArtifactStore` traits and their `PostgresMetadataConfig`/`S3ArtifactConfig` remain config-only future seams.

## Recorder configuration

Current `RecorderConfigV1` contains recorder, WAL/epoch, ETL, storage, and shutdown configuration, reflecting the co-located implementation. Configuration structs are not split in this change. Architecture documentation does not imply that future independently deployed ETL workers must share the Recorder process lifecycle, process memory, Recorder-local filesystem namespace, or Recorder-local configuration ownership; configuration separation happens only when independently deployed ETL becomes a real implementation requirement.

## What remains unchanged

- Crate set, crate boundaries, and `validation/architecture.toml` (the existing allowlist already represents the desired direction; `chronicle-wal` stays free of remote storage; `chronicle-etl` keeps its storage dependency and stays free of application/CLI/eBPF; `chronicle-cli` stays application-only).
- Co-located local runtime (`ContinuousRecorderService`), incremental ETL implementation, WAL formats, checkpoint formats, canonical schemas, recording behavior, and the hidden `internal` ETL surface.
- Domain-lock behavior, quota accounting, and reliability authorities.

## Deferred implementation (non-goals)

S3/MinIO implementation and authentication; WAL shipper; object discovery/polling protocol; event notifications; distributed ETL worker scheduling; distributed leases; Kubernetes manifests; horizontal scaling; PostgreSQL metadata repository; new storage adapters; causal request correlation; new protocol support; public ETL CLI; migration layers for unreleased internal formats; splitting the co-located runtime or configuration structs; speculative runtime APIs.
