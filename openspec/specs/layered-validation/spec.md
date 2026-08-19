## Purpose

Layered validation modes, dependency selection, gate coverage, fingerprinting, evidence reuse, artifact policy, VM-local caches, and recorder readiness.

## Requirements

### Requirement: Unified validation modes

Validation SHALL expose one entry point with `fast`, `targeted`, `live-capture`, `recorder`, and `release` modes. `fast` SHALL run formatting, warnings-denied workspace Clippy, workspace unit tests, strict OpenSpec validation, and applicable changed-crate tests without privileged acceptance. `live-capture` and `recorder` SHALL preserve complete named gate coverage. `release` SHALL execute or validly reuse every required live-capture/recorder check and fail when any obligation is neither executed nor reused. Every mode SHALL execute potentially long-running commands beneath configurable hard deadlines, and complete gate/release modes SHALL have an outer gate deadline that terminates their process trees and retains failed evidence. Contributor guidance SHALL recommend fast, then targeted recorder/acceptance validation, then complete privileged gates only when required; no development workflow SHALL rely on an unbounded foreground daemon or poll.

#### Scenario: Fast local validation

- **WHEN** developer runs `scripts/validate.sh fast`
- **THEN** lightweight checks run with command deadlines and no full evidence bundle is generated

#### Scenario: Explicit gate

- **WHEN** developer runs `scripts/validate.sh live-capture` or `recorder`
- **THEN** corresponding existing complete privileged gate is run and reported beneath an outer gate deadline

#### Scenario: Release completeness

- **WHEN** release mode finishes
- **THEN** every required live-capture/recorder check is executed or matched by valid reusable evidence, otherwise command fails

#### Scenario: Agent recorder workflow

- **WHEN** recorder-related source or acceptance tooling changes during development
- **THEN** guidance selects fast and targeted bounded validation before requesting complete privileged gate evidence

### Requirement: Changed-path dependency selection

Targeted mode SHALL accept `--changed-since <git-ref>`, inspect changed paths, select affected validation groups from declarative configuration, and print changed paths, selected groups, skipped groups, and a reason for every selection decision. Documentation-only changes SHALL select no privileged gate. ETL/checkpoint changes SHALL select ETL/checkpoint validation without selecting the full eBPF gate. WAL/recovery/retention/quota changes SHALL select focused recovery and recorder checks. eBPF changes SHALL select decoder/build checks and a minimal privileged smoke check. Acceptance-script or build-tooling changes SHALL invalidate affected privileged evidence and select the corresponding gate.

#### Scenario: Documentation-only change

- **WHEN** only documentation paths changed
- **THEN** targeted mode selects no privileged group and reports that reason

#### Scenario: ETL-only change

- **WHEN** ETL or checkpoint source changes
- **THEN** ETL/checkpoint group is selected and full eBPF gate is skipped with reason

#### Scenario: WAL recovery change

- **WHEN** WAL recovery, retention, or quota paths change
- **THEN** focused WAL/recorder groups are selected

#### Scenario: eBPF source change

- **WHEN** eBPF capture source or eBPF build input changes
- **THEN** decoder/build checks and minimal privileged smoke are selected, and matching prior evidence is invalid

### Requirement: Fingerprinted evidence reuse

Each live-capture/recorder acceptance profile SHALL compute a deterministic acceptance-sensitive fingerprint from gate-owned source/build inputs, Cargo.lock, toolchain inputs, acceptance runner and scenario definitions, evidence schema/version behavior, and validation configuration. Environment identity (OS, Ubuntu version, architecture, kernel, BTF/cgroup capabilities, compiler, and executor) SHALL be stored and compared separately, never used as source fingerprint alone. Evidence identity SHALL consist of matching validation fingerprint and validation-relevant source/configuration, compatible evidence schema, unchanged required scenario set, compatible privileged environment, valid complete artifact manifest, and successful completion of every required check without `not_checked`, timeout, or incomplete state. Evidence SHALL record fingerprint, source provenance including commit/tree SHA and dirty state, creation date, environment, profile scenario coverage, and executed/reused status. Git commit SHA SHALL remain provenance metadata and SHALL NOT serve as reuse, cache, or release eligibility identity or by itself invalidate otherwise compatible evidence. `scripts/acceptance.sh` SHALL reuse only compatible successful evidence by default; `--no-reuse` SHALL force a fresh run. Acceptance-sensitive source or validation-contract changes SHALL prevent reuse; unrelated documentation, OpenSpec archive housekeeping, rebases, equivalent recommits, and commit-history-only changes SHALL not.

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

Recorder privileged acceptance SHALL consume an explicit machine-readable recorder state distinguishing `starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, and `failed`. Workload admission SHALL wait for `ready`, which SHALL imply recovery complete, writable WAL, reachable status control plane, initialized capture, required eBPF attachment, and no fatal startup error. Polling SHALL use configurable timeout and interval, print state transitions, tolerate temporary status unavailability, stop on terminal failure, reject status older than latest systemd activation, and retain bounded diagnostics on timeout or failure. General acceptance convergence helpers SHALL compose with this contract and SHALL NOT replace or weaken its freshness, stale-owner, lifecycle, or terminal-state semantics.

#### Scenario: Startup transition

- **WHEN** recorder status transitions from starting through recovery and eBPF loading
- **THEN** acceptance prints each transition and admits workload only at ready

#### Scenario: Terminal failure

- **WHEN** status reports failed or stale ownership
- **THEN** acceptance stops polling immediately and retains readiness diagnostics

#### Scenario: Recovery restart

- **WHEN** recorder restarts after crash or VM reboot with WAL/checkpoint state
- **THEN** polling waits for fresh ready state rather than trusting systemd active or stale metadata

#### Scenario: General convergence integration

- **WHEN** destructive scenario requires both stable systemd state and recorder readiness
- **THEN** general bounded barriers first observe unit convergence and then use existing recorder-readiness contract for fresh ready state

### Requirement: Acceptance reports preserve scenario execution state

Unified acceptance execution SHALL atomically persist ordered selected scenarios, current scenario, completed scenarios, failed scenario, and per-scenario `passed`, `failed`, or `not_checked` status under run artifacts. Central reporting SHALL prefer valid execution state over coarse profile exit inference and remain deterministic and machine-readable. A profile-level failure SHALL NOT classify scenarios never started as failed.

#### Scenario: Middle scenario fails

- **WHEN** scenario A passes, scenario B fails, and scenarios C and D are never started
- **THEN** central report records A `passed`, B `failed`, C and D `not_checked`, and profile status `failed`

#### Scenario: Active scenario times out

- **WHEN** scenario deadline or gate deadline expires while scenario B is current after A completed
- **THEN** timeout evidence and execution state attribute B `failed`, preserve A `passed`, and leave later scenarios `not_checked`

#### Scenario: Infrastructure fails before scenario start

- **WHEN** profile setup fails before dispatcher starts any selected scenario
- **THEN** profile status is `failed` while every selected scenario remains `not_checked`

### Requirement: Retention guidance

CI and validation documentation SHALL specify no upload or 1-3 day retention for successful fast/targeted runs, compact approximately 7-day retention for successful gates, approximately 14-day retention for failed gates, and long-term retention for release evidence. Cache directories SHALL not be artifact paths.

#### Scenario: Documented retention policy

- **WHEN** contributor or CI operator reads validation guidance
- **THEN** success, failure, release, and cache retention rules are explicit

### Requirement: Canonical acceptance entrypoint

Acceptance SHALL expose exactly one user-facing entrypoint, `scripts/acceptance.sh`, accepting `--profile live-capture|recorder|all` and `--executor local|multipass`. Compatibility wrappers SHALL NOT exist without a concrete external caller or documented compatibility obligation; a self-referential test asserting that a wrapper delegates to canonical tooling does not constitute such an obligation. Development-only deprecated aliases and compatibility-named test runners that only forward to canonical tooling SHALL be removed, and documentation and CI SHALL reference only the canonical entrypoint.

#### Scenario: Single entrypoint

- **WHEN** a contributor or CI step runs acceptance
- **THEN** they invoke `scripts/acceptance.sh` and no deprecated alias or forwarding test-name runner exists in the repository

#### Scenario: Wrapper removal is regression-tested

- **WHEN** the repository is scanned after removal
- **THEN** no tracked file references a removed wrapper path, and tooling tests fail if such a reference reappears

### Requirement: Scenario ownership and dispatch convention

Every acceptance scenario SHALL have exactly one implementation owner resolved by an explicit convention: a `shared/<scenario>.sh` providing `scenario_<name>()` when the behavior is identical across profiles, otherwise a `<profile>/<scenario>.sh` providing `scenario_<profile>_<name>()`. The dispatcher SHALL source the shared implementation when present and otherwise the profile implementation, and SHALL NOT require profile-specific extension modules that merely forward to the shared implementation. The shell dispatcher and the Python runner SHALL validate the same convention so no dynamic scenario implementation can become undiscoverable.

#### Scenario: Shared behavior scenario

- **WHEN** a scenario executes identical behavior under live-capture and recorder
- **THEN** a single shared implementation runs in both profiles with no per-profile extension files

#### Scenario: Profile-specific scenario

- **WHEN** live-capture and recorder runtimes require different scenario assertions
- **THEN** the profile directory provides the implementation and the dispatcher resolves it without a shared stub

#### Scenario: Missing implementation fails loudly

- **WHEN** a selected scenario has neither a shared nor a profile implementation
- **THEN** both dispatcher and runner reject the run before execution with an explicit error naming the scenario and profile

### Requirement: Documentation and installation command truthfulness

Portable validation SHALL run a rootless documentation-command contract that exercises the built binary's documented public surface: `--version`, `--help` (five public commands, internal mechanics hidden), `doctor` (non-destructive with actionable output), `list` empty contract, public record-syntax preflight failure with no mutation when live capture cannot operate (rootless Linux host or non-Linux build), and fixture-recorded public inspect/replay forms where rootless-compatible. The contract SHALL execute commands and assert contracts; it SHALL NOT match README prose. The contract SHALL run in the `cli_docs` targeted group and in CI; README relative links SHALL resolve. Privileged live-capture/recorder acceptance SHALL include a quick-start scenario executing the exact documented command-mode forms (`chronicle doctor`, `chronicle record --name checkout -- ./my-app`, `chronicle list`, `chronicle inspect checkout`, `chronicle replay checkout -- ./my-app`) against a release-built Linux binary, asserting the published recording, catalog entry, inspectable session, replay verification, and no recorded-destination contact. Documentation-only changes SHALL still select no privileged gate.

#### Scenario: Rootless documentation contract

- **WHEN** the portable layer runs the documentation-command contract
- **THEN** version/help/doctor/list contracts pass, record-syntax preflight fails safely with a typed error and no mutation when live capture cannot operate, and README relative links resolve

#### Scenario: Privileged quick-start scenario

- **WHEN** the live-capture or recorder gate runs the quick-start scenario on supported Linux with a release-built binary
- **THEN** the documented record -> list -> inspect -> replay forms succeed and produce retained machine-readable evidence

#### Scenario: Docs-only change stays portable

- **WHEN** only documentation paths change
- **THEN** targeted mode selects the documentation-command contract and no privileged gate, preserving existing evidence-economy rules

### Requirement: Installer validation wiring

The shell installer SHALL be covered by the portable validation layer, not privileged acceptance. `install.sh` SHALL own path selection in `validation/groups.toml` so installer changes select the installer tests in targeted mode without selecting privileged gates. The `cli_docs` targeted case and the CI checks job SHALL run the installer test suite, which SHALL exercise the installer against controlled local fixtures (no real GitHub API dependency) covering at minimum: Linux x86_64 asset selection, Linux aarch64 asset selection, unsupported OS, unsupported architecture, explicit version selection, latest release selection, failed download, checksum mismatch, missing checksum, custom install directory, install directory not on `PATH`, cleanup on failure, and replacement of an existing binary only after successful verification.

#### Scenario: Installer change selects installer tests

- **WHEN** `install.sh` (or installer fixtures/tests) changes
- **THEN** targeted validation runs the installer tests through the `cli_docs` group and selects no privileged gate

#### Scenario: Installer tests pass portable

- **WHEN** the installer test suite runs in CI or targeted mode
- **THEN** the full fixture-driven coverage above passes without network access to GitHub

### Requirement: Release workflow validation

Release-workflow behavior SHALL be covered by portable tests in `scripts/tests/validation/test_release_workflow.py`, wired into fast and release qualification tooling validation. Tests SHALL verify semantic properties rather than snapshotting the complete YAML, including: no pushed `v*` production trigger; dispatch inputs and `main` restriction; strict stable/prerelease version behavior and workspace matching; frozen SHA propagation and explicit checkout refs; canonical `./scripts/validate.sh release` dependency before build/tag; tag target and immutable retry state handling; preparation/checksum ordering; no tag or GitHub Release mutation path for `prepare`; draft creation/reuse, expected-asset verification, `--verify-tag`/equivalent behavior, no generated-notes fallback, prerelease flags, least-privilege permissions, non-cancelling concurrency, and repository-managed release-note prompt behavior.

#### Scenario: Workflow contract is portable

- **WHEN** release workflow or context/state scripts regress
- **THEN** portable validation fails without requiring a real tag, GitHub Release, or privileged environment

#### Scenario: Dry preparation mutation boundary is protected

- **WHEN** a change makes a `prepare` path able to create a tag, create/upload/edit a GitHub Release, or publish a draft
- **THEN** portable validation fails

#### Scenario: Workflow regression detected without privileged gates

- **WHEN** the release workflow, prompt file, or version-context/state script changes
- **THEN** release-workflow validation tests run as part of portable tooling validation and fail on a protected regression without requiring a privileged environment

#### Scenario: Dry-run safety protected

- **WHEN** a regression would allow a manual `prepare` dispatch to publish or would skip checksum verification before tag promotion
- **THEN** release-workflow validation tests fail

### Requirement: Release snapshots prove released state

Release validation SHALL verify more than the existence of a versioned documentation directory. For the requested release snapshot, it SHALL inspect the committed English, Traditional Chinese, and Japanese Markdown content and fail when semantic release-state contradictions claim that no public release exists or that the GitHub Release installer is only future/conditional. The guard SHALL remain a small list of release-state predicates, not an exact prose snapshot or localization equality test, and SHALL run from the release preparation path.

#### Scenario: Stale English snapshot fails

- **WHEN** a committed 0.1 snapshot says that a public release does not exist or that the installer starts with a future public release
- **THEN** release documentation validation fails even though all required snapshot directories and sidebars exist

#### Scenario: Localized stale snapshot fails

- **WHEN** a committed localized 0.1 snapshot contains the equivalent unreleased-state claim
- **THEN** release documentation validation fails and names the affected locale/file

#### Scenario: Released snapshot passes

- **WHEN** all required 0.1 snapshots present the installer as supported and retain only accurate capability limitations
- **THEN** release documentation validation passes without requiring exact prose identity across locales
