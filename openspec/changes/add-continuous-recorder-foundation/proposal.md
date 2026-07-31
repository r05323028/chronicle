## Why

P1 proved Chronicle can capture, durably commit, recover, canonicalize, inspect, and replay one bounded production recording. Production operators still cannot run Chronicle continuously: recorder state is process-scoped, WAL retention is terminal rather than cyclical, ETL rejects live recordings, storage ownership is filesystem-specific, and no supported deployment model defines readiness, restart, resource, or failure behavior.

P2 turns that validated vertical slice into a continuously operating capture foundation while preserving P1 durability, deterministic replay, provenance, and fail-closed recovery. Protocol breadth remains unchanged.

## What Changes

- Add an always-on recorder service that owns one configured cgroup scope and state root, persists lifecycle state, recovers before readiness, rolls bounded recording epochs without stopping capture, handles SIGTERM/SIGINT idempotently, and resumes after process or host restart.
- Add explicit lifecycle states `starting`, `recovering`, `running`, `draining`, `stopped`, and `failed`, plus versioned active-recorder metadata, a single-writer lease, epoch identity, configuration digest, current WAL/ETL watermarks, shutdown reason, and last recovery result.
- Extend WAL operation from one terminal 4 GiB recording into successive bounded epochs with size- and age-based segment rotation, a crash-safe manifest, sealed-segment integrity digests, configurable retention, synchronized per-filesystem quota/minimum-free-space protection, and checkpoint-plus-final-session-gated cleanup. Existing in-WAL commit markers remain sole authority for committed record bytes.
- Add incremental ETL over recovery-authoritative committed snapshots while recording continues. Persist bounded reconstruction/decoder state and exact input lineage, publish deterministic immutable outputs before advancing checkpoints, and make publication/checkpoint crash windows idempotent.
- Introduce a `RecordingStore` boundary for immutable recorder artifacts. P2 ships only a filesystem backend and keeps active WAL on required local durable storage; future S3-compatible upload begins only at sealed immutable artifacts and is not implemented here.
- Recommend a systemd-managed node recorder rather than an application sidecar. Document placement, privileges, state directories, configuration, readiness, restart policy, resource bounds, failure containment, security, retention, and operator ownership. Kubernetes manifests, operator logic, and cloud control plane remain out of scope.
- Extend diagnostics and CLI/application contracts for daemon preflight, health/readiness, current epoch/segment, committed boundary, ETL lag, retained bytes, quota headroom, loss counters, recovery state, and safe machine-readable lifecycle control.
- Add rootless crash/fault/conformance tests and separate privileged Ubuntu 24.04 evidence for continuous eBPF capture, rotation, restart, cleanup, incremental ETL, signal handling, and leak-free process/cgroup/eBPF teardown.
- Preserve Capture Event v1, WAL v1 framing and commit semantics, Canonical Session v1, existing HTTP/1.1 behavior, replay safety, bounded queues, and current fixture/one-shot commands. Any mutable-v1 artifact additions are repository-wide and reject unknown versions; no compatibility dispatcher or dual writer is added.

## User Value

- Operators can run Chronicle as a supervised production service instead of coordinating repeated one-shot recordings.
- Committed evidence survives recorder or ETL crashes and continues from verified boundaries without duplicate canonical output.
- WAL growth is bounded by explicit retention and quota policy without deleting unprocessed or unverified committed evidence.
- Existing local filesystem workflows remain supported while storage ownership gains a future-compatible immutable-artifact boundary.
- Deployment, health, resource, and failure behavior become reviewable and automatable.

## Non-Goals

- PostgreSQL, MySQL, MariaDB, MongoDB, Kafka, Oracle, Redis, or any new protocol decoder.
- Dynamic protocol plugins or a protocol plugin ecosystem.
- Complete distributed-trace correlation.
- UI/dashboard, Kubernetes operator/manifests, hosted or cloud control plane.
- S3-compatible backend implementation, remote active-WAL append, multi-node coordination, or distributed ETL.
- Encryption at rest, comprehensive secret discovery/redaction, tenant isolation, or compliance certification.
- Canonical schema redesign, replacement of P1 WAL framing/commit markers, or weaker replay authorization.

## Capabilities

### New Capabilities

- `continuous-recorder-lifecycle`: Persistent daemon ownership, state transitions, recording-epoch rollover, graceful shutdown, readiness, restart, and crash recovery.
- `recording-store`: Required immutable non-session artifact storage semantics, filesystem backend compatibility, upload boundary, integrity, publication, and lifecycle operations while existing FilesystemSessionStore retains final-session authority.
- `production-recorder-operation`: Supported deployment model, configuration, resource/security expectations, observability, failure containment, and operational ownership.

### Modified Capabilities

- `recoverable-recording-wal`: Add age rotation, manifest/reconciliation, sealed-segment lifecycle, final-session-gated retention eligibility, cleanup transactions, synchronized per-filesystem quota protection, and continuously recoverable epoch rollover without changing commit-marker authority.
- `restartable-recording-etl`: Add live committed-snapshot consumption, durable state checkpoints, deterministic immutable incremental publication, resume, and duplicate prevention.
- `recording-diagnostics`: Add daemon/storage/quota/checkpoint/readiness probes and safe runtime health fields.
- `runnable-http-cli`: Add recorder service/control/status contracts and P2 acceptance/documentation gates while retaining one-shot P1 commands.

## Compatibility Considerations

- P1 one-shot `record`, `etl`, `inspect`, and `replay` behavior remains supported. Recorder daemon composes existing capture/WAL/ETL services rather than replacing them.
- Existing WAL segment/envelope/commit-marker bytes remain unchanged. Manifest and lifecycle metadata never promote uncommitted frames; recovery-authoritative commit validation remains mandatory.
- Existing Canonical Session v1 and deterministic identity rules remain authoritative. Incremental outputs use existing canonical values plus provenance wrappers/checkpoints rather than redefining HTTP semantics.
- Active WAL remains local because append, `fdatasync`, advisory lock, repair, and directory-sync guarantees do not map safely to generic object storage. Future remote storage receives only sealed immutable artifacts.
- Filesystem backend preserves private modes, checksum validation, atomic no-replace publication, and current local session compatibility.
- P2 remains Linux production-first. Fixture capture, ETL, inspect, replay, and storage conformance remain rootless/portable where already supported.

## Impact

- **Code:** primarily `chronicle-application`, `chronicle-wal`, `chronicle-etl`, `chronicle-storage`, `chronicle-cli`, diagnostics, acceptance harnesses, and operations documentation. Capture/protocol/canonical/replay boundaries remain intact.
- **Artifacts:** new recorder state, WAL manifest, ETL state checkpoint, incremental publication index, retention tombstones, and versioned health/status reports. All are private, checksummed or atomically persisted as appropriate, bounded, and payload-safe by default.
- **Operations:** supported production placement becomes one systemd-managed recorder instance per configured cgroup scope/state root; dedicated service identity and local durable filesystem are required.
- **Security:** continuous plaintext capture increases retained sensitive-data exposure. P2 requires restrictive ownership/modes, bounded retention, payload-safe logs/metrics, capability minimization, and explicit operator acknowledgement that encryption/redaction remain out of scope.
- **Migration:** no automatic conversion of historical repository artifacts or live P1 recordings is required. P2 starts new recorder state; stopped P1 recordings remain processable through existing commands.
- **Risk:** crash ordering across writer/ETL/retention, long-lived decoder state, disk exhaustion, manifest/checkpoint contradiction, and over-broad deployment privilege are primary review risks.
