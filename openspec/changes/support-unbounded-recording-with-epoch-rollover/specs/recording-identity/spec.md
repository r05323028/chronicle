## MODIFIED Requirements

### Requirement: Stable recording identity

The primary public recording reference SHALL remain `rec_<full-uuid>`, and one public `RecordingId` SHALL identify an entire recording/capture run across every epoch and recorder restart. Public input SHALL accept `rec_<full-uuid>` and, for compatibility only, bare UUID; prefixes SHALL be rejected and resolution exact. Each epoch SHALL additionally have a unique `EpochId` and stable ordinal. Epoch IDs are operational lineage identities, not additional user-visible recordings.

New parent/run metadata and catalog entries SHALL retain the stable `RecordingId`; epoch WAL directories and existing WAL v1 UUID fields SHALL retain their epoch-local identity without rewriting WAL bytes. A finalized epoch's `SessionId` MAY differ from the parent `RecordingId` and SHALL be deterministic from parent ID, epoch identity/ordinal, pipeline version, and verified epoch snapshot. A legacy one-epoch recording remains resolvable when parent and epoch IDs are equal. Public human/JSON output SHALL render the parent ID with `rec_` and expose epoch IDs only in epoch-scoped views.

#### Scenario: Display stable parent form

- **WHEN** a recording with three epochs is listed or inspected
- **THEN** all parent-level output renders one unchanged `rec_<full-uuid>` and epoch details show three distinct ordered epoch identities

#### Scenario: Rollover preserves identity

- **WHEN** an epoch rolls over because of age or bytes
- **THEN** the successor receives a new `EpochId` but the parent `RecordingId`, name, and created timestamp remain unchanged

#### Scenario: Parse bare UUID for compatibility

- **WHEN** a script passes a bare full UUID from the legacy surface
- **THEN** it resolves to the same parent recording

#### Scenario: Short prefix rejected

- **WHEN** a user passes a partial prefix such as `rec_abc12`
- **THEN** resolution fails with exit 3 and an actionable error

#### Scenario: Display canonical form

- **WHEN** a recording is listed or inspected
- **THEN** its ID renders as `rec_<full-uuid>`

### Requirement: Recording catalog

Chronicle SHALL maintain bounded private/advisory parent catalog state under the data directory: atomically updated `catalog.json`, one `recording-intent.json`/run allocation, one parent run manifest, and an ordered epoch catalog for each parent recording. Existing WAL commit markers, epoch-local WAL metadata, canonical session manifests, and session artifacts remain authoritative for their domains; the parent catalog SHALL not promote bytes or invent lineage.

A catalog parent entry SHALL contain stable recording ID, optional name, allocation/start/end facts, run status, aggregate published-session/operation counters, epoch counts, current/last epoch references, and safe child/source summary. Each epoch entry SHALL contain parent ID, unique epoch ID, ordinal, predecessor, relative path, state, commit/ETL/publication/retention summaries, and deletion/compaction proof as applicable. The successor relationship SHALL be derived from the validated next ordinal and predecessor; no conflicting second lineage is accepted. The recording-intent sidecar is written once for the parent before capture attachment, not once per rollover.

Public mutation SHALL acquire the same exact normalized `<domain_lock_root>/.chronicle-domain.lock` path as the recorder lease and hold it through parent/epoch allocation, capture, rollover, ETL, publication, retention, and catalog update. Read-only reconciliation MAY run without the lock and SHALL never mutate while another owner holds it. Canonical/session and recovery-authoritative WAL evidence SHALL win advisory catalog facts; contradictions SHALL surface as `inconsistent`.

Catalog operations SHALL reject symlinks, non-regular entries, path escape, ID/directory mismatch, duplicate parent/epoch identity, broken ordinal/predecessor lineage, oversized input, and unbounded scans. Parent catalog limits and epoch-index compaction SHALL preserve enough first/last identity, digest-chain, tombstone, and aggregate proof to detect missing or forked history.

#### Scenario: Catalog updated across rollover

- **WHEN** one run finalizes epoch 0 and activates epoch 1
- **THEN** one parent entry remains, its epoch index contains both ordered entries, and no second public recording/name is allocated

#### Scenario: Catalog rebuilt from artifacts

- **WHEN** the parent catalog is missing or stale but run/epoch metadata, WAL, and session artifacts exist
- **THEN** bounded reconciliation rebuilds an in-memory parent/epoch view without re-publishing sessions or changing WAL authority

#### Scenario: Catalog is not authority

- **WHEN** catalog, epoch metadata, canonical session, or WAL evidence disagree
- **THEN** commands use canonical facts and recovery-authoritative WAL state, surface `inconsistent`, and never overwrite authoritative artifacts

#### Scenario: Ambiguous legacy grouping

- **WHEN** existing UUID directories lack a valid predecessor/ordinal chain proving one parent
- **THEN** reconciliation keeps them separate or reports inconsistency and never guesses that they are epochs of one recording

#### Scenario: Bounded hostile catalog input

- **WHEN** catalog/sidecar/epoch-index input exceeds limits, uses symlinks, mismatches IDs, or escapes the data directory
- **THEN** reconciliation fails safely with exit 3, follows no link, and publishes/mutates nothing

#### Scenario: Read while domain is owned

- **WHEN** a live recorder owns the exact domain lock and catalog/epoch state is stale
- **THEN** list/inspect may return bounded in-memory reconciliation but does not persist catalog changes

#### Scenario: Catalog updated after publish

- **WHEN** record finalizes and publishes a recording
- **THEN** the catalog gains an entry for the stable ID with name/created/duration/status/session linkage

### Requirement: Recording resolution

Commands SHALL resolve references to stable parent recordings deterministically: `latest` SHALL mean the newest successfully published and inspectable parent by canonical run creation/start timestamp, ties broken by parent recording ID; an exact `rec_<uuid>` or bare UUID SHALL resolve by parent identity; a name SHALL resolve by exact match. Epoch IDs SHALL not be accepted as parent references by the public resolver. Names remain exact UTF-8 values with existing validation/reservation rules. Unresolved or ambiguous references SHALL fail with exit 3 and an actionable safe message.

`list` SHALL show one row per parent recording, including in-progress, recoverable, failed, and inconsistent runs with status. Aggregate sessions/operations count only verified published epoch sessions. `latest` SHALL consider only parents whose selected replayable/published state is inspectable; a live parent with only an unfinalized epoch is not a newer published `latest` candidate.

#### Scenario: Latest resolves parent deterministically

- **WHEN** multiple parent recordings have published epoch sessions
- **THEN** `latest` selects by parent start timestamp and stable parent ID, not by newest epoch directory or rollover time

#### Scenario: Name resolution

- **WHEN** a multi-epoch recording was created with `--name checkout`
- **THEN** `inspect checkout` and `replay checkout -- ...` resolve the same parent recording

#### Scenario: Epoch ID is not parent reference

- **WHEN** a user passes an epoch UUID where a recording reference is required
- **THEN** the command rejects it unless an explicit future epoch-scoped internal API is being used

#### Scenario: Name collision rejected

- **WHEN** a second run tries to claim an existing name
- **THEN** record fails with exit 3 before capture and does not create an epoch

#### Scenario: Reserved name rejected

- **WHEN** a user passes `--name latest` or a case-sensitive `rec_*` name
- **THEN** the application rejects the name with usage exit 2

#### Scenario: Latest resolves deterministically

- **WHEN** multiple published recordings exist with different start times
- **THEN** `latest` resolves to the newest by start time, ties broken by recording ID

## ADDED Requirements

#### Scenario: List long-running recording

- **WHEN** a parent has one active epoch and four finalized epochs
- **THEN** list returns one parent row with aggregate published counts, epoch count, active epoch number, and in-progress status

#### Scenario: Inspect epoch lineage

- **WHEN** a user inspects a parent with multiple epochs
- **THEN** inspect shows parent lifecycle/deadline/stop reason followed by ordered epoch ID/ordinal/state, committed range, ETL/publication state, retention state, and safe warnings

#### Scenario: Raw epoch deleted after retention

- **WHEN** a finalized epoch's source segments are safely deleted after downstream proof
- **THEN** parent list/inspect retains epoch identity, deletion/tombstone proof, published-session association, and honest raw-source-unavailable status

#### Scenario: Restart preserves parent

- **WHEN** the recorder process restarts before the parent reaches a terminal state
- **THEN** the next catalog reconciliation returns the same parent ID and continues epoch order rather than creating a new catalog entry
