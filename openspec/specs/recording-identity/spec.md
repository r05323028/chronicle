## Purpose

Stable recording identity as the primary user-facing abstraction: `rec_<uuid>` IDs, default data-directory resolution, an application-owned recording catalog, and deterministic `latest`/ID/name resolution.

## Requirements

### Requirement: Stable recording identity

The primary public recording reference SHALL remain `rec_<full-uuid>`, and one public `RecordingId` SHALL identify an entire recording/capture run across every epoch and recorder restart. Public input SHALL accept `rec_<full-uuid>` and bare UUID as ordinary accepted input forms; prefixes SHALL be rejected and resolution exact. Each epoch SHALL additionally have a unique `EpochId` and stable ordinal. Epoch IDs are operational lineage identities, not additional user-visible recordings.

New parent/run metadata and catalog entries SHALL retain the stable typed `RecordingId`; epoch WAL directories and existing WAL v1 UUID fields SHALL retain their epoch-local identity without rewriting WAL bytes. A finalized epoch's `SessionId` MAY differ from the parent `RecordingId` and SHALL be deterministic from parent ID, epoch identity/ordinal, pipeline version, verified epoch snapshot, and continuation seed. Public human/JSON output SHALL render the parent ID with `rec_` and expose epoch IDs only in epoch-scoped views; epoch IDs SHALL render as `epoch_<full-uuid>` operationally.

`chronicle-common` owns the two newtypes. `chronicle-wal` owns only its private legacy v1 wire adapter. New checkpoint/session/artifact schemas SHALL carry parent and epoch fields separately; no untyped UUID field may select between them.

#### Scenario: Display stable parent form

- **WHEN** a recording with three epochs is listed or inspected
- **THEN** all parent-level output renders one unchanged `rec_<full-uuid>` and epoch details show three distinct ordered epoch identities

#### Scenario: Rollover preserves identity

- **WHEN** an epoch rolls over because of age or bytes
- **THEN** the successor receives a new `EpochId` but the parent `RecordingId`, name, and created timestamp remain unchanged

#### Scenario: Parse bare UUID

- **WHEN** a script passes a bare full UUID
- **THEN** it resolves to the same parent recording

#### Scenario: Short prefix rejected

- **WHEN** a user passes a partial prefix such as `rec_abc12`
- **THEN** resolution fails with exit 3 and an actionable error

#### Scenario: Display canonical form

- **WHEN** a recording is listed or inspected
- **THEN** its ID renders as `rec_<full-uuid>`

### Requirement: Default data directory

Chronicle SHALL resolve the public data directory in this precedence: explicit global `--data-dir`, configured `AppConfig.data_dir`, `CHRONICLE_DATA_DIR`, then platform default — `$XDG_DATA_HOME/chronicle` (or `~/.local/share/chronicle`) on Linux and `~/Library/Application Support/chronicle` on macOS. Unsupported platforms without explicit/configured/environment path SHALL return a typed unsupported resolution rather than guess. Creation SHALL be lazy/private and SHALL reject symlinked roots or children. `--data-dir` is the only root resolution path.

Session-to-recording association SHALL use explicit `source_provenance.recording_id` / `epoch_id` / `epoch_ordinal` only. Identifier equality (for example `recording_id == session_id`) is not lineage: sessions without explicit provenance are unresolved, are never associated with a recording in catalog/list/latest, and are not resolvable for parent replay. The WAL v1 identity adapter remains the narrow bridge from the unchanged v1 `recording_id` field to a typed `EpochId`; it is not a parent/epoch compatibility mapping.

#### Scenario: Default resolution

- **WHEN** no data-dir override is present on Linux
- **THEN** Chronicle uses `$XDG_DATA_HOME/chronicle` or `~/.local/share/chronicle` and creates it privately on first use

#### Scenario: Environment override

- **WHEN** `CHRONICLE_DATA_DIR` is set and no explicit or configured data directory exists
- **THEN** every public data-dir-aware command uses that directory

#### Scenario: Explicit override

- **WHEN** user passes `--data-dir /custom`
- **THEN** Chronicle uses `/custom` and every command resolves recordings there

### Requirement: Recording catalog

Chronicle SHALL maintain bounded private parent catalog state under the data directory: atomically updated global `catalog.json` for listing/indexing, one `recording-intent.json`/run allocation, one derived parent run manifest, and exactly one authoritative ordered epoch catalog (`epochs.json`) for each parent recording. The epoch catalog SHALL contain stable parent `RecordingId`, typed `EpochId`, ordinal, predecessor/successor links, relative path, state, topology digest, and bounded commit/ETL/publication/retention summaries. Global catalog and run manifest may cache these facts but SHALL NOT participate in topology recovery or select an active epoch.

Existing WAL commit markers, epoch-local WAL metadata, canonical session manifests, checkpoints, and continuation artifacts remain authoritative for their own domains. `epochs.json` is the sole application-level parent-to-epoch topology authority. `recording-intent.json` establishes the parent allocation only. A valid transition journal proves one in-flight change and may complete/rollback it only against catalog and WAL evidence.

A run-summary disagreement SHALL be repaired by regenerating the summary from `epochs.json`, unless its parent ID conflicts. A catalog/transition disagreement SHALL complete or roll back only from exact phase, IDs, ordinals, paths, checksums, and WAL marker evidence. A transition/WAL disagreement SHALL defer to commit markers for bytes and fail closed unless stale derived state can be regenerated with exact identity/digest proof. Missing/corrupt/ambiguous topology SHALL fail closed or be deterministically rebuilt from unambiguous stronger evidence under the lease; it SHALL never guess by mtime or newest path.

Public mutation SHALL acquire the same exact normalized `<domain_lock_root>/.chronicle-domain.lock` path as the recorder lease and hold it through parent/epoch allocation, capture, rollover, ETL, publication, retention, and catalog update. Read-only reconciliation MAY run without the lock and SHALL never mutate while another owner holds it. Catalog operations SHALL reject symlinks, non-regular entries, path escape, ID/directory mismatch, duplicate parent/epoch identity, broken ordinal/predecessor lineage, oversized input, and unbounded scans.

#### Scenario: Catalog updated across rollover

- **WHEN** one run finalizes epoch 0 and activates epoch 1
- **THEN** one parent entry remains, its epoch index contains both ordered entries, and no second public recording/name is allocated

#### Scenario: Catalog rebuilt from artifacts

- **WHEN** the parent catalog is missing or stale but run/epoch metadata, WAL, and session artifacts exist
- **THEN** bounded reconciliation rebuilds an in-memory parent/epoch view without re-publishing sessions or changing WAL authority

#### Scenario: Catalog is not authority

- **WHEN** a derived global catalog/run summary, transition journal, session, or checkpoint disagrees with authoritative `epochs.json` topology or WAL commit markers
- **THEN** recovery rebuilds derived facts from the stronger authority when exact identity/digest proof exists, otherwise surfaces `inconsistent` and never overwrites authoritative artifacts

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

### Requirement: Parent and epoch identity types stay distinct through the WAL v1 boundary

`RecordingId` SHALL mean stable parent recording in application/common/canonical/CLI contracts. `EpochId` SHALL mean one bounded epoch in application/common/catalog/ETL/WAL adapter contracts. `chronicle-wal` SHALL retain `WalV1RecordingIdentity` as the narrow adapter for the unchanged v1 `recording_id` field, and no public API may use that field as an untyped parent/epoch discriminator. Checkpoint/session/artifact schemas SHALL carry typed parent and epoch fields. Equality of parent and epoch UUID bytes SHALL not erase the semantic type distinction.

#### Scenario: Explicit WAL compatibility conversion

- **WHEN** a WAL v1 header exposes a UUID in `recording_id`
- **THEN** only the WAL compatibility adapter maps it to an `EpochId` using validated epoch context and never exposes it as parent `RecordingId`

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
