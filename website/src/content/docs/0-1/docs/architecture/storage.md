---
title: Storage
description: The local filesystem publication boundary for canonical sessions and payloads.
slug: 0-1/docs/architecture/storage
---

Chronicle currently stores recordings and canonical sessions on the local filesystem. Storage owns durable persistence and publication primitives; ETL owns the publication decision, publication verification, and checkpoint advancement ordering. Replay reads persisted canonical artifacts rather than WAL internals.

## Public data directory

Public commands resolve the data directory in this order:

1. `--data-dir DIR`;
2. configured `data_dir`;
3. `CHRONICLE_DATA_DIR`;
4. platform default.

A mutating command lazily creates a private directory and rejects unsafe root or symlink forms. `doctor` reports the existing or prospective location without creating probe artifacts.

```text
<data-dir>/
  .chronicle-domain.lock
  catalog.json
  recordings/<bare-recording-uuid>/
  sessions/<session-uuid>/
```

Within one local filesystem deployment, one normalized `.chronicle-domain.lock` protects name claim, capture, ETL, publication, and catalog update as one transaction. The lock is a local deployment coordination mechanism, not the architectural ownership mechanism between Recorder and ETL.

## Canonical publication

Each session is published as:

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

Staging directories use `0700` and files use `0600` on Unix. The manifest is written last, and publication renames only to an absent destination. Inspect verifies artifact metadata; replay hydrates payloads and checks SHA-256.

## What is not here

PostgreSQL metadata storage, S3-compatible artifact storage, remote WAL archival, encryption at rest, redaction policy, and tenant isolation are not implemented. Do not infer them from the storage interfaces or protocol registry.

:::caution
Local artifacts may contain production headers, bodies, credentials, and personal data. Treat the data directory as sensitive and apply your own host-level access controls.
:::
