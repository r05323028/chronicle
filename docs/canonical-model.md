# Canonical Session Model

## Mutable MVP v1

Canonical Session has one active schema: `CANONICAL_SCHEMA_VERSION = 1`. Writer and reader accept only schema value 1. Rust model is unversioned `CanonicalSession`; no DTO dispatch, migration defaults, or compatibility reader exists. Contract remains mutable until explicit compatibility-freeze OpenSpec change.

Model uses strong IDs, timestamps, deterministic operation sequences, relative nanosecond offsets, canonical client/server endpoints, protocol IDs, typed operation kind/effect, request and recorded-response payload references, protocol extension bytes, warnings, redaction records, and source provenance.

Canonical endpoints come only from capture socket evidence:

- active socket: local = client, remote = server;
- passive socket: remote = client, local = server.

Ingress/egress selects byte direction only. ETL returns typed failure for missing or conflicting socket evidence and never stores `unknown:0` endpoints.

Session-level `source_provenance`, `connection_completeness`, `operation_completeness`, and `replay_attributes` are ordinary v1 fields. Completeness maps are authoritative. `timeline` is sole operation order. `PayloadRef` supports bounded inline bytes, filesystem artifacts, S3-compatible references with checksum and size, redacted values, and explicit missing values. `BTreeMap` attributes keep serialization deterministic.

Protocol data declares `PROTOCOL_DATA_SCHEMA_VERSION = 1`; canonical validation rejects any other value. Protocol extension bytes remain narrow and typed by media type rather than replacing core canonical fields.

## Storage

Filesystem publication atomically stores `sessions/<id>/manifest.json`, `session.json`, and `payloads/<sha256>`. Artifact keys are session-qualified. Sole mutable manifest v1 records session identity, canonical version, checksum, payload count/size, WAL checkpoint, bounded issues, completeness, and replay blockers. Store stages private `0700` directories and `0600` files on Unix, writes manifest last, and renames only to absent destination. Inspect verifies artifact metadata; replay hydrates and checks SHA-256.

Repository artifacts are rewritten with model changes while compatibility remains unfrozen. Any future version increase requires explicit compatibility-freeze change defining frozen scope, reader/writer matrix, migration, and deprecation policy.

## Planned MVP

PostgreSQL stores session/connection/operation indexes, replay metadata, verification summaries, ETL checkpoints, and artifact references. Large bodies, raw capture artifacts, BSON, compressed frames, and replay artifacts move to S3-compatible storage after redaction.

Canonical operations do not assume alternating requests/responses. Protocol canonicalizers own correlation: Kafka uses correlation IDs; PostgreSQL handles extended-query state; NATS may emit publishes/subscriptions without direct responses. Opaque bytes remain recoverable when semantic decode is unavailable.
