## Why

Chronicle has not released 0.1.0 and has no external users or production compatibility obligations. The unbounded-recording implementation introduced migration-era V1/V2 runtime models and compatibility adapters (EpochCatalogV1/V2, RolloverTransitionV1/V2, IncrementalEtlCheckpointV1/V2, one-shot vs continuous orchestration branches, parent-UUID==epoch-UUID synthesis). Carrying parallel runtime implementations solely for unreleased internal formats is baggage 0.1.0 should not start with.

## What Changes

- Collapse dual runtime models into one current production model per domain. Persisted schema version numbers may remain; duplicate live implementations must not.
- Remove EpochCatalogV1, its V1->V2 / V2->V1 converters, the compatibility loader, and the legacy one-epoch mapping abstraction. Keep one `EpochCatalog` (schema version 2) as parent/epoch topology authority with checksum and lineage validation.
- Remove RolloverTransitionV1 and the duplicated `load_transition`/`load_transition_v2`, `write_transition_atomic`/`write_transition_v2_atomic` APIs. Keep one `RolloverTransition` with the current `prepared -> successor_created -> boundary_committed -> topology_activated -> complete` phases. Capture/WAL rollover stays independent of ETL continuation.
- Remove IncrementalEtlCheckpointV1 and the dual checkpoint publication/recovery paths. Keep one `IncrementalEtlCheckpoint` (ETL cursor/decoder authority) and keep `EpochContinuationCheckpoint`/`ContinuationDependency` as the separate cross-epoch processing dependency.
- Remove the legacy one-shot recording runtime (record_production/record_production_with_lifetime/record_live_ebpf) and the hidden `--source ebpf` CLI entrypoint. Command, PID, cgroup, and daemon recording all route through the same continuous coordinator, which always owns an epoch catalog; finalization no longer synthesizes parent/epoch compatibility by setting parent UUID == epoch UUID.
- Trim the epoch-catalog summary to a derived/cache-only `published` flag; ETL cursor, continuation state, retention state, and warning codes no longer live in the topology catalog.
- WAL v1 byte/framing format, WAL commit-marker semantics, Capture Event v1, Canonical Session v1, replay safety contracts, and the recording catalog are intentionally stable and out of scope for removal.
- User-visible behavior of the public commands (record, list, inspect, replay, recorder, doctor) is unchanged except that the hidden legacy `--source ebpf` entrypoint is removed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `recording-identity`: remove the legacy one-epoch parent/epoch compatibility mapping requirement and scenario; one `EpochCatalog` remains the sole topology authority.
- `mvp-schema-versioning`: restate the pre-0.1 policy - unreleased internal schemas may be replaced rather than supported through permanent runtime compatibility layers; stable wire formats preserved by the project remain versioned separately.
