# Architecture

## Status labels

- **Current:** compiled and tested now.
- **Planned MVP:** intended before MVP acceptance.
- **Future:** explicit extension point, not a current promise.

## MVP versioning policy

Until explicit compatibility freeze, every internal artifact domain has one mutable schema or format: v1. Capture events, private eBPF ABI records, WAL, canonical sessions, manifests, recording metadata, ETL checkpoints, fixtures, and machine-readable reports evolve by updating current v1 plus repository fixtures, tests, and documentation in same change.

Readers reject values other than 1. Chronicle does not retain V1/V2/V3 dispatch enums, migration adapters, dual writers, or compatibility readers for repository history. A version increase requires explicit OpenSpec change declaring compatibility freeze, frozen contract scope, supported reader/writer combinations, migration policy, and deprecation policy.

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
chronicle-etl -> capture + WAL + session + protocol + canonical
chronicle-application -> capture + WAL + ETL + protocol + storage + replay
chronicle-cli -> chronicle-application
```

Arrows point from dependent crate to its compile-time dependencies. No crate depends on CLI. Protocol-owned stream views keep replay free of capture, session, WAL, ETL, and eBPF dependencies.

Thirteen crates consolidate real protocol modules into `chronicle-protocol-builtins`; one empty crate per protocol would add coupling and maintenance without behavior. Modules remain independent registration boundaries. Future independently distributed plugins require a versioned ABI/process boundary, not Rust trait objects across dynamic libraries.

## P1 vertical slice

Fixture and eBPF sources emit same `CaptureEvent` v1 model. `SocketConnected` carries stable socket identity, local/remote IP endpoints, and active/passive role before endpoint-free payload fragments. Capture adapter caches complete evidence by socket identity and rejects conflicting tuple or role.

A bounded 4096-event queue feeds segmented WAL v1. Each group appends data, one in-WAL `CommitMarker`, flushes, and performs one `fdatasync`; recording metadata is non-authoritative and reconciled from validated markers. Recovery mutates only a verified incomplete final tail. ETL consumes the final recovery-authoritative prefix, reconstructs socket generations/TCP positions, decodes bounded HTTP/1.1, and publishes one deterministic Canonical Session v1. Active maps local→client and remote→server; passive maps remote→client and local→server. Ingress/egress controls byte direction only. Missing or conflicting evidence is typed failure; production never fabricates `unknown:0` endpoints.

`FilesystemSessionStore` atomically writes sole-v1 manifest, session JSON, and SHA-256-addressed payloads. Replay hydrates only persisted artifacts and requires explicit loopback target, matching allow-host, effect flag, and `--execute`; dry-run stays default. Fake protocol remains available for boundary tests.

JSON is current capture-event payload encoding; WAL framing remains encoding-independent. Live capture is bounded to 600 seconds by default (maximum 3600) and a 4 GiB physical WAL ceiling; always-on capture, rotation, and incremental ETL are deferred.

## Ownership and backpressure

Capture owns event production. WAL append/flush is local durability boundary. ETL owns decoded event/session memory with explicit connection, per-connection byte, per-connection chunk, and diagnostic issue limits. Reaching a limit returns a typed error; data and diagnostics are never silently dropped. No unbounded channels exist. Future async pipelines must use bounded queues and surface capacity errors or backpressure; dropping events is forbidden.

## Dependency choices

Rust standard library does not provide derive-based wire serialization, UUIDs, wall-clock serialization, CRC32C, typed error derives, TOML parsing, CLI derivation, async test runtime, or structured diagnostics. Chronicle therefore uses `serde`/`serde_json`, `uuid`, `time`, `crc32c`, `thiserror`, `toml`, `clap`, `tokio`, and `tracing`. `tracing-subscriber` is confined to CLI setup. `tokio` is used only where async execution is exercised.

`httparse` provides bounded HTTP head parsing and `sha2` identifies filesystem payloads. `aya` is isolated in `chronicle-capture-ebpf`; kernel ABI types never cross capture boundary. `sqlx` and `object_store` remain absent until PostgreSQL and S3 adapters have real behavior. `bytes`, `blake3`, `anyhow`, plugin loaders, queues, and embedded databases remain unnecessary.

## Deferred implementations

- PostgreSQL metadata repository.
- S3-compatible artifact store and WAL archival.
- Additional protocol decoders/canonicalizers/adapters/verifiers.
- Always-on/rotating capture, incremental ETL, segment retention, scheduling, and expanded redaction policies.

## Capture semantics and TLS

Capture events are socket lifecycle evidence plus ordered socket byte fragments, not application operations. Private eBPF ABI v1 emits active connect intent before autobind and complete active/passive endpoint evidence at establishment. Payload carries TCP/continuation sequence evidence but never duplicates endpoint tuple or role. Exact ordering and missing-data semantics remain bounded by selected hooks and explicit loss windows.

Plaintext only is supported by target design. TCP payload capture cannot decode arbitrary TLS ciphertext. Unknown and encrypted payloads remain opaque. Future uprobes may observe bytes before encryption or after decryption without weakening TLS or exporting production certificates.

## Risks

Hook coverage and kernel compatibility; incomplete socket lifecycle observation; temporal loss attribution; disk exhaustion/corruption; sensitive WAL/session data; Oracle protocol uncertainty; stateful/multiplexed replay; authentication replacement; destructive side effects; and redaction before remote persistence remain major risks.
