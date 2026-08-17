---
title: Architecture overview
description: The boundaries that keep Chronicle's capture, durability,
  transformation, storage, and replay concerns separate.
slug: 0-1/docs/architecture/overview
---

Chronicle is a Rust workspace with thirteen crates. Each crate has one primary owner; outer adapters communicate through application-owned contracts rather than lower-layer vocabulary.

## Runtime path

```text
capture-ebpf → capture events → WAL → session reconstruction → ETL
                                                     ↓
                                               canonical session
                                                     ↓
                                               local storage
                                                     ↓
                                                  replay
```

The application crate composes the use cases. The CLI parses arguments, renders application results, and maps exit codes; it does not decode protocols, scan WALs, load eBPF, or own replay policy.

## Logical service boundaries

The production pipeline separates distinct logical boundaries:

- **Recorder** — capture, protocol-neutral evidence, local WAL append/commit/recovery, segment and epoch rollover, loss accounting, and future durable evidence shipping. It does not decode protocols or own canonical storage layout.
- **Local WAL** — the capture durability and recovery authority.
- **Durable Evidence Store** — immutable evidence handoff between Recorder and ETL: checksums, parent/epoch lineage, idempotent publication, and independent lifecycles. A future S3-compatible store is a durable handoff/distribution boundary; it does not replace local WAL durability in the capture hot path.
- **ETL** — reconstruction, protocol decoding, canonicalization, incremental and final publication, verification, and checkpoint advancement ordering.
- **Canonical Store** — persisted canonical sessions and payload artifacts consumed by inspect and replay.
- **Replay** — consumes canonical evidence and stays independent from Recorder, WAL, ETL, and evidence-store internals.

Recorder and ETL may be co-located in the current local deployment, but correctness does not depend on sharing a process, memory, capture ownership, or a local filesystem namespace; ETL remains independently deployable. WAL segment, epoch, object-store object, and ETL batch boundaries are not protocol or logical interaction boundaries.

## Ownership

| Boundary | Responsibility |
| --- | --- |
| `chronicle-capture-ebpf` | Linux eBPF socket lifecycle and payload evidence; Aya and kernel ABI stay private. |
| `chronicle-capture` | Normalized capture evidence and fixture source. |
| `chronicle-wal` | Append-only framing, commit authority, recovery, retention, and local durability. |
| `chronicle-session` | Socket generation and evidence reconstruction. |
| `chronicle-etl` | Complete Extract–Transform–Load through canonical publication and checkpoint ordering. |
| `chronicle-canonical` | Protocol-independent session model and validation. |
| `chronicle-storage` | Filesystem and in-memory session stores; atomic publication. |
| `chronicle-protocol` | Protocol SPI and registry contracts. |
| `chronicle-protocol-builtins` | Concrete protocol implementations, including current HTTP/1.1 behavior. |
| `chronicle-replay` | Planning, execution, verification, and safety-aware result reporting. |
| `chronicle-application` | User-facing use-case composition. |
| `chronicle-cli` | Parsing, rendering, and exit mapping. |

## Reliability boundaries

WAL commit-marker durability and recovery authority, canonical schema compatibility, checkpoint ordering, replay default-deny policy, deterministic replay behavior, and eBPF privacy are deliberate boundaries. Website explanations should make these boundaries understandable without suggesting that future adapters already work.

## Current versus planned

Current end-to-end behavior is bounded plaintext HTTP/1.1 on supported Linux. Protocol registry entries are extension scaffolding unless the full detector/decoder/canonicalizer/replay/verifier path is implemented. PostgreSQL, MySQL/MariaDB, MongoDB, Kafka, NATS, and Oracle research entries are not current support.

Read the source repository's [crate boundary policy](https://github.com/r05323028/chronicle/blob/main/docs/architecture/crate-boundaries.md) when changing dependency direction.
