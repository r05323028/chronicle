## MODIFIED Requirements

### Requirement: One foreground recorder owns one filesystem domain

Recorder service SHALL run in foreground and own one configured cgroup subtree, private state root, and Chronicle data-domain identity. Data-domain identity SHALL include canonical filesystem identity plus one exact normalized `<domain-lock-root>/.chronicle-domain.lock` path; a physical device number alone does not make two different flock files equivalent. Recorder and every standalone mutator targeting the same configured data domain SHALL resolve and acquire that exact path before subordinate state locks, recovery, capture attachment, name reservation, WAL/catalog/sidecar mutation, ETL, publication, or retention. Read-only commands need no lock.

Public intent commands default domain-lock root to their resolved data directory. When daemon and public commands share a domain, configuration SHALL align both to one exact lock path. A command that is configured for the same data root/domain but resolves a different lock path SHALL fail preflight as incompatible rather than claim exclusion. Operating multiple Chronicle data domains on one physical filesystem with different lock paths remains unsupported and SHALL be documented; device equality SHALL NOT be reported as an acquired conflict. A second owner of the exact configured domain SHALL fail before capture, WAL append, or metadata/catalog mutation. OS process death SHALL release locks; persisted evidence, not PID existence, drives recovery.

#### Scenario: First owner starts

- **WHEN** no live owner holds the exact configured domain lock
- **THEN** recorder acquires it before subordinate state lock, recovery, or capture

#### Scenario: Concurrent owner rejected

- **WHEN** public record, second recorder, or another mutator targets same configured data domain with live owner
- **THEN** second process uses same exact lock path, exits with stable ownership error, and performs no mutation or attachment

#### Scenario: Public and daemon configuration align

- **WHEN** public record and daemon recorder resolve same data-domain identity
- **THEN** both use same exact domain-lock path

#### Scenario: Incompatible lock mapping rejected

- **WHEN** supplied public/daemon configuration names the same Chronicle data root/domain but resolves different lock paths
- **THEN** preflight rejects configuration and does not treat device identity or state lock as mutual exclusion

#### Scenario: Standalone mutator rejected while recorder owns domain

- **WHEN** standalone mutator targets configured data domain owned by live recorder
- **THEN** command exits with stable ownership error before mutation or attachment

#### Scenario: Read-only command

- **WHEN** list or inspect reads a domain owned by recorder
- **THEN** it may produce bounded reconciled output without acquiring or mutating domain lock

#### Scenario: Process dies

- **WHEN** owner process terminates without application cleanup
- **THEN** OS releases lock and next exact-domain owner recovers from persisted evidence rather than PID existence
