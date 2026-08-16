# Architecture

## Status labels

- **Current:** compiled and tested now.
- **Planned:** intended for a future milestone.
- **Future:** explicit extension point, not a current promise.

## Pre-0.1 versioning policy

Before the first public release, every internal artifact domain has one current runtime model, reader, and writer. Capture events, private eBPF ABI records, WAL, canonical sessions, manifests, recording metadata, epoch catalogs, rollover transitions, ETL checkpoints, continuations, fixtures, and machine-readable reports evolve by updating the current model plus repository fixtures, tests, and documentation in same change. Unreleased internal persisted schemas may be replaced rather than supported through permanent runtime compatibility layers; persisted structures may keep explicit schema version numbers (for example epochs.json version 2). No hidden pre-release CLI compatibility surface is retained, and runtime lineage is never inferred from identifier equality.

Chronicle does not retain V1/V2/V3 dispatch enums, migration adapters, dual writers, or compatibility readers for repository history. Stable wire formats intentionally preserved by the project (WAL v1 framing and commit markers, Capture Event v1, Canonical Session v1, replay safety contracts) remain versioned separately. A freeze requires explicit OpenSpec change declaring compatibility freeze, frozen contract scope, supported reader/writer combinations, migration policy, and deprecation policy.

## Boundaries

```text
chronicle-capture-ebpf -> chronicle-capture -> chronicle-common
chronicle-wal (local standalone durability)
chronicle-session -> chronicle-capture + chronicle-common
chronicle-canonical -> chronicle-common
chronicle-protocol -> chronicle-canonical + chronicle-common
chronicle-protocol-builtins -> chronicle-protocol + chronicle-canonical + chronicle-common
chronicle-storage -> chronicle-canonical
chronicle-replay -> chronicle-canonical + chronicle-protocol
chronicle-etl -> capture + WAL + session + protocol + canonical + storage
chronicle-application -> capture + WAL + ETL + protocol + storage + replay
chronicle-cli -> chronicle-application
```

Arrows point from dependent crate to its compile-time dependencies. No crate depends on CLI. Protocol-owned stream views keep replay free of capture, session, WAL, ETL, and eBPF dependencies.

Thirteen crates consolidate real protocol modules into `chronicle-protocol-builtins`; one empty crate per protocol would add coupling and maintenance without behavior. Modules remain independent registration boundaries. Future independently distributed plugins require a versioned ABI/process boundary, not Rust trait objects across dynamic libraries.

## Recording pipeline

Fixture and eBPF sources emit the same `CaptureEvent` v1 model. `SocketConnected` carries stable socket identity, local/remote IP endpoints, and active/passive role before endpoint-free payload fragments. Capture adapter caches complete evidence by socket identity and rejects conflicting tuple or role.

A bounded 4096-event queue feeds segmented WAL v1. Each group appends data, one in-WAL `CommitMarker`, flushes, and performs one `fdatasync`; recording metadata is non-authoritative and reconciled from validated markers. Recovery mutates only a verified incomplete final tail. ETL consumes the recovery-authoritative committed WAL prefix, reconstructs socket generations and TCP positions, and decodes bounded HTTP/1.1. During recording it runs incrementally over committed evidence, publishing canonical delta batches under durable incremental checkpoints; epoch rollover persists bounded, checksummed, lineage-verified continuation evidence for cross-epoch state. On finalization, one-shot ETL performs the final authoritative publication of one deterministic Canonical Session v1 per finalized epoch. Active maps local→client and remote→server; passive maps remote→client and local→server. Ingress/egress controls byte direction only. Missing or conflicting evidence is typed failure; production never fabricates `unknown:0` endpoints.

`FilesystemSessionStore` atomically writes sole-v1 manifest, session JSON, and SHA-256-addressed payloads. Replay hydrates only persisted artifacts. Command mode infers one owned loopback listener for supervised target and grants only execution/read/host/target gates; explicit-target mode requires loopback target, matching allow-host, effect flag, and `--execute`. Fake protocol remains available for boundary tests.

JSON is current capture-event payload encoding; WAL framing remains encoding-independent. Public recording lifetime has no implicit time deadline when `--duration` is omitted; explicit deadlines are checked independently from bounded epoch and segment WAL limits. A recording may own many finalized epoch sessions, and a WAL epoch boundary is not a protocol reconstruction boundary: cross-epoch state requires bounded, checksummed, lineage-verified continuation evidence.

## Service boundaries

The production pipeline separates five logical components: **Recorder** (eBPF capture, protocol-neutral evidence, local WAL append/commit/recovery, segment and epoch rollover, capture-side loss accounting, future durable evidence shipping), **Local WAL** (capture durability and recovery authority), **Durable Evidence Store** (immutable evidence handoff preserving checksums and parent/epoch lineage, idempotent publication, independent Recorder/ETL lifecycles), **ETL** (reconstruction, protocol decoding, canonicalization, incremental and final publication, publication verification, checkpoint advancement ordering), and **Canonical Store** (persisted canonical sessions and payload artifacts consumed by inspect/replay).

Recorder and ETL MAY be co-located in the current local deployment, but their correctness must not depend on sharing a process, memory, capture ownership, or a local filesystem namespace. A future S3-compatible evidence store is a durable handoff/distribution boundary and does not replace local WAL durability in the capture hot path. WAL segment, epoch, object-store object, and ETL batch boundaries never define protocol or logical interaction boundaries. Replay consumes canonical evidence and remains independent from Recorder, WAL, ETL, and evidence-store internals.

## Public data directory and recording identity

Public commands resolve data directory in fixed precedence: `--data-dir`, configured `AppConfig.data_dir`, `CHRONICLE_DATA_DIR`, then platform default. Resolution is non-mutating. Mutating commands lazily create private directory and reject unsafe root/symlink forms. Doctor reports source plus existing/prospective access without creating probe artifact.

```text
<data-dir>/
  .chronicle-domain.lock
  catalog.json                         # bounded advisory v1 view
  recordings/<bare-recording-uuid>/   # WAL, metadata, private intent
  sessions/<session-uuid>/             # canonical manifest/session/artifacts
```

`RecordingId` is user-facing stable identity rendered `rec_<uuid>`; `latest` and exact names resolve through bounded reconciled catalog view. Catalog and `recording-intent.json` sidecars are advisory. Recovery-authoritative WAL metadata and canonical session facts win contradictions. Within one local filesystem deployment, one exact normalized `.chronicle-domain.lock` path protects name claim, capture, ETL, publication, and catalog update as one transaction. The lock is a local deployment coordination mechanism, not a universal distributed transaction lock; future independent authority domains (Recorder-local, evidence-store, ETL) are not required to share it.

Canonical `SessionId` is independently deterministic and may differ from recording ID. Association uses `source_provenance.recording_id` / `epoch_id` only; identifier equality is never lineage, and sessions without explicit provenance are unresolved. Public users operate on recording references; the hidden `internal` namespace is the operational surface for recorder, recorder-status, ETL, and deterministic fixture recording.

## Ownership and backpressure

Capture owns event production. WAL append/flush is local durability boundary. ETL owns decoded event/session memory with explicit connection, per-connection byte, per-connection chunk, and diagnostic issue limits. Reaching a limit returns a typed error; data and diagnostics are never silently dropped. No unbounded channels exist. Future async pipelines must use bounded queues and surface capacity errors or backpressure; dropping events is forbidden.

## Dependency choices

Rust standard library does not provide derive-based wire serialization, UUIDs, wall-clock serialization, CRC32C, typed error derives, TOML parsing, CLI derivation, async test runtime, or structured diagnostics. Chronicle therefore uses `serde`/`serde_json`, `uuid`, `time`, `crc32c`, `thiserror`, `toml`, `clap`, `tokio`, and `tracing`. `tracing-subscriber` is confined to CLI setup. `tokio` is used only where async execution is exercised.

`httparse` provides bounded HTTP head parsing and `sha2` identifies filesystem payloads. `aya` is isolated in `chronicle-capture-ebpf`; kernel ABI types never cross capture boundary. `sqlx` and `object_store` remain absent until PostgreSQL and S3 adapters have real behavior. `bytes`, `blake3`, `anyhow`, plugin loaders, queues, and embedded databases remain unnecessary.

## Deferred implementations

- PostgreSQL metadata repository.
- S3-compatible artifact store and WAL archival.
- Additional protocol decoders/canonicalizers/adapters/verifiers.
- Expanded segment-retention policy, scheduling, and redaction policies.
- Docker and Kubernetes packaging/orchestration (future follow-up only; no implementation).

## Capture semantics and TLS

Capture events are socket lifecycle evidence plus ordered socket byte fragments, not application operations. Private eBPF ABI v1 emits active connect intent before autobind and complete active/passive endpoint evidence at establishment. Payload carries TCP/continuation sequence evidence but never duplicates endpoint tuple or role. Exact ordering and missing-data semantics remain bounded by selected hooks and explicit loss windows.

Plaintext only is supported by target design. TCP payload capture cannot decode arbitrary TLS ciphertext. Unknown and encrypted payloads remain opaque. Future uprobes may observe bytes before encryption or after decryption without weakening TLS or exporting production certificates.

## Risks

Hook coverage and kernel compatibility; incomplete socket lifecycle observation; temporal loss attribution; disk exhaustion/corruption; sensitive WAL/session data; Oracle protocol uncertainty; stateful/multiplexed replay; authentication replacement; destructive side effects; and redaction before remote persistence remain major risks.

## Crate boundaries

[`docs/architecture/crate-boundaries.md`](architecture/crate-boundaries.md) documents one primary responsibility, allowed dependencies, forbidden knowledge, the current dependency graph (which equals the target graph), and the reliability boundaries that must not change for every root-workspace crate. `validation/architecture.toml` is the executable mirror; `AGENTS.md` carries the normative rules.
