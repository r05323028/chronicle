# Design

## Decision 1: Remove the hidden 0.1.x CLI compatibility surface before 0.1.0

No release ever shipped these entrypoints; they exist only because an earlier
design planned to carry pre-release syntax through 0.1.x and remove it at 0.2.
Carrying them means shipping a deprecation schedule for unreleased interfaces,
keeping `LegacyInvocation`/deprecation machinery alive, and keeping an acceptance
scenario whose only purpose is proving legacy forms. The previous change's own
proposal says "Chronicle has not released 0.1.0 and has no external users or
production compatibility obligations" - the same argument applies here.

### Removed (all in `crates/chronicle-cli/src/main.rs`)

| Surface | Current replacement (kept) |
| --- | --- |
| top-level hidden `recorder` | `internal recorder` (systemd unit, acceptance) |
| top-level hidden `recorder-status` | `internal recorder-status` (readiness polling) |
| top-level hidden `etl` | `internal etl` (recovery scenarios) |
| `record --source fixture --input --root` | `internal record-fixture --input --root` |
| `replay SESSION --root` / `inspect SESSION --root` | `--data-dir` resolution |
| `LegacyInvocation`, `DeprecationJson`, warning emission, raw-arg scanning, legacy error hints | none |
| dead hidden record options (`--wal-dir`, `--allow-shared-cgroup`, `--segment-bytes`, `--duration-seconds`, `--max-wal-bytes`) | none |

### Kept because they are current surface, not compatibility

- `internal recorder|recorder-status|etl|record-fixture|bootstrap`: referenced by
  `docs/systemd/chronicle-recorder.service`, `docs/continuous-recorder.md`,
  `docs/continuous-recorder-runbook.md`, `scripts/acceptance/lib/scenarios/**`,
  and `tests/support/process.py`.
- `--timing` hidden replay flag: feeds the public replay path.
- `doctor --wal-dir/--output/--state-root`: functional diagnostic probes
  (`path_doctor_probe`, `recorder_metadata_doctor_probe`, ...); only the stale
  "0.1.x compatibility/advanced probes" comment is removed.
- `wal_size_limit` shutdown-reason readability (documented in
  `docs/operations.md`): a read-only legacy value with no dispatch machinery.

## Decision 2: No standalone feasibility implementation

Feasibility experiments are complete; their findings are production behavior now.
The production eBPF adapter (`ebpf/` probe, embedded object, `chronicle-capture-ebpf`,
privileged acceptance) is the only validation authority for kernel behavior.
`ebpf-feasibility/` is a Gate A harness: a second probe crate
(`chronicle-ebpf-feasibility`, aya-ebpf 0.2.1) consumed by exactly one test
(`crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs`, which requires
`cd ebpf-feasibility && cargo build --release` first). Two probes and two object
pipelines create architectural ambiguity: kernel results depend on which probe was
loaded, and the harness object pipeline is not exercised by production builds,
CI freshness checks (`.github/workflows/ci.yml` ebpf-compile), or the embedded
fallback object (`crates/chronicle-capture-ebpf/objects/chronicle-ebpf-capture-bpfel.o`).

- Future kernel validation must live in production validation paths: privileged
  tests in `crates/chronicle-capture-ebpf/tests/` loading the production object,
  and privileged acceptance scenarios in `scripts/acceptance/`.
- Capabilities previously proven only by the harness are ported before removal
  (see Decision 7 audit table). Feasibility-probe-only claims (for example any
  behavior that differs between the harness probe and the production probe) are
  dropped or re-proven against the production probe - they are not inherited.

### eBPF coverage audit (harness -> production destination)

| Capability proven by harness | Production destination |
| --- | --- |
| kernel hook availability (connect4/6, sockops, cgroup-skb attach/detach) | privileged production-object test |
| connect4/connect6 behavior (tuple, active/passive role) | `privileged_adapter.rs` (extends existing establishment proofs) |
| sockops behavior (state semantics, unknown close/reset) | privileged production-object test |
| cgroup-skb behavior (direction, TCP sequence, plaintext payload) | privileged production-object test |
| socket-cookie correlation (connect/lifecycle/request/response) | `privileged_adapter.rs` + privileged production-object test |
| PID/TGID identity (host-visible connect TGID; no sockops pid helper) | privileged production-object test; caveats -> `docs/architecture.md` |
| cgroup identity (cgroup-skb id is not socket-owner identity) | privileged production-object test; caveats -> `docs/architecture.md` |
| GSO/nonlinear packet visibility | privileged production-object test with production-observed behavior |
| truncation behavior | privileged production-object test + rootless source tests |
| ring buffer behavior (8 MiB ring, per-CPU complete samples) | privileged production-object test |
| loss accounting (100 ms-or-later, delayed, mandatory final sample, forced loss) | privileged production-object test |
| decoder correctness | already rootless (`crates/chronicle-capture-ebpf/src`, chronicle-capture tests); verify, keep |

Removal follows only after the ported coverage passes on a privileged Linux host.

## Decision 3: No implicit lineage recovery

Chronicle requires explicit evidence lineage: `source_provenance.recording_id` /
`epoch_id` / `epoch_ordinal` on canonical sessions, continuation evidence across
epoch boundaries. Identifier equality is not evidence. Two code paths still infer
lineage from identifier coincidence for pre-catalog development recordings:

- `crates/chronicle-application/src/recording_catalog.rs` `recording_for_summary`
  (~line 180): `summary.recording_id.unwrap_or(RecordingId(summary.session_id.0))`
  - feeds `summary_by_recording`, `reconcile_catalog`, `list_recordings`,
    `resolve_latest`.
- `crates/chronicle-application/src/replay_inspect.rs` `build_parent_replay_plan`
  (~lines 570, 584): `recording_id.is_none() && session_id.0 == parent_id.0`
  match and `epoch_id.unwrap_or(EpochId(parent_id.0))` synthesis.

Both are removed. Sessions without explicit provenance are unresolved: they are
not associated with a recording in catalog/list/latest, and parent replay
selection fails with a typed error. Historical artifacts are handled through Git
history and archived OpenSpec changes, never runtime compatibility. The
`legacy_session_id_provides_parent_replay_identity` test is removed and replaced
with missing-provenance failure + explicit-provenance association tests.

## Decision 4: Fixture support is deterministic test infrastructure

`FixtureCaptureSource` (chronicle-capture), `record_fixture_file`
(chronicle-application), `fixtures/http/*`, and `internal record-fixture` feed
unit tests, rootless smoke/e2e/acceptance suites, and privileged acceptance
scenarios. They are the deterministic stand-in for eBPF capture on rootless hosts
and validate the real current boundary (fixture -> capture events -> WAL v1 -> ETL ->
canonical v1 -> inspect/replay). Only the legacy product CLI form is removed.

## Decision 5: Planned protocol registrations stay

`chronicle-protocol-builtins` keeps the honest `PLANNED`/research registrations
(postgres, mysql_family, mysql, mariadb, oracle, mongodb, kafka, nats) and `fake`.
This is a deliberate extension point, not roadmap-as-code pretending to work:

- ADR 0001: "honest partial capability status" is the reason for the registry shape.
- `docs/architecture/crate-boundaries.md` "Must not change: registration honesty".
- `docs/engineering-taste.md` principle 7: honest planned registrations are an
  enforced-by-design invariant.
- Doctor probes registry status (`crates/chronicle-application/src/doctor.rs`).
- Future protocol plans are also documented in `docs/protocol-plugin-model.md`.

## Decision 6: Validation and release infrastructure is intentionally retained

All of the following serve release qualification, privileged acceptance,
architecture enforcement, deterministic validation, or CI/local parity and are
retained unchanged:

- `scripts/validate.sh`, `scripts/validation.py`, `scripts/acceptance.sh`,
  `scripts/acceptance/` (runner/report/scenarios/recorder-readiness),
  `scripts/run-with-timeout.sh`, `scripts/pre-push-validation.sh`,
  `scripts/install-pre-push-hook.sh`, `scripts/privileged/preflight.py`,
  `scripts/release/`, `scripts/tests/**` tooling tests, `scripts/lib/**`.
- `validation/groups.toml`, `validation/architecture.toml`,
  `validation/test-architecture/**` (catalog/path classification/decisions).
- The production eBPF pipeline: `ebpf/` probe, `crates/chronicle-capture-ebpf/`
  (including `build.rs`, the tracked fallback object
  `crates/chronicle-capture-ebpf/objects/chronicle-ebpf-capture-bpfel.o`, and the
  `--ignored` privileged suites), `privileged_adapter.rs`, and the privileged
  acceptance scenarios that run them. The CI `ebpf-compile` freshness job stays.
- `legacy_live_capture_checks`/`legacy_recorder_checks` fields and
  `compatibility_version` in `scripts/acceptance/scenarios.toml` are evidence
  schema naming, not compatibility code; retained.

Not retained: `ebpf-feasibility/`, `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs`
(the harness loader), and every validation/group/check reference to them - they are
replaced by production-object kernel coverage (Decision 2/7).

## Decision 7: Test disposition

| Test/suite | Disposition |
| --- | --- |
| `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` | port kernel-behavior coverage to production-object privileged test (extend `privileged_adapter.rs` or new suite), then delete |
| `crates/chronicle-capture-ebpf/tests/privileged_adapter.rs` | keep + extend (loads production object via `production_object()`) |
| rootless eBPF tests (`crates/chronicle-capture-ebpf/src`, chronicle-capture) | keep (decoder correctness stays rootless) |
| `tests/support/process.py` `inspect_session`/`replay` helpers | rewrite to `--data-dir` current forms |
| `tests/acceptance/test_inspect.py`, `test_replay.py` | rewrite via support helpers (current interface) |
| `tests/smoke/test_smoke.py` `test_missing_fixture_returns_sane_error` | rewrite to `internal record-fixture` |
| `tests/smoke/test_documented_commands.py` | retained; help assertions updated (legacy flags absent) |
| `scripts/tests/test-user-intent-cli-rollback.sh` | keep; current-binary side uses current forms (validates stable WAL v1/canonical v1 cross-version reads) |
| `crates/chronicle-application/src/replay_inspect.rs` `legacy_session_id_provides_parent_replay_identity` | remove; replace with missing-provenance failure + explicit-provenance tests |
| recording-catalog/replay-plan tests constructing provenance-less sessions and expecting id-equality association | rewrite to explicit provenance or missing-provenance failure |
| `scripts/acceptance/lib/scenarios/shared/cli-compatibility.sh` | remove (its `internal ...` coverage is duplicated by user-intent-lifecycle/wal-recovery scenarios) |
| `scripts/tests/validation/test_legacy_names.py` | keep + extend |
| rootless suites in `tests/smoke | e2e | acceptance/` | retained (Python is not a criterion) |

## Decision 8: Documentation authority

| Concern | Authoritative document |
| --- | --- |
| product intent | `docs/PRODUCT.md`, `website/` |
| current system architecture | `docs/architecture.md` |
| crate boundaries | `docs/architecture/crate-boundaries.md` + `validation/architecture.toml` |
| operational behavior | `docs/operations.md`, `docs/continuous-recorder-runbook.md`, `docs/systemd/` |
| continuous recorder internals | `docs/continuous-recorder.md` |
| validation/release procedures | `CONTRIBUTING.md`, `validation/test-architecture/README.md`, `scripts/validate.sh`, `scripts/acceptance.sh` |
| replay safety | `docs/replay-safety.md` |
| protocol extension model | `docs/protocol-plugin-model.md` |
| WAL format / canonical model | `docs/wal-format.md` / `docs/canonical-model.md` |
| engineering principles | `docs/engineering-taste.md` |
| historical design decisions | `openspec/changes/archive/` + Git history |
| website design | `docs/DESIGN.md`, `docs/branding/` |

Overlap resolutions: `docs/feasibility/` is deleted as an active documentation
area; `docs/release-notes.md` deprecation schedule is removed and its durable
rollback content moves to `docs/operations.md`; the orphaned ADR folds into
`docs/protocol-plugin-model.md`. `docs/architecture.md` (system architecture)
and `docs/architecture/` (crate boundaries) keep their clear split.

## Decision 9: Lasting feasibility knowledge moves into current docs

| Lasting knowledge (from `docs/feasibility/README.md`) | Destination |
| --- | --- |
| hook capabilities (connect4/6, sockops, cgroup-skb), socket-cookie correlation, PID/TGID + cgroup-id caveats, GSO/nonlinear visibility, 8 MiB ring loss sampling, plaintext-only/TLS opaque | `docs/architecture.md` "Capture semantics" (merge into existing text) |
| verified matrix (Ubuntu 24.04, Linux 6.8, aarch64, cgroup v2, BTF) + "other targets not verified" | `docs/operations.md` "Safety and scope" |
| pointer to the privileged production-object kernel test | `validation/test-architecture/README.md` |

Deleted as historical evidence (Git history and archived OpenSpec changes retain
it): `docs/feasibility/` entirely (Gate A JSON, run log details, historical
measurements, stale `ebpf-feasibility` build commands, duplicate Multipass
validation instructions).

## Decision 10: Reintroduction guard

`scripts/validation.py legacy-names` (fast-gate wired) rejects removed
migration-era identifiers. Extend it with:

- removed CLI identifiers (`LegacyInvocation`, `DeprecationJson`,
  `record_fixture_legacy`, `run_replay_legacy`, raw-arg scanning helpers, ...);
- filesystem-presence checks: `ebpf-feasibility/`, `docs/feasibility/`, and
  `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` must not return;
- a scenario-id check rejecting `cli-compatibility` in
  `scripts/acceptance/scenarios.toml`.

`openspec/specs/mvp-schema-versioning/spec.md` gains the CLI-surface and
lineage clauses so reintroduction fails strict spec validation without an explicit
OpenSpec change. `AGENTS.md` records the durable rules.

## Open design decisions (recorded, not blocking)

| Question | Decision |
| --- | --- |
| Bare-UUID recording references accepted by `RecordingId::parse_cli` ("for compatibility only") | keep as ordinary accepted input (leniency, no runtime layer); spec reworded |
| Orphaned sessions (no `source_provenance.recording_id` after fallback removal) | unresolved: excluded from recording-based catalog/list/latest and parent replay; typed error on resolve; no new compatibility reader |
| `docs/adr/` fate | fold pointer into protocol-plugin-model.md, remove ADR (decision already captured in architecture.md, crate-boundaries.md, protocol-plugin-model.md, engineering-taste #7, OpenSpec archive) |
| Feasibility-probe-only kernel claims (e.g. harness-specific payload/continuation behavior that the production probe does not exhibit) | dropped unless re-proven against the production probe; never inherited as production claims |

## Migration order

1. Port eBPF kernel-behavior coverage to production-object privileged tests; extend `privileged_adapter.rs` or add a new privileged suite; run on a privileged Linux host.
2. Remove `ebpf-feasibility/` and `privileged_feasibility.rs`; update `validation/groups.toml`, test-catalog, decisions, README, report.py, and the acceptance scenarios that invoked the harness test.
3. Remove the session-id equality fallback and the epoch-id synthesis; add missing-provenance failure + explicit-provenance tests.
4. Rewrite rootless tests + rollback harness to current CLI forms (keeps tree green).
5. Remove the CLI compatibility surface in `main.rs`; prune proven-unused application exports.
6. Remove the `cli-compatibility` scenario + registrations + validation catalog entries.
7. Extend the legacy-names guard + meta-test.
8. Update specs (user-intent-cli, recording-identity, replay-safety, mvp-schema-versioning, ebpf-capture-adapter) in place.
9. Update docs + website (en/zh-tw/ja).
10. Update AGENTS.md.
11. Run `./scripts/validate.sh fast` and strict OpenSpec validation; archive
    `remove-pre-release-migration-baggage`; on Linux, run a bounded recorder
    gate because acceptance scenario configuration changed.
