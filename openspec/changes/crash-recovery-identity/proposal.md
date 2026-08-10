# Crash-Recovery Identity Consistency

## Why

The full P2 privileged gate intermittently crash-loops at checkpoint-kill-restart: after a SIGKILL lands inside a group-commit publication window, the restarted recorder's final session publication fails closed with `timeline operation … belongs to …, not …`, and the unit restarts into the same failure forever (restarts=44+). The session is rebuilt deterministically from the same WAL every restart, so the loop never converges.

Root cause: a crashed group commit can leave the same committed envelope reachable from two reconstruction connections — an exchange split by the per-connection reconstruction byte limit after the kill. Both sides decode to the same deterministic operation identity (content + WAL sequence), but the one-shot ETL deduplicated deterministic ids per connection, so the final session carried one operation id under two connections with two timeline entries. Canonical validation correctly rejects that as `TimelineOperationConnectionMismatch`, and the recorder fails closed. The daemon's incremental paths already deduplicate session-globally (persisted `emitted_operation_keys`, and `finalize`'s key set); the one-shot `finish_reconstructed` was the only per-connection site.

## What Changes

- Make `chronicle_etl::EtlPipeline::finish_reconstructed` deduplicate deterministic operation identities session-globally instead of per connection, matching the incremental batch and finalize paths. A colliding identity is kept exactly once, with its single authoritative timeline entry.
- This does not change the identity seed or weaken canonical validation: session/connection/operation ids remain content-derived from the persisted WAL snapshot, and the `validate()` fail-closed guard is untouched. Identical traffic on distinct connections with distinct WAL sequences already produced distinct ids and is unaffected.

## Capabilities

- `crash-recovery-identity`: deterministic operation identities must never appear under two connections in one session; a cross-connection collision is deduplicated to one occurrence with one timeline entry.

## Impact

- One functional change in `chronicle-etl` (`finish_reconstructed` dedup scope) plus one regression test.
- Canonical session format unchanged (schema version 1). WAL, ETL pipeline version, and checkpoint formats unchanged.
- Behavior on duplicate envelopes: the duplicate operation is now dropped instead of producing an unpublishable session. No legitimate traffic is lost (collisions require identical content AND identical WAL sequence, i.e. genuine duplication).
