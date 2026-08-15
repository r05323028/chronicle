---
title: WAL
description: Why Chronicle persists captured evidence before transformation.
slug: 0-1/docs/concepts/wal
---

Chronicle writes captured evidence to a segmented, append-only write-ahead log before ETL interprets it. The WAL is the local durability boundary and the recovery authority for what downstream processing may consume.

## Commit markers define the durable prefix

Each group appends data frames, one in-WAL `CommitMarker`, flushes, and performs one `fdatasync`. Acknowledgement is recorded only after sync succeeds. The last valid marker proves the persisted prefix; it is not replaced by an external watermark file.

```text
segment 00000000000000000000.chwal
  capture event
  capture event
  commit marker  ← durable prefix
  capture event  ← complete but uncommitted suffix
```

ETL reads only the recovery-authoritative committed prefix. A complete frame after the final valid marker remains visible as an uncommitted suffix and never becomes canonical evidence.

## Recovery behavior

Recovery validates segment headers, envelope versions, recording identity, sequence continuity, frame checksums, marker references, cumulative totals, and marker digests. It may repair only an incomplete final frame or final marker tail after verification. Complete corruption, identity mismatch, invalid references, unsupported versions, and sequence gaps fail closed.

Reopening a recording resumes after the last authoritative marker, removes only the reported uncommitted suffix, and preserves sequence continuity. The system never infers that a caller observed an acknowledgement from bytes alone.

## Physical bounds

Segment size is bounded between 16 MiB and 4 GiB. Total bytes under `segments/` default to and never exceed 4 GiB. The writer reserves room for a complete data frame and its final marker before writing; rotation also reserves the next header and temporary publication peak.

Queue admission loss and WAL-limit loss remain typed evidence. They are not hidden as successful capture.

:::caution
WAL and payload artifacts can contain production headers, bodies, credentials, and personal data. Private file modes are safeguards, not encryption at rest, comprehensive redaction, or tenant isolation.
:::

See the repository's [WAL format](https://github.com/r05323028/chronicle/blob/main/docs/wal-format.md) for the current v1 framing contract.
