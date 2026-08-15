## Why

Chronicle main already has two incompatible lifecycles: public command/PID/cgroup recording goes through `record_production`, where the 600-second default and 3,600-second maximum are recording-wide stop bounds, while `ContinuousRecorderService` has crash-recoverable epoch rollover for the daemon. The result is that a user-visible recording cannot naturally span epochs, `--pid`/`--cgroup` cannot run indefinitely, and each continuous epoch is exposed internally as a different `RecordingId` even though it belongs to one capture run.

This change makes recording lifetime, epoch lifetime, and segment lifetime separate contracts. A recording may run until an explicit lifecycle condition; epochs and segments remain bounded, roll over without planned capture detach, and retain deterministic lineage, quota, retention, recovery, and incremental ETL behavior.

## What Changes

- Add an explicit recording/capture-run model above the existing bounded WAL epoch and segment model. One stable public `RecordingId` owns an ordered lineage of unique epoch identities and bounded segments.
- Converge public command mode, PID mode, cgroup mode, and the existing continuous recorder on one application-owned long-lived orchestration path. Keep one capture source and bounded ingest queue alive across planned epoch rollover; do not add a parallel rollover mechanism.
- Split `ProductionRecordingBounds` into recording lifecycle/deadline policy, epoch bounds, and segment bounds. Remove the implicit public 600-second recording deadline and the public 3,600-second recording-wide cap. Keep the current one-hour/4-GiB values as epoch ceilings where appropriate; never replace `MAX_RECORDING_DURATION_SECONDS` with an arbitrary larger recording limit.
- Make `--duration` optional whole-recording deadline. `chronicle record -- ./app`, `--pid PID`, and `--cgroup PATH` without it continue until child/source completion, explicit stop, or unrecoverable failure. Attached workloads are never terminated; supervised command children retain bounded cleanup semantics.
- Make age/byte thresholds epoch rollover triggers only. Durably finalize the old epoch, persist the handoff outcome, create/link one successor, and continue capture. Segment size/age rotation remains the smaller WAL unit.
- Define stable run identity, epoch identity, ordinal, predecessor/successor derivation, parent/run metadata, epoch catalog, per-epoch sessions, list/inspect aggregation, replay selection, and restart behavior. Existing one-epoch 0.1.x artifacts remain readable through an explicit compatibility mapping. `epochs.json` is the sole authoritative parent-to-epoch topology; run summaries and global catalogs are derived.
- Preserve in-WAL commit-marker authority, WAL v1 framing, bounded ingest, final-tail repair, incremental ETL publication-before-checkpoint ordering, quota reservations, retention proof, and safe deletion. Finalized epochs may ETL/publish while capture writes the successor; the WAL epoch boundary is not inherently a protocol reconstruction boundary.
- Permit bounded cross-epoch protocol continuation through versioned, checksummed, lineage-verified predecessor-to-successor checkpoints. A logical operation that completes after rollover is emitted once by its deterministic completion-owner epoch; unsupported continuation is explicit incomplete evidence.
- Specify crash-safe rollover decisions for every transition boundary, including successor allocation, predecessor finalization, catalog activation, metadata publication, quota reservation, and ETL/checkpoint races. Recovery must choose one authoritative lineage, never duplicate publication, and never silently lose accepted observations.
- Extend list, inspect, status, and replay to operate on a stable recording aggregate while exposing ordered epoch state and honest partial/in-progress information. Replay uses immutable published epoch sessions in deterministic order and never treats an incomplete/gapped epoch as absent.
- Document bounded resource behavior under sustained quota pressure, retention modes, recorder restarts, host/pod restarts, and long-lived daemon operation. Add migration notes for 0.1.x flags and artifacts.

The public lifecycle change is **BREAKING** relative to the current normal `record` behavior: an omitted `--duration` no longer defaults to 600 seconds, and `--duration` no longer means an epoch/one-recording hard bound. Hidden legacy 0.1.x entrypoints may retain old one-shot behavior during their deprecation window, but they are not the public model.

## Capabilities

### New Capabilities

None. Existing continuous-recorder, identity, WAL, ETL, CLI, and operations capabilities already own these boundaries; adding a second lifecycle capability would duplicate authority.

### Modified Capabilities

- `continuous-recorder-lifecycle`: define recording/run versus epoch versus segment responsibilities, explicit recording stop conditions, stable parent lineage, rollover, and restart continuation.
- `recording-identity`: make one catalog identity span multiple epochs and define epoch/session association, list, inspect, and migration semantics.
- `user-intent-cli`: change public duration defaults, command/PID/cgroup termination behavior, aggregate list/inspect/replay, and compatibility wording.
- `recoverable-recording-wal`: keep segment/WAL bounds per epoch, preserve commit authority, and connect quota/retention to repeated epoch rollover.
- `recorder-durability`: define crash-safe successor transition, authoritative active-epoch recovery, and quota/checkpoint ordering across rollover.
- `restartable-recording-etl`: process finalized epochs as immutable publication units while restoring bounded predecessor continuation state, binding checkpoints/publications to parent and epoch lineage, and aggregating without duplicate output.
- `local-session-artifacts`: publish per-epoch canonical sessions with immutable continuation references and stable parent provenance while preserving session-store authority.
- `safe-local-http-replay`: select ordered immutable epoch sessions safely without replaying an active or gapped tail by accident.
- `recording-store`: retain parent/epoch/WAL-range provenance in immutable artifact references and keep cleanup gated by authoritative downstream state.
- `recording-diagnostics`: expose stable run identity, active/previous epoch, aggregate counters, per-epoch lag/publication state, and restart/rollover remediation.
- `production-recorder-operation`: document days/months operation, persistent lineage recovery under a systemd supervisor, and supported deployment constraints without adding a Chronicle operator or Kubernetes packaging.

## Impact

Affected implementation areas are primarily `chronicle-application` (`record.rs`, `command_record.rs`, `continuous_recorder.rs`, `recorder_orchestration.rs`, `epoch_catalog.rs`, `rollover_transition.rs`, `recording_catalog.rs`, metadata/config/quota/retention modules), `chronicle-common` identity primitives, `chronicle-wal` epoch/manifest APIs, `chronicle-etl` incremental checkpoints and publication keys, `chronicle-storage` summaries/artifact provenance, `chronicle-replay` only if application-owned multi-session planning requires a neutral adapter, and `chronicle-cli` parsing/rendering/exit mapping. `chronicle-capture-ebpf` remains behind its existing application boundary and must not own lifecycle or rollover policy.

Affected contracts include private run/epoch manifests, one authoritative parent-to-epoch catalog, per-epoch v2 checkpoints and continuation artifacts, parent/epoch session provenance, public list/inspect/replay view models, recorder status, configuration validation, and compatibility readers for existing single-epoch/continuous artifacts. WAL v1 frame bytes, commit markers, Capture Event v1, Canonical Session validation, replay deny-by-default policy, and crate dependency direction remain reliability boundaries.

Implementation must update canonical English CLI/concepts/architecture/ETL/recorder/deployment/troubleshooting documentation, versioned website pages plus `zh-tw`/`ja` counterparts, acceptance scenario descriptions, and `AGENTS.md` if the run/epoch/segment distinction becomes a durable agent invariant. This change creates planning artifacts only; it does not implement production code.
