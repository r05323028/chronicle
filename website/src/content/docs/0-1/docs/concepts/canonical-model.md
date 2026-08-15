---
title: Canonical model
description: The protocol- and storage-independent session representation used
  by inspect and replay.
slug: 0-1/docs/concepts/canonical-model
---

The canonical session is Chronicle's handoff between capture and replay. It contains application behavior in a stable model rather than eBPF hooks, WAL framing, or a particular storage backend.

## What it contains

The current v1 model includes:

* strong recording, session, connection, and operation identities;
* canonical client/server endpoints derived from socket evidence;
* deterministic operation timeline with relative nanosecond offsets;
* typed operation kind and effect;
* request and recorded-response payload references;
* connection and operation completeness maps;
* source provenance, loss windows, integrity, and WAL checkpoint information;
* replay attributes and explicit blockers;
* protocol extension bytes kept separate from core fields.

The timeline is the sole operation order. Completeness maps are authoritative. ETL fails for missing or conflicting endpoint evidence; it never stores a fabricated `unknown:0` endpoint.

## One mutable v1 contract

The current canonical schema is `CANONICAL_SCHEMA_VERSION = 1`. Readers reject other versions. Chronicle is not maintaining historical migration readers before an explicit compatibility freeze; a future version change must define its compatibility and migration policy in a separate design change.

## Filesystem artifacts

The local store publishes:

```text
sessions/<session-id>/
  manifest.json
  session.json
  payloads/<sha256>
```

The manifest records identity, canonical version, checksum, payload counts and sizes, WAL checkpoint, bounded issues, completeness, and replay blockers. Files use private modes on Unix. Replay hydrates payloads and checks SHA-256 before sending anything.

## Portability boundary

Replay consumes canonical sessions and protocol interfaces. It does not depend on capture, eBPF, WAL, ETL, or the original storage implementation. This separation lets the same replay core work with fixture-produced sessions and production-produced sessions.
