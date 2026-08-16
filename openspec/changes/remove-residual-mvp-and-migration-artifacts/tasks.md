## 1. Migrate eBPF feasibility coverage into production validation paths

- [ ] 1.1 Audit every capability proven by `ebpf-feasibility/src/main.rs` + `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` against production coverage (hook availability, connect4/6, sockops, cgroup-skb, socket-cookie correlation, PID/TGID, cgroup identity, GSO/nonlinear, truncation, ring buffer, loss accounting, decoder correctness) and record the audit result in the change notes.
- [ ] 1.2 Extend `crates/chronicle-capture-ebpf/tests/privileged_adapter.rs` (or add a sibling privileged suite) with kernel-behavior assertions that load the production object (`production_object()` from `ebpf/target/...` or the embedded fallback object): hook attach/detach matrix for connect4/6, sockops, cgroup-skb; connect tuple/role; socket-cookie correlation; PID/TGID + cgroup identity; cgroup-skb direction/sequence/plaintext; GSO/nonlinear visibility as observed by the production probe; truncation; 8 MiB ring per-CPU sampling; loss accounting (100 ms-or-later, delayed, mandatory final sample, forced loss).
- [ ] 1.3 Drop feasibility-probe-only claims that the production probe does not exhibit; do not inherit them as production claims.
- [ ] 1.4 Verify decoder correctness is fully covered rootless (`crates/chronicle-capture-ebpf/src` + chronicle-capture tests); add rootless cases only for gaps.
- [ ] 1.5 Run the new privileged suite on a privileged Linux host (Multipass VM on macOS per AGENTS.md) and record the passing evidence before any removal task.

## 2. Remove ebpf-feasibility

- [ ] 2.1 Delete `ebpf-feasibility/Cargo.toml`, `ebpf-feasibility/Cargo.lock`, `ebpf-feasibility/.cargo/config.toml`, `ebpf-feasibility/rust-toolchain.toml`, `ebpf-feasibility/src/main.rs` (after 1.5 passes).
- [ ] 2.2 Delete `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` (the harness loader; includes the `ebpf-feasibility/target/...` path join at ~line 1449 and the "run cd ebpf-feasibility" error at ~line 374).
- [ ] 2.3 Remove `ebpf-feasibility/**` from `validation/groups.toml` (paths at line ~20; `build_inputs` at lines ~109 and ~120).
- [ ] 2.4 Update `scripts/acceptance/lib/scenarios/live-capture/capture-basic.sh` phase 15 (`privileged_feasibility` cargo invocation) to run the production-object kernel suite from task 1.2.
- [ ] 2.5 Update `scripts/acceptance/lib/scenarios/recorder/wal-recovery.sh` (`run_compat_command wal-feasibility ...`, ~lines 199-201) to the production-object kernel suite.
- [ ] 2.6 Update `validation/test-architecture/test-catalog.toml`: replace the `integration:capture-ebpf-privileged-feasibility` entries (lines ~43, 90, 142, 357-361, 619, 703) with the new production-object test id.
- [ ] 2.7 Update `validation/test-architecture/decisions.md` (~lines 21, 45) and `validation/test-architecture/README.md` (~line 71): "kernel feasibility" -> production-object kernel acceptance.
- [ ] 2.8 Update `scripts/acceptance/report.py` check set (~line 481): replace `privileged_feasibility` with the new check id.
- [ ] 2.9 Verify no remaining references: grep for `ebpf-feasibility` and `privileged_feasibility` across `ebpf/`, `crates/`, `scripts/`, `validation/`, `docs/`, `.github/` returns nothing.

## 3. Remove the implicit session identity fallback

- [ ] 3.1 `crates/chronicle-application/src/recording_catalog.rs` `recording_for_summary` (~lines 180-183): require explicit `source_provenance.recording_id`; remove `unwrap_or(RecordingId(summary.session_id.0))`. Callers (`summary_by_recording` ~189, `resolve_latest` ~257/266, `reconcile_catalog` ~484, `list_recordings` ~622) then associate only explicitly-provenanced sessions.
- [ ] 3.2 `crates/chronicle-application/src/replay_inspect.rs` `build_parent_replay_plan` (~line 570): remove the `recording_id.is_none() && session_id.0 == parent_id.0` fallback match; remove the `epoch_id.unwrap_or(EpochId(parent_id.0))` synthesis (~line 584); missing provenance yields a typed unresolved error (e.g. `RecordingNotFound` with an actionable message naming the missing provenance) instead of implicit association.
- [ ] 3.3 Do not add a compatibility reader or conversion for provenance-less sessions; orphan sessions remain resolvable only as session artifacts, never through recording lineage.

## 4. Rewrite affected tests

- [ ] 4.1 Remove `legacy_session_id_provides_parent_replay_identity` (`crates/chronicle-application/src/replay_inspect.rs` ~line 1459); add a missing-provenance failure test for `build_parent_replay_plan` and an explicit-provenance association test.
- [ ] 4.2 Rewrite `crates/chronicle-application/src/recording_catalog.rs` tests that construct provenance-less sessions and expect id-equality grouping (catalog/list/latest); replace with explicit-provenance and unresolved-session cases.
- [ ] 4.3 `tests/support/process.py`: `inspect_session` drops `--root` (uses `--data-dir`); `replay` helper drops `--root` (current `--data-dir` explicit-target form).
- [ ] 4.4 `tests/acceptance/test_inspect.py` and `tests/acceptance/test_replay.py`: remove legacy `--root` invocations via the updated support helpers; assertions unchanged.
- [ ] 4.5 `tests/smoke/test_smoke.py` `test_missing_fixture_returns_sane_error`: replace `record --source fixture --input --root` with `internal record-fixture --input --root`.
- [ ] 4.6 `tests/smoke/test_documented_commands.py`: update help assertions so legacy flags are absent (not merely hidden).
- [ ] 4.7 `scripts/tests/test-user-intent-cli-rollback.sh`: current-binary side uses `internal record-fixture` and current inspect/etl forms; the controlled PREVIOUS binary keeps its own forms; update the header comment.
- [ ] 4.8 Run the rootless suites bounded before proceeding (`tests/smoke/*.py`, `tests/acceptance/*.py`, `tests/e2e/test_rootless_pipeline.py`, `scripts/tests/validation/test_legacy_names.py`).

## 5. Remove the CLI compatibility surface and obsolete validation references

- [ ] 5.1 `crates/chronicle-cli/src/main.rs`: remove `Command::Recorder`/`RecorderStatus`/`Etl` variants + dispatch arms; keep `InternalCommand` arms.
- [ ] 5.2 `crates/chronicle-cli/src/main.rs`: remove `Source` enum, `RecordArgs.source/input/root`, `record_fixture_legacy`, dead hidden record options (`wal_dir`, `allow_shared_cgroup`, `segment_bytes`, `duration_seconds`, `max_wal_bytes`), and the `validate_record_arguments` source branch.
- [ ] 5.3 `crates/chronicle-cli/src/main.rs`: remove `ReplayArgs.root` + `run_replay_legacy` (keep `--timing`); remove `InspectArgs.root`.
- [ ] 5.4 `crates/chronicle-cli/src/main.rs`: remove `LegacyInvocation`, `DeprecationJson`, `write_deprecation_warning`, `legacy_error_message`, `exit_legacy_error`, `legacy_invocation`, `legacy_invocation_from_raw_args`, `raw_legacy_record`, `raw_has_flag`, `format_from_raw_args`, and raw-arg scanning in `async_main`.
- [ ] 5.5 `crates/chronicle-cli/src/main.rs` tests: rename `legacy_ebpf_source_flag_is_rejected` to a generic `record_source_flag_is_rejected`; remove `legacy_invocations_use_fixed_non_secret_diagnostics` and legacy-invocation parse tests; keep the `internal`-namespace parse test.
- [ ] 5.6 Delete `scripts/acceptance/lib/scenarios/shared/cli-compatibility.sh`; remove `cli-compatibility` from `scripts/acceptance/scenarios.toml` (live-capture + recorder profiles) and any `compatibility` capability entry.
- [ ] 5.7 Update `validation/test-architecture/test-catalog.toml` (`cli-compatibility` entries), `validation/test-architecture/path-classification.md` (~lines 7, 14), and `validation/test-architecture/decisions.md` (~line 34); check `scripts/acceptance/report.py` and `scripts/tests/validation/test_layered_validation.py` for `cli-compatibility`/capability references.
- [ ] 5.8 Run `cargo check`/clippy; remove `chronicle-application` exports proven unused by the CLI removal (verify each, do not guess).

## 6. Extend the reintroduction guard

- [ ] 6.1 `scripts/validation.py` `legacy_names_check`: add removed CLI identifiers (`LegacyInvocation`, `DeprecationJson`, `record_fixture_legacy`, `run_replay_legacy`, `raw_legacy_record`, `legacy_invocation_from_raw_args`, `legacy_error_message`, `exit_legacy_error`, `format_from_raw_args`, `write_deprecation_warning`); add filesystem-presence checks for `ebpf-feasibility/`, `docs/feasibility/`, `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs`; add a scenario-id check rejecting `cli-compatibility` in `scripts/acceptance/scenarios.toml`.
- [ ] 6.2 `scripts/tests/validation/test_legacy_names.py`: add one case per new identifier, per filesystem-presence check, and the scenario-id case.

## 7. Update specs (edited in place; `skip_specs: true`)

- [ ] 7.1 `openspec/specs/user-intent-cli/spec.md`: replace the "Legacy forms SHALL remain hidden deprecated compatibility entrypoints through 0.1.x ... removal targeted for 0.2" requirement and its legacy scenarios with removal-before-0.1.0; five public commands plus hidden `internal`; no deprecation machinery.
- [ ] 7.2 `openspec/specs/recording-identity/spec.md`: remove the "legacy one-epoch recording remains resolvable" and "Legacy `--root` behavior remains exact" clauses; add: session-to-recording association requires explicit `source_provenance`; identifier equality is not lineage; provenance-less sessions are unresolved; reword bare-UUID acceptance to an ordinary accepted input form.
- [ ] 7.3 `openspec/specs/replay-safety/spec.md`: add: parent replay selection uses only explicit provenance; missing provenance is invalid/unresolved; no id-equality fallback.
- [ ] 7.4 `openspec/specs/mvp-schema-versioning/spec.md`: extend the pre-0.1 policy - unreleased CLI compatibility surfaces are removed before 0.1.0; runtime lineage is never inferred from identifiers; feasibility-only implementations must not be reintroduced when production validation paths exist.
- [ ] 7.5 `openspec/specs/ebpf-capture-adapter/spec.md`: privileged kernel validation runs only against the production probe; no standalone feasibility harness.
- [ ] 7.6 Verify the completed `openspec/changes/remove-pre-release-migration-baggage` and archive it (openspec archive workflow).

## 8. Documentation

- [ ] 8.1 Delete `docs/feasibility/` (`README.md`, `gate-a-ubuntu-24.04-kernel-6.8-aarch64.json`) after migrating lasting knowledge (tasks 8.2-8.3); no experiment artifacts remain in active docs.
- [ ] 8.2 `docs/architecture.md`: merge lasting kernel/eBPF constraints from `docs/feasibility/README.md` into "Capture semantics"/"Risks" (hook capabilities, socket-cookie correlation, PID/TGID + cgroup-id caveats, GSO/nonlinear visibility, ring/loss sampling semantics, plaintext-only); remove "hidden compatibility paths ... through 0.1.x"; rename "One-shot recording pipeline" to "Recording pipeline"; drop the "Planned MVP" status label.
- [ ] 8.3 `docs/operations.md`: merge the verified environment matrix (Ubuntu 24.04, Linux 6.8, aarch64, cgroup v2, BTF; other targets not verified) into "Safety and scope"; delete the "Legacy 0.1.x syntax migration" appendix; reword the `internal etl` "deployment/compatibility mechanism" phrasing; keep the `wal_size_limit` readability note.
- [ ] 8.4 `validation/test-architecture/README.md`: add a pointer to the privileged production-object kernel test; replace "kernel feasibility" wording.
- [ ] 8.5 `docs/continuous-recorder.md` (~line 57) and `docs/continuous-recorder-runbook.md` (final section): remove "hidden deprecated aliases through 0.1.x ... removed at the 0.2 boundary" sentences.
- [ ] 8.6 `docs/release-notes.md`: delete the deprecation schedule; move durable artifact/rollback compatibility content (WAL v1/canonical v1 stability, cross-version check command) into `docs/operations.md`; reduce to a pre-release record or delete and update the `README.md` "Release notes" link.
- [ ] 8.7 `docs/adr/0001-compile-time-protocol-registry.md`: add a one-line decision-history pointer in `docs/protocol-plugin-model.md`; remove `docs/adr/`.
- [ ] 8.8 Normalize residual milestone terminology: "Mutable MVP v1"/"Planned MVP" labels in `docs/canonical-model.md`, `docs/replay-safety.md`, `docs/wal-format.md`, `docs/protocol-plugin-model.md` -> "Planned"/"Future" (content unchanged).
- [ ] 8.9 `crates/chronicle-cli/src/main.rs`: update stale "0.1.x compatibility" doc comments on the `internal` namespace and `doctor` hidden probe args.
- [ ] 8.10 Website: `website/src/content/docs/docs/reference/cli.md` + `ja/`, `zh-tw/`, `0-1/` copies - remove hidden-invocation migration notes and `docs/release-notes.md` links; run `cd website && npm run verify:localization`.

## 9. AGENTS.md

- [ ] 9.1 Add short durable rules (navigation-map entry, no implementation details): do not add migration adapters for unreleased formats; do not add compatibility aliases before a public contract exists; do not add feasibility-only implementations when production validation paths exist; historical behavior belongs in Git history and archived OpenSpec changes; runtime lineage must be explicit and never inferred from identifiers; the legacy-names guard covers removed CLI identifiers, retired paths, and scenario ids.

## 10. Final validation

- [ ] 10.1 `./scripts/validate.sh fast` passes (fmt, warnings-denied clippy, workspace tests, strict OpenSpec validation, ownership, architecture, catalog, legacy-names, tooling tests).
- [ ] 10.2 `openspec validate --all --strict --no-interactive` passes from the repository root.
- [ ] 10.3 On Linux (or Multipass VM on macOS per AGENTS.md): run a bounded recorder acceptance profile plus the production-object privileged kernel suite, because eBPF validation and acceptance scenario configuration changed. Do not run privileged gates on macOS without the VM.
