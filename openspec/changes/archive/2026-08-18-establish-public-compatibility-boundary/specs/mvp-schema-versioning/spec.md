## REMOVED Requirements

### Requirement: Internal artifact domains have one current model

**Reason**: Replaced by `public-compatibility-boundary`, which establishes the released 0.1 compatibility line while leaving explicitly advisory and private implementation details evolvable.

**Migration**: Apply the public 0.1 frozen-contract, reader/writer, unsupported-version, migration, and deprecation requirements. Existing domain-specific specs remain authoritative for WAL, capture, canonical, storage, CLI, and replay behavior.

### Requirement: Readers have no obsolete compatibility dispatch

**Reason**: Its pre-release framing is retired; fail-closed version behavior is now part of the public compatibility contract and domain-specific specifications.

**Migration**: Keep unsupported-version rejection and no silent fallback. Introduce any public compatibility reader or migration only through an explicit versioned OpenSpec change.

### Requirement: No pre-release CLI compatibility surface and explicit lineage

**Reason**: The released CLI and explicit-provenance rules are owned by `user-intent-cli`, `runnable-http-cli`, `recording-identity`, and `safe-local-http-replay`; the capability name must no longer describe an unreleased MVP policy.

**Migration**: Preserve the current five-command public surface, hidden `internal` boundary, explicit lineage, and no unreleased compatibility aliases through those active specifications.

### Requirement: Repository artifacts migrate atomically

**Reason**: Repository synchronization remains a general engineering rule, not an active pre-release schema capability.

**Migration**: Update fixtures, tests, documentation, and checked-in artifacts with every contract change, and validate unsupported-version rejection explicitly.

### Requirement: Compatibility freeze requires explicit change

**Reason**: The freeze gate is complete for 0.1 and is replaced by the released public compatibility process.

**Migration**: Use `public-compatibility-boundary` for frozen scope, compatibility expectations, unsupported-version behavior, migrations, deprecations, and future incompatible changes.
