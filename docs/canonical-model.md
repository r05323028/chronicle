# Canonical Session Model

## Current

Canonical Session has one evolving MVP schema: `CANONICAL_SCHEMA_VERSION = 1`. Reader accepts only `schema_version == 1`; no v2/v3 discriminator, migration, or compatibility defaults exist because no stable canonical artifact has shipped. It uses strong IDs, timestamps, deterministic operation sequences, relative nanosecond offsets, connection endpoints, protocol IDs, typed operation kind/effect, request and recorded-response payload references, protocol extension bytes, warnings, and redaction records.

Session-level `source_provenance`, `connection_completeness`, `operation_completeness`, and `replay_attributes` are ordinary v1 fields. Completeness maps are authoritative; connection/operation boolean flags are not duplicated. `timeline` is sole authoritative operation order; no separate operation-order list exists. `PayloadRef` supports bounded inline bytes, filesystem Artifact references, S3-compatible object references with checksum and size, redacted values, and explicit missing values. Filesystem publication stores `sessions/<id>/manifest.json`, `session.json`, and `payloads/<sha256>` atomically; Artifact keys are session-qualified (`sessions/<id>/payloads/<sha256>`). Manifest v1 records schema/version, session checksum, payload count/size, WAL checkpoint, bounded issue summary, completeness, and replay blockers. Store stages private `0700` directories and `0600` files on Unix, writes manifest last, then renames only to absent final destination. Inspect checks artifact metadata while replay hydrates and verifies SHA-256. `BTreeMap` attributes keep serialized ordering deterministic. Protocol extension bytes are versioned and narrow; core fields are not an unstructured JSON blob.

Append optional MVP fields without changing canonical schema version. Reserve a future canonical v2 for first stable-release breaking change: changed semantics, removed required field, incompatible payload meaning, or incompatible replay behavior. WAL, protocol payload, and manifest versions remain separate domains.

## Planned MVP

PostgreSQL stores session/connection/operation indexes, replay metadata, verification summaries, ETL checkpoints, and artifact references. Large bodies, raw capture artifacts, BSON, compressed frames, and replay artifacts move to S3-compatible storage before metadata commit. Redaction runs before remote persistence.

Canonical operations preserve replay intent but do not assume alternating requests/responses. Protocol canonicalizers own correlation: Kafka uses correlation IDs; PostgreSQL handles extended-query state; NATS may emit publishes/subscriptions without direct responses.

## Future

Schema evolution tooling, richer protocol extension schemas, normalization policies, and content-addressed artifact layout remain future work. Opaque bytes must remain recoverable when semantic decode is unavailable.
