## MODIFIED Requirements

### Requirement: Privileged acceptance proves P2 truthfully

P2 SHALL execute or compatibly reuse all required lower-layer, smoke, acceptance, E2E, and privileged evidence selected by milestone contract. Privileged P2 assertions SHALL be limited to behavior whose truth depends on real supported Linux/kernel environment, including applicable real process interruption/restart/reboot, eBPF capture, cgroup/process attribution, quota resources, kernel cleanup, and supported-platform compatibility. Portable WAL, ETL, corruption, replay, CLI, parser, repository-validation, and equivalent deterministic assertions SHALL execute in their lowest conclusive functional layer and SHALL be referenced as P2 prerequisites rather than rerun inside privileged scenarios.

P2 privileged execution SHALL use unified preflight/executor/evidence contracts; retain artifacts under `acceptance/<profile>/<fingerprint>/<run-id>/` or its versioned successor; verify validation fingerprint and validation-relevant content, compatible evidence schema, required classified test/scenario coverage, compatible supported environment, artifact manifest integrity, and every required check; and fail gate completion when any required check failed, is `not_checked`, timed out, unsupported, infrastructure-incomplete, or otherwise incomplete. Every destructive privileged scenario SHALL establish explicit preconditions and SHALL NOT pass until asynchronous postconditions converge, including removal of scenario-owned pressure, pause/control markers, temporary mounts/loop devices, stale ownership, pending checkpoint/cleanup state, or unstable service state as applicable. Git commit SHA SHALL be retained as provenance metadata but SHALL NOT by itself invalidate otherwise compatible acceptance evidence or determine release eligibility.

#### Scenario: Development provenance report

- **WHEN** normal P2 validation completes, including with uncommitted source
- **THEN** report records commit SHA, tree SHA when known, acceptance fingerprint, dirty state, environment, run identity, classified test coverage, preflight/infrastructure outcome, and actual test/scenario execution status
- **AND THEN** status reflects actual results without using commit equality as evidence identity

#### Scenario: Release-grade content identity

- **WHEN** `scripts/acceptance.sh --profile p2 --executor multipass --release` or its versioned successor executes or reuses evidence from clean identifiable current checkout
- **THEN** report is `release_eligible` only when current source remains unchanged and fingerprint, validation contract, compatible schema and supported environment, manifest, required classified coverage, every required check, and every privileged scenario postcondition are complete and passed
- **AND THEN** historical evidence commit and tree SHA may differ from current checkout

#### Scenario: Release request integrity

- **WHEN** any P2 release request starts
- **THEN** current checkout fingerprint, commit, tree, dirty state, and identity status SHALL remain unchanged through validation
- **AND THEN** release fails before reused evidence is accepted when current source is dirty or unidentifiable, and fails if current source changes during validation
- **AND THEN** current-checkout integrity does not impose commit or tree equality on historical evidence provenance

#### Scenario: Unsupported privileged environment

- **WHEN** P2 preflight cannot prove supported Ubuntu, kernel, architecture, privileges/capabilities, cgroup v2, BTF, bpffs, tooling, or executor requirements
- **THEN** result is `unsupported_environment` or `infrastructure_error`, no product assertion failure is claimed, and P2 remains unsatisfied

#### Scenario: Crash and cleanup evidence

- **WHEN** recorder or cleanup is interrupted at durable boundary in supported privileged environment
- **THEN** privileged proof covers real process/kernel lifecycle and cleanup while portable recovery/format matrices remain lower-layer prerequisites
- **AND THEN** restart convergence, immutable evidence, protected lineage, process cleanup, cgroup cleanup, eBPF cleanup, absence of pause/control files, and stable service ownership are machine-validated before scenario passes

#### Scenario: Quota pressure convergence

- **WHEN** isolated quota or minimum-free pressure requiring real mount/loop/system service resources is introduced and then removed
- **THEN** acceptance waits for pressure entry and recovery conditions and does not pass until recorder state is stable, filler is absent, unit is stopped, mount is absent, loop device is detached, and scenario cgroup is removed

#### Scenario: Checkpoint recovery convergence

- **WHEN** recorder is killed at publication/checkpoint boundary in supported privileged runtime
- **THEN** privileged test proves real process interruption/restart and waits for fresh recorder readiness and consistent checkpoint/output lineage with no pending checkpoint artifact
- **AND THEN** exhaustive checkpoint transaction/fault correctness remains owned by portable integration coverage

#### Scenario: Compatible evidence reuse

- **WHEN** prior P2 run has matching fingerprint, compatible schema and supported environment, complete valid manifest, unchanged required classified test/scenario set, and all required tests/scenarios passed without timeout, unsupported/infrastructure state, incomplete state, or unresolved postcondition
- **THEN** it may satisfy P2 release eligibility or P1 request when P1 obligations are subset
- **AND THEN** P1 evidence never satisfies P2 and commit equality is not required

#### Scenario: Reboot evidence

- **WHEN** supported Linux VM reboots during recorder operation
- **THEN** pre-reboot handoff proves clean stop, post-reboot fresh readiness is observed, and checkpoint lineage, catalog identity, counters, and WAL digests prove monotonic continuation

#### Scenario: Portable corruption proof

- **WHEN** corruption behavior can be proved by mutating deterministic copied WAL and invoking ETL rootlessly
- **THEN** exhaustive fail-closed/preservation coverage runs as integration or acceptance prerequisite rather than consuming privileged runtime
