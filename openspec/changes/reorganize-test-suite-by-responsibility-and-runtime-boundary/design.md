## Context

Chronicle currently has substantial automated coverage but no single classification model:

- Repository scan found 49 Rust test-bearing files, including 41 implementation files with colocated test modules and 8 crate integration-test files, with 487 Rust `#[test]`/`#[tokio::test]` attributes. Colocated tests broadly match local-logic ownership.
- Crate integration tests mix component contracts, executable acceptance, rootless vertical slices, and privileged kernel tests. Examples: `crates/chronicle-application/tests/{fake_vertical,fixtures_http_vertical}.rs` are synthetic full-pipeline slices; `crates/chronicle-cli/tests/cli_contract.rs` is a 1,100-line black-box binary suite spanning CLI grammar, user features, replay safety, ETL corruption, doctor, storage errors, and process bootstrap; `crates/chronicle-capture-ebpf/tests/privileged_feasibility.rs` combines real kernel feasibility with a rootless loss-state-machine test.
- Root `tests/` contains only `tests/e2e/http_acceptance_driver.py` and its helper self-test. Driver is workload/replay-target infrastructure for privileged acceptance, not itself Chronicle E2E coverage. No explicit root `tests/smoke/` or `tests/acceptance/` exists.
- `scripts/acceptance/scenarios.toml` organizes 13 scenarios by P1/P2 profile. Scenario implementations live under gate-oriented `scripts/acceptance/lib/scenarios/{shared,extensions/p1,extensions/p2,p2}` rather than responsibility-oriented test locations.
- P1 `capture-basic` is simultaneously environment preflight, eBPF build/load, cgroup policy acceptance, record/WAL/ETL/inspect/replay full E2E, privileged feasibility, WAL fault matrix, and application ingest matrix. Acceptance scenarios invoke at least 20 `cargo test` commands; P2 `resource-cleanup` also runs formatting and workspace checks. These are direct lower-layer and repository-validation duplicates inside privileged gates.
- Several nominally privileged assertions do not require privilege: P1/P2 replay matrices, cgroup decision logic unit tests, WAL fault/ingest matrices, ETL idempotence, corruption of a copied WAL, CLI compatibility, and format/workspace checks. Other assertions genuinely require supported Linux: real eBPF load/attach and socket capture, cgroup placement/filtering, bpftool cleanup, systemd/capability behavior, reboot, loop-device quota pressure, and real process attribution.
- Existing rootless vertical slices already prove much post-capture composition: `fake_capture_wal_etl_storage_replay_verify` and `fixture_wal_http_artifact_replay_and_verification_vertical_slice`. They are useful migration seeds, but their direct lower-layer imports mean they are component integration/E2E tests rather than black-box acceptance.
- Process management exists in many forms: Rust `Command`, listeners, sleeps, and manual signals; Python `Popen`/poll/terminate; shell background processes, wait loops, traps, kill escalation, and cleanup. `scripts/acceptance/lib/wait.sh` now gives bounded condition waits and `recorder-readiness.sh` carries specialized freshness/stale-owner semantics, but ownership is acceptance-script-specific and duplicated patterns remain in crate/root test helpers.
- `scripts/lib/env.sh::validate_environment` checks Linux, capabilities, Ubuntu 24.04, kernel >= 6.8, cgroup v2, BTF, bpffs, and tools, but most unsupported cases call `die` and exit 1. Non-Linux P1 exits 77; other unsupported environments and Multipass/tooling failures can be reported like product failures. Environment preparation, preflight, test assertions, and evidence production remain intertwined across runner, Multipass executor, profile setup, and scenarios.
- `scripts/validate.sh` provides required `fast`, `targeted`, `gate p1`, `gate p2`, and `release` commands. `fast` runs fmt, Clippy, workspace tests, OpenSpec, ownership, and architecture checks. `targeted` selects path groups, but acceptance/build-tooling changes select both full gates, and eBPF changes run a P1 profile called smoke. P1/P2 profiles are release milestones with required evidence, not intrinsic test layers.
- CI runs `validate.sh fast`, then validation/acceptance-tooling self-tests, while privileged evidence remains outside ordinary hosted CI. No distinct smoke, acceptance, or rootless full-pipeline command is visible.
- Existing OpenSpec contracts must remain intact: bounded hierarchical deadlines, condition-based convergence, scenario-level `passed`/`failed`/`not_checked` attribution, content-addressed evidence reuse, supported Ubuntu release proof, and crate dependency enforcement across normal/dev/build edges. OpenSpec validation remains structural evidence only.

This design reorganizes proof ownership without changing product behavior or weakening P1/P2 obligations.

## Goals / Non-Goals

**Goals:**

- Give unit, integration, smoke, acceptance, E2E, and privileged tests one explicit purpose each.
- Require every significant current test/assertion to receive a functional layer and, separately, an environment requirement.
- Put exhaustive correctness at the lowest conclusive layer and keep higher layers focused on contract/composition invariants.
- Make deterministic synthetic-capture E2E the normal proof for post-capture pipeline composition.
- Reserve supported-Linux privileged execution for assertions whose truth depends on real kernel/environment behavior.
- Separate VM/environment preparation from test behavior and report unsupported environments separately from failed product assertions.
- Keep validation as selector/orchestrator; keep P1/P2 as selectors over required coverage and evidence.
- Reuse minimal deterministic process/readiness/lifecycle support without introducing a new framework or violating crate direction.
- Preserve existing coverage until replacement evidence is mapped and passing.

**Non-Goals:**

- Implementing helpers, moving or rewriting tests, fixing flakiness, or changing scripts in this planning task.
- Changing production behavior, public APIs, persisted formats, CLI design, protocols, eBPF behavior, supported environment, P1/P2 feature requirements, or crate architecture.
- Creating empty directory scaffolds for aesthetics.
- Forcing every test into root `tests/`; colocated Rust unit tests remain colocated and crate contracts remain in owning crate `tests/`.
- Adding a third-party test framework or a new workspace crate without demonstrated need and full architecture-policy review.
- Replacing real privileged proof with simulation where existing product requirements demand supported-kernel evidence.

## Decisions

### 1. Classification has two axes

Every significant test receives one functional classification—unit, integration, smoke, acceptance, or E2E—and one environment classification—portable/rootless or privileged. `privileged capture`, `privileged acceptance`, and `privileged E2E` are valid combinations; `P1` and `P2` are selectors, not classifications.

Alternative: keep privileged as a peer layer above E2E. Rejected because privilege says where an assertion can run, not what contract it proves. Alternative: preserve P1/P2 directories as architecture. Rejected because milestone membership changes independently from test responsibility.

### 2. Lowest conclusive layer owns exhaustive correctness

Local branches/state machines/parsers belong in unit tests; public crate and multi-component contracts belong in integration tests; executable liveness belongs in smoke; one documented user feature belongs in acceptance; only complete cross-subsystem invariants belong in E2E. Higher tests may sample a detail needed to prove their own contract but must not repeat lower-layer matrices.

Migration cannot delete coverage first. Each moved/split/deleted assertion needs a ledger entry naming old proof, replacement proof, layer rationale, environment requirement, and verification command. Duplicate removal occurs only after replacement passes.

Alternative: leave duplicates because extra tests are safer. Rejected because expensive duplicates increase failure surface without independent proof and make privileged evidence less reliable.

### 3. Target locations follow ownership, not aesthetics

- Unit: `crates/<crate>/src/**` under `#[cfg(test)]`.
- Crate/component integration: `crates/<owner>/tests/` using public APIs and allowed dev edges.
- Shipped executable smoke: `tests/smoke/`.
- User-feature black-box acceptance: `tests/acceptance/{record,inspect,etl,replay,...}/`.
- Rootless complete composition: `tests/e2e/`.
- Assertions requiring supported Linux/kernel: `tests/privileged/{capture,recovery,compatibility,e2e}/`, retaining functional classification in metadata/naming.
- Environment preparation/execution/evidence: `scripts/privileged/` or equivalently named infrastructure location chosen during implementation after caller migration is mapped.

Existing files may stay when responsibility already matches ownership. No move happens solely to match this tree. `crates/chronicle-capture-ebpf/tests/privileged_adapter.rs`, for example, may remain crate-owned privileged integration if gate discovery can classify it clearly; root relocation is optional. Root `tests/privileged/` is intended for cross-crate/process/system tests, not to centralize every kernel-aware crate contract.

Alternative: centralize all tests under root. Rejected because it weakens crate ownership and encourages forbidden test dependencies.

### 4. Rootless E2E becomes post-capture composition authority

Canonical rootless E2E uses deterministic synthetic capture evidence through WAL, ETL, canonical publication, replay, and verification. Existing application vertical slices are migration seeds. Target tests assert only stage compatibility, identity/provenance continuity, operation survival, replay execution, and verification—not exhaustive WAL corruption, protocol parsing, ETL fault, or replay policy matrices.

One or very few privileged E2E tests add real workload and eBPF capture before the same path. They prove real capture can feed pipeline and cleanup succeeds; lower layers remain authority for detailed post-capture correctness.

Alternative: every complete flow starts with real eBPF. Rejected due production cost, supported-host scarcity, and duplication of deterministic downstream proof.

### 5. Acceptance is black-box; internal CLI forms do not define user acceptance

Acceptance uses supported external surfaces: public CLI/processes, documented APIs/endpoints, filesystem/storage artifacts, and network targets. Direct crate imports are disallowed in acceptance assertions. Internal compatibility commands may remain lower-level CLI contract tests where needed but do not substitute for documented feature acceptance.

Large `cli_contract.rs` is audited and split by responsibility only where doing so clarifies proof: argument/output/exit mapping can remain CLI integration; executable liveness becomes smoke; public record/inspect/ETL/replay behavior becomes acceptance; complete record-to-replay composition becomes E2E. Churn without such reason is prohibited.

### 6. Shared process support stays minimal and outer-layer-owned

Implementation first inventories repeated operations. Introduce only helpers with multiple concrete consumers, likely concepts equivalent to `ChronicleProcess`/`RecorderProcess`, `WorkloadServer`, condition/readiness waits, timeout, evidence capture, and cleanup. Reuse existing `scripts/run-with-timeout.sh`, shell `wait_until`, and specialized recorder-readiness semantics instead of replacing them with a generic framework.

Rust crate tests keep crate-local support modules unless a helper has legitimate cross-crate ownership and allowed dependencies. Root black-box suites may own language-local support under root test infrastructure. No shared helper may import `chronicle-cli` as a library, expose Aya/kernel ABI outside capture-eBPF, make CLI depend on lower crates, or bypass `validation/architecture.toml` through dev dependencies.

Alternative: create a shared test-support crate immediately. Rejected as speculative; current 13-crate contract and dev-edge policy make a new crate expensive. Add only after duplication and dependency ownership prove need.

### 7. Privileged preflight is a typed contract

Privileged execution has three result classes:

1. `supported`: run product assertions;
2. `unsupported_environment`: do not claim product pass/fail; report failed probes and remediation, with stable machine-readable status/exit distinct from assertion failure;
3. `infrastructure_error`: preparation/executor failure, distinct from both unsupported and product failure.

Preflight covers Linux, architecture, Ubuntu version, kernel, effective root/capabilities, cgroup v2, BTF, bpffs, required hooks/tooling, and executor readiness. Product failures occur only after supported preflight. Release eligibility still requires supported compatible environment and complete passed evidence; unsupported/not-checked never satisfies a gate.

Existing doctor probe vocabulary (`supported`, `supported_with_warnings`, `unsupported`, `not_checked`) should be reused where practical, without making production doctor mutate test environment.

Alternative: treat unsupported as ordinary failure. Rejected because it conflates runner placement with regression. Alternative: skip silently. Rejected because gates could appear satisfied.

### 8. Environment preparation and test behavior are separate

Multipass source snapshot, VM lifecycle/readiness, bootstrap, remote command, artifact transfer, and evidence sealing remain orchestration. Scenario assertions move to classified test locations. Executors receive selected test IDs plus preflight contract; they do not embed product correctness. Evidence collection records environment, selected functional layers/scenarios, execution results, unsupported/infrastructure state, timeouts, and provenance.

Current scenario ledger, timeout hierarchy, freshness-aware recorder readiness, cleanup barriers, and content-addressed reuse remain required. Migration may wrap existing scripts before moving bodies, but final gate definitions reference classified tests rather than gate-owned scenario implementations.

### 9. Validation modes compose layers but preserve current obligations

Target composition is resolved during implementation against measured cost and existing gate coverage:

- `fast`: fmt, warnings-denied Clippy, architecture/source/OpenSpec checks, all unit tests, and explicitly cheap portable integration tests. Current workspace test command may remain while classification matures; optimization must not silently drop coverage.
- `targeted`: dependency/path-aware affected unit/integration plus relevant smoke/acceptance/E2E, and privileged tests only when changed behavior plus milestone policy requires their assertions. Selection output explains layer, environment, and gate reason.
- `gate p1` / `gate p2`: required portable lower-layer prerequisites plus selected smoke, acceptance, E2E, and privileged proof for that milestone. Gate files/config select test IDs; tests are not stored as P1/P2 implementations.
- `release`: complete required hierarchy plus compatible supported-platform privileged evidence and current checkout integrity.

P2 remains a P1 superset where current evidence contract requires it. Compatible evidence reuse stays valid. Exact command membership is an implementation task backed by the classification ledger and no-coverage-loss matrix, not guessed in this planning artifact.

### 10. Migration is incremental and evidence-preserving

Migration order:

1. Build complete assertion-level classification ledger and identify gate requirements.
2. Add meta-validation for classification/selection and preflight result handling before moving tests.
3. Separate portable Cargo/repository checks from privileged profiles.
4. Establish rootless smoke/acceptance/E2E coverage using existing fixtures and public surfaces.
5. Extract environment preparation and route classified privileged tests through it.
6. Remove duplicate/gate-oriented implementations only after lower-cost replacement and required privileged sample both pass; post-migration cleanup of obsolete infrastructure (tasks section 11) completes BEFORE fresh final supported-platform proof is collected (task 10.4), so retained evidence reflects the post-cleanup checkout.
7. Update validation groups, CI, docs, and `AGENTS.md`; then run final coverage comparison (10.3), the no-dead-infrastructure invariant (11.10), and collect fresh/reused compatible gate evidence as required.

Rollback keeps old gate path available until new selection demonstrates equivalent required scenario coverage. Persisted product data needs no migration.

## Target Test Directory Architecture

This section is the architectural contract for where Chronicle tests live. It defines responsibility and ownership, not aesthetics. Every directory and representative file below has one owner and one purpose; anything that does not fit a listed location is either misclassified or missing an owner.

Status legend used throughout:

- **[ok]** — already compliant; current layout matches target responsibility.
- **[trans]** — transitional; exists at an interim location and migrates to the target owner as implementation tasks land.
- **[plan]** — planned; does not exist yet and is created only when real tests migrate (never as empty scaffolding).
- **[keep]** — intentionally retained at current location with an explicit reason.

`validation/test-architecture/test-catalog.toml` is the authoritative machine-readable classification, ownership, execution, and selector catalog (test ID, functional layer, environment requirement, owner, command, status, gate membership). Actual file locations conform to this directory responsibility contract; migration destinations are tracked in the migration ledger; the catalog is not required to be a file-manifest database and gains no path field without a demonstrated implementation need.

### 1. Crate-local unit tests

    crates/<crate>/
    └── src/
        └── **/*.rs                 # #[cfg(test)] mod tests — unit

**[ok]**. All 42 colocated Rust unit-test files stay under `crates/<crate>/src/**`; no centralized `tests/unit/` directory exists or will be created.

Responsibility: local function/module correctness — parsing edge cases, serialization rules, state machines, normalization, local retry/checkpoint calculations, validation rules, pure correlation/session logic, WAL primitive encoding/checksum logic.

Rules: deterministic; no subprocess; no complete Chronicle binary; no real daemon lifecycle; no Multipass; no real eBPF requirement.

> Unit tests stay colocated with implementation.

### 2. Crate integration tests

    crates/<crate>/
    └── tests/
        └── *.rs                   # integration — public contract of owning crate

Responsibility: prove the public contract of the owning crate or a deliberately owned component composition, through public APIs and allowed dev edges.

Representative target tree (current files in parentheses):

    crates/chronicle-wal/tests/
      wal_matrix.rs                # [ok] WAL encode/decode/recovery/corruption matrix
      recovery.rs                  # [example] crash-point recovery contracts (split from privileged)
      corruption.rs                # [example] deterministic copied-WAL corruption matrix (task 3.2)

    crates/chronicle-etl/tests/
      checkpoint.rs                # [example] checkpoint transaction/fault matrix (task 3.3)
      publication.rs               # [example] publication/restart/idempotence contracts (task 3.3)
      incremental.rs               # [example] incremental ETL continuation (task 3.3)
      corruption.rs                # [example] ETL corruption fail-closed matrix (task 3.3)

    crates/chronicle-replay/tests/
      planner.rs                   # [example] replay plan construction (task 3.4)
      executor.rs                  # [example] replay execution contracts (task 3.4)
      partial_execution.rs         # [example] partial/aborted replay behavior (task 3.4)
      verifier.rs                  # [example] verification matching/policy (task 3.4)

    crates/chronicle-application/tests/
      fake_vertical.rs             # [keep] rootless synthetic composition contract via public APIs
      fixtures_http_vertical.rs    # [keep] rootless deterministic fixture pipeline
      http_test_server.rs          # [keep] crate-local HTTP support, single consumer
      support/mod.rs               # [keep] crate-local test support, single consumer
      etl_contract.rs              # [ok] ETL command contract through application seam

    crates/chronicle-cli/tests/
      cli_contract.rs              # [trans] split by responsibility (task 3.5): arg/exit/rendering
                                   #         stays CLI integration; liveness→smoke; features→acceptance;
                                   #         composition→E2E

These are target responsibilities; exact filenames follow current Chronicle ownership as implemented and recorded in `migration-ledger.toml`, not this tree verbatim. `[example]` files are illustrative possible decompositions, not required deliverables: **exact file splitting is not required if existing consolidated integration suites already satisfy the responsibility and ownership contract**. No speculative directory or file creation is required; real test ownership drives structure.

Rules: temporary directories allowed; deterministic real WAL files allowed; fixtures allowed; fake/in-memory adapters allowed; local loopback networking allowed for an owned contract when justified; full product orchestration does not normally live here; portable correctness must not be moved into privileged tests just because Linux can run it.

### 3. Privileged crate integration tests

Privilege is an environment dimension, not a functional layer. Some crate integration tests are therefore integration tests whose environment is privileged:

    crates/chronicle-capture-ebpf/tests/
      privileged_adapter.rs        # [keep] layer=integration, env=privileged — real eBPF attach/capture
      privileged_feasibility.rs    # [trans] split (task 3.6): kernel feasibility stays privileged;
                                   #         loss state machine moved rootless to
                                   #         crates/chronicle-capture/tests/loss_window_model.rs [ok]

    crates/chronicle-cli/tests/
      privileged_signal.rs         # [keep] layer=integration, env=privileged — real cgroup/signal behavior

Assertions must genuinely require supported real OS/kernel behavior: real eBPF program load, attach, BTF, ring-buffer behavior, kernel ABI behavior, process/cgroup signal behavior. Portable state machines or data-model assertions are split out (task 3.6 shows the pattern).

### 4. Root black-box support

    tests/
    └── support/                   # shared black-box infrastructure — not a test classification
        ├── __init__.py
        ├── process.py             # [ok] bounded process/readiness/deadline/cleanup support
        ├── http_driver.py         # [plan] shared workload/replay target (migrate from tests/e2e/ per 4.4)
        ├── fixtures.py            # [plan] optional — only shared path/fixture construction with real duplication
        └── assertions.py          # [plan] optional — only small reusable black-box assertions with real duplication

These files are infrastructure, not product-test classifications; nothing in `tests/support/` is itself proof of Chronicle behavior.

- `process.py` — **[ok]**, 3 consumers (smoke/acceptance/e2e). Bounded command execution, subprocess ownership, readiness waiting, deadline handling, deterministic shutdown, temp roots, output parsing, reusable public CLI invocation. Must not contain broad product assertions.
- `http_driver.py` — **[plan]**. Local HTTP server, deterministic workload generation, request logging, mismatch response mode, readiness signaling. The current `tests/e2e/http_acceptance_driver.py` **[trans]** carries this responsibility; task 4.4 (reopened) requires its migration to `tests/support/`, moves its self-test to a support/tooling-appropriate location, and updates every consumer to depend only on `tests/support/`. It is support infrastructure, not E2E proof by itself.
- `fixtures.py` / `assertions.py` — **[plan]**, created only when genuine duplication exists. Must not recreate Chronicle's internal domain model in Python; one-owner fixtures stay local.

### 5. Smoke tests

    tests/smoke/
    ├── test_smoke.py              # [keep] single combined file — acceptable while the suite is small
    └── (conceptual per-surface split: test_cli.py, test_record.py, test_inspect.py,
         test_etl.py, test_replay.py — split only when a surface's smoke contract outgrows one file)

The current single `tests/smoke/test_smoke.py` covers help, usage error, empty-list, missing-fixture error, unsupported-build record failure, and minimal fixture liveness. This tree expresses responsibility, not a required file split.

Responsibilities: `test_cli.py` — `--help`, basic usage errors, binary liveness, minimal argument-handling sanity; `test_record.py` — only minimal record startup/liveness or supported fast-failure behavior, never full capture-to-replay; `test_inspect.py` — minimal successful or safe-failure invocation; `test_etl.py` — minimal supported ETL entry-point invocation (internal ETL liveness belongs to CLI/application integration contract, not pretend user acceptance — see §6); `test_replay.py` — minimal replay/dry-run liveness.

Smoke rules: black-box, fast, bounded, shallow, no exhaustive correctness, no arbitrary time-based synchronization, no full workflow.

### 6. Acceptance tests

    tests/acceptance/
    ├── test_record.py             # [ok] documented record behavior through public surfaces
    ├── test_inspect.py            # [ok] user-visible inspection output and safe errors
    ├── test_etl.py                # [keep] internal-command fail-closed contracts (see note)
    └── test_replay.py             # [ok] dry-run, authorization, successful replay, verification, mismatch exit

Flat per-feature files are acceptable while the suite is small. Subdirectories (`tests/acceptance/{record,inspect,etl,replay}/`) become useful when a feature accumulates several independent suites or shared per-feature fixtures; do not create them speculatively.

Responsibility: prove one documented user-facing Chronicle capability through public surfaces — CLI, process behavior, documented network surfaces, filesystem/storage artifacts users observe.

Rules: no direct imports of internal Chronicle crates; no exhaustive parser/WAL/ETL/checkpoint/replay matrices; no assertion merely because an internal file is easy to inspect.

Classification rule for command surfaces: a documented user-facing feature contract → acceptance; public executable liveness / minimal invocation → smoke; CLI argument / rendering / exit-code contract → integration when proving a component or binary contract; internal command / internal compatibility seam → integration/internal contract owned by the appropriate application/CLI boundary. Merely using a public CLI/API surface does not make a test Acceptance. An internal command MUST NOT be the sole evidence for a documented user-facing capability.

Record acceptance note: successful real record requires eBPF, so portable failure/fixture-support contracts and privileged public-record acceptance are split; an internal fixture command must not become the sole evidence for the public feature.

ETL coverage note — internal command, not user acceptance: the CLI-only fixture path cannot produce WAL recorder metadata (recording identity is written by the real recorder), so the current `tests/acceptance/test_etl.py` proves the internal ETL command's rootless fail-closed contracts (metadata-less WAL rejected, identity mismatch rejected). ETL currently has no documented public user surface, so the catalog ID `acceptance:etl-process` is **[trans]**: it reclassifies to `integration:internal-etl-cli-contract` owned by the chronicle-application/CLI boundary (task 4.3) and must not remain gated as acceptance. Exhaustive ETL idempotence/corruption matrices move to `chronicle-etl` integration (task 3.3). If ETL is later exposed as a documented public command, public ETL acceptance is added separately; an internal command MUST NOT be the sole evidence for a documented user-facing capability.

### 7. Rootless E2E

    tests/e2e/
    ├── test_rootless_pipeline.py  # [ok] synthetic capture → WAL → ETL → canonical → replay → verification
    └── http_acceptance_driver.py  # [trans] infrastructure; migrates to tests/support/ per task 4.4
        test_http_acceptance_driver.py  # [trans] its rootless self-test

Canonical rootless flow: synthetic capture input → record/WAL → ETL → canonical publication → inspect/stage consumption → replay → verification.

Responsibility: prove complete Chronicle subsystem composition without a real privileged capture environment. Assertions limited to important cross-stage invariants: stage output consumable by the next stage; session identity continuity; correlation continuity; operation survival; replay executes expected operations; verification succeeds. Do not repeat lower-level correctness matrices.

As the protocol matrix grows this may evolve into `tests/e2e/{http,postgres,mysql}/...`; speculative empty directories are not created.

### 8. Privileged tests (root black-box tree)

    tests/privileged/              # [plan] created only as real privileged black-box tests migrate
    ├── capture/
    │   ├── test_real_capture.py
    │   ├── test_cgroup_filtering.py
    │   └── test_process_attribution.py
    ├── acceptance/
    │   ├── test_record.py
    │   ├── test_recorder_readiness.py
    │   └── test_systemd_lifecycle.py
    ├── e2e/
    │   └── test_capture_pipeline.py
    ├── recovery/
    │   └── test_reboot_recovery.py
    └── resources/
        └── test_quota_pressure.py

These are ownership categories, not required immediate directories. Crate-owned kernel contracts (e.g. `crates/chronicle-capture-ebpf/tests/privileged_adapter.rs`) may remain in their crate; root `tests/privileged/` hosts cross-crate/process/system black-box assertions.

Directory names under `tests/privileged/` (capture, acceptance, e2e, recovery, resources) are **organizational topic groupings**, not new test-layer definitions. A directory name under `tests/privileged/` MUST NOT determine the functional layer; every privileged test still has exactly one functional layer (unit, integration, smoke, acceptance, or e2e) recorded in the catalog, independent of its topic directory. `recovery`, `capture`, and `resources` are not functional test layers.

The design distinguishes **functional layer** from **privileged environment requirement**:

- `tests/privileged/e2e/test_capture_pipeline.py` → layer = e2e, environment = privileged.
- `crates/chronicle-capture-ebpf/tests/privileged_adapter.rs` → layer = integration, environment = privileged.
- `tests/privileged/acceptance/test_record.py` → layer = acceptance, environment = privileged.
- `tests/privileged/recovery/test_reboot_recovery.py` → layer = acceptance or integration depending on its unique assertion, environment = privileged.

### 9. Minimal privileged E2E

Canonical privileged E2E: real workload → real eBPF capture → WAL → ETL → canonical → replay → verification.

Unique purpose: prove real kernel capture output feeds a replayable Chronicle pipeline. It must not rerun the entire WAL matrix, complete ETL corruption matrix, replay algorithm matrix, fmt, Clippy, workspace tests, or repository architecture validation — those are prerequisites selected separately by validation. There are very few privileged E2E cases (one; task 5.3).

### 10. Scripts directory is not a product test tree

    scripts/
    ├── validate.sh                # [ok] stable developer/release entry point (fast/targeted/gate/release)
    ├── validation.py              # [ok] selection, catalog, gate mapping, fingerprinting, evidence reuse, reporting
    ├── run-with-timeout.sh        # [ok] bounded command execution wrapper
    ├── acceptance/                # [trans] gate-oriented scenario storage being replaced by selectors
    │   └── lib/scenarios/{shared,extensions/p1,extensions/p2,p2}/   # [trans] task 8.x; removed after mapping
    ├── privileged/                # [trans] environment preparation/execution/evidence
    │   ├── preflight.py           # [ok] supported/unsupported_environment/infrastructure_error contract
    │   ├── prepare-vm.sh          # [plan] task 7.2
    │   ├── multipass-run.sh       # [plan] task 7.2
    │   ├── executor.py            # [plan] task 7.2 — selected test IDs only, no product assertions
    │   ├── collect-evidence.py    # [plan] task 7.2
    │   └── cleanup.sh             # [plan] task 7.2
    └── tests/                     # [ok] validation/runner tooling self-tests
        ├── validation/            # selector/catalog/preflight/ledger/architecture meta-tests
        └── acceptance/            # runner/report/wait/readiness tooling tests

`scripts/validate.sh` orchestrates and selects tests; it must not encode Chronicle product correctness assertions. `scripts/validation.py` owns changed-path selection, dependency-aware test selection, test catalog handling, gate mapping, fingerprinting, evidence reuse decisions, reporting, and orchestration metadata.

`scripts/privileged/` obtains and manages a supported privileged execution environment: Multipass VM ensure/start, readiness, bootstrap, source/artifact transfer, preflight, remote execution, reboot handoff, cleanup, evidence collection/sealing.

> Privileged orchestration knows how and where to run a selected test; it does not own the product assertion.

Eventually the executor consumes selected catalog IDs (`integration:capture-ebpf-privileged-feasibility`, `e2e:privileged-capture-pipeline`) without hardcoding portable WAL/ETL/replay correctness.

### 11. Script tooling tests

    scripts/tests/validation/
      test_layered_validation.py   # [ok] mode composition / selection semantics
      test_test_architecture.py    # [ok] catalog + ledger structural validation
      test_preflight.py            # [ok] preflight classifier outcomes
      test_sleep_policy.py         # [ok] sleep/condition-wait policy
      test_lifecycle_cleanup.py    # [ok] deterministic cleanup contracts
      test_select_fixtures.py      # [ok] dependency-aware selection fixtures
      test_evidence_reuse.py       # [ok] evidence reuse decisions
      test_architecture_boundaries.py  # [ok] dev/build edge enforcement

    scripts/tests/acceptance/
      test_runner.py               # [ok] runner/selector orchestration
      test-wait.sh                 # [ok] bounded condition waits
      test-p1-privileged-runner.sh # [ok] profile runner contracts
      test-p2-privileged-runner.sh # [ok] profile runner contracts
      test-p2-readiness.sh         # [ok] readiness convergence

Responsibility: test Chronicle's testing/validation infrastructure itself. These are not product acceptance tests. `scripts/tests/validation/test_preflight.py` tests the privileged preflight classifier; it does not prove Chronicle's eBPF product behavior.

### 12. Validation architecture metadata

    validation/
    └── test-architecture/         # [ok] machine-readable policy + coverage mapping
        ├── README.md
        ├── test-catalog.toml      # test ID, layer, environment, owner, command, gate membership, status
        ├── gate-coverage.toml     # P1/P2/release obligations → classified tests
        ├── migration-ledger.toml  # old assertion → target layer → owner → replacement → disposition
        ├── privileged-preflight-schema.toml  # stable environment/result classification
        ├── baseline.json          # before-state inventory
        ├── decisions.md           # deliberate per-suite migration decisions
        ├── boundary-review.md     # helper ownership / dependency direction
        ├── process-patterns.md    # lifecycle pattern inventory
        └── path-classification.md # per-path layer/environment classification

Metadata does not itself prove product correctness; it makes classification and gate mapping machine-checkable.

### 13. P1/P2 are selectors, not test directories

These are undesirable as structural locations:

    tests/p1/            ✗
    tests/p2/            ✗
    tests/privileged/p1/ ✗
    tests/privileged/p2/ ✗

P1 selects classified proofs required by P1; P2 selects classified proofs required by P2. They are not architectural ownership, and files are not named by milestone solely because a milestone currently invokes them. The current gate-oriented `scripts/acceptance/lib/scenarios/{extensions/p1,extensions/p2,p2}/` storage is **[trans]** and is replaced by selector configuration (tasks 8.x); obsolete wrappers are deleted only after mapped replacements pass (task 8.5).

### 14. Desired dependency direction between root test directories

    tests/smoke --------┐
    tests/acceptance ---+----> tests/support
    tests/e2e ----------┘

No dependency such as `tests/acceptance → tests/e2e` or `tests/support → tests/e2e` exists merely to obtain shared infrastructure. If a helper is shared by acceptance and E2E, ownership belongs in `tests/support/`. This is particularly important for HTTP workload/replay drivers: both suites may consume `tests/support/http_driver.py`, but neither may import the other's suite to reach it. Crate-local support stays crate-local (e.g. `chronicle-application/tests/http_test_server.rs`) when only one consumer exists.

Current state **[trans]**: `tests/support/process.py` still references `tests/e2e/http_acceptance_driver.py`, creating a support → e2e dependency; task 4.4 (reopened) removes that reference and relocates the driver to `tests/support/`.

### 15. Test assertion ownership decision tree

    Does it prove local implementation logic?                 → unit
    Does it prove a crate/component/binary contract?          → integration
    Does it only prove an executable/entry point is alive?    → smoke
    Does it prove a documented user-facing feature contract?  → acceptance
    Does it prove the complete Chronicle composed path?       → E2E
    Does the assertion itself require real supported
    Linux/kernel/eBPF/system resources?                       → same functional layer + privileged environment

The final question must not replace the functional classification:

- real eBPF adapter test = integration + privileged
- real record CLI feature = acceptance + privileged
- real capture → replay pipeline = E2E + privileged

### 16. Lowest-cost conclusive-proof rule

> Every behavior MUST be proven at the lowest-cost layer that can conclusively prove it.

    parser correctness                     → unit
    WAL writer/reader/recovery             → integration
    chronicle --help                       → smoke
    replay user contract                   → acceptance
    capture → WAL → ETL → replay           → E2E
    real kernel capture                    → privileged integration/acceptance/E2E depending on the assertion

Higher layers retain only the cross-boundary invariant they exist to prove.

### 17. Final target overview

    chronicle/
    ├── crates/
    │   └── <crate>/
    │       ├── src/
    │       │   └── **/*.rs            # unit
    │       └── tests/
    │           └── *.rs               # integration (privileged variant stays crate-owned where kernel contract)
    │
    ├── tests/
    │   ├── support/                   # shared black-box infrastructure
    │   ├── smoke/                     # executable liveness
    │   ├── acceptance/                # individual user-facing features
    │   ├── e2e/                       # complete rootless system path
    │   └── privileged/                # black-box assertions whose environment must be privileged
    │
    ├── scripts/
    │   ├── validate.sh
    │   ├── validation.py
    │   ├── run-with-timeout.sh
    │   ├── privileged/                # environment preparation/execution/evidence
    │   └── tests/                     # validation/runner tooling self-tests
    │
    └── validation/
        └── test-architecture/         # catalog, gate mapping, migration metadata, schemas

Architectural summary:

> `tests/` defines what Chronicle proves.
> `scripts/` defines how tests are selected, prepared, executed, bounded, and evidenced.
> `validation/` defines machine-readable policy and coverage mapping.
> P1/P2/release select classified tests; they do not own test implementations.

No speculative directory or file creation is required to reach this target; real test ownership drives structure.

### Current-state vs target-state reconciliation

| Area | Status | Note |
| --- | --- | --- |
| Colocated unit tests | **[ok]** | 42 files stay under `crates/*/src/**`; no `tests/unit/`. |
| Crate integration suites | **[ok]**/**[trans]** | Existing contracts stay; WAL/ETL/replay matrices planned per tasks 3.2–3.4; `cli_contract.rs` split (3.5). |
| Privileged crate integration | **[keep]** | `privileged_adapter.rs`, `privileged_signal.rs` remain; `privileged_feasibility.rs` split (3.6, done). |
| `tests/support/process.py` | **[ok]** | 3 consumers. |
| `tests/support/http_driver.py` | **[plan]**/**[trans]** | Mandatory migration of `tests/e2e/http_acceptance_driver.py` + its self-test (task 4.4, reopened); not moved in this task. |
| `tests/smoke/` | **[ok]** | Single combined `test_smoke.py` retained while small. |
| `tests/acceptance/` | **[ok]** | Flat per-feature files; subdirs when suites grow. ETL entry reclassifying to `integration:internal-etl-cli-contract` (4.3, reopened). |
| support → e2e dependency | **[trans]** | `tests/support/process.py` DRIVER reference; removed by task 4.4 (reopened). |
| `tests/e2e/test_rootless_pipeline.py` | **[ok]** | Canonical rootless composition. |
| `tests/privileged/` | **[plan]** | Ownership categories defined (§8); created only as tests migrate (8.x). |
| `scripts/privileged/` | **[trans]** | `preflight.py` exists; executor/prepare/collect/cleanup planned (7.2). |
| `scripts/tests/` | **[ok]** | Tooling self-tests in place. |
| `scripts/acceptance/lib/scenarios/{p1,p2,...}` | **[trans]** | Gate-owned storage being replaced by selectors (8.x); removed after mapping (8.5). |
| `validation/test-architecture/` | **[ok]** | Catalog/ledger/gate/schema metadata in place. |
| P1/P2 as directories | **[obsolete]** | Prohibited (§13); existing gate-oriented storage is transitional. |
| Post-migration cleanup | **[plan]** | Section 11 of tasks.md: remove obsolete wrappers/runner logic/helpers/fixtures/generated artifacts/empty scaffolding after replacement proof and gate mapping and BEFORE final supported proof (10.4). |

## Risks / Trade-offs

- **[Risk] Classification becomes subjective.** → Require assertion-level purpose, public surface, environment dependency, owner, and lowest-conclusive-layer rationale in ledger; review ambiguous cases before moves.
- **[Risk] Moving tests silently drops P1/P2 obligations.** → Map every current scenario/check to replacement test ID and gate selector; fail migration validation on unmapped required check.
- **[Risk] Lowering tests masks real integration faults.** → Keep sampled acceptance/E2E invariants and very few real-capture privileged paths; lower only exhaustive detail.
- **[Risk] Shared helpers become framework bloat or dependency escape hatch.** → Reuse existing primitives, require two consumers, keep language/local ownership, run architecture validation on dev edges.
- **[Risk] Unsupported status could be used to evade release gates.** → Unsupported/infrastructure states never count as passed or reusable evidence.
- **[Risk] Directory moves churn history without value.** → Every move requires responsibility, cost, black-box, or reliability rationale; staying is explicit option.
- **[Risk] Existing dirty concurrent work overlaps validation specs/scripts.** → Implementation rebases/coordinates before edits and never overwrites unrelated work; this planning change modifies only its own OpenSpec directory.
- **[Trade-off] Fast may initially remain broad.** → Preserve coverage first; narrow only after classified cheap/expensive boundaries are measured and selection tests exist.

## Open Questions

None blocking specification. Implementation must decide exact helper language/location and exact gate membership from completed ledger, measured runtime, and current milestone contract; those are constrained choices, not architecture reinterpretation.
