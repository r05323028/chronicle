---
title: ETL
description: How recovered evidence becomes deterministic canonical sessions — one per finalized epoch.
slug: 0-1/docs/architecture/etl
---

ETL is complete Extract–Transform–Load, not just a decoder. It owns the path from recovered WAL evidence through session reconstruction, protocol handling, canonical validation, and atomic storage publication.

## Extract

ETL scans the recording WAL through the recovery authority. Only envelopes through the final valid commit marker are eligible. It carries loss windows, sequence gaps, endpoint evidence, and provenance forward instead of discarding them.

## Transform

Session reconstruction groups socket generations and directions. The current HTTP/1.1 path handles bounded origin-form requests, exact `Content-Length`, bounded chunked responses, trusted close-delimited responses, and sequential keep-alive exchanges. Missing or conflicting socket evidence is typed failure; operations are not fabricated.

Protocol modules own detection, decoding, correlation, canonicalization, replay, and verification contracts. A registry entry without a complete implementation is scaffolding, not support.

## Load

ETL validates one canonical session, stages its manifest, session JSON, and content-addressed payloads in a private directory, writes the manifest last, and atomically publishes the destination. Checkpoint ordering follows publication: a checkpoint cannot claim progress that has not been published.

```text
recovered WAL prefix
       │
       ▼
socket/session reconstruction
       │
       ▼
bounded protocol decode
       │
       ▼
canonical validation
       │
       ▼
atomic filesystem publication
```

## Restartability

A failed finalization can resume from the persisted WAL and checkpoint/publication state. The checkpoint is progress evidence, not a replacement for WAL durability and not an identity binding to arbitrary output. Contradictory metadata fails closed and remains available for diagnosis.
