# Design

## Decision 1: One runtime model per domain; version numbers stay in persisted schemas only

Removing a legacy model deletes its struct, its converters, and its
dispatch branches. Surviving models keep their persisted `version` field
and const. Type names drop the V1/V2 suffix where only one model exists:

| Removed | Surviving rename |
| --- | --- |
| `EpochCatalogV1`, `EpochCatalogEntry` (V1) | `EpochCatalogV2` -> `EpochCatalog`, `EpochCatalogEntryV2` -> `EpochCatalogEntry` |
| `EpochCatalogSummaryV2` fields beyond `published` | `EpochCatalogSummaryV2` -> `EpochCatalogSummary` (derived/cache-only) |
| `RolloverTransitionV1`, `RolloverTransitionPhase` (V1) | `RolloverTransitionV2` -> `RolloverTransition`, `RolloverTransitionV2Phase` -> `RolloverTransitionPhase`; `load_transition_v2` / `write_transition_v2_atomic` take the short names |
| `IncrementalEtlCheckpointV1`, `read_checkpoint`, `write_checkpoint_atomic`, `RecoveryAuthoritativeSnapshot`, `reconcile_delta_checkpoint` | `IncrementalEtlCheckpointV2` -> `IncrementalEtlCheckpoint`; single `publish_delta_then_checkpoint` (keeps fault injection) and `recover_pending_checkpoint` |
| `EpochContinuationCheckpointV1` / `ContinuationDependencyV2` (single models with version-suffixed names) | `EpochContinuationCheckpoint` / `ContinuationDependency` |
| `LegacyOneEpochMapping`, `EpochEvidenceAuthority::LegacyOneEpochMapping` | removed; `EpochEvidenceAuthority` keeps remaining authorities |
| one-shot runtime `record_production`, `record_production_with_lifetime`, `record_live_ebpf`, `record_live_ebpf_with_lifetime` | removed; CLI `--source ebpf` hidden entrypoint removed |

Renames are Rust-identifier-only; serialized field names and persisted
version numbers are untouched (epoch catalog stays version 2, rollover
transition stays version 2, incremental checkpoint stays version 2).

## Decision 2: epochs.json stays topology authority; summary is cache-only

`EpochCatalogSummary` keeps only `published: bool`, documented as a
derived cache written during finalization. It is never consulted by
recovery (checksum, lineage, and WAL markers drive recovery) and never
gates ETL or retention. `committed_through`, `committed_bytes`,
`checkpoint_through`, `continuation_state`, `retention_state`, and
`warning_codes` are removed because nothing reads them; ETL cursor lives
in `IncrementalEtlCheckpoint`, cross-epoch dependency in continuation
state files, publication authority in the session store/manifest, and
retention authority in retention metadata.

## Decision 3: Rollover journal keeps capture-only phases

`RolloverTransition` keeps the V2 phase machine
`prepared -> successor_created -> boundary_committed ->
topology_activated -> complete`. Continuation completion never advances
the rollover transaction; the pending continuation state file is written
durably before successor metadata so successor ETL cannot start without
restored predecessor state, but that dependency is tracked by
`ContinuationDependency`, not by the rollover journal.

## Decision 4: One incremental checkpoint

`IncrementalEtlCheckpoint` (parent_id/epoch_id/epoch_ordinal, marker and
segment lineage, decoder state, outputs, published operation keys,
continuation dependency, checksum) is the only runtime checkpoint. The
legacy `incremental-etl-checkpoint.json` (V1, owner Recorder) is no
longer written, read, or recovered; only
`incremental-etl-checkpoint-v2.json` survives under its existing file
name. The standalone `chronicle etl` final-session checkpoint
(`etl-checkpoint.json`, `RecordingEtlCheckpoint`) is a separate
one-shot ETL contract and stays.

## Decision 5: One-shot runtime removal

`record_production*` and `record_live_ebpf*` exist only behind the
hidden CLI `--source ebpf` flag (LegacyInvocation::RecordEbpf). No test,
script, or doc references them. Removing them removes the second
production lifecycle; public `record` (command/PID/cgroup) and daemon
`recorder` already route through `record_continuous_ebpf_with_source`.
`finalize_and_publish` fails closed when epochs.json is missing instead
of synthesizing a parent==epoch catalog. `--source fixture` (offline
fixture tool) stays.

## Decision 6: No production path may reference removed legacy names

A static check in `scripts/validation.py` (`legacy-names` subcommand)
greps `crates/` production sources for the removed identifiers and fails
the fast gate if any appear, with a meta-test in
`scripts/tests/validation/`. This prevents migration baggage from
returning without an explicit OpenSpec change.

## Out of scope

WAL v1 framing/commit-marker semantics, Capture Event v1, Canonical
Session v1, replay safety contracts, recording catalog
(`CatalogV1`/`CatalogEntryV1`/`RecordingIntentV1`),
`RecorderMetadataV1`, `RecorderConfigV1`, `RecorderStatusV1`,
`RecordingRunV2`/run manifest family, and `EpochOutcomeJournalV1`
stay as-is except where their APIs reference removed types.
