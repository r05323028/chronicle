## Context

P2 currently spans recorder startup, epoch catalog/WAL recovery, incremental ETL, quota, retention, and privileged acceptance. Existing helpers and unit tests cover pieces, but several helpers are not wired into production and acceptance scripts leave required scenarios `not_checked`. Recovery must be owned by the recorder lease and quota authority. Acceptance evidence identifies tested content with an acceptance-sensitive fingerprint; commit/tree SHA remains provenance and becomes an exact identity gate only for release evidence.

## Goals / Non-Goals

**Goals:**

- Make startup and rollover recovery single-owner, checkpoint-authoritative, and rollback-safe.
- Preserve recording independence at epoch boundaries and publish incremental results from verified lineage.
- Integrate quota, retention, corruption, and lifecycle-index behavior into production paths.
- Make privileged acceptance execute real crash/restart/delete/reboot scenarios and reject incomplete or stale evidence.

**Non-Goals:**

- New capture adapters, new protocols, cloud storage, or P3 features.
- Weakening fail-closed behavior to make acceptance pass.
- Treating OpenSpec validation or unit fault injection as privileged runtime evidence.

## Decisions

1. **Lease first, recovery second.** `EpochCatalogV1::load` becomes read-only. After domain lease and quota authorities are owned, startup performs prepared-tail recovery, WAL reopen/create, checkpoint reconciliation, metadata publication, and capture attachment in one ordered path.
2. **Durable transition records over distributed rollback.** Rollover writes a recoverable transition record before successor mutation. Recovery either completes the transition or removes only unreferenced successor state and restores reservations.
3. **Authoritative snapshots before capture.** Startup validates checkpoint epoch, marker, segment lineage, pipeline, configuration, and output artifacts against read-only committed WAL evidence before invoking capture source `start`.
4. **Incremental output is authoritative.** Stop/finalization reconstructs the session from verified delta/checkpoint lineage, compares it with clean one-shot ETL, and publishes through the session store. Standalone ETL remains an independent diagnostic path.
5. **No cross-epoch identity inheritance.** Boundary-incomplete payload is preserved as typed incomplete/non-replayable evidence. Predecessor socket envelopes and protocol state never enter successor reconstruction.
6. **Quota and retention are application-owned.** Every actual durable peak is reserved. Finalized cleanup requires lifecycle-index proof and protected-lineage checks; corruption remains preserved and fail-closed.
7. **Evidence is profile-aware and fingerprinted.** One runner selects P1/P2 scenarios and local/Multipass execution modes. Evidence uses `acceptance/<profile>/<fingerprint>/<run-id>/`, verifies scenario coverage, schema, environment, and artifact manifests, and permits compatible P2 evidence to satisfy P1. Normal development accepts dirty trees and reuses equivalent content without commit equality. Release validation additionally requires clean known start/end commit/tree identity, complete coverage, and release-eligible evidence.

## Risks / Trade-offs

- **[Migration]** Existing prepared tails may lack transition records. **Mitigation:** recover only states with verifiable metadata and WAL lineage; preserve ambiguous evidence and fail closed.
- **[Disk usage]** Retaining corrupt or protected evidence can consume quota. **Mitigation:** account retention/protected bytes explicitly and surface terminal quota pressure.
- **[Complexity]** One ordered startup path touches multiple crates. **Mitigation:** keep authority boundaries in `RecorderStartup` and expose narrow recovery helpers with focused tests.
- **[Acceptance duration]** Real crash and Multipass scenarios are slower. **Mitigation:** retain portable tests for fast feedback; reserve privileged evidence for release gates.
