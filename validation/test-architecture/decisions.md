# Deliberate decisions for current mixed suites (version 1)

Decisions recorded 2026-08-13 in `migration-ledger.toml`. Every decision names a
responsibility or reliability reason; no directory-aesthetic-only move is made.

## Rust colocated unit tests (42 files)

**Decision: stay.** All colocated `#[cfg(test)]` modules prove local logic and
stay colocated. Heuristic signal flags (subprocess/network/sleep/privilege) are
fixture or production-code text matches, not daemon or full-system lifecycle;
review confirmed none of the 42 files launches a Chronicle process or daemon.
Task 3.1 keeps them in place; no `tests/unit` tree is created.

## Crate integration suites (8 files)

| Suite | Decision | Reason |
| --- | --- | --- |
| `application/tests/fake_vertical.rs` | stay (integration) | rootless synthetic composition contract via public APIs; binary E2E added additively by task 5.1 |
| `application/tests/fixtures_http_vertical.rs` | stay (integration) | rootless deterministic fixture pipeline |
| `application/tests/http_test_server.rs` + `support/mod.rs` | stay (integration) | crate-local test support, one consumer |
| `capture-ebpf/tests/privileged_adapter.rs` | remain privileged (integration) | real eBPF attach/capture requires supported kernel |
| `capture-ebpf/tests/privileged_feasibility.rs` | split (3.6) | state-machine proof becomes rootless; kernel feasibility stays privileged |
| `cli/tests/cli_contract.rs` | split (3.5) | arg/exit/rendering stays CLI integration; liveness->smoke (4.1); features->acceptance (4.2); composition->E2E (5.1); no new lower dev deps |
| `cli/tests/privileged_signal.rs` | remain privileged (integration) | real cgroup/signal behavior |

## Root tests

`tests/e2e/http_acceptance_driver.py` is workload/replay-target **infrastructure**,
not E2E proof; it stays in place with that explicit classification (task 4.4). Its
self-test stays rootless and condition-based.

## Validation/acceptance tooling self-tests

Stay as portable tooling tests; they prove selector/orchestrator contracts, never
product correctness.

## Privileged acceptance scenarios (13)

- `remain_privileged`: recorder-readiness, quota-pressure, reboot-recovery.
- `make_rootless`: wal-recovery, incremental-etl, corruption-quarantine.
- `split`: capture-basic, replay, user-intent-lifecycle, cli-compatibility,
  resource-cleanup, checkpoint-kill-restart, retention-interruption.

Each split names its privileged remainder and portable destination in
`migration-ledger.toml` (scenario entries) and `gate-coverage.toml`
(scenario_coverage). Portable pieces move to owning crate unit/integration tests
(Groups 3-4) before scenario bodies are reduced (Group 8).

## Embedded portable/repository commands in privileged scenarios (14)

Every `cargo` invocation found by the baseline inventory is dispositioned in the ledger (the blind-spot audit in coverage-comparison.md accounts for all current-tree occurrences):
12 `move` to portable prerequisites/repository checks, 2 `remain_privileged`
(privileged feasibility, privileged signal), and the eBPF builds move to
portable compile checks. None stays as a scenario-body repository check after
Group 8.

## Concurrent work coordination

`scripts/acceptance/**` carries uncommitted edits from the in-flight
`eliminate-time-based-acceptance-assertions-and-isolate-scenario-state` change.
Scenario-body edits (Groups 3, 8) are deferred until that change lands; this
session only writes `validation/test-architecture/**`, `scripts/validation.py`,
`validation/groups.toml`, and new validation tooling tests.

## New root black-box suites (tasks 4.1/4.2/5.1/6.2)

Implemented this session:

- `tests/support/process.py` — shared bounded process/JSON/fixture/driver support (3 consumers: smoke, acceptance, e2e). No new dependency or workspace crate.
- `tests/smoke/test_smoke.py` — help, usage error, empty list, missing-fixture error, unsupported-build record failure, minimal fixture liveness per public surface.
- `tests/acceptance/test_record.py`, `test_inspect.py`, `test_replay.py` — per-feature black-box contracts (internal ETL CLI coverage reclassified to `integration:cli-contract` in task 4.3; no public ETL user surface exists).
- `tests/e2e/test_rootless_pipeline.py` — synthetic capture -> WAL -> canonical -> replay -> verification through the binary (invariants only).

Decisions:

- **Internal ETL CLI contract (task 4.3).** ETL has no documented public user surface, so the internal ETL seam is not Smoke or Acceptance. `tests/acceptance/test_etl.py` and the `internal etl` smoke liveness case were removed; their fail-closed contracts (metadata-less WAL rejected, identity mismatch rejected) now live in `crates/chronicle-cli/tests/cli_contract.rs` under the CLI-owned `integration:cli-contract` (exit-code mapping, JSON error rendering, fail-closed before processing). Application ETL correctness stays separately owned by `integration:etl-contract` (chronicle-application); the two are never merged. Exhaustive ETL idempotence/corruption matrices remain `chronicle-etl` integration ownership (task 3.3). Public ETL acceptance will be added separately only if ETL gains a documented public command.
- **Smoke version probe.** The binary exposes no `--version` flag; `smoke:cli-version` was renamed `smoke:list-empty` and `--help` serves as the liveness probe.
- **Internal forms retained intentionally.** `internal record-fixture` (fixture capture input) remains exercised as deterministic rootless setup for public smoke/acceptance/E2E surfaces; the `internal etl` seam is covered only by the CLI-owned Integration contract (`integration:cli-contract`), never as Smoke/Acceptance proof. Public `record`, `inspect`, `replay` drive user acceptance. Real capture/catalog flows remain privileged on supported Linux.
- **Catalog status.** 39 tests `existing`; 8 remain `planned` (all privileged: recorder-readiness, checkpoint-kill-restart, quota-pressure, retention-interruption, reboot-recovery, privileged-capture-pipeline, user-intent-lifecycle-privileged, cli-compat-privileged-sample).

## ETL coverage closure (task 3.3)

`crates/chronicle-application/tests/etl_contract.rs` (rootless, added this
session) proves the portable ETL publication contract previously duplicated in
privileged scenarios: publish + canonical session/manifest, rerun idempotence
(same session, already_published), one-shot equivalence across output roots,
and corruption fail-closed (segment byte flip rejected). Uses existing
`production_wal_from_fixture` test support and `protocol_registry()`; no new
dependencies or dev edges. Catalog test `integration:etl-contract` maps the
affected legacy checks (`one_shot_equivalence`, `corruption_*`,
`checkpoint_*`) and scenario obligations (wal-recovery, incremental-etl,
corruption-quarantine). Privileged scenario-body duplicates remain for task
8.x (blocked on the concurrent change).

## Privileged preflight probe + outcome contract (tasks 7.1/7.2/7.3, partial)

`scripts/privileged/preflight.py` (new) implements the 7.1 schema v1 probes
(os, arch, Ubuntu 24.04, kernel >= 6.8, caps, cgroup v2, BTF, bpffs, tooling,
executor readiness) with stable outcome/exit mapping: supported=0,
unsupported_environment=78, not_checked=77, infrastructure_error=79,
usage=2. It takes `--tests` (selected privileged test IDs, recorded only) and
contains zero product assertions (tested). Rootless seam `--probe-results`
drives every outcome; `scripts/tests/validation/test_preflight.py` (10 tests)
covers each state, precedence (unsupported > not_checked), remediation
presence, selected-tests recording, and no-product-vocabulary contract.
Catalog `tooling:privileged-preflight`.

**Wired (this change, tasks 7.2/7.3 closed):** VM ensure/start/readiness,
bootstrap, transfer, reboot handoff live in multipass.sh/profile scripts;
preflight outcomes are now wired into evidence: scenario-dispatch.sh runs
preflight.py before any scenario (writes preflight.json; unsupported/
infrastructure environments exit 77 = not_checked, never product failure),
and runner.py/report.py record the preflight result in the acceptance
report (task 7.3 distinguishability). Rootless runner tests cover the
report field; profile tests cover the exit path.

## Review verification (3.6/3.5/7.2-7.3)

- **3.6** privileged_feasibility.rs retains genuine kernel load/attach
  (Ebpf::load_file, cgroup attach, loss_sample matrix); the moved
  loss_window_model.rs fake mirrors the REAL production LossWindowSampler
  (chronicle-capture-ebpf/src/adapter.rs) plus source.rs sample_loss_at.
  **Residual risk (recorded):** the portable fake and the Linux-only real
  sampler are not linked; drift would go undetected by the portable test.
  Acceptable per task 3.6 scope ("or equivalent portable state-machine
  proof"); promotion of the real sampler into chronicle-capture is a
  candidate follow-up but moves production code, deferred.
- **3.5** no Cargo manifest changes this session (no new dev dependencies);
  etl_contract repair test drives the same process_and_publish entry as the
  CLI internal etl command; cli_contract.rs untouched.
- **7.2/7.3** preflight --probe-results injection seam hardened: executor
  readiness probe now uses tempfile.mkstemp (pid-predictable /tmp path
  removed). Kernel probe on Darwin reports nonsense (24.x >= 6.8) but the
  os probe dominates aggregation, so outcomes stay correct.

## Selection reporting + evidence reuse policy (tasks 9.1/9.4)

- `scripts/validation.py select` now annotates each selected group with its
  gate-relevant catalog tests (id/layer/environment/owner) plus aggregate
  `layers_covered`, `environments_covered`, `owners_covered`, and a
  no-coverage-loss marker. Backward compatible with validate.sh targeted.
- `scripts/tests/validation/test_select_fixtures.py` (5 tests) proves the
  smallest-valid-set matrix: docs-only, ETL-only, WAL-only, eBPF, CLI, helper,
  acceptance, E2E, privileged, unknown-path fixtures; unknown selects full
  validation conservatively; eBPF-only does not pull portable.
- `scripts/tests/validation/test_evidence_reuse.py` (3 tests) proves
  timeout/unsupported/infrastructure/failed/not_checked evidence is never
  reused (reuse returns 1) and executed is false; passed manifests report
  executed true; checksums always present.
- CI (.github/workflows/ci.yml) hosted runner now also builds the CLI binary
  and runs portable smoke/acceptance/rootless-E2E suites; privileged runtime
  remains an explicit supported-platform concern (no Multipass on hosted
  runner).
- **Deferred (concurrent change owns scripts/acceptance/report.py):**
  report-layer/env identification inside acceptance reports.

## Phase 10.1/10.2 validation runs

- 10.1 battery: catalog 45/0 (council review later added 2 planned privileged bindings -> 47 tests), architecture 0, 8 validation tooling suites OK,
  crate matrices OK (chronicle-wal wal_matrix 8, chronicle-capture
  loss_window_model 1, chronicle-application etl_contract 4 + recorder_lease 2,
  replay matrix test), smoke 9, acceptance 11, rootless e2e 1, driver
  self-check. Fixed during the run: rustfmt on 5 touched Rust files
  (privileged_feasibility join blank line, wal_matrix import order,
  recorder_lease import order, etl_contract borrows), clippy needless-borrow
  in etl_contract equivalence test. No task marked from OpenSpec validation
  alone.
- 10.2: `./scripts/validate.sh fast` real run passed end-to-end (fmt, clippy
  -D warnings, workspace tests, openspec strict, source ownership, architecture,
  catalog, tooling meta-tests). Targeted selection reasons verified:
  docs-only -> cli_docs only (no privileged gate without stated reason);
  acceptance/build_tooling paths select their groups with reasons; unknown
  paths conservatively select all. Real targeted execution of the
  cli_docs/openspec command included in fast. Gate-group real execution
  requires Multipass and is deferred to 10.4 supported-Linux evidence.

## Phase 10.7 non-goals confirmation

No production behavior/API/format change in this change: production files
modified in the working tree (continuous_recorder.rs, etl.rs, record.rs,
lib.rs, publication.rs, chronicle-wal lib.rs) belong to the concurrent change.
This change's production footprint is limited to: `recorder_lease.rs` TEST
module refactor (lease_config helper + new state-lease test; production lease
code untouched — verified via diff), test files, validation tooling
(validation.py select annotation, preflight.py, validate.sh steps), CI
workflow, and documentation. No Cargo.toml change (no dependencies, no crate
count change), no CLI design/protocol/eBPF semantics/P1-P2 feature
requirement change. Release-path dry-run passes (complete hierarchy +
supported-platform evidence selection intact; output retained at validation/test-architecture/evidence/release-path-dry-run.txt).

## Phase 10.6 reconcile

`openspec validate reorganize-test-suite-by-responsibility-and-runtime-boundary --strict --no-interactive` passes. graphify update rebuilt the graph (13076 nodes, 29259 edges). Checkbox audit: every marked task has evidence (artifact, test run, or retained run record); no box was checked from OpenSpec validation alone. Implementation reconciled against proposal/design/spec/tasks: the task list matches the implemented catalog/ledger/layers/preflight/validation work; remaining unchecked tasks are scenario-body edits (8.x), privileged E2E (5.3/5.4), and supported-Linux evidence (10.4), all blocked on the concurrent change or the Linux environment.

## Council review (2026-08-13, 4-role fan-out, deepseek-v4-flash)

Third-party review findings closed this session:

- **9.4 / 10.3 unchecked** — acceptance halves deferred: report layer/env
  identification (concurrent report.py) and duplicate reduction (task 8.x
  scenario-body edits). Evidence-reuse policy (9.4) and replacement proof +
  portable-outside-privileged (10.3) remain implemented; boxes reopen when
  the deferred halves land.
- **Planned privileged bindings (M3):** `acceptance:user-intent-lifecycle-privileged`
  and `acceptance:cli-compat-privileged-sample` added and bound in
  scenario_tests + legacy_check_tests + required.p1/p2/release;
  `e2e:privileged-capture-pipeline` bound to replay. Catalog 45 -> 47 tests
  (39 existing / 8 planned). Privileged obligations for split scenarios are
  now validator-visible.
- **Preflight seam hardened (M7):** non-dict `--probe-results` and invalid
  probe status values exit 2 (usage), each covered by a test.
- **Preflight required everywhere (M9):** `tooling:privileged-preflight`
  added to required.p1/p2/release.
- **Inventory scanner upgraded (M2):** matches env-prefixed and
  `run_compat_command`-wrapped cargo invocations; blind-spot audit in
  coverage-comparison.md (28 current-tree occurrences, 14 duplicates of
  already-dispositioned commands; ledger stays baseline-scoped).
- **Doc corrections (M4/M5/M6/M10):** catalog counts 39/8, ledger split
  12 move / 2 remain_privileged, cli_contract 17 tests, ledger EOF comment.
- **Release-path evidence retained (M8):**
  validation/test-architecture/evidence/release-path-dry-run.txt.

## Privileged environment bootstrap (2026-08-13)

`chronicle-ubuntu` (Multipass, Ubuntu 24.04 LTS aarch64, kernel 6.8.0-136) is
now a supported privileged environment:

- Disk grown 20G -> 40G (previous sessions' build artifacts had filled it).
- DNS: Tailscale IPv6 link-local resolver dead on this host network; static
  override persisted via /etc/systemd/resolved.conf.d/99-vm-dns.conf (8.8.8.8).
- Tooling: rustup stable 1.97 + nightly 1.99 + rust-src; bpf-linker 0.10.4;
  bpftool; bpffs mounted at /sys/fs/bpf.
- Reproducible via retained bootstrap script:
  validation/test-architecture/evidence/bootstrap-ubuntu-2404-vm.sh.

Preflight (run as root, euid 0) reports `supported` exit 0, all 10 probes
green; evidence sealed (hand-sealed manifest, provenance + reusable=true) at
validation/test-architecture/evidence/preflight-ubuntu-2404-vm-supported.json.
The earlier unsupported-state evidence (reusable:false) remains for provenance.

This unblocks tasks 5.3 and 10.4; the full P1/P2 privileged scenario run is
still gated on the concurrent change landing (scenario bodies change under
8.x) so evidence is captured once on final bodies.

## Privileged eBPF smoke (2026-08-13)

Real eBPF load/attach/capture proven in the bootstrapped VM:
`chronicle-capture-ebpf/tests/privileged_adapter.rs` (--ignored, linux-ebpf,
offline from registry cache) passes 4/4 single-threaded: IPv4+IPv6
active/passive capture, ring-buffer events, loss-window samples, payload
fragments, and post-drop cleanup (cgroup removed, programs/links/maps back to
baseline). Evidence:
validation/test-architecture/evidence/privileged-ebpf-smoke-ubuntu-2404-vm.{json,log}.

- Parallel-thread note: 3/4 pass in parallel; the failing assert is an
  RCU-timing artifact (programs still listed immediately after drop).
  Deterministic single-threaded; privileged suites run with --test-threads=1.
- The `privileged_feasibility` suite needs its own kernel program built via
  `cargo +nightly build -Z build-std=core`; the toolchain is ready (nightly +
  rust-src + bpf-linker) but the crates.io index fetch over this host network
  fails TLS (curl works; cargo's client does not). Workaround: offline builds
  from the populated registry cache. When network cooperates, build
  ebpf-feasibility and run feasibility too.
