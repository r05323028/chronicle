# Boundary and dependency-direction review (version 1)

Review date: 2026-08-13. Scope: every shared fixture/helper named in
`migration-ledger.toml` must have one owner and no target plan may introduce a
forbidden workspace edge (CLI lower-layer, protocol-to-builtins, session-to-WAL,
common-upward, Aya leakage, or unclassified dev/build edge).

## Verified current graph

`python3 scripts/validation.py architecture --root . --config validation/architecture.toml`
exits 0 with 0 issues, 38 normal edges, 2 dev edges
(`chronicle-application -> chronicle-wal [dev]`, `chronicle-etl ->
chronicle-protocol-builtins [dev]`), 0 build edges, acyclic. `chronicle-cli`
depends only on `chronicle-application` in every dependency kind.

## Shared fixture/helper ownership

| Helper | Owner | Consumers | Constraint |
| --- | --- | --- | --- |
| `crates/chronicle-application/tests/support/mod.rs` | chronicle-application | application integration tests | crate-local; no cross-crate use |
| `crates/chronicle-application/tests/http_test_server.rs` | chronicle-application | application integration tests | crate-local test support |
| `tests/e2e/http_acceptance_driver.py` | root-black-box-support | acceptance/privileged suites | classified infrastructure; not E2E proof (task 4.4) |
| `scripts/acceptance/lib/wait.sh` | acceptance-tooling | acceptance scenarios | reused, not replaced (task 6.2) |
| `scripts/acceptance/recorder-readiness.sh` | acceptance-tooling | acceptance scenarios | specialized semantics preserved (task 6.5) |
| `scripts/run-with-timeout.sh` | validation-tooling | all suites | reused as deadline wrapper |
| `validation/test-architecture/*` | validation-tooling | this migration | new; owned by validation tooling, no product deps |

## Constraints confirmed for the target plan

- No new shared test-support crate is proposed; every helper stays language/crate
  local or root-suite local. Task 2.4 adds fixture tests proving the existing
  architecture validator rejects a test-helper dev-edge escape hatch.
- No target plan adds `chronicle-cli` dev dependencies on lower crates; CLI
  black-box tests keep invoking the binary or `chronicle-application` surfaces.
- Aya/kernel ABI stays private to `chronicle-capture-ebpf`; privileged adapter
  tests are the only kernel-touching surface and remain crate-owned.
- `validation/test-architecture/**` becomes owned by the `build_tooling`
  validation group (path update) so catalog/ledger changes select tooling
  validation rather than privileged gates.
- Privileged preflight (task 7.x) will reuse doctor probe vocabulary but must not
  mutate production state; it stays separate from product assertions.

## Verdict

Ledger and gate matrix introduce no forbidden edge, no unclassified dev/build
dependency, and no shared-helper ownership ambiguity. All current 13-crate
ownership rules remain satisfied.

## Task 3.5 — cli_contract.rs assertion-by-assertion audit

All 17 tests in `crates/chronicle-cli/tests/cli_contract.rs` (1130 lines) classified
by assertion purpose; binary-level contracts stay in place:

| Test | Purpose | Verdict |
| --- | --- | --- |
| cli_record_inspect_replay_and_exit_contract | binary record→inspect→replay + exit mapping | keep |
| denied_legacy_replay_emits_one_hint_error_and_no_result | legacy replay denial hint | keep |
| public_explicit_replay_requires_gates_and_never_contacts_recorded_target | replay safety (no target contact) | keep |
| cli_json_replay_error_has_no_partial_stdout | error rendering/exit | keep |
| cli_dry_run_ignores_unneeded_missing_runtime_credential | argument mapping | keep |
| cli_etl_publishes_repairs_checkpoint_and_rejects_corruption | ETL CLI surface + corruption/checkpoint matrix | **split** |
| cli_reports_usage_and_data_errors_as_safe_json | rendering/exit mapping | keep |
| doctor_config_failure_keeps_independent_probes_and_human_remediation | doctor contract | keep |
| legacy_warnings_and_failure_hints_are_atomic_and_non_secret | rendering/exit | keep |
| malformed_legacy_parse_error_has_safe_new_syntax_hint | parse error mapping | keep |
| empty_list_and_legacy_inspect_contract | list/inspect mapping (overlaps smoke:list-empty) | keep |
| public_inspect_resolves_recording_identity | inspect identity resolution | keep |
| doctor_prospective_data_dir_is_non_mutating_and_actionable | doctor non-mutation | keep |
| rootless_public_record_fails_before_target_on_unsupported_build | unsupported-build fail-closed (overlaps smoke) | keep |
| public_list_storage_error_exits_three_without_stdout | storage error exit mapping | keep |
| rootless_command_replay_fails_before_target_without_access_separation | safety | keep |
| bootstrap_blocks_until_ready_then_hardens_and_execs_target | bootstrap readiness | keep |

**Split executed (3.3/3.5):** ETL corruption fail-closed, cross-output equivalence,
rerun idempotence, and missing-checkpoint repair now proven portably in
`crates/chronicle-application/tests/etl_contract.rs` (4 tests). The CLI test keeps
only the binary surface (JSON shape, exit code, checkpoint version fields).
Remaining overlaps with smoke/acceptance suites are deliberate compatibility
redundancy, removed only under 8.5 after replacement coverage is final.

## Task 3.4 — protocol/session/correlation + replay matrices audit

All protocol/session/correlation and replay planner/executor/verifier matrices
verified rootless in owning crates: `chronicle-replay` src (27 unit tests,
including `execution_stops_on_transport_or_verification_and_accounts_for_skips`
— the test P1/P2 replay scenarios invoke as a compat step; passes on macOS),
`chronicle-session` src (25 tests), `chronicle-protocol-builtins` src (28
tests). No protocol/replay/session matrix lives privileged-only; the privileged
scenario step re-runs a portable unit test. Coverage already mapped:
`replay` scenario obligations -> unit:chronicle-replay + acceptance + e2e;
legacy `replay_matrix` -> unit:chronicle-replay; legacy `inspect_replay` ->
acceptance:inspect-display + replay-report. Scenario steps that re-invoke the
cargo matrix are removed under task 8.3 (blocked on the concurrent change).
