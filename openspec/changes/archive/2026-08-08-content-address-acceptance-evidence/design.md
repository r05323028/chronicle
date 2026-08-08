## Context

Privileged evidence already has a deterministic source fingerprint, profile scenario coverage, environment metadata, schema version, report status, and artifact manifest. Reuse validation checks most of these, but release mode additionally requires historical evidence commit SHA to equal current `HEAD`, conflating provenance with evidence identity. Legacy profile scripts also use expected SHA to decide release status, while start/end identity remains useful for detecting mutation during a fresh run.

## Goals / Non-Goals

**Goals:**

- Define one content-addressed compatibility rule for normal and release evidence reuse.
- Keep commit/tree/dirty/start/end metadata for audit and same-run integrity.
- Reject incomplete, incompatible, failed, timed-out, or manifest-invalid evidence.
- Ensure fingerprint covers all validation-relevant source, build, acceptance contract, scenario, schema, and configuration inputs.

**Non-Goals:**

- Remove Git provenance fields.
- Permit a dirty, unidentifiable, or mutated current checkout to pass any release request, whether evidence is reused or freshly executed.
- Broaden supported privileged environments.
- Run privileged gates when compatible retained evidence already proves them.

## Decisions

1. **Commit SHA is metadata, not compatibility input.** Reuse and release eligibility use fingerprint, schema, required scenario set, environment compatibility, manifest integrity, required-check completion, and successful status. Alternative—accept equivalent Git tree SHA—still makes unrelated tracked content part of identity and duplicates fingerprint semantics.

2. **Current release candidate integrity remains separate.** Every release request validates a clean, known current commit/tree before reuse or execution, captures start/end identity, and fails when current source changes during the command. Historical evidence may have another commit or tree SHA and may record dirty origin provenance. Alternative—validate only fresh execution—would let a dirty checkout become release-eligible through reuse; alternative—compare historical identity—would restore commit-addressed semantics.

3. **Reuse validation stays centralized.** Shared report/runner compatibility logic decides candidate validity; wrappers pass provenance but do not impose historical SHA equality. Alternative—special release checks in each shell profile—would recreate divergent rules.

4. **Fingerprint contract is strengthened narrowly.** Fingerprint includes gate-owned Rust/eBPF source and build inputs, acceptance runner/scripts/scenario definitions, evidence schema/version behavior, Cargo/toolchain inputs, and validation configuration. Environment remains separately compared because compatible machines may differ in non-source metadata.

5. **Release receipts reference origin evidence.** Reuse writes current release candidate provenance plus original evidence location and manifest digest. `release_eligible` means current checkout integrity and historical compatibility/completeness passed, not SHA equality.

## Risks / Trade-offs

- [Fingerprint omission could reuse stale evidence] → Include acceptance runner/report/schema/scenario/config and gate-owned source/build inputs; test material changes.
- [Over-broad fingerprint could force redundant gates] → Exclude unrelated docs and OpenSpec archive housekeeping unless they define validation contract inputs.
- [Legacy reports may lack new compatibility fields] → Evidence schema v3 and compatibility v2 fail closed unless schema, scenarios, manifest, environment, and checks are complete and compatible.
- [Environment comparison may be too strict or weak] → Preserve existing explicit compatibility function and add mismatch tests.
