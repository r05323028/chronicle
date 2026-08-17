---
title: Sessions
description: The identity and completeness model for Chronicle recordings.
---

Chronicle exposes a **recording** to users. A recording is a user-visible capture lifecycle composed of bounded epochs, together with its WAL, metadata, catalog identity, and published canonical results. The canonical result is a **session**: the portable unit consumed by inspect and replay.

## Recording identity

Public references are stable and human-oriented:

- `rec_<uuid>` is the user-facing recording ID.
- `latest` resolves through the catalog to the newest published recording.
- An exact name such as `checkout` can resolve a recording when it is unique.
- A bare UUID is accepted for direct identity lookup.

The catalog is advisory. Recovery-authoritative WAL facts and canonical session facts win contradictions. `chronicle record --retry RECORDING` retries recoverable finalization and publication without recapturing the workload.

## Session identity

A canonical `SessionId` is independently deterministic and may differ from the recording ID. Session association requires explicit source provenance (`recording_id` / `epoch_id`). Identifier equality is never lineage; sessions without sufficient provenance are unresolved. Users should address recordings rather than depending on internal session IDs.

## Completeness is explicit

Operations can be `complete`, `incomplete`, `truncated`, `malformed`, `unmatched`, or `unsupported`. A temporal loss window can make overlapping operations incomplete. Chronicle does not silently turn missing evidence into a replayable operation.

`inspect` reports one of these high-level replay states:

- `fully_replayable`
- `partially_replayable`
- `not_replayable`

A partially replayable session may execute operations proven outside a loss window while leaving unsafe or ambiguous operations visible as skipped.

## Local publication

ETL publishes the canonical session atomically. The manifest, session JSON, and content-addressed payload artifacts are written under a private staging directory; the manifest is written last and the destination is renamed only when publication is complete.
