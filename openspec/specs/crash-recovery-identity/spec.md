## Purpose

Canonical session identity consistency under crash recovery: deterministic operation identities are deduplicated session-globally so every published session remains structurally valid.

## Requirements

### Requirement: R1: Session-global operation deduplication

`EtlPipeline::finish_reconstructed` MUST deduplicate decoded operations by deterministic identity across the whole session, not per connection. The first occurrence (in connection order) is kept; later occurrences with the same identity are dropped together with their timeline entries. The session's `timeline` contains exactly one entry per kept operation.

#### Scenario: cross-connection duplicate identity

- Given a WAL whose reconstruction yields two connections carrying the same envelope sequence and payload (e.g. an exchange split by the per-connection byte limit after a crashed group commit)
- When `process_envelopes` canonicalizes that WAL
- Then the deterministic operation identity appears in exactly one connection
- And the session's timeline has exactly one entry for that identity
- And `CanonicalSession::validate()` succeeds

#### Scenario: distinct traffic on distinct connections

- Given two connections with identical payloads but distinct WAL sequences
- When `process_envelopes` canonicalizes that WAL
- Then both operations keep distinct deterministic identities and both appear in the session and timeline

### Requirement: R2: Canonical validation remains fail-closed

`CanonicalSession::validate()` MUST remain unchanged: duplicate operation ids, timeline/connection mismatches, and unknown references still fail publication. Deduplication runs before validation so the session reaches `validate()` in a consistent state.

#### Scenario: validation guard preserved

- Given a hand-constructed session that is structurally invalid (duplicate id, timeline mismatch, or unknown reference)
- When `validate()` runs
- Then it returns the corresponding `CanonicalValidationError` exactly as before this change

### Requirement: R3: Identity seed is unchanged

Operation identities MUST remain derived from operation content plus WAL sequence via `deterministic_operation_id`. The change is only the dedup scope.

#### Scenario: deterministic ids unchanged

- Given a WAL with unique envelopes
- When canonicalized before and after this change
- Then the session ids, connection ids, and operation ids are identical

### Requirement: R4: Incremental paths already global

The daemon's incremental batch dedup (`emitted_operation_keys`, persisted across checkpoints) and `IncrementalProcessor::finalize` already deduplicate session-globally. `finish_reconstructed` MUST match them.

#### Scenario: incremental and one-shot agree

- Given the same committed WAL
- When processed through the incremental processor and through `finish_reconstructed`
- Then both outputs contain the same set of operation identities with one timeline entry each
