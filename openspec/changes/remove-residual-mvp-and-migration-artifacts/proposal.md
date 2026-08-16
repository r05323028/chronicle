# Remove Residual MVP and Migration Artifacts

## Why

Chronicle has not released 0.1.0 and has no external users or production
compatibility obligations. The previous change (`remove-pre-release-migration-baggage`)
removed migration-era runtime layers (EpochCatalogV1/V2, RolloverTransitionV1/V2,
one-shot recording runtime, `record --source ebpf`) but left three classes of
pre-release baggage:

1. **Hidden 0.1.x CLI compatibility surface** - top-level hidden
   `recorder`/`recorder-status`/`etl`, `record --source fixture`,
   legacy `--root` forms, `LegacyInvocation` and deprecation-warning machinery,
   a `cli-compatibility` acceptance scenario, and specs/docs promising removal
   "at the 0.2 boundary". A deprecation schedule for interfaces no release ever
   shipped is the same class of baggage the previous change removed from the runtime.

2. **Standalone feasibility harness** - `ebpf-feasibility/` is a Gate A
   experiment crate (`chronicle-ebpf-feasibility`, aya-ebpf 0.2.1) whose probe
   object is loaded only by `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs`.
   The kernel behaviors it probes are now the production eBPF adapter's job
   (`ebpf/` probe, embedded object, `privileged_adapter.rs`, privileged
   acceptance scenarios). A parallel experimental harness is architectural
   ambiguity: two probes, two object pipelines, one production truth.

3. **Implicit session identity fallback** - session-to-recording association
   falls back to `recording_id == session_id` identifier equality when
   `source_provenance.recording_id` is absent
   (`crates/chronicle-application/src/recording_catalog.rs` `recording_for_summary`,
   `crates/chronicle-application/src/replay_inspect.rs` `build_parent_replay_plan`).
   Identifier coincidence is not evidence lineage. Chronicle's own model says
   lineage is explicit (`source_provenance`, `epoch_id`, continuation evidence);
   the fallback exists only for pre-catalog development recordings.

End state: the repository contains only current production architecture, current
validation infrastructure, and deterministic testing infrastructure. Historical
implementation stages belong in Git history and archived OpenSpec changes. No
runtime compatibility for unreleased formats; no feasibility-only implementations
when production validation paths exist.

## What Changes

### 1. Remove the unreleased CLI compatibility surface (`crates/chronicle-cli/src/main.rs`)

- Remove hidden top-level `Command::Recorder`, `Command::RecorderStatus`,
  `Command::Etl` variants and dispatch arms.
- Remove `record --source fixture` (`Source` enum, `RecordArgs.source/input/root`,
  `record_fixture_legacy`) and the dead hidden eBPF-era record options
  (`--wal-dir`, `--allow-shared-cgroup`, `--segment-bytes`,
  `--duration-seconds`, `--max-wal-bytes`).
- Remove legacy `replay --root` (`run_replay_legacy`) and `inspect --root`.
- Remove `LegacyInvocation`, `DeprecationJson`, `write_deprecation_warning`,
  legacy error hinting, raw-arg legacy scanning, and `format_from_raw_args`.
- Keep the hidden `internal` namespace
  (`internal recorder|recorder-status|etl|record-fixture|bootstrap`) as the
  current operational/test surface (systemd unit, runbooks, acceptance scenarios,
  test support all use it today).

### 2. Retire `ebpf-feasibility/` (migration phase, not blind deletion)

Migration order: audit every capability the harness proves, port missing coverage
into production validation paths, then remove the harness and its references.

**Coverage audit** (feasibility harness -> production destination):

| Capability proven by harness | Production destination |
| --- | --- |
| kernel hook availability (connect4/6, sockops, cgroup-skb attach/detach) | privileged test against the production probe (`ebpf/` object) |
| connect4/connect6 behavior (tuple, active/passive role) | `privileged_adapter.rs` (already proves establishment); extend |
| sockops behavior (state semantics, unknown close/reset) | privileged production-object test |
| cgroup-skb behavior (direction, TCP sequence, plaintext payload) | privileged production-object test |
| socket-cookie correlation | `privileged_adapter.rs` + privileged production-object test |
| PID/TGID identity (host-visible, no sockops pid helper) | privileged production-object test; caveats -> `docs/architecture.md` |
| cgroup identity (cgroup-skb id is not socket owner) | privileged production-object test; caveats -> `docs/architecture.md` |
| GSO/nonlinear packet visibility | privileged production-object test with production-observed behavior |
| truncation behavior | privileged production-object test + rootless source tests |
| ring buffer behavior (8 MiB, per-CPU sampling) | privileged production-object test |
| loss accounting (100 ms/delayed/final sample, forced loss) | privileged production-object test |
| decoder correctness | already rootless (`crates/chronicle-capture-ebpf/src`, chronicle-capture tests); verify coverage, keep |

**Removal after coverage migration:**

- Delete `ebpf-feasibility/` (`Cargo.toml`, `Cargo.lock`, `.cargo/config.toml`,
  `rust-toolchain.toml`, `src/main.rs`).
- Delete `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` (the
  suite that loads the harness object) after porting.
- Remove `ebpf-feasibility/**` from `validation/groups.toml` (paths and
  `build_inputs`).
- Update acceptance scenarios that invoke the feasibility test
  (`scripts/acceptance/lib/scenarios/live-capture/capture-basic.sh`,
  `scripts/acceptance/lib/scenarios/recorder/wal-recovery.sh`) to the
  production-object kernel test.
- Update `validation/test-architecture/test-catalog.toml`,
  `validation/test-architecture/decisions.md`,
  `validation/test-architecture/README.md`, `scripts/acceptance/report.py`.
- Preserve lasting engineering knowledge (verified kernel assumptions, supported
  environment matrix, capture constraints) in `docs/architecture.md`,
  `docs/operations.md`, `validation/test-architecture/README.md`. No experiment
  artifacts remain in active documentation.

The only active eBPF implementation and validation paths after this change:
`ebpf/`, `crates/chronicle-capture-ebpf/`, `scripts/acceptance/`, `tests/`.

### 3. Remove the implicit session identity fallback

- `crates/chronicle-application/src/recording_catalog.rs` `recording_for_summary`
  (lines ~180-183): drop `unwrap_or(RecordingId(summary.session_id.0))`.
  Sessions without explicit `source_provenance.recording_id` are unresolved; they
  are not associated with any recording in `reconcile_catalog`, `list_recordings`,
  or `resolve_latest`.
- `crates/chronicle-application/src/replay_inspect.rs` `build_parent_replay_plan`
  (lines ~570, ~584): drop the `recording_id.is_none() && session_id == parent_id`
  match and the `epoch_id.unwrap_or(EpochId(parent_id))` synthesis. Parent replay
  selection uses explicit `recording_id`/`epoch_id` provenance only; missing
  provenance is invalid/unresolved with a typed error.
- Remove the fallback test
  (`legacy_session_id_provides_parent_replay_identity`, replay_inspect.rs ~1459);
  add explicit-provenance and missing-provenance failure tests.
- Pre-catalog development recordings are not a runtime compatibility obligation;
  Git history and archived OpenSpec changes are their record.

### 4. Tests

- Rewrite rootless suites that exercise legacy CLI forms against the current
  interface: `tests/support/process.py`, `tests/acceptance/test_inspect.py`,
  `tests/acceptance/test_replay.py`, `tests/smoke/test_smoke.py`,
  `scripts/tests/test-user-intent-cli-rollback.sh` (cross-version WAL/canonical
  harness kept; current-binary side rewritten).
- Rewrite/remove identity-fallback tests; add explicit-provenance and
  missing-provenance failure tests.
- Remove the `cli-compatibility` acceptance scenario and its registrations.
- Keep deterministic fixture testing and the layered test architecture.

### 5. Guards

- Extend `scripts/validation.py` `legacy-names`: removed CLI identifiers plus
  filesystem-presence checks that `ebpf-feasibility/`, `docs/feasibility/`,
  and `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` do not
  return. Extend `scripts/tests/validation/test_legacy_names.py`.

### 6. Documentation

- `docs/feasibility/` is removed as an active documentation area: Gate A
  evidence JSON, experiment JSON, old feasibility commands, historical
  measurements, and duplicated validation instructions are deleted. Lasting
  knowledge moves to `docs/architecture.md` (capture semantics, verified kernel
  matrix, unsupported assumptions), `docs/operations.md` (supported environment
  matrix), and `validation/test-architecture/README.md` (privileged test pointer).
- Remove language implying the feasibility phase is ongoing, old compatibility
  paths exist, or MVP migration stages are supported
  (`docs/operations.md` "Legacy 0.1.x syntax migration" appendix,
  `docs/continuous-recorder.md`/`-runbook.md` alias sentences,
  `docs/release-notes.md` deprecation schedule, "One-shot recording pipeline"
  heading and hidden-compat sentence in `docs/architecture.md`).
- `docs/adr/0001` folds into `docs/protocol-plugin-model.md`; `docs/adr/` removed.
- Normalize "MVP"/"Planned MVP" milestone labels to "Planned"/"Future".
- Website (en/zh-tw/ja + 0-1 copies): remove hidden-invocation migration notes
  and release-notes links; run `npm run verify:localization`.

### 7. Specs (edited in place; `skip_specs: true`)

- `openspec/specs/user-intent-cli/spec.md`: legacy forms removed before 0.1.0;
  five public commands plus hidden `internal`; no deprecation machinery.
- `openspec/specs/recording-identity/spec.md`: remove stale "legacy one-epoch
  recording remains resolvable" and "Legacy `--root` behavior remains exact"
  clauses; association requires explicit provenance; identifier equality is not
  lineage; bare-UUID input stays as an ordinary accepted form.
- `openspec/specs/replay-safety/spec.md`: sessions without explicit provenance
  are invalid/unresolved for parent replay; no id-equality fallback.
- `openspec/specs/mvp-schema-versioning/spec.md`: pre-0.1 policy covers the CLI
  surface and runtime lineage; feasibility-only implementations must not be
  reintroduced when production validation paths exist.
- `openspec/specs/ebpf-capture-adapter/spec.md`: privileged kernel validation
  runs only against the production probe; no standalone feasibility harness.
- Archive the completed `remove-pre-release-migration-baggage` change.

### 8. AGENTS.md

- Add short durable rules: no migration adapters for unreleased formats; no
  compatibility aliases before a public contract exists; no feasibility-only
  implementations when production validation paths exist; historical behavior
  belongs in Git history and archived OpenSpec changes; runtime lineage is
  explicit and never inferred from identifiers.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `user-intent-cli`: hidden 0.1.x compatibility entrypoints removed before the
  first release; exactly five public commands plus the hidden `internal`
  namespace.
- `recording-identity`: legacy one-epoch resolution and legacy `--root`
  behavior removed; session-to-recording association requires explicit
  provenance; identifier equality is not lineage.
- `replay-safety`: parent replay selection requires explicit provenance;
  missing provenance is invalid/unresolved.
- `mvp-schema-versioning`: the single-model pre-0.1 policy explicitly covers the
  CLI surface and runtime lineage, not only persisted schemas.
- `ebpf-capture-adapter`: privileged kernel validation runs against the
  production probe only; the standalone feasibility harness is retired.
