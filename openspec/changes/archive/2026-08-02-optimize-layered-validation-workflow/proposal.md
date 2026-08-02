## Why

Chronicle's current P1/P2 workflow rebuilds and reruns expensive privileged checks for unrelated changes, while successful runs retain large caches and raw data. This slows normal development and obscures the evidence needed for failures and releases.

## What Changes

- Add one `scripts/validate.sh` entry point with `fast`, `targeted`, `gate p1`, `gate p2`, and `release` modes.
- Add declarative `validation/groups.toml` path ownership and targeted selection reporting.
- Separate VM-local build/cache paths from compact failure-first evidence.
- Add deterministic per-gate fingerprints, manifest-backed evidence reuse, and invalidation rules.
- Preserve existing P1/P2 acceptance coverage through explicit gates and release validation.
- Add path-selection, fingerprint, artifact-retention, and evidence-reuse tests.
- Document runtime, retention, Multipass reuse, and cache/evidence policy.

## Capabilities

### New Capabilities

- `layered-validation`: Layered validation modes, dependency selection, gate coverage, fingerprinting, evidence reuse, and artifact policy.

### Modified Capabilities

## Impact

Affected shell validation and acceptance wrappers, CI artifact configuration, Multipass operational documentation, validation tests, and OpenSpec validation commands. No production capture, WAL, ETL, replay, or protocol behavior changes. Existing privileged scripts remain the authoritative P1/P2 checks and are invoked by the new entry point.
