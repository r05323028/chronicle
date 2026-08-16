## 1. Migrate tests to the current interface (do first, keeps tree green)

- [ ] 1.1 `tests/support/process.py`: `inspect_session` drops the legacy `--root` argument and uses `--data-dir`; `replay` helper drops `--root` and uses the current `--data-dir` explicit-target form.
- [ ] 1.2 `tests/acceptance/test_inspect.py` and `tests/acceptance/test_replay.py`: remove legacy `--root` invocations (via the updated support helpers); assertions unchanged.
- [ ] 1.3 `tests/smoke/test_smoke.py` `test_missing_fixture_returns_sane_error`: replace `record --source fixture --input --root` with `internal record-fixture --input --root`.
- [ ] 1.4 `tests/smoke/test_documented_commands.py`: `test_record_help_hides_legacy_flags` updated so it asserts the legacy flags are absent (not merely hidden).
- [ ] 1.5 `scripts/tests/test-user-intent-cli-rollback.sh`: current-binary side uses `internal record-fixture` and current inspect/etl forms; the controlled PREVIOUS binary keeps its own (old) forms; update the header comment.
- [ ] 1.6 Run the rootless suites bounded (`python3 tests/smoke/test_smoke.py`, `test_documented_commands.py`, `tests/acceptance/*.py`, `tests/e2e/test_rootless_pipeline.py`) before proceeding.

## 2. Remove the CLI compatibility surface (`crates/chronicle-cli/src/main.rs`)

- [ ] 2.1 Remove `Command::Recorder`, `Command::RecorderStatus`, `Command::Etl` variants and their merged dispatch arms; keep the `InternalCommand` arms.
- [ ] 2.2 Remove `Source` enum, `RecordArgs.source/input/root`, `record_fixture_legacy`, the dead hidden record options (`wal_dir`, `allow_shared_cgroup`, `segment_bytes`, `duration_seconds`, `max_wal_bytes`), and the `validate_record_arguments` source branch.
- [ ] 2.3 Remove `ReplayArgs.root` and `run_replay_legacy`; keep `--timing` (used by the public replay path).
- [ ] 2.4 Remove `InspectArgs.root`.
- [ ] 2.5 Remove `LegacyInvocation`, `DeprecationJson`, `write_deprecation_warning`, `legacy_error_message`, `exit_legacy_error`, `legacy_invocation`, `legacy_invocation_from_raw_args`, `raw_legacy_record`, `raw_has_flag`, `format_from_raw_args`, and the raw-arg scanning in `async_main`.
- [ ] 2.6 Update `legacy_ebpf_source_flag_is_rejected` to `record_source_flag_is_rejected` (any `--source` is rejected by clap); remove legacy-specific tests (`legacy_invocations_use_fixed_non_secret_diagnostics`, legacy-invocation parse tests); keep the `internal`-namespace parse test.
- [ ] 2.7 `cargo check`/clippy; remove `chronicle-application` exports proven unused by the CLI removal (verify each via `cargo check`; do not guess).

## 3. Remove the compatibility acceptance scenario

- [ ] 3.1 Delete `scripts/acceptance/lib/scenarios/shared/cli-compatibility.sh`.
- [ ] 3.2 Remove `cli-compatibility` from `scripts/acceptance/scenarios.toml` (live-capture and recorder profiles; also drop the `compatibility` capability entry if listed).
- [ ] 3.3 Update `validation/test-architecture/test-catalog.toml` (entries referencing `cli-compatibility`), `validation/test-architecture/path-classification.md`, and `validation/test-architecture/decisions.md`.
- [ ] 3.4 Check `scripts/acceptance/report.py` capability set for a `compatibility` capability and remove it if present.
- [ ] 3.5 Update `scripts/tests/validation/test_layered_validation.py` scenario-coverage assertions if they enumerate `cli-compatibility`.

## 4. Extend the reintroduction guard

- [ ] 4.1 `scripts/validation.py` `legacy_names_check`: add `LegacyInvocation`, `DeprecationJson`, `record_fixture_legacy`, `run_replay_legacy`, `raw_legacy_record`, `legacy_invocation_from_raw_args`, `legacy_error_message`, `exit_legacy_error`, `format_from_raw_args`, `write_deprecation_warning`; add a check rejecting the `cli-compatibility` scenario id in `scripts/acceptance/scenarios.toml`.
- [ ] 4.2 `scripts/tests/validation/test_legacy_names.py`: add one case per new identifier plus a scenario-id case.

## 5. Specs (edited in place; `skip_specs: true`, same convention as remove-pre-release-migration-baggage)

- [ ] 5.1 `openspec/specs/user-intent-cli/spec.md`: replace the "Legacy forms SHALL remain hidden deprecated compatibility entrypoints through 0.1.x ... removal targeted for 0.2" requirement and its scenarios (Legacy record remains bounded and hidden, Legacy help hidden, deprecated invocation failures) with removal-before-0.1.0; state that exactly five public commands plus the hidden `internal` namespace exist and no deprecation warning machinery is retained.
- [ ] 5.2 `openspec/specs/recording-identity/spec.md`: remove the "legacy one-epoch recording remains resolvable ... explicit compatibility adapter" clause; remove "Legacy `--root` behavior remains exact and is not a data-directory alias" and its scenario; reword bare-UUID acceptance from "for compatibility only" to an ordinary accepted input form; keep the WAL v1 identity adapter requirement.
- [ ] 5.3 `openspec/specs/mvp-schema-versioning/spec.md`: extend the pre-0.1 policy - unreleased CLI compatibility surfaces are removed before 0.1.0, hidden pre-release entrypoints SHALL NOT be reintroduced, and the legacy-names guard enforces this.
- [ ] 5.4 Verify the completed `openspec/changes/remove-pre-release-migration-baggage` and archive it (openspec archive workflow).

## 6. Documentation

- [ ] 6.1 `docs/operations.md`: delete the "Legacy 0.1.x syntax migration" appendix; reword "Hidden `chronicle internal etl` ... remains deployment/compatibility mechanism" to deployment mechanism; keep the `wal_size_limit` readability note.
- [ ] 6.2 `docs/architecture.md`: remove "hidden compatibility paths may still address session IDs and explicit roots through 0.1.x"; rename the "One-shot recording pipeline" section to "Recording pipeline" (verify body text against current runtime); migrate lasting kernel/eBPF constraints from `docs/feasibility/` into "Capture semantics" and "Risks"; drop the "Planned MVP" status label (use "Planned"/"Future").
- [ ] 6.3 `docs/continuous-recorder.md` and `docs/continuous-recorder-runbook.md`: remove the "hidden deprecated aliases through 0.1.x ... removed at the 0.2 boundary" sentences.
- [ ] 6.4 `docs/release-notes.md`: delete the deprecation schedule; move the durable artifact/rollback compatibility content (WAL v1/canonical v1 stability, cross-version check command) into `docs/operations.md`; then either reduce `docs/release-notes.md` to a pre-release record or delete it and update the `README.md` "Release notes" link accordingly.
- [ ] 6.5 `docs/feasibility/`: delete `gate-a-ubuntu-24.04-kernel-6.8-aarch64.json`, the "Verified 2026-07-29" run details, "Historical verification measurements", the stale `(cd ebpf-feasibility ...)` commands, and the duplicate validation instructions; keep only a one-line pointer to the retained `privileged_feasibility` acceptance test and the lasting constraints (already moved in 6.2/6.1); if the pointer has no home there, put it in `validation/test-architecture/README.md` instead.
- [ ] 6.6 `docs/adr/0001-compile-time-protocol-registry.md`: add a one-line decision-history pointer in `docs/protocol-plugin-model.md`; remove `docs/adr/`.
- [ ] 6.7 Normalize residual milestone terminology: "Mutable MVP v1"/"Planned MVP" labels in `docs/canonical-model.md`, `docs/replay-safety.md`, `docs/wal-format.md`, `docs/protocol-plugin-model.md` -> "Planned"/"Future" (content unchanged).
- [ ] 6.8 `crates/chronicle-cli/src/main.rs`: update the stale "0.1.x compatibility" doc comments on the `internal` namespace and `doctor` hidden probe args.

## 7. Website (documentation contract: en/zh-tw/ja)

- [ ] 7.1 `website/src/content/docs/docs/reference/cli.md` and the `ja/`, `zh-tw/`, and `0-1/` copies: remove the "hidden 0.1.x invocation" migration notes and `docs/release-notes.md` links; reflect the removal decision.
- [ ] 7.2 Run `cd website && npm run verify:localization`.

## 8. AGENTS.md

- [ ] 8.1 Add durable rules: no hidden pre-release CLI compatibility surface may be introduced (historical stages live in Git history and `openspec/changes/archive/`); enumerate the authoritative documentation set; note that the legacy-names guard covers removed CLI identifiers and scenario ids. Keep it a short navigation-map entry; do not add implementation details.

## 9. Final validation

- [ ] 9.1 `./scripts/validate.sh fast` (fmt, warnings-denied clippy, workspace tests, strict OpenSpec validation, ownership, architecture, catalog, legacy-names, tooling tests) passes.
- [ ] 9.2 `openspec validate --all --strict --no-interactive` passes from the repository root.
- [ ] 9.3 On Linux (or in a Multipass VM on macOS hosts, per AGENTS.md): run a bounded recorder acceptance profile, because acceptance scenario configuration changed. Do not run privileged gates on macOS without the VM.
