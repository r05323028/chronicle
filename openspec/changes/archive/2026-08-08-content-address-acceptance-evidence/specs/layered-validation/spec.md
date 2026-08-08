## MODIFIED Requirements

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
