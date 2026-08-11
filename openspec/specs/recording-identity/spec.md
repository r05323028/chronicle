## Purpose

Stable recording identity as the primary user-facing abstraction: `rec_<uuid>` IDs, default data-directory resolution, an application-owned recording catalog, and deterministic `latest`/ID/name resolution.

## Requirements

### Requirement: Stable recording identity

The primary public recording reference SHALL be `rec_<full-uuid>`. Public input SHALL accept `rec_<full-uuid>` and, for compatibility only, bare UUID; prefixes SHALL be rejected and resolution exact. Internal storage SHALL retain `RecordingId(UUID)`, bare-UUID directory names, and `RecordingId == SessionId` unchanged. Public human/JSON output SHALL use `rec_`; hidden legacy output SHALL retain its existing byte-compatible ID representation.

#### Scenario: Display canonical form

- **WHEN** a recording is listed or inspected
- **THEN** its ID renders as `rec_<full-uuid>`

#### Scenario: Parse bare UUID for compatibility

- **WHEN** a script passes a bare UUID from the legacy surface
- **THEN** it resolves to the same recording

#### Scenario: Short prefix rejected

- **WHEN** a user passes a partial prefix such as `rec_abc12`
- **THEN** resolution fails with exit 3 and an actionable error

### Requirement: Default data directory

Chronicle SHALL resolve the public data directory in this precedence: explicit global `--data-dir`, configured `AppConfig.data_dir`, `CHRONICLE_DATA_DIR`, then platform default — `$XDG_DATA_HOME/chronicle` (or `~/.local/share/chronicle`) on Linux and `~/Library/Application Support/chronicle` on macOS. Unsupported platforms without explicit/configured/environment path SHALL return a typed unsupported resolution rather than guess. Creation SHALL be lazy/private and SHALL reject symlinked roots or children. Legacy `--root` behavior remains exact and is not a data-directory alias.

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

Chronicle SHALL maintain additive private/advisory v1 artifacts under the data directory: atomically updated `catalog.json` plus `recordings/<bare-uuid>/recording-intent.json`. Existing WAL, recording-metadata, canonical-session, and session-manifest formats remain authoritative and unchanged. Public mutation SHALL acquire the same exact normalized `<domain_lock_root>/.chronicle-domain.lock` path as `RecorderLease`; filesystem device equality alone is not lock equivalence. Default public lock root is data directory. Daemon/public configurations for same Chronicle data root/domain SHALL resolve one exact path; mismatches SHALL fail preflight. Multiple differently locked Chronicle domains on one physical filesystem remain unsupported; device equality alone SHALL NOT be reported as exclusion. Lock is acquired before name reservation or durable recording allocation and held through capture, ETL, publication, and catalog update.

The sidecar SHALL contain ID, optional validated name, and allocation timestamp and SHALL be persisted after preflight/scope creation but before capture attachment. Reconciliation SHALL combine bounded sidecars, recovery-authoritative WAL metadata, and bounded canonical summaries without ETL or re-publication. Canonical evidence wins identity/start/end/operation facts; WAL evidence determines recoverability; contradictions surface as `inconsistent`. Effective status values SHALL be `in_progress`, `recoverable`, `published`, `failed`, or `inconsistent`.

Catalog operations SHALL reject symlinks, non-regular entries, path escape, and ID/directory mismatch; scan immediate children only; cap catalog JSON at 16 MiB/10,000 entries and each sidecar at 4 KiB. Reconciliation builds a read-only in-memory view without lock; persisted rebuild requires the exact domain lock. While another recorder owns it, reads remain available from that view and no catalog file is mutated.

#### Scenario: Catalog updated after publish

- **WHEN** record finalizes and publishes a recording
- **THEN** the catalog gains an entry for the stable ID with name/created/duration/status/session linkage

#### Scenario: Catalog rebuilt from artifacts

- **WHEN** the catalog is missing or stale but WAL and session artifacts exist
- **THEN** reconciliation rebuilds the catalog from existing artifacts without re-publishing or changing WAL/session authority

#### Scenario: Catalog is not authority

- **WHEN** catalog and canonical/WAL evidence disagree
- **THEN** commands use canonical facts and recovery-authoritative WAL state, surface `inconsistent`, and never overwrite authoritative artifacts

#### Scenario: Bounded hostile catalog input

- **WHEN** catalog/sidecar input exceeds limits, uses symlinks, mismatches its ID directory, or escapes the data directory
- **THEN** reconciliation fails safely with exit 3, follows no link, and publishes/mutates nothing

#### Scenario: Read while domain is owned

- **WHEN** daemon owns the exact domain lock and catalog is missing or stale
- **THEN** list/inspect may use bounded in-memory reconciliation but do not persist catalog changes

### Requirement: Recording resolution

Commands SHALL resolve recording references deterministically: `latest` SHALL mean the newest successfully published and inspectable recording by canonical creation/start timestamp, tie-broken lexicographically by recording ID; an exact `rec_<uuid>` or bare UUID SHALL resolve by identity; a name SHALL resolve by exact match. Names SHALL be optional exact-match UTF-8 of 1–128 bytes with no control characters, SHALL be unique, SHALL reject `latest` and case-sensitive `rec_*` with usage exit 2, and collisions SHALL exit 3 before capture rather than silently rebound. Unresolved references SHALL fail with exit 3 and an actionable message. `list` SHALL show every catalog entry, including in-progress and failed recordings with their STATUS; `latest` SHALL consider only successfully published and inspectable entries.

#### Scenario: Latest resolves deterministically

- **WHEN** multiple published recordings exist with different start times
- **THEN** `latest` resolves to the newest by start time, ties broken by recording ID

#### Scenario: Name resolution

- **WHEN** a recording was created with `--name checkout`
- **THEN** `inspect checkout` and `replay checkout -- ...` resolve to that recording

#### Scenario: Name collision rejected

- **WHEN** a second recording tries to claim an existing name
- **THEN** record fails with exit 3 and an actionable collision error before capture

#### Scenario: Reserved name rejected

- **WHEN** a user passes `--name latest` or `--name rec_abc`
- **THEN** Clap/application rejects the name as reserved with usage exit 2
