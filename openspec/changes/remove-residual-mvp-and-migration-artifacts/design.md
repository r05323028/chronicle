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

## Decision 2: Fixture support is deterministic test infrastructure

`FixtureCaptureSource` (chronicle-capture), `record_fixture_file`
(chronicle-application), `fixtures/http/*`, and `internal record-fixture` feed
unit tests, rootless smoke/e2e/acceptance suites, and privileged acceptance
scenarios. They are the deterministic stand-in for eBPF capture on rootless hosts
and validate the real current boundary (fixture -> capture events -> WAL v1 -> ETL ->
canonical v1 -> inspect/replay). Only the legacy product CLI form is removed.

## Decision 3: Planned protocol registrations stay

`chronicle-protocol-builtins` keeps the honest `PLANNED`/research registrations
(postgres, mysql_family, mysql, mariadb, oracle, mongodb, kafka, nats) and `fake`.
This is a deliberate extension point, not roadmap-as-code pretending to work:

- ADR 0001: "honest partial capability status" is the reason for the registry shape.
- `docs/architecture/crate-boundaries.md` "Must not change: registration honesty".
- `docs/engineering-taste.md` principle 7: honest planned registrations are an
  enforced-by-design invariant.
- Doctor probes registry status (`crates/chronicle-application/src/doctor.rs`).
- Future protocol plans are also documented in `docs/protocol-plugin-model.md`
  ("Planned MVP" section), which stays authoritative.

No code change; the change records the decision and keeps the doc link fresh.

## Decision 4: Validation and release infrastructure is intentionally retained

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
- The `privileged_feasibility` test in `crates/chronicle-capture-ebpf` and the
  `ebpf-feasibility/` harness: live privileged kernel acceptance, gate-selected;
  only the `docs/feasibility/` documentation about them is restructured.
- `legacy_live_capture_checks`/`legacy_recorder_checks` fields and
  `compatibility_version` in `scripts/acceptance/scenarios.toml` are evidence
  schema naming, not compatibility code; retained.

## Decision 5: Test disposition

| Test/suite | Disposition |
| --- | --- |
| `tests/support/process.py` `inspect_session`/`replay` helpers | rewrite to `--data-dir` current forms |
| `tests/acceptance/test_inspect.py`, `test_replay.py` | rewrite via support helpers (current interface) |
| `tests/smoke/test_smoke.py` `test_missing_fixture_returns_sane_error` | rewrite to `internal record-fixture` |
| `tests/smoke/test_documented_commands.py` | retained; help assertions updated (legacy flags absent) |
| `scripts/tests/test-user-intent-cli-rollback.sh` | keep; current-binary side uses current forms (validates stable WAL v1/canonical v1 cross-version reads) |
| `scripts/acceptance/lib/scenarios/shared/cli-compatibility.sh` | remove (its `internal ...` coverage is duplicated by user-intent-lifecycle/wal-recovery scenarios) |
| `scripts/tests/validation/test_legacy_names.py` | keep + extend |
| rootless suites in `tests/smoke | e2e | acceptance/` | retained (Python is not a criterion) |

## Decision 6: Documentation authority

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

Overlap resolutions in this change: `docs/feasibility/README.md` validation
instructions (duplicate of CONTRIBUTING.md/operations.md) are deleted;
`docs/release-notes.md` deprecation schedule is removed and its durable rollback
content moves to `docs/operations.md`; the orphaned ADR is folded into
`docs/protocol-plugin-model.md`. `docs/architecture.md` (system architecture) and
`docs/architecture/` (crate boundaries) keep their clear split.

## Decision 7: Lasting feasibility knowledge moves into current docs

| Lasting knowledge (from `docs/feasibility/README.md`) | Destination |
| --- | --- |
| hook capabilities (connect4/6, sockops, cgroup-skb), socket-cookie correlation, PID/TGID + cgroup-id caveats, GSO/nonlinear visibility, 8 MiB ring loss sampling, plaintext-only/TLS opaque | `docs/architecture.md` "Capture semantics" (merge into existing text) |
| verified matrix (Ubuntu 24.04, Linux 6.8, aarch64, cgroup v2, BTF) + "other targets not verified" | `docs/operations.md` "Safety and scope" |
| pointer to the retained `privileged_feasibility` acceptance test | `validation/test-architecture/README.md` |

Deleted as historical evidence (Git history and archived OpenSpec changes retain
it): `docs/feasibility/gate-a-ubuntu-24.04-kernel-6.8-aarch64.json`, the
"Verified 2026-07-29" run log details, "Historical verification measurements",
the stale `(cd ebpf-feasibility && ...)` commands, and the duplicate Multipass
validation instructions.

## Decision 8: Reintroduction guard

`scripts/validation.py legacy-names` (fast-gate wired) already rejects removed
migration-era identifiers. Extend its list with the removed CLI identifiers and add
a scenario-id check for `cli-compatibility` in `scripts/acceptance/scenarios.toml`.
`openspec/specs/mvp-schema-versioning/spec.md` gains the CLI-surface clause so a
future change reintroducing hidden compatibility forms fails strict spec
validation without an explicit OpenSpec change. `AGENTS.md` records the rule.

## Open design decisions (recorded, not blocking)

| Question | Recommendation | Status |
| --- | --- | --- |
| Bare-UUID recording references accepted by `RecordingId::parse_cli` ("for compatibility only") | keep as ordinary accepted input (leniency, no runtime layer) | decided: keep; spec reworded |
| Session association id-equality fallback (`crates/chronicle-application/src/recording_catalog.rs` `legacy` comment; `replay_inspect.rs` `legacy_session_id_provides_parent_replay_identity` test) | keep: narrow documented read-only fallback for pre-catalog dev recordings; optional follow-up change can remove under mvp-schema-versioning policy | decided: keep for now |
| `docs/adr/` fate | fold pointer into protocol-plugin-model.md, remove ADR (decision already captured in architecture.md, crate-boundaries.md, protocol-plugin-model.md, engineering-taste #7, OpenSpec archive) | decided: fold + remove |

## Migration order

1. Rewrite rootless tests + rollback harness to current forms (keeps tree green).
2. Remove CLI compatibility surface in `main.rs`; prune proven-unused application exports.
3. Remove `cli-compatibility` scenario + registrations + validation catalog entries.
4. Extend legacy-names guard + meta-test.
5. Update specs (user-intent-cli, recording-identity, mvp-schema-versioning) in place.
6. Update docs + website (en/zh-tw/ja).
7. Update AGENTS.md.
8. Run `./scripts/validate.sh fast` and strict OpenSpec validation; archive
   `remove-pre-release-migration-baggage`; on Linux, run a bounded recorder gate
   because acceptance scenario configuration changed.
