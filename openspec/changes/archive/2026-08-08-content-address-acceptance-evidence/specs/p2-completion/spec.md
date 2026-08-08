## MODIFIED Requirements

### Requirement: Privileged acceptance proves P2 truthfully

Privileged acceptance SHALL execute or compatibly reuse evidence for real process interruption, restart, reboot, quota, corruption, cleanup, replay, and resource-cleanup scenarios through unified profile runner; retain artifacts under `acceptance/<profile>/<fingerprint>/<run-id>/`; verify validation fingerprint and validation-relevant content, compatible evidence schema, required scenario coverage, compatible environment, artifact manifest integrity, and every required check; and fail when any required check failed, is `not_checked`, timed out, or is incomplete. Git commit SHA SHALL be retained as provenance metadata but SHALL NOT by itself invalidate otherwise compatible acceptance evidence or determine release eligibility.

#### Scenario: Development provenance report

- **WHEN** normal privileged acceptance completes, including with uncommitted source
- **THEN** report records commit SHA, tree SHA when known, acceptance fingerprint, dirty state, environment, and run identity
- **AND THEN** status reflects actual scenario results without using commit equality as evidence identity

#### Scenario: Release-grade content identity

- **WHEN** `scripts/acceptance.sh --profile p2 --executor multipass --release` executes or reuses evidence from a clean, identifiable current checkout
- **THEN** report is `release_eligible` only when current source remains unchanged and fingerprint, validation contract, compatible schema and environment, manifest, required scenario coverage, and every required check are complete and passed
- **AND THEN** historical evidence commit and tree SHA may differ from the current checkout

#### Scenario: Release request integrity

- **WHEN** any release request starts
- **THEN** current commit and tree SHALL be known, current working tree SHALL be clean, and start/end fingerprint, commit, tree, dirty state, and identity status SHALL remain unchanged through validation
- **AND THEN** release fails before reused evidence is accepted when current source is dirty or unidentifiable, and fails if current source changes during validation
- **AND THEN** current-checkout integrity does not impose commit or tree equality on historical evidence provenance

#### Scenario: Crash and cleanup evidence

- **WHEN** recorder or cleanup is interrupted at a durable boundary
- **THEN** restart convergence, immutable evidence, protected lineage, process cleanup, cgroup cleanup, and eBPF cleanup are machine-validated

#### Scenario: Compatible evidence reuse

- **WHEN** prior P2 run has matching fingerprint, compatible schema and environment, complete valid manifest, unchanged required scenario set, and all required scenarios and checks passed without timeout or incomplete state
- **THEN** it may satisfy P2 release eligibility or P1 request when P1 scenarios are subset
- **AND THEN** P1 evidence never satisfies P2 and commit equality is not required

#### Scenario: Reboot evidence

- **WHEN** supported Linux VM reboots during recorder operation
- **THEN** post-reboot markers, checkpoint lineage, catalog identity, counters, and WAL digests prove monotonic continuation
