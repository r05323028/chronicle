## Why

Chronicle still has unproven and partially wired P2 behavior around lease-owned recovery, epoch rollover, incremental ETL, quota, retention, and privileged crash evidence. These gaps can produce stale ownership, cross-epoch reconstruction, orphaned publication state, or false-positive acceptance reports.

## What Changes

- Move catalog/WAL/checkpoint recovery behind recorder lease and quota ownership.
- Make epoch rollover recoverable, quota-safe, and free of empty epochs.
- Finalize incremental ETL from verified delta/checkpoint lineage and preserve epoch-boundary incompleteness without predecessor state injection.
- Wire production quota accounting, retention cleanup, corruption fail-closed behavior, and lifecycle-index bounds.
- Strengthen privileged acceptance with real kill/restart/reboot/cleanup evidence, unified profile/executor orchestration, fingerprinted reusable artifacts, complete check status, and release-only exact source identity checks.
- Correct OpenSpec task/evidence records after implementation and independent review.

## Capabilities

### New Capabilities

- `p2-completion`: Lease-owned recovery, crash-safe rollover, authoritative incremental publication, quota/retention safety, and truthful privileged acceptance required before P2 release.

### Modified Capabilities

<!-- Existing requirements remain intact; this change adds a release-readiness contract over their combined behavior. -->

## Impact

Affected crates: `chronicle-application`, `chronicle-etl`, `chronicle-wal`, and `chronicle-storage`. Affected acceptance scripts, validation groups, runbook, and OpenSpec evidence. No external API or dependency change intended.
