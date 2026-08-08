## Purpose

Layered validation modes, dependency selection, gate coverage, fingerprinting, evidence reuse, artifact policy, VM-local caches, and recorder readiness.

## Requirements

### Requirement: Unified validation modes

Validation SHALL expose one entry point with `fast`, `targeted`, `gate p1`, `gate p2`, and `release` modes. `fast` SHALL run formatting, warnings-denied workspace Clippy, workspace unit tests, strict OpenSpec validation, and applicable changed-crate tests without privileged acceptance. `gate p1` and `gate p2` SHALL preserve complete named gate coverage. `release` SHALL execute or validly reuse every required P1/P2 check and fail when any obligation is neither executed nor reused. Every mode SHALL execute potentially long-running commands beneath configurable hard deadlines, and complete gate/release modes SHALL have an outer gate deadline that terminates their process trees and retains failed evidence. Contributor guidance SHALL recommend fast, then targeted recorder/acceptance validation, then complete privileged gates only when required; no development workflow SHALL rely on an unbounded foreground daemon or poll.

#### Scenario: Fast local validation

- **WHEN** developer runs `scripts/validate.sh fast`
- **THEN** lightweight checks run with command deadlines and no full evidence bundle is generated

#### Scenario: Explicit gate

- **WHEN** developer runs `scripts/validate.sh gate p1` or `gate p2`
- **THEN** corresponding existing complete privileged gate is run and reported beneath an outer gate deadline

#### Scenario: Release completeness

- **WHEN** release mode finishes
- **THEN** every required P1/P2 check is executed or matched by valid reusable evidence, otherwise command fails

#### Scenario: Agent recorder workflow

- **WHEN** recorder-related source or acceptance tooling changes during development
- **THEN** guidance selects fast and targeted bounded validation before requesting complete privileged gate evidence

### Requirement: Changed-path dependency selection

Targeted mode SHALL accept `--changed-since <git-ref>`, inspect changed paths, select affected validation groups from declarative configuration, and print changed paths, selected groups, skipped groups, and a reason for every selection decision. Documentation-only changes SHALL select no privileged gate. ETL/checkpoint changes SHALL select ETL/checkpoint validation without selecting the full eBPF gate. WAL/recovery/retention/quota changes SHALL select focused recovery and P2 checks. eBPF changes SHALL select decoder/build checks and a minimal privileged smoke check. Acceptance-script or build-tooling changes SHALL invalidate affected privileged evidence and select the corresponding gate.

#### Scenario: Documentation-only change

- **WHEN** only documentation paths changed
- **THEN** targeted mode selects no privileged group and reports that reason

#### Scenario: ETL-only change

- **WHEN** ETL or checkpoint source changes
- **THEN** ETL/checkpoint group is selected and full eBPF gate is skipped with reason

#### Scenario: WAL recovery change

- **WHEN** WAL recovery, retention, or quota paths change
- **THEN** focused WAL/P2 groups are selected

#### Scenario: eBPF source change

- **WHEN** eBPF capture source or eBPF build input changes
- **THEN** decoder/build checks and minimal privileged smoke are selected, and matching prior evidence is invalid

### Requirement: Fingerprinted evidence reuse

Each P1/P2 acceptance profile SHALL compute a deterministic acceptance-sensitive fingerprint from gate-owned source/build inputs, Cargo.lock, toolchain inputs, acceptance runner and scenario definitions, evidence schema/version behavior, and validation configuration. Environment identity (OS, Ubuntu version, architecture, kernel, BTF/cgroup capabilities, compiler, and executor) SHALL be stored and compared separately, never used as source fingerprint alone. Evidence identity SHALL consist of matching validation fingerprint and validation-relevant source/configuration, compatible evidence schema, unchanged required scenario set, compatible privileged environment, valid complete artifact manifest, and successful completion of every required check without `not_checked`, timeout, or incomplete state. Evidence SHALL record fingerprint, source provenance including commit/tree SHA and dirty state, creation date, environment, profile scenario coverage, and executed/reused status. Git commit SHA SHALL remain provenance metadata and SHALL NOT serve as reuse, cache, or release eligibility identity or by itself invalidate otherwise compatible evidence. `scripts/acceptance.sh` SHALL reuse only compatible successful evidence by default; `--no-reuse` SHALL force a fresh run. Acceptance-sensitive source or validation-contract changes SHALL prevent reuse; unrelated documentation, OpenSpec archive housekeeping, rebases, equivalent recommits, and commit-history-only changes SHALL not.

#### Scenario: Unchanged gate reuse

- **WHEN** valid evidence from another commit has matching fingerprint, validation-relevant content, scenario coverage, compatible schema and environment, complete manifest, and all required checks passed
- **THEN** expensive acceptance is skipped and reuse status, source provenance, environment, and covered scenarios are printed
- **AND THEN** commit equality is not required in development or release mode

#### Scenario: Relevant input invalidation

- **WHEN** gate-owned source, acceptance scenario or runner behavior, evidence schema changes incompatibly, build input, or validation contract changes
- **THEN** fingerprint or compatibility contract changes and prior evidence is not reused

#### Scenario: Environment or evidence invalidation

- **WHEN** environment is incompatible, manifest is invalid or incomplete, required check failed or is `not_checked`, timeout occurred, or report is incomplete
- **THEN** prior evidence is not reused even when fingerprint matches

#### Scenario: Unrelated or history-only change

- **WHEN** only unrelated documentation, OpenSpec archive housekeeping, rebase metadata, equivalent recommit, or commit history changes
- **THEN** gate fingerprint remains unchanged and compatible evidence remains reusable

### Requirement: Failure-first evidence

Successful fast, targeted, and gate runs SHALL retain at most compact metadata (`summary.json`, `environment.json`, `manifest.json`, and `checksums.txt`) when artifacts are enabled. They SHALL NOT retain or upload Cargo target directories, Cargo caches, VM filesystems, complete successful WAL data, raw logs for every successful test, or duplicated build/eBPF outputs. Failed runs SHALL retain only failure summary, failed log, reproducer, kernel log, and failure-related WAL/session data, compressing large reproducer data with zstd when available. `--no-artifact`, `--artifact-on-failure`, and `--keep-workdir` SHALL be supported; artifact-on-failure SHALL be default outside release.

#### Scenario: Successful compact run

- **WHEN** targeted or gate validation succeeds
- **THEN** only compact metadata is retained and cache directories/full WAL data are absent from evidence

#### Scenario: Failed gate

- **WHEN** a validation check fails
- **THEN** minimal reproducible evidence is retained and unrelated WAL/session data is excluded

#### Scenario: No artifact

- **WHEN** `--no-artifact` is supplied
- **THEN** no evidence copy is created

### Requirement: VM-local caches

Privileged validation SHALL reuse an existing Multipass Ubuntu instance when available, transfer an immutable snapshot of the acceptance-sensitive checkout (including dirty development changes), use VM-local `/home/ubuntu/chronicle-target` for `CARGO_TARGET_DIR` and `/home/ubuntu/.cargo` for Cargo cache, and bootstrap missing dependencies without reinstalling an existing toolchain on every run. Build outputs SHALL be treated as caches and SHALL never be uploaded as evidence. Every release request SHALL require a clean, identifiable current source checkout with known commit and tree before evidence lookup and SHALL require unchanged end fingerprint, commit, tree, dirty state, and identity status through the release command. Historical evidence reuse SHALL use content and compatibility identity and SHALL NOT require evidence commit or tree SHA to equal the current release candidate.

#### Scenario: Cache reuse

- **WHEN** consecutive privileged validations run on same VM
- **THEN** compatible compilation reuses VM-local target and Cargo cache

#### Scenario: Missing dependency

- **WHEN** required VM dependency is absent
- **THEN** bootstrap installs it once and later runs skip reinstall

#### Scenario: Clean cross-identity release reuse

- **WHEN** a release request starts from a clean, identifiable current checkout and compatible historical evidence has another commit or tree SHA
- **THEN** evidence may be reused and release eligibility depends on current checkout stability plus evidence compatibility and completeness

#### Scenario: Dirty current release checkout

- **WHEN** a release request starts from a dirty current checkout, including when compatible historical evidence exists
- **THEN** release source validation fails before historical evidence is accepted

#### Scenario: Source mutation during release command

- **WHEN** current fingerprint, commit, tree, dirty state, or identity status changes after any release request starts
- **THEN** release fails independently from historical evidence origin identity

### Requirement: Privileged recorder readiness

P2 privileged acceptance SHALL consume an explicit machine-readable recorder state distinguishing `starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, and `failed`. Workload admission SHALL wait for `ready`, which SHALL imply recovery complete, writable WAL, reachable status control plane, initialized capture, required eBPF attachment, and no fatal startup error. Polling SHALL use configurable timeout and interval, print state transitions, tolerate temporary status unavailability, stop on terminal failure, and retain bounded diagnostics on timeout or failure.

#### Scenario: Startup transition

- **WHEN** recorder status transitions from starting through recovery and eBPF loading
- **THEN** acceptance prints each transition and admits workload only at ready

#### Scenario: Terminal failure

- **WHEN** status reports failed or stale ownership
- **THEN** acceptance stops polling immediately and retains readiness diagnostics

#### Scenario: Recovery restart

- **WHEN** recorder restarts after crash or VM reboot with WAL/checkpoint state
- **THEN** polling waits for a fresh ready state rather than trusting systemd active or stale metadata

### Requirement: Retention guidance

CI and validation documentation SHALL specify no upload or 1-3 day retention for successful fast/targeted runs, compact approximately 7-day retention for successful gates, approximately 14-day retention for failed gates, and long-term retention for release evidence. Cache directories SHALL not be artifact paths.

#### Scenario: Documented retention policy

- **WHEN** contributor or CI operator reads validation guidance
- **THEN** success, failure, release, and cache retention rules are explicit
