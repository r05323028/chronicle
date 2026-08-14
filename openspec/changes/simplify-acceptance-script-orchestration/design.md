## Context

`scripts/acceptance.sh` is the single user-facing acceptance entrypoint and execs `scripts/acceptance/runner.py`, which selects scenarios from `scenarios.toml`, fingerprints source, and runs a profile under a gate deadline through `run-with-timeout.sh`. The profile scripts (`lib/profile-p1.sh`, `lib/profile-p2.sh`) source shared libraries and `lib/scenario-dispatch.sh`, which loads and runs ordered scenario modules with per-scenario watchdogs, timeout evidence, and persisted scenario state.

The current scenario convention requires, for shared scenarios, both `shared/<scenario>.sh` and `extensions/<profile>/<scenario>.sh`; four shared files are pure `case "$CHRONICLE_ACCEPTANCE_PROFILE"` dispatch stubs whose real implementations live in `extensions/`, and two shared files carry the real implementation plus a dispatch stub while their extension files are one-line forwards. Four `scripts/acceptance/pX-*.sh` aliases and two `scripts/tests/acceptance/test-pX-privileged-runner.sh` files forward to canonical tooling.

## Goals / Non-Goals

**Goals:**

- One canonical acceptance entrypoint; no wrapper without a concrete obligation.
- One owner per scenario implementation; no one-line profile adapters for identical behavior.
- Convention over duplicated routing boilerplate, explicit and validated in both shell and Python.
- Keep runtime/environment setup separate from product assertions; keep recorder readiness specialized.
- Preserve scenario sets/order/timeouts, evidence schema, preflight classification, cleanup, Multipass behavior, and gate guarantees.
- Preserve the lowest-cost conclusive-proof test architecture.

**Non-Goals:**

- No production behavior, public CLI, persisted format, WAL, canonical schema, eBPF, protocol, crate boundary, or dependency changes.
- No rewrite of the acceptance framework into another language.
- No change to timeout values, cleanup semantics, unsupported-environment classification, or failure/infrastructure distinction.
- No collapse of `recorder-readiness.sh` into generic wait helpers: it owns lifecycle state, systemd state, stale ownership, bounded polling, cgroup/BTF/eBPF diagnostics, process diagnostics, and readiness transitions.

## Current Script Inventory and Call Graph

### Entrypoints (keep)

| Script | Role | Callers |
| --- | --- | --- |
| `scripts/acceptance.sh` | canonical acceptance entrypoint | `validate.sh`, docs, AGENTS.md |
| `scripts/validate.sh` | layered validation modes | CI `./scripts/validate.sh fast`, docs, AGENTS.md |
| `scripts/validation.py` | selection/orchestration/reporting helper | `validate.sh`, `runner.py`, tests |
| `scripts/run-with-timeout.sh` | process-tree timeout wrapper | every bounded step, profile gate wrap, multipass |

### Libraries (keep)

`scripts/lib/common.sh`, `scripts/lib/env.sh`, `scripts/lib/assertions.sh`, `scripts/acceptance/lib/wait.sh`, `scripts/acceptance/recorder-readiness.sh`, `scripts/acceptance/lib/multipass.sh`, `scripts/acceptance/lib/profile-p1.sh`, `scripts/acceptance/lib/profile-p2.sh`, `scripts/acceptance/lib/scenario-dispatch.sh`, `scripts/acceptance/runner.py`, `scripts/acceptance/report.py`, `scripts/acceptance/scenarios.toml`, `scripts/acceptance/separated-supervisor.py`, `scripts/privileged/preflight.py`.

### Compatibility wrappers (remove)

| Script | Content | References |
| --- | --- | --- |
| `scripts/acceptance/p1-privileged.sh` | DEPRECATED shim to `acceptance.sh --profile p1 --executor local` | self-test `test_compatibility_wrappers_delegate`, `docs/operations.md:128`, `test_layered_validation.py` fixtures (path only), generated `baseline.json` |
| `scripts/acceptance/p2-privileged.sh` | same for p2/local | same |
| `scripts/acceptance/p1-multipass.sh` | shim to p1/multipass, optional `--vm` arg | same |
| `scripts/acceptance/p2-multipass.sh` | shim to p2/multipass, optional `--vm` arg | same |
| `scripts/tests/acceptance/test-p1-privileged-runner.sh` | `exec python3 .../test_runner.py` | `baseline.json` script_tests only; no catalog verify command, no CI |
| `scripts/tests/acceptance/test-p2-privileged-runner.sh` | `exec python3 .../test_runner.py` | same |

No CI workflow, `validate.sh`, catalog command, or documented release step invokes any of these six files. The only behavioral assertion is `test_runner.py:test_compatibility_wrappers_delegate`, which asserts the aliases self-report DEPRECATED and delegate — a self-imposed obligation, not an external one.

### Scenario layer (restructure)

| File | Kind | Disposition |
| --- | --- | --- |
| `shared/cli-compatibility.sh` | real impl `cli_compatibility_impl` + case-dispatch stub | keep; `scenario_cli_compatibility()` calls impl directly |
| `shared/user-intent-lifecycle.sh` | real impl `user_intent_lifecycle_impl` + case-dispatch stub | keep; same simplification |
| `extensions/p1/cli-compatibility.sh`, `extensions/p2/cli-compatibility.sh` | one-line forward to impl | delete |
| `extensions/p1/user-intent-lifecycle.sh`, `extensions/p2/user-intent-lifecycle.sh` | one-line forward to impl | delete |
| `shared/capture-basic.sh`, `shared/wal-recovery.sh`, `shared/replay.sh`, `shared/resource-cleanup.sh` | case-dispatch stubs only | delete (stub) |
| `extensions/p1/{capture-basic,wal-recovery,replay,resource-cleanup}.sh` | real P1 implementations | move to `p1/` |
| `extensions/p2/{capture-basic,wal-recovery,replay,resource-cleanup}.sh` | real P2 implementations | move to `p2/` |
| `p2/{checkpoint-kill-restart,corruption-quarantine,incremental-etl,quota-pressure,reboot-recovery,recorder-readiness,retention-interruption}.sh` | P2-only implementations, `scenario_p2_*` | keep |

### Current call graph

```text
scripts/acceptance.sh
└─ runner.py ─┬─ report.py (scenarios.toml, scenario resolution)
              └─ validation.py (fingerprint/environment)
runner.py → local: lib/profile-{p1,p2}.sh | multipass: lib/multipass.sh → guest profile-{p1,p2}.sh
profile-{p1,p2}.sh → lib/common.sh, lib/env.sh, lib/assertions.sh, lib/wait.sh
                   └─ p2 adds acceptance/recorder-readiness.sh
                   └─ lib/scenario-dispatch.sh
scenario-dispatch.sh → shared/<s>.sh AND extensions/<profile>/<s>.sh | <profile>/<s>.sh
validate.sh → validation.py (select/ownership/architecture/catalog), run-with-timeout.sh, acceptance.sh
```

### Dynamic-loading facts (verified)

- `scenario-dispatch.sh:source_scenarios` loads scenario modules by convention (shared+extension, or profile-only); `run_scenario_plan` selects `scenario_<name>` vs `scenario_<profile>_<name>` by the same convention.
- `report.py:load_scenarios` validates the same convention (`extensions/<profile>/<s>.sh` required when shared exists).
- `validation/test-architecture/inventory.py` scans `scripts/acceptance/lib/scenarios` by `rglob` (path-independent).
- `test_runner.py:test_dispatch_order_and_implementations_are_complete` asserts the shared+extension layout and that no `p1/<s>.sh` exists today.
- No other tracked file references `extensions/` or the removed wrapper basenames.

## Target Script Ownership and Call Graph

```text
scripts/
├── acceptance.sh                  # canonical entrypoint (unchanged)
├── validation.py                  # unchanged
├── validate.sh                    # unchanged
├── run-with-timeout.sh            # unchanged
├── lib/                           # common/env/assertions (unchanged)
├── acceptance/
│   ├── runner.py                  # unchanged
│   ├── report.py                  # scenario resolution updated to new convention
│   ├── scenarios.toml             # unchanged (scenario sets/order/timeouts)
│   ├── recorder-readiness.sh      # unchanged
│   ├── separated-supervisor.py    # unchanged
│   └── lib/
│       ├── profile-p1.sh / profile-p2.sh / multipass.sh / wait.sh   # unchanged
│       ├── scenario-dispatch.sh   # source_scenarios simplified (no extensions)
│       └── scenarios/
│           ├── shared/            # cli-compatibility.sh, user-intent-lifecycle.sh (impl + scenario fn)
│           ├── p1/                # capture-basic, wal-recovery, replay, resource-cleanup
│           └── p2/                # same four, plus the seven P2-only scenarios
└── tests/
```

## Deletion Criteria

A `scripts/` file may be removed when all of the following hold:

1. No tracked file references its path or basename — CI, scripts, tests, docs, AGENTS.md, OpenSpec, validation artifacts — after legitimate doc/test updates in the same change.
2. It is not loaded dynamically by convention (scenario source, profile source, wrapper exec), or that convention is intentionally changed in the same change with identical behavior.
3. It provides no behavioral boundary (setup vs assertion, profile-specific runtime) that is not preserved at its new owner file.
4. Removal does not alter P1/P2 scenario sets, execution order, timeout values, evidence schema, preflight classification, cleanup semantics, Multipass behavior, or gate guarantees.

## Compatibility-Wrapper Policy

Only two obligations justify keeping a wrapper: (a) an external caller (CI workflow, documented release step, published evidence contract), or (b) a documented compatibility requirement (e.g., the `chronicle` binary's 0.1.x aliases, which are product surface). A test that asserts a wrapper delegates to canonical tooling is neither — it is updated to a regression test asserting the wrapper is absent and unreferenced. Under this policy all six forwarding files are removed.

## Scenario Loading/Dispatch Model

Convention (unchanged naming, simplified resolution):

```text
if shared/<scenario>.sh exists:
    source shared/<scenario>.sh          # provides scenario_<name>()
else:
    source <profile>/<scenario>.sh       # provides scenario_<profile>_<name>(); die if missing
```

- `scenario-dispatch.sh:source_scenarios` drops the shared=>extension pairing requirement and the "missing $profile extension" die; the profile-only die remains.
- `run_scenario_plan` function-name selection stays identical, so scenario bodies, evidence phase names, timeout markers, and scenario-state records are unchanged.
- `report.py:load_scenarios` mirrors the convention: shared file is the implementation when present; otherwise the profile file; missing implementation still raises.
- Shared scenario files no longer need a `case "$CHRONICLE_ACCEPTANCE_PROFILE"` stub; `scenario_cli_compatibility()` and `scenario_user_intent_lifecycle()` call their impl directly.
- Profile-specific files keep their exact function names (`scenario_p1_*`, `scenario_p2_*`), so profile cleanup/summary globals, `set_check`, and `phase` wiring are untouched.

## Evidence and Timeout Preservation

- `scenarios.toml` is byte-identical: scenario IDs, `timeout_seconds` (300/600/900), capabilities, phases, `legacy_p1_checks`/`legacy_p2_checks`, and P1/P2 `execution_order` unchanged. P2 remains a superset of P1.
- Scenario watchdog/timeout machinery (`start_scenario_watchdog`, `scenario_timeout_handler`, `terminate_scenario_descendants`, timeout markers, `current-phase.txt`, wait evidence) unchanged.
- `run-with-timeout.sh`, profile gate wrapping (`CHRONICLE_ACCEPTANCE_GATE_WRAPPED`), and multipass timeout arithmetic unchanged.
- Report schema (`report.py`), artifact manifest, preflight.json classification, and content-addressed reuse mechanism unchanged.
- Impact: because file paths/content under `scripts/acceptance/**` change, the acceptance fingerprint changes and previously retained evidence is not reused. That is the intended contract ("acceptance-sensitive content changes invalidate evidence"); the reuse mechanism itself remains and new evidence is produced by the next gates. `CHRONICLE_ACCEPTANCE_*` override variables and timeout evidence names are unaffected.

## Test and Validation-Artifact Migration

- `test_runner.py:test_dispatch_order_and_implementations_are_complete`: assert the new convention (each selected scenario has shared XOR profile implementation; shared scenarios have no `p1/`/`p2/` files; profile-specific scenarios have no shared file); keep P2-superset and dispatcher-content guards.
- `test_runner.py:test_compatibility_wrappers_delegate`: replace with a regression test that the six wrapper paths do not exist and no tracked file references their basenames.
- Add/extend a regression that every selected scenario resolves to exactly one implementation (no dynamic implementation becomes undiscoverable) and that missing implementations fail loudly in both `scenario-dispatch.sh` and `report.py`.
- `test_layered_validation.py`: replace the `scripts/acceptance/p1-privileged.sh` fixture path with `scripts/acceptance/scenarios.toml` (same acceptance-path semantics).
- Regenerate `validation/test-architecture/baseline.json` with `validation/test-architecture/inventory.py`; update `migration-ledger.toml` `embedded:*` entries (path and line) for moved scenario files; update `test_test_architecture.py` `LedgerLinkageTests` path normalization to strip new profile-directory prefixes.
- `test-catalog.toml`/`migration-ledger.toml` script-test entries: verify-command lists unchanged (`test_runner.py ; test-p2-readiness.sh ; test-wait.sh` etc.); confirm no entry references deleted wrappers.

## Documentation Updates

- `docs/operations.md`: remove the deprecated-wrapper sentence; document that `scripts/acceptance.sh` is the sole entrypoint and that scenario code lives in `lib/scenarios/{shared,p1,p2}/` with one owner per scenario.
- `AGENTS.md`: after implementation, add canonical script entrypoints and ownership rules (acceptance entrypoint, dispatcher convention, scenario ownership, wrapper policy, validation entrypoints).
- No other docs reference removed paths.

## Migration Plan

1. Audit inventory and record owner/caller/classification (this design).
2. Change the convention in `scenario-dispatch.sh` and `report.py` together with the file moves in one commit so the tree is never half-conventioned.
3. Rewrite the two shared scenario files; `git mv` eight implementation files; delete four stubs and ten extension files.
4. Delete the six forwarding files.
5. Migrate tests/fixtures; regenerate baseline; update ledger and ledger-linkage tests.
6. Update docs and AGENTS.md.
7. Rootless verification, then privileged P1/P2 with fresh evidence.
8. Final hygiene scan.

## Rollback

The change is a pure re-organization of tracked files. Rollback is `git revert` of the change commit; `git mv` preserves file history for the moved implementations; function names and `scenarios.toml` are unchanged so old scenario expectations keep working; the six removed forwarding files can be restored from history if a caller ever emerges. Evidence for the old fingerprint is not reusable after this change by design; a reverted tree restores the old fingerprint and its evidence.

## Risks / Trade-offs

- Fingerprint invalidation: all previously retained acceptance evidence becomes non-reusable. Mitigation: expected, documented, mechanism intact; gates produce fresh evidence once.
- Ledger/baseline drift: moved scenario paths change `embedded:` ledger IDs and baseline entries; `test_ledger_embedded_entries_match_baseline` enforces consistency. Mitigation: regenerate baseline and update ledger + normalization test in the same change.
- Undiscoverable scenario risk: the convention is enforced in two places (shell + Python) plus the tooling test, so a missing file fails before execution.
- No new abstraction is introduced: the simplification removes files and branches; no dispatcher metaprogramming beyond the existing explicit source loop.

## Open Questions

- None blocking. (Recorder readiness stays specialized per its domain scope; no generic-wait merge is proposed.)
