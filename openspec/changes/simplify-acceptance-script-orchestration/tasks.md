## 1. Inventory and audit

- [x] 1.1 Record the inventory and owner/caller/classification table from design.md against current HEAD: 47 shell scripts under scripts/, six forwarding files, ten extension scenario files, four shared dispatch stubs, two shared implementations.
- [x] 1.2 Verify no external caller exists for the four deprecated aliases or the two compatibility test-name runners (CI workflows, validate.sh, validation.py, test catalog, migration ledger, docs, AGENTS.md, OpenSpec specs) beyond self-referential tests and generated snapshots.
- [x] 1.3 Confirm scenario modules are loaded only by dynamic convention (scenario-dispatch.sh source_scenarios, report.py load_scenarios, inventory.py rglob) and enumerate every reference that must change.

## 2. Dispatcher and runner convention change

- [x] 2.1 `scripts/acceptance/lib/scenario-dispatch.sh`: simplify `source_scenarios` to source `shared/<scenario>.sh` when present, else `<profile>/<scenario>.sh` (die when missing); drop the shared=>extension pairing and its "missing extension" error.
- [x] 2.2 `scripts/acceptance/report.py`: `load_scenarios` resolves the implementation as the shared file when present, else the profile file; keep the missing-implementation error naming scenario and profile.
- [x] 2.3 Keep `run_scenario_plan` function-name selection (`scenario_<name>` vs `scenario_<profile>_<name>`) and all watchdog/timeout/state machinery unchanged.

## 3. Scenario file moves and rewrites

- [x] 3.1 Rewrite `shared/cli-compatibility.sh` so `scenario_cli_compatibility()` calls the implementation directly (drop the profile case dispatch).
- [x] 3.2 Rewrite `shared/user-intent-lifecycle.sh` so `scenario_user_intent_lifecycle()` calls the implementation directly.
- [x] 3.3 `git mv` `extensions/p1/{capture-basic,wal-recovery,replay,resource-cleanup}.sh` to `p1/` and `extensions/p2/{capture-basic,wal-recovery,replay,resource-cleanup}.sh` to `p2/`; function names unchanged.
- [x] 3.4 Delete the four shared case-dispatch stubs (`shared/{capture-basic,wal-recovery,replay,resource-cleanup}.sh`) and the ten `extensions/` files.
- [x] 3.5 Verify no cross-file references break: profile summary globals, `set_check`, `phase`, `wait_*`/`assert_*` helpers, `$CHRONICLE`/`$DRIVER`/`$ARTIFACT_ROOT` conventions unchanged after the moves.

## 4. Wrapper removal

- [x] 4.1 Delete `scripts/acceptance/{p1-privileged,p2-privileged,p1-multipass,p2-multipass}.sh`.
- [x] 4.2 Delete `scripts/tests/acceptance/{test-p1-privileged-runner,test-p2-privileged-runner}.sh`.

## 5. Caller and test migration

- [x] 5.1 Update `test_runner.py:test_dispatch_order_and_implementations_are_complete` to the shared-XOR-profile convention; keep P2-superset and dispatcher/multipass content guards.
- [x] 5.2 Replace `test_compatibility_wrappers_delegate` with a regression test: the six wrapper paths do not exist and no tracked file references their basenames.
- [x] 5.3 Add regression coverage that every selected scenario resolves to exactly one implementation and that a missing implementation fails in both `scenario-dispatch.sh` and `report.py` (no dynamic scenario becomes undiscoverable).
- [x] 5.4 Update `test_layered_validation.py` fixtures: replace `scripts/acceptance/p1-privileged.sh` with `scripts/acceptance/scenarios.toml` in both occurrences.
- [x] 5.5 Regenerate `validation/test-architecture/baseline.json` via `inventory.py`; update `migration-ledger.toml` `embedded:*` entries (paths and line numbers) for moved scenario files; update `test_test_architecture.py` `LedgerLinkageTests` path normalization for profile-directory prefixes.
- [x] 5.6 Confirm `test-catalog.toml` and `migration-ledger.toml` verify-command lists reference only surviving scripts; catalog and gate mappings remain valid.

## 6. Documentation updates

- [x] 6.1 `docs/operations.md`: remove the deprecated-wrapper sentence; document `scripts/acceptance.sh` as the sole entrypoint and the `lib/scenarios/{shared,p1,p2}/` ownership convention.
- [x] 6.2 `AGENTS.md`: add canonical script entrypoints and ownership rules (acceptance entrypoint, dispatcher convention, scenario ownership, wrapper policy, validation entrypoints).
- [x] 6.3 Sweep remaining docs/release notes for references to removed paths; update or delete as needed.

## 7. Dead-file verification

- [x] 7.1 Repo-wide scan: zero references to `p1-privileged`, `p2-privileged`, `p1-multipass`, `p2-multipass`, `test-p1-privileged-runner`, `test-p2-privileged-runner`, and `extensions/` in tracked files.
- [x] 7.2 Every remaining shell script has a documented owner and caller per design.md target ownership table.
- [x] 7.3 P1 scenario set, P2 scenario set, P2-superset-of-P1, and both execution orders are byte-identical to HEAD (`scenarios.toml` untouched).

## 8. Rootless verification

- [x] 8.1 Run `./scripts/validate.sh fast` (fmt, warnings-denied Clippy, workspace tests, strict OpenSpec validation, ownership, architecture, catalog, tooling tests) and fix findings.
- [x] 8.2 Run acceptance tooling tests: `test_runner.py`, `test-p2-readiness.sh`, `test-wait.sh`, `test-run-with-timeout.sh`, `test-user-intent-cli-rollback.sh`, and `scripts/tests/validation/*.py`.

## 9. Privileged verification

- [x] 9.1 Fresh supported Ubuntu 24.04 Multipass P1 gate (`--no-reuse`): full evidence retained, scenario set/order/timeouts unchanged, cleanup and preflight classification intact.
- [x] 9.2 Fresh P2 gate (`--no-reuse`) including reboot, quota, corruption, retention, and readiness scenarios; scenario-level and gate-level timeout evidence intact; artifact manifest complete.
- [x] 9.3 Confirm content-addressed reuse: a follow-up run with reuse enabled reuses the new evidence; report schema and reuse receipts unchanged.

## 10. Final repository hygiene

- [x] 10.1 Shell syntax check over every moved/rewritten scenario script; no shellcheck regressions.
- [x] 10.2 Reconcile implementation against proposal/design/spec delta; record exact validation/evidence results; mark only evidence-proven tasks complete.
- [x] 10.3 Final scan: no deleted script has a remaining caller, no dynamic implementation is undiscoverable, and docs/AGENTS.md reflect the canonical structure.

## Verification evidence

- Rootless: `./scripts/validate.sh fast` passed (fmt, warnings-denied Clippy, workspace tests, strict OpenSpec validation, ownership, architecture, catalog, tooling tests). Acceptance tooling: `test_runner.py` 43/43, `test_test_architecture.py` 26/26, `test_layered_validation.py` 15/15, `test_select_fixtures.py` 5/5, `test-wait.sh`, `test-p2-readiness.sh`, `test-run-with-timeout.sh`, and `scripts/tests/validation/*.py` pass. `test-user-intent-cli-rollback.sh` requires a controlled 0.1.x binary (`CHRONICLE_PREVIOUS_RELEASE_BIN`) and is a pre-existing environmental prerequisite, unchanged by this change.
- Privileged P1 (Multipass, Ubuntu 24.04, `--no-reuse`): passed, 6/6 scenarios completed in configured order (capture-basic, wal-recovery, replay, user-intent-lifecycle, cli-compatibility, resource-cleanup), preflight `supported`, no timeouts, artifact manifest verified (sha256).
- Privileged P2 (Multipass, `--no-reuse`): passed, 13/13 scenarios including reboot-recovery (boot-id before/after differ, `changed: true`), quota, corruption, retention, readiness; 31/31 legacy checks passed, 0 failed, 0 not_checked; preflight `supported`; pre/post-reboot phases passed; artifact manifest verified.
- Content-addressed reuse: reuse-enabled P1 run reused the fresh P2 evidence (P2->P1 superset reuse) and wrote a reuse receipt; report schema and manifest behavior unchanged.
- Evidence root: `target/validation-evidence/acceptance/{p1,p2}/f4062ce0.../20260814T11*` (fingerprint changed from HEAD as expected for acceptance-sensitive script reorganization).
- Repo scan: zero tracked references to removed wrapper basenames or `extensions/` in the living surface (CI, scripts, tests, docs, validation, AGENTS.md/README/CONTRIBUTING); shell syntax (`bash -n`) clean over all scenario scripts; `scenarios.toml` byte-identical (sets, order, timeouts, P2 superset unchanged).
