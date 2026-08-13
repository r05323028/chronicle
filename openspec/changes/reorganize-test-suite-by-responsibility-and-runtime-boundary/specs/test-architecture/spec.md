## ADDED Requirements

### Requirement: Tests use the lowest conclusive functional layer

Every significant Chronicle test SHALL be classified as exactly one functional layer: unit, integration, smoke, acceptance, or end-to-end. Selected layer MUST be lowest-cost layer that can conclusively prove behavior. Privileged execution, P1, and P2 SHALL NOT be functional test layers.

Unit tests SHALL prove local function, module, parser, serializer, validation, calculation, or state-machine logic. Integration tests SHALL prove public crate/component/binary contracts and meaningful deterministic combinations. Smoke tests SHALL prove shipped executables and basic entry points start or reject invalid startup sanely. Acceptance tests SHALL prove a documented user-facing feature contract through supported public/external surfaces. End-to-end tests SHALL prove important invariants across complete composed Chronicle path. Merely using a public CLI/API surface does not make a test Acceptance, and merely invoking a subprocess does not make a test Smoke: public executable liveness is smoke, and CLI argument/rendering/exit-code contracts are integration when proving a component or binary contract.

#### Scenario: Local parser behavior

- **WHEN** behavior depends only on parser input and local state
- **THEN** exhaustive cases are owned by colocated unit tests and are not repeated as acceptance or E2E matrix

#### Scenario: Crate contract

- **WHEN** behavior crosses public APIs of one or more components without requiring complete Chronicle process graph
- **THEN** integration tests prove it using deterministic files, fixtures, fakes, or in-memory adapters

#### Scenario: Executable liveness

- **WHEN** required proof is that `chronicle --help` (and `--version` when the binary exposes it), startup, or one minimal valid/invalid public invocation works
- **THEN** bounded black-box smoke test proves only that entry-point contract

#### Scenario: Fixture setup is not the assertion target

- **WHEN** smoke prepares state with an internal fixture seam and then invokes a public entry point (for example internal fixture setup → public inspect invocation)
- **THEN** smoke may still prove public liveness because the internal helper is setup, not the assertion target

#### Scenario: Internal command as smoke

- **WHEN** a test only proves an internal command can run as a subprocess (for example `chronicle internal etl`) and the command has no documented public surface
- **THEN** it classifies as an integration/internal contract, not smoke

#### Scenario: User feature

- **WHEN** required proof is a documented public record, inspect, replay, or other user-facing feature contract — including ETL only when ETL has a documented public user surface
- **THEN** black-box acceptance test uses public CLI, process, filesystem/storage, documented API, or network surfaces; `chronicle internal etl` is not public ETL Acceptance

#### Scenario: Complete composition

- **WHEN** required proof spans capture input, WAL, ETL, canonical publication, replay, and verification
- **THEN** E2E test asserts only cross-stage compatibility and identity, operation, and verification invariants

### Requirement: Test location follows responsibility and crate ownership

Colocated Rust unit tests SHALL remain under `crates/<crate>/src/**`. Public crate/component integration tests SHALL normally live under owning crate's `tests/`. Root black-box tests SHALL use responsibility-oriented locations equivalent to `tests/smoke/`, `tests/acceptance/`, `tests/e2e/`, and `tests/privileged/`. Existing tests MAY remain when owner and responsibility are already clear; directory movement without responsibility, runtime-cost, black-box, dependency, or reliability reason SHALL NOT be required.

Acceptance tests MUST NOT import internal crate implementation details to facilitate assertions. E2E tests MUST NOT become exhaustive parser, WAL, ETL, protocol, or replay correctness matrices. Privileged crate integration tests MAY remain crate-owned when testing that crate's real kernel contract and discoverable as privileged.

#### Scenario: Colocated local tests

- **WHEN** maintainer classifies existing deterministic module tests
- **THEN** tests stay colocated rather than moving to centralized unit directory

#### Scenario: Existing correctly owned integration test

- **WHEN** crate integration test proves its public contract and uses only allowed dependencies
- **THEN** migration ledger records `stay` and no aesthetic move is required

#### Scenario: Black-box feature assertion reaches internals

- **WHEN** proposed acceptance test imports Chronicle implementation crates
- **THEN** review rejects it or reclassifies it as integration coverage

### Requirement: Privilege is an independent environment dimension

Every test SHALL separately declare whether it is portable/rootless or requires privileged supported environment. Test SHALL be privileged only when assertion itself depends on real Linux/kernel/environment behavior that cannot reasonably be proved rootlessly, including real eBPF loading/attachment, cgroup filtering, kernel/BTF compatibility, ring-buffer behavior, process attribution, kernel-dependent truncation, supported-platform compatibility, or genuinely non-simulatable reboot/crash behavior.

WAL encoding/checksums, deterministic recovery/corruption matrices, ETL transformations, CLI parsing/rendering, protocol parsing, replay matching/policy, repository format checks, and equivalent portable assertions MUST NOT run as privileged proof merely because existing Multipass profile can execute them.

#### Scenario: Portable assertion in privileged profile

- **WHEN** audit finds Cargo unit/integration test or rootless copied-WAL assertion inside privileged scenario
- **THEN** portable proof moves to its functional layer and gate selection references result separately

#### Scenario: Real eBPF assertion

- **WHEN** assertion loads and attaches production eBPF and observes real socket/cgroup behavior
- **THEN** it remains privileged and is additionally classified as integration, acceptance, or E2E by purpose

#### Scenario: Privileged E2E

- **WHEN** milestone requires proof from real workload through real eBPF capture, WAL, ETL, canonical publication, replay, and verification
- **THEN** very small privileged E2E test proves composition without duplicating downstream correctness matrices

### Requirement: Rootless E2E is normal post-capture composition proof

Chronicle SHALL provide deterministic rootless E2E coverage from synthetic capture evidence through WAL, ETL, canonical model/storage, replay, and verification. Rootless E2E SHALL prove recorded operations survive pipeline, stage outputs are consumable by next stage, session/correlation identity survives, expected operations execute, and verification succeeds. Detailed protocol, WAL, ETL, storage, and replay error matrices SHALL remain unit/integration-owned.

#### Scenario: Synthetic full pipeline

- **WHEN** deterministic capture fixture enters pipeline
- **THEN** rootless E2E proves WAL-to-ETL-to-canonical-to-replay compatibility and verification without eBPF or Multipass

#### Scenario: Fixture seam as E2E setup

- **WHEN** rootless E2E injects synthetic capture evidence through an internal fixture command (for example record-fixture → WAL → ETL → canonical → replay → verification)
- **THEN** the suite remains E2E because the unique assertion is complete pipeline composition, and the fixture seam is setup, not public Record acceptance evidence

#### Scenario: Downstream corruption matrix

- **WHEN** many WAL corruption boundaries need proof
- **THEN** integration tests own matrix and E2E samples only corruption invariant required for complete-system behavior

### Requirement: High-level tests do not duplicate exhaustive lower-layer proof

Every acceptance, E2E, and privileged test SHALL identify its unique user-contract, composition, or environment invariant. It MAY sample lower-level output needed to establish that invariant but MUST NOT rerun exhaustive lower-layer suite as scenario step. Repository validation such as formatting, Clippy, architecture checks, OpenSpec validation, and broad workspace Cargo tests SHALL NOT be product assertions inside acceptance or privileged tests.

Lower-cost replacement MUST pass and be mapped to every displaced gate obligation before duplicate high-level coverage is removed.

#### Scenario: Cargo tests embedded in acceptance

- **WHEN** acceptance scenario invokes portable `cargo test`, formatting, or workspace checks
- **THEN** checks become independent prerequisites selected by validation and are removed from scenario behavior after coverage mapping passes

#### Scenario: Sampled invariant remains useful

- **WHEN** E2E must confirm ETL output is replay-consumable
- **THEN** E2E checks consumability and successful verification but leaves ETL field/fault matrix to ETL integration tests

### Requirement: Process-based tests use deterministic lifecycle support

Black-box process tests SHALL use bounded startup, readiness, shutdown, cleanup, and port/resource allocation. Condition-based readiness SHALL replace arbitrary synchronization sleeps wherever observable condition exists. Elapsed-time waits SHALL be explicit only when time passage is behavior under test, followed by bounded convergence proof.

Implementation SHALL reuse existing timeout, bounded wait, and specialized recorder-readiness contracts. Shared helpers SHALL be introduced only for repeated concrete concepts and MAY provide responsibilities equivalent to Chronicle/Recorder process ownership, workload server lifecycle, readiness/wait conditions, evidence collection, cleanup, and timeout. Generic helpers MUST NOT weaken recorder status freshness, stale-owner, terminal-state, or supported-kernel semantics.

#### Scenario: Recorder startup

- **WHEN** process test starts recorder
- **THEN** it waits on machine-observable fresh readiness with command and overall deadlines rather than fixed sleep

#### Scenario: Process test exits

- **WHEN** black-box test passes, fails, or times out
- **THEN** owned subprocesses and resources are deterministically reaped and focused diagnostics remain available

#### Scenario: One-off helper proposal

- **WHEN** abstraction has only one concrete consumer
- **THEN** implementation keeps support local rather than adding general framework

### Requirement: Shared test support obeys dependency direction

Test helpers, fixtures, and dev dependencies SHALL have named owner and SHALL follow `workspace-dependency-boundaries`. Dev/build edges MUST NOT bypass normal-layer architecture. CLI black-box support SHALL invoke binary or application-owned surfaces and MUST NOT add CLI dependencies on lower Chronicle crates. Aya/kernel ABI support SHALL remain private to `chronicle-capture-ebpf`. No new shared test crate SHALL be created without demonstrated multi-crate need and corresponding architecture documentation, `AGENTS.md`, and policy updates.

#### Scenario: CLI fixture needs lower-layer artifact

- **WHEN** CLI test requires WAL/capture fixture preparation
- **THEN** it uses existing application-owned test support, deterministic external fixture, or black-box setup rather than adding forbidden CLI dev dependencies

#### Scenario: Proposed shared test crate

- **WHEN** implementation can keep helpers crate-local or root-suite-local
- **THEN** no workspace crate or dependency is added

#### Scenario: Dev edge violates architecture

- **WHEN** helper introduces forbidden workspace dev dependency
- **THEN** portable architecture validation rejects migration

### Requirement: Privileged preflight distinguishes environment from product results

Privileged execution SHALL perform bounded preflight before product assertions and emit machine-readable `supported`, `unsupported_environment`, or `infrastructure_error` outcome. Supported preflight SHALL check required operating system, architecture, Ubuntu/kernel version, effective privileges/capabilities, cgroup v2, BTF, bpffs, required hooks/tooling, and executor readiness applicable to selected tests.

`unsupported_environment` SHALL identify failed/unknown support probes and remediation and SHALL NOT be reported as `test_failed`. `infrastructure_error` SHALL identify VM preparation, transfer, bootstrap, or executor failure and SHALL NOT be reported as product assertion failure. Neither outcome SHALL satisfy, become reusable evidence for, or make release eligible any required privileged gate. Product test failure SHALL be possible only after supported preflight.

#### Scenario: Unsupported host

- **WHEN** selected privileged test runs where required BTF, capability, kernel, or supported Ubuntu contract is absent
- **THEN** report says `unsupported_environment`, names probes/remediation, executes no product assertion, and gate remains unsatisfied

#### Scenario: VM preparation fails

- **WHEN** Multipass VM cannot start, bootstrap, transfer source, or become ready
- **THEN** report says `infrastructure_error` and does not blame Chronicle product behavior

#### Scenario: Supported environment assertion fails

- **WHEN** preflight is supported and real eBPF attach assertion fails
- **THEN** report says `test_failed` with product evidence

### Requirement: Validation and environment tooling do not own product correctness

Validation tooling SHALL select, order, bound, and report classified tests. Privileged executor tooling SHALL prepare environment, run selected test IDs, and collect evidence. Neither SHALL embed product correctness logic belonging to test files. P1/P2 gate configuration SHALL map required milestone obligations to classified tests and SHALL NOT require tests to live under P1/P2 directories.

#### Scenario: Gate selects acceptance and privileged E2E

- **WHEN** P2 requires public replay behavior and real capture/reboot evidence
- **THEN** gate selects independently owned acceptance/E2E/privileged tests plus lower-layer prerequisites

#### Scenario: Validation helper contains product field assertions

- **WHEN** migration finds WAL, ETL, CLI, protocol, or replay correctness in selector/executor code
- **THEN** assertion moves to owning functional test while orchestration keeps only result/evidence validation

### Requirement: Migration uses complete assertion classification ledger

Before moving or deleting coverage, implementation SHALL inventory every significant Rust test, root test, acceptance scenario/check, privileged runner assertion, and product assertion embedded in shell/Python tooling. Ledger SHALL record current path/test or assertion ID, current responsibility, target functional layer, environment dimension, crate/feature owner, gate obligations, and one disposition: `stay`, `move`, `split`, `delete_duplicate`, `deterministic_fixture`, `make_rootless`, `remain_privileged`, or `shared_lifecycle_migration`.

Every non-stay disposition SHALL state responsibility/reliability reason, replacement test ID or destination, dependency implications, and verification command. Required gate coverage SHALL remain mapped throughout migration. Empty target directories or broad path moves without classified assertions SHALL NOT satisfy migration.

#### Scenario: Mixed scenario

- **WHEN** one privileged scenario contains real eBPF capture, portable WAL matrix, CLI acceptance, and full E2E assertions
- **THEN** ledger splits assertions across appropriate layers and retains only kernel-dependent proof as privileged

#### Scenario: Duplicate deletion

- **WHEN** lower-cost test proves same behavior more strongly and required gate mapping references it
- **THEN** duplicate high-level assertion may be deleted after replacement passes

#### Scenario: Ambiguous test

- **WHEN** reviewer cannot identify unique purpose or environment requirement
- **THEN** test remains unmoved and migration is incomplete until classification is resolved

### Requirement: Internal command coverage is integration, not acceptance

Acceptance SHALL prove a documented user-facing feature contract through supported public/external surfaces. Merely using a public CLI/API surface does not make a test Acceptance: public executable liveness is smoke, CLI argument/rendering/exit-code contracts are integration when proving a component or binary contract, and only documented user-facing feature behavior is acceptance. Internal commands and internal compatibility seams SHALL be classified as integration/internal-contract tests owned by the appropriate application/CLI boundary, never as Acceptance. An internal command MUST NOT be the sole evidence for a documented user-facing capability; public acceptance of that capability SHALL be added separately when a documented public surface exists.

#### Scenario: Public command acceptance

- **WHEN** behavior is a documented user-facing command invoked through supported external surfaces
- **THEN** acceptance proves it through public CLI, process, filesystem/storage, API, or network surfaces

#### Scenario: Public surface is not acceptance

- **WHEN** a test exercises the public binary only for liveness or for argument/rendering/exit-code contract
- **THEN** it classifies as smoke or integration, not acceptance

#### Scenario: Smoke convergence for internal ETL

- **WHEN** internal ETL coverage appears in both smoke (`smoke:etl-minimal-start` / `internal etl` liveness case) and acceptance (`acceptance:etl-process`)
- **THEN** all internal ETL CLI coverage converges to one CLI-owned Integration owner (for example `integration:cli-contract`) while application ETL behavior remains separately owned by `integration:etl-contract` under chronicle-application; the seam is not simultaneously Smoke + Acceptance + Integration

#### Scenario: Internal command is not public feature evidence

- **WHEN** a documented user-facing capability can only be exercised through an internal command seam
- **THEN** the internal command is not counted as Acceptance for that capability; Acceptance requires a documented public surface and is added in a future change if one is exposed

#### Scenario: Internal seam convenience

- **WHEN** an internal command is convenient for rootless execution but has no documented public user surface
- **THEN** its coverage classifies as an integration/internal contract (for example `integration:internal-etl-cli-contract`) and no gate lists it as acceptance

### Requirement: Privileged directories are topic groupings, not layers

Subdirectory names under `tests/privileged/` (capture, acceptance, e2e, recovery, resources) SHALL be organizational topic groupings, not functional test layers. A directory name under `tests/privileged/` MUST NOT determine the functional layer. Every privileged test SHALL have exactly one functional layer (unit, integration, smoke, acceptance, or e2e) recorded in the catalog, independent of its topic directory. `recovery`, `capture`, and `resources` SHALL NOT be introduced as functional test layers.

#### Scenario: Recovery topic directory

- **WHEN** a reboot-recovery assertion lives under `tests/privileged/recovery/`
- **THEN** the catalog records its unique functional layer (acceptance or integration depending on the assertion) with environment = privileged, and `recovery` is not a layer

### Requirement: Test catalog is authoritative classification and selector metadata

`validation/test-architecture/test-catalog.toml` SHALL be the authoritative machine-readable classification, ownership, execution, and selector catalog, representing test ID, functional layer, environment requirement, owner, command, status, and gate membership. Actual file locations SHALL conform to the directory responsibility contract in design.md; migration destinations SHALL be tracked in the migration ledger. The catalog SHALL NOT be required to become a file-manifest database; no path field SHALL be added without a demonstrated implementation need.

#### Scenario: Colocated unit tests in catalog

- **WHEN** maintainer checks a colocated Rust unit module against the catalog
- **THEN** the catalog entry identifies layer, owner, environment, command, and gate membership while the file remains colocated under `crates/<crate>/src/**` rather than being enumerated as a manifest path

### Requirement: No dead testing infrastructure after migration

After migration completes, every retained product test, helper, fixture, scenario wrapper, validation entry, and privileged orchestration file SHALL have a current owner and caller or a documented historical/audit purpose. Dead compatibility wrappers, duplicate assertions, unused fixtures/helpers, generated files, empty scaffolding, and obsolete gate-owned test implementations SHALL be removed before the change is considered complete. Cleanup MUST NOT occur before replacement proof passes, gate coverage mapping is complete, no-coverage-loss checks pass, final selector behavior is proven, and required supported privileged evidence is retained.

#### Scenario: Wrapper with no caller

- **WHEN** a scenario wrapper exists only to run another classified test or to carry milestone naming
- **THEN** it is removed only after selector equivalence is proven and no caller remains

#### Scenario: Duplicate portable matrix in privileged path

- **WHEN** a privileged scenario still reruns portable WAL/ETL/replay/repository checks after classified prerequisites pass
- **THEN** the duplicate is removed and the gate references the independent classified prerequisite result

### Requirement: Testing guidance is durable

After migration implementation completes, `AGENTS.md` and detailed testing documentation SHALL state: unit tests prove local logic; integration tests prove component/crate contracts; smoke tests prove executables are alive; acceptance tests prove individual user-facing features; E2E tests prove complete system composition; privileged tests prove only behavior requiring real supported kernel/environment; validation scripts select and orchestrate tests and contain no product test logic; and always use lowest-cost test layer that conclusively proves behavior.

Guidance SHALL also describe target locations, privilege/preflight outcomes, gate-selection semantics, helper ownership, and crate-boundary constraints.

#### Scenario: Future test contribution

- **WHEN** maintainer or agent reads repository guidance before adding coverage
- **THEN** guidance identifies correct layer, location, environment classification, and lowest-cost rule
