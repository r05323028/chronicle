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
6. Remove duplicate/gate-oriented implementations only after lower-cost replacement and required privileged sample both pass.
7. Update validation groups, CI, docs, and `AGENTS.md`; then collect fresh/reused compatible gate evidence as required.

Rollback keeps old gate path available until new selection demonstrates equivalent required scenario coverage. Persisted product data needs no migration.

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
