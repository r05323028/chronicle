# Remove Residual MVP and Migration Artifacts

## Why

Chronicle has not released 0.1.0 and has no external users or production
compatibility obligations. The previous change (`remove-pre-release-migration-baggage`)
removed the migration-era runtime layers (EpochCatalogV1/V2, RolloverTransitionV1/V2,
one-shot recording runtime, `record --source ebpf`) but intentionally retained the
hidden 0.1.x CLI compatibility surface and its deprecation machinery:

- hidden top-level `recorder`, `recorder-status`, `etl` commands;
- `record --source fixture --input --root` legacy form;
- legacy `replay SESSION --root` and `inspect SESSION --root` forms;
- `LegacyInvocation`, `DeprecationJson`, raw-arg legacy scanning, and deprecation
  warning emission ("removal in 0.2");
- an acceptance scenario (`cli-compatibility`) whose only purpose is proving those
  legacy forms, plus validation catalog references to it;
- specs and documentation promising removal "at the 0.2 boundary".

A deprecation schedule for interfaces no release ever shipped is the same class of
pre-release baggage the previous change removed from the runtime. The repository
should describe and exercise only the current pre-0.1 architecture; Git history and
archived OpenSpec changes are the record of historical implementation stages.

This is not indiscriminate cleanup. Release/validation infrastructure, deterministic
fixture-based testing, the hidden `internal` operational namespace, and honest
planned protocol registrations are intentionally retained (see design decisions).

## What Changes

### Remove the unreleased CLI compatibility surface (`crates/chronicle-cli/src/main.rs`)

- Remove hidden top-level `Command::Recorder`, `Command::RecorderStatus`,
  `Command::Etl` variants and their dispatch arms.
- Remove `record --source fixture` (`Source` enum, `RecordArgs.source/input/root`,
  `record_fixture_legacy`) and the dead hidden eBPF-era record options
  (`--wal-dir`, `--allow-shared-cgroup`, `--segment-bytes`,
  `--duration-seconds`, `--max-wal-bytes`).
- Remove legacy `replay --root` (`run_replay_legacy`) and `inspect --root`.
- Remove `LegacyInvocation`, `DeprecationJson`, `write_deprecation_warning`,
  legacy error hinting, raw-arg legacy scanning, and `format_from_raw_args`.
- Keep the hidden `internal` namespace (`internal recorder|recorder-status|etl|record-fixture|bootstrap`)
  as the current operational and test surface: the systemd unit
  (`docs/systemd/chronicle-recorder.service`), recorder runbooks, and acceptance
  scenarios all invoke `internal ...` forms today.

### Keep fixture support, positioned as test/internal infrastructure

`FixtureCaptureSource` (`chronicle-capture`), `record_fixture_file`
(`chronicle-application`), `fixtures/http/*`, and `internal record-fixture` are
deterministic test infrastructure and stay. The legacy product-facing form
(`record --source fixture`) is the only fixture surface removed:

```text
fixture support
    ↓
internal record-fixture / tests / validation scenarios   (kept)
    ↓
legacy product CLI compatibility surface                (removed)
```

### Tests

- Rewrite rootless suites that exercise legacy forms against the current interface:
  `tests/support/process.py` (`inspect_session`, `replay` helpers),
  `tests/acceptance/test_inspect.py`, `tests/acceptance/test_replay.py`,
  `tests/smoke/test_smoke.py`, `scripts/tests/test-user-intent-cli-rollback.sh`.
- Remove the `cli-compatibility` acceptance scenario
  (`scripts/acceptance/lib/scenarios/shared/cli-compatibility.sh`) and its
  registrations in `scripts/acceptance/scenarios.toml` and
  `validation/test-architecture/*`.
- Keep the cross-version rollback harness
  (`scripts/tests/test-user-intent-cli-rollback.sh`): it validates the
  intentionally stable WAL v1 / canonical session v1 read compatibility, not the
  legacy CLI. Its current-binary side is rewritten to current forms.

### Guards

- Extend the `legacy-names` gate in `scripts/validation.py` with the removed CLI
  identifiers and the `cli-compatibility` scenario id so the removed surface cannot
  return without an explicit OpenSpec change; extend
  `scripts/tests/validation/test_legacy_names.py` accordingly.

### Documentation

- `docs/operations.md`: delete the "Legacy 0.1.x syntax migration" appendix;
  reword the `internal etl` "deployment/compatibility mechanism" phrasing.
- `docs/architecture.md`: drop the "hidden compatibility paths ... through 0.1.x"
  sentence and the "One-shot recording pipeline" heading; migrate lasting
  kernel/eBPF capture constraints from `docs/feasibility/` into "Capture semantics".
- `docs/continuous-recorder.md` and `docs/continuous-recorder-runbook.md`: remove
  the "hidden deprecated aliases ... removed at 0.2" sentences.
- `docs/feasibility/`: extract lasting kernel knowledge into current architecture/
  operations docs; remove the Gate A machine evidence JSON, historical measurements,
  stale feasibility commands, and duplicated validation instructions.
- `docs/release-notes.md`: remove the pre-release deprecation schedule; durable
  artifact/rollback compatibility content moves to `docs/operations.md`.
- `docs/adr/0001-compile-time-protocol-registry.md`: fold a one-line decision
  pointer into `docs/protocol-plugin-model.md` and remove the ADR file; the
  decision is already captured in current architecture docs and OpenSpec archives.
- Normalize residual "MVP"/"Planned MVP" milestone terminology in active docs to
  "Planned"/"Future" (content preserved).
- Website (en/zh-tw/ja + 0-1 copies): remove "hidden 0.1.x invocation" migration
  notes and release-notes links; run `npm run verify:localization`.

### Specs (edited in place; no delta copies, same convention as the previous change)

- `openspec/specs/user-intent-cli/spec.md`: replace the "Legacy forms SHALL remain
  hidden deprecated compatibility entrypoints through 0.1.x ... removal targeted for
  0.2" requirement and its legacy scenarios with removal-before-0.1; keep the five
  public commands plus hidden `internal`.
- `openspec/specs/recording-identity/spec.md`: remove the stale "legacy one-epoch
  recording remains resolvable" clause and the "Legacy `--root` behavior remains
  exact" clause; keep bare-UUID input acceptance as an ordinary accepted form and
  keep the WAL v1 identity adapter.
- `openspec/specs/mvp-schema-versioning/spec.md`: extend the pre-0.1 policy to the
  CLI surface - unreleased compatibility forms are removed before 0.1.0, and the
  legacy-names guard prevents reintroduction.
- Archive the completed `remove-pre-release-migration-baggage` change.

### AGENTS.md

- Record durable rules: no hidden pre-release CLI compatibility surface; historical
  stages live in Git history and archived OpenSpec changes; a documentation
  authority list; the extended legacy-names guard.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `user-intent-cli`: hidden 0.1.x compatibility entrypoints are removed before
  the first release; exactly five public commands plus the hidden `internal`
  namespace exist.
- `recording-identity`: legacy one-epoch resolution and legacy `--root` behavior
  are removed; `--data-dir` precedence is the only root resolution path.
- `mvp-schema-versioning`: the single-model pre-0.1 policy explicitly covers the
  CLI compatibility surface, not only persisted schemas.
