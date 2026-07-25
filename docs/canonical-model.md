# Canonical Session Model

## Current

Canonical Session schema version 2 adds defaultable structured operation warnings and backend-neutral `PayloadRef::Artifact { key, checksum, size, content_type }` while retaining schema v1 read compatibility. It uses strong IDs, timestamps, deterministic operation sequences, relative nanosecond offsets, connection endpoints, protocol IDs, typed operation kind/effect, request and recorded-response payload references, protocol extension bytes, incompleteness/truncation flags, and redaction records.

`PayloadRef` supports bounded inline bytes, filesystem Artifact references, S3-compatible object references with checksum and size, redacted values, and explicit missing values. Filesystem publication stores `sessions/<id>/manifest.json`, `session.json`, and `payloads/<sha256>` atomically; Artifact keys are session-qualified (`sessions/<id>/payloads/<sha256>`). Manifest v1 records schema/version, session checksum, payload count/size, WAL checkpoint, bounded issue summary, completeness, and replay blockers. Store stages private `0700` directories and `0600` files on Unix, writes manifest last, then renames only to absent final destination. Inspect checks artifact metadata while replay hydrates and verifies SHA-256. `BTreeMap` attributes keep serialized ordering deterministic. Protocol extension bytes are versioned and narrow; core fields are not an unstructured JSON blob.

Canonical JSON serialization is used for tests and application interchange during initialization. Schema version changes require migration or multi-version readers.

## Planned MVP

PostgreSQL stores session/connection/operation indexes, replay metadata, verification summaries, ETL checkpoints, and artifact references. Large bodies, raw capture artifacts, BSON, compressed frames, and replay artifacts move to S3-compatible storage before metadata commit. Redaction runs before remote persistence.

Canonical operations preserve replay intent but do not assume alternating requests/responses. Protocol canonicalizers own correlation: Kafka uses correlation IDs; PostgreSQL handles extended-query state; NATS may emit publishes/subscriptions without direct responses.

## Future

Schema evolution tooling, richer protocol extension schemas, normalization policies, and content-addressed artifact layout remain future work. Opaque bytes must remain recoverable when semantic decode is unavailable.
