## ADDED Requirements

### Requirement: Unified validation modes

Validation SHALL expose one entry point with `fast`, `targeted`, `gate p1`, `gate p2`, and `release` modes. `fast` SHALL run formatting, warnings-denied workspace Clippy, workspace unit tests, strict OpenSpec validation, and applicable changed-crate tests without privileged acceptance. `gate p1` and `gate p2` SHALL preserve complete named gate coverage. `release` SHALL execute or validly reuse every required P1/P2 check and fail when any obligation is neither executed nor reused.

#### Scenario: Fast local validation

- **WHEN** developer runs `scripts/validate.sh fast`
- **THEN** lightweight checks run and no full evidence bundle is generated

#### Scenario: Explicit gate

- **WHEN** developer runs `scripts/validate.sh gate p1` or `gate p2`
- **THEN** corresponding existing complete privileged gate is run and reported

#### Scenario: Release completeness

- **WHEN** release mode finishes
- **THEN** every required P1/P2 check is executed or matched by valid reusable evidence, otherwise command fails

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

Each P1/P2 gate SHALL compute a deterministic fingerprint from only gate-relevant source/build/acceptance inputs, Cargo.lock, compiler version, target architecture, Ubuntu/kernel/BTF/cgroup capability values, and validation configuration. Evidence SHALL record fingerprint, original commit, creation date, environment, covered checks, and executed/reused status. `--reuse-evidence` SHALL reuse only matching valid successful evidence and SHALL report reuse explicitly; `--force` SHALL disable reuse for a fresh gate run. Acceptance script, relevant source, compiler, kernel, architecture, or validation-contract changes SHALL prevent reuse.

#### Scenario: Unchanged gate reuse

- **WHEN** valid evidence with matching fingerprint exists and reuse is requested
- **THEN** expensive gate is skipped and original fingerprint, commit, date, environment, and covered checks are printed

#### Scenario: Relevant input invalidation

- **WHEN** a gate-owned source, acceptance script, compiler, kernel, architecture, eBPF input, or validation contract changes
- **THEN** fingerprint changes and prior evidence is not reused

#### Scenario: Unrelated documentation change

- **WHEN** only unrelated documentation changes
- **THEN** gate fingerprint remains unchanged and existing valid evidence remains reusable

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

Privileged validation SHALL reuse an existing Multipass Ubuntu instance when available, mount source at `/mnt/chronicle`, use VM-local `/home/ubuntu/chronicle-target` for `CARGO_TARGET_DIR` and `/home/ubuntu/.cargo` for Cargo cache, and bootstrap missing dependencies without reinstalling an existing toolchain on every run. Build outputs SHALL be treated as caches and SHALL never be uploaded as evidence.

#### Scenario: Cache reuse

- **WHEN** consecutive privileged validations run on same VM
- **THEN** compatible compilation reuses VM-local target and Cargo cache

#### Scenario: Missing dependency

- **WHEN** required VM dependency is absent
- **THEN** bootstrap installs it once and later runs skip reinstall

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
