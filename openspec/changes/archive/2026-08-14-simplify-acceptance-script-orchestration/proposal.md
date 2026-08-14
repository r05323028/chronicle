## Why

Chronicle's acceptance orchestration carries structural wrappers that no longer earn their place: four deprecated aliases and two compatibility-named test runners that merely forward to canonical tooling, plus a scenario dispatch convention that forces per-profile extension files and case-dispatch stubs even when they add no behavioral boundary. Each wrapper costs reading, maintenance, debugging, and acceptance-fingerprint surface without providing a caller or compatibility obligation.

## What Changes

- **BREAKING** Remove `scripts/acceptance/p1-privileged.sh`, `p2-privileged.sh`, `p1-multipass.sh`, `p2-multipass.sh` — deprecated aliases whose only references are a self-imposed delegation test, a docs sentence, and generated validation snapshots. No CI, cataloged test, or documented workflow calls them.
- **BREAKING** Remove `scripts/tests/acceptance/test-p1-privileged-runner.sh` and `test-p2-privileged-runner.sh` — compatibility test names that only exec the unified `test_runner.py` and appear in no catalog verify command or CI step.
- Remove the `scenarios/extensions/<profile>/` layer (10 files) and the four shared case-dispatch stubs (`capture-basic`, `wal-recovery`, `replay`, `resource-cleanup`). Profile-specific implementations move to `scenarios/p1/` and `scenarios/p2/`; the two genuinely shared scenarios (`cli-compatibility`, `user-intent-lifecycle`) keep one implementation under `shared/` and drop their one-line extension forwards.
- Simplify the dispatch convention to one owner per scenario: `shared/<scenario>.sh` provides `scenario_<name>()`; otherwise `<profile>/<scenario>.sh` provides `scenario_<profile>_<name>()`. The shell dispatcher and the Python runner validate the same convention.
- Update the acceptance tooling tests (dispatch assertions, wrapper regression), validation fixtures, `baseline.json`, the migration ledger, and docs so nothing references removed paths.
- Preserve: `scripts/acceptance.sh` as the single entrypoint, all scenario sets/order/timeouts, evidence schema, preflight classification, cleanup, Multipass behavior, and gate guarantees.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `layered-validation`: add requirements that acceptance exposes exactly one canonical entrypoint with no compatibility wrappers absent a concrete obligation, and that every scenario has exactly one implementation owner resolved by an explicit shared-or-profile convention validated by both dispatcher and runner.

## Impact

- `scripts/acceptance/lib/scenarios/` restructures (`extensions/` deleted; `p1/` and `p2/` directories hold profile implementations; `shared/` holds only shared implementations).
- `scripts/acceptance/lib/scenario-dispatch.sh` and `scripts/acceptance/report.py` change implementation-resolution logic; `scripts/tests/acceptance/test_runner.py` and `scripts/tests/validation/test_layered_validation.py` updated.
- `validation/test-architecture/baseline.json` regenerated; `migration-ledger.toml` embedded-entry paths updated for moved scenario files; `test_test_architecture.py` ledger-linkage path normalization updated.
- Acceptance-sensitive fingerprint changes (file set under `scripts/acceptance/**` changes), so previously retained evidence is not reused; the content-addressed reuse mechanism itself is unchanged and new evidence is produced by the next gates.
- No production code, CLI surface, persisted format, WAL/canonical/eBPF/protocol behavior, crate boundary, dependency, timeout value, cleanup semantics, or preflight classification changes.
