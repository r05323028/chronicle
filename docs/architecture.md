# Architecture

## Status labels

- **Current:** compiled and tested now.
- **Planned MVP:** intended before MVP acceptance.
- **Future:** explicit extension point, not a current promise.

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

## Current vertical slice

`FixtureCaptureSource` emits ordered socket-byte chunks. JSON capture envelopes are framed in WAL v1 records, then ETL validates, assembles, detects, decodes, and canonicalizes bounded plaintext HTTP/1.1 traffic into Canonical Session v2. `FilesystemSessionStore` atomically writes a manifest, session JSON, and SHA-256-addressed payloads. Replay hydrates only persisted artifacts and requires explicit loopback target, matching allow-host, effect flag, and `--execute`; dry-run stays default. Fake protocol remains available for boundary tests.

JSON is only current capture-envelope payload encoding; WAL framing does not depend on JSON and any replacement requires its own schema/version migration plan.

## Ownership and backpressure

Capture owns event production. WAL append/flush is local durability boundary. ETL owns decoded event/session memory with explicit connection, per-connection byte, per-connection chunk, and diagnostic issue limits. Reaching a limit returns a typed error; data and diagnostics are never silently dropped. No unbounded channels exist. Future async pipelines must use bounded queues and surface capacity errors or backpressure; dropping events is forbidden.

## Dependency choices

Rust standard library does not provide derive-based wire serialization, UUIDs, wall-clock serialization, CRC32C, typed error derives, TOML parsing, CLI derivation, async test runtime, or structured diagnostics. Chronicle therefore uses `serde`/`serde_json`, `uuid`, `time`, `crc32c`, `thiserror`, `toml`, `clap`, `tokio`, and `tracing`. `tracing-subscriber` is confined to CLI setup. `tokio` is used only where async execution is exercised.

`httparse` provides bounded HTTP head parsing and `sha2` identifies filesystem payloads. `sqlx`, `object_store`, and `aya` remain absent until PostgreSQL, S3, and eBPF adapters have real behavior. `bytes`, `blake3`, `anyhow`, plugin loaders, queues, and embedded databases remain unnecessary.

## Planned implementations

- Linux eBPF adapter isolated in `chronicle-capture-ebpf`.
- PostgreSQL metadata repository and ETL checkpoints.
- S3-compatible artifact store and WAL archival.
- Protocol decoders/canonicalizers/adapters/verifiers.
- Restart repair, segment retention, scheduling, redaction policies, inspect/doctor probes.

## Capture semantics and TLS

Capture events are ordered socket byte chunks, not packets. Exact ordering and missing-data semantics depend on chosen eBPF hooks. TCP sequence reconstruction must not be claimed unless packet/sequence information is actually captured.

Plaintext only is supported by target design. TCP payload capture cannot decode arbitrary TLS ciphertext. Unknown and encrypted payloads remain opaque. Future uprobes may observe bytes before encryption or after decryption without weakening TLS or exporting production certificates.

## Risks

Hook coverage and kernel compatibility; incomplete socket lifecycle observation; Oracle protocol uncertainty; stateful/multiplexed protocol replay; authentication replacement; destructive side effects; and redaction before persistence remain major MVP risks.
