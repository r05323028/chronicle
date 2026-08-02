## ADDED Requirements

### Requirement: Size-and-age segment rotation

P2 writer SHALL preserve existing size rotation and additionally rotate a non-empty active segment when configured maximum age expires. Default size SHALL remain 256 MiB with existing 16 MiB through 4 GiB range. Default age SHALL be five minutes with documented bounded configuration; age SHALL use monotonic elapsed time for trigger and persisted wall time only for observability. Rotation check SHALL run on append and periodic writer tick so low-traffic segments eventually seal. Empty segment SHALL NOT rotate only because age elapsed.

Age rotation SHALL use existing order: commit/sync all current-segment data with recovery-authoritative marker, seal segment, persist integrity/lifecycle metadata, create/sync/publish next header, then append. Rotation SHALL not alter 64-byte segment header, 48-byte envelope frame, 76-byte commit marker, one-sync authority, sequence order, or acknowledgement semantics.

#### Scenario: Size rotation remains compatible

- **WHEN** next complete frame plus final-marker reservation exceeds configured segment size
- **THEN** existing size-triggered commit/sync/rotation behavior occurs unchanged

#### Scenario: Age rotation under low traffic

- **WHEN** non-empty segment reaches configured age without another event
- **THEN** periodic writer tick commits pending data, seals segment, and opens successor

#### Scenario: Empty segment age

- **WHEN** empty active segment reaches age threshold
- **THEN** writer does not create endless empty segments

#### Scenario: Crash during age rotation

- **WHEN** process fails at any age-rotation write/sync/publish step
- **THEN** recovery recognizes only existing validated commit/segment authority and resumes or fails by existing rules

### Requirement: Versioned crash-safe WAL manifest

Each recording epoch SHALL have layout:

```text
wal/
  manifest.json
  segments/
    <20-digit-first-sequence>.chwal
```

Manifest SHALL be private versioned JSON containing recording/epoch identity, generation, segment policy, current active segment, final recovery-authoritative marker identity, and ordered entries for every active/sealed/retention-transition segment. Published segment entry SHALL contain ordinal, exact file name, first/last sequence, first/last commit-marker location, creation/seal times, physical length, SHA-256 of sealed bytes, lifecycle state, ETL checkpoint reference, retention eligibility time, and deletion/tombstone state where applicable.

Manifest replacement SHALL use private temp write, file sync, atomic rename, then directory sync. It SHALL be an index/lifecycle journal, not authority for committed frames. Manifest SHALL never promote frame beyond fully validated in-WAL commit marker, reconstruct acknowledgement delivery, or override segment/header/envelope corruption. Unknown version, duplicate identity/range, impossible transition, or manifest claim ahead of validated WAL SHALL fail closed.

#### Scenario: Manifest update succeeds

- **WHEN** segment seals after successful marker sync
- **THEN** atomic manifest revision records exact sealed length/digest/range and predecessor revision

#### Scenario: Crash before manifest rename

- **WHEN** valid sealed segment exists but new manifest revision was not published
- **THEN** recovery derives segment facts from validated bytes and repairs manifest without changing WAL

#### Scenario: Manifest leads WAL

- **WHEN** manifest claims segment/range not supported by validated bytes
- **THEN** recovery reports contradiction and performs no capture/cleanup

#### Scenario: Manifest missing

- **WHEN** segment set is complete and valid but manifest is absent
- **THEN** recovery deterministically rebuilds manifest before readiness

### Requirement: Explicit segment lifecycle

Segment lifecycle SHALL be `active -> sealed -> processed -> retention_eligible -> deleting -> deleted`, with `quarantined` terminal alternative for corruption or contradiction. Exactly one segment MAY be active per epoch. Only recovery-authoritative marker ending at or before sealed length MAY establish sealed range. Processed transition SHALL require durable ETL checkpoint that names exact segment digest/range and every required immutable publication. Retention eligibility SHALL additionally require configured retention policy. No transition SHALL regress or skip proof requirements.

Deleted entries SHALL remain bounded tombstones in manifest/epoch index until entire epoch retention record expires. Tombstone SHALL retain identity, range, digest, checkpoint/publication references, deletion reason/time, and cleanup transaction ID but no captured bytes. Quarantined segments SHALL never be auto-deleted.

#### Scenario: Segment processed

- **WHEN** ETL checkpoint durably references sealed segment digest/range and required outputs
- **THEN** manifest may transition sealed segment to processed

#### Scenario: Retention age alone expires

- **WHEN** retention time expires before ETL/output proof
- **THEN** segment remains sealed and cannot become retention eligible

#### Scenario: Corrupt sealed segment

- **WHEN** sealed segment digest or committed content fails validation
- **THEN** segment is quarantined/preserved and automatic cleanup stops for affected epoch

### Requirement: Checkpoint-gated cleanup transaction

Cleanup SHALL delete only sealed segments from stopped/finalized epochs or prefixes explicitly supported by durable incremental ETL state; P2 default SHALL clean finalized epochs and SHALL NOT require active-epoch prefix deletion. Eligibility SHALL require: validated recovery-authoritative committed range; matching sealed digest; durable deterministic incremental output publication; durable ETL checkpoint naming input/output lineage; for finalized epoch, verified existing Canonical Session v1 plus manifest/payload integrity from `FilesystemSessionStore`; no unresolved corruption/uncertainty; retention policy authorization; and quota safety reserve for transaction metadata.

Cleanup ordering SHALL be: persist `deleting` intent with transaction ID and exact proof references; atomically rename segment to private trash name in same filesystem and directory-sync; persist `deleted` tombstone/revision and directory-sync; then unlink trash and directory-sync. Crash recovery SHALL complete or roll back using manifest intent, checkpoint, and file presence. It SHALL never infer eligibility from missing file, wall-clock age alone, directory listing order, or object-store upload attempt.

#### Scenario: Crash before rename

- **WHEN** process crashes after deleting intent is durable but segment still has published name
- **THEN** recovery revalidates proof and either completes transaction or returns segment to eligible state

#### Scenario: Crash after rename

- **WHEN** process crashes with segment in trash name and durable deleting intent
- **THEN** recovery verifies identity/digest and completes tombstone/unlink without treating segment as unexplained loss

#### Scenario: Missing without intent

- **WHEN** expected segment file is absent and no valid deletion tombstone/transaction proves removal
- **THEN** recovery fails with missing committed data and does not continue

### Requirement: Retention policy is explicit and conservative

Recorder configuration SHALL declare retention mode and bounds. P2 SHALL support `retain` and `delete_after` policy for local WAL; implicit deletion SHALL NOT occur. `delete_after` SHALL specify minimum age and MAY specify maximum retained bytes, but both remain subordinate to checkpoint-gated eligibility. Policy changes SHALL be recorded by configuration digest and SHALL NOT retroactively authorize cleanup until next successful reconciliation.

Canonical outputs, checkpoints, manifests, and tombstones SHALL have separate retention classes from source WAL. Deleting source WAL SHALL preserve provenance that source was intentionally expired and SHALL not claim raw evidence remains available. Failed, quarantined, unprocessed, uncheckpointed, or policy-unexpired segments SHALL not be deleted automatically.

#### Scenario: Retain policy

- **WHEN** segment is processed and quota pressure exists under retain mode
- **THEN** recorder does not delete it and may stop capture at quota boundary

#### Scenario: Delete-after policy

- **WHEN** segment is processed, unambiguous, older than configured minimum, and cleanup proof passes
- **THEN** recorder may perform cleanup transaction and retain tombstone

#### Scenario: Failed epoch

- **WHEN** epoch ends failed with valid committed prefix and unresolved uncertain tail
- **THEN** automatic cleanup preserves its WAL until operator resolves or policy explicitly covers verified data and uncertainty handling

### Requirement: Global quota and minimum-free-space protection

Continuous recorder SHALL enforce configured physical quota domain per filesystem while retaining existing per-epoch 4 GiB hard maximum. P2 supports one active Chronicle mutating owner per filesystem domain. Recorder lease ownership SHALL cover the domain, not only the recorder process; multiple recorder instances sharing the same WAL/store filesystem are unsupported. Standalone mutating commands SHALL reject execution when a live recorder owns the domain, while read-only commands may continue. Different cgroup scopes requiring independent recorder instances SHALL use isolated filesystem domains.

Within one owner, one synchronized reservation authority SHALL account for active/sealed WAL, manifests/checkpoints, RecordingStore objects, final Canonical Session staging/objects when co-located, temporary files, and cleanup trash peak. This is not a cross-process quota ledger. Each distinct filesystem root SHALL have independent configured quota and minimum-free-space reserve. Admission SHALL reserve complete next frame, final marker, required segment/epoch header, manifest/checkpoint update, concurrent ETL/session publication peak, and temp/trash bytes before write or rollover; separate roots SHALL not borrow one another's reserve.

At pressure threshold recorder SHALL first run only already-eligible cleanup. It SHALL never delete unprocessed/unverified/unexpired data to make space. If reservation still fails, it SHALL stop intake, persist typed quota-pressure loss/failure evidence, finalize current authoritative prefix where possible, enter failed/not-ready, and rely on supervisor/operator recovery. It SHALL not loop-roll empty epochs or continue capture without durable capacity.

#### Scenario: Eligible cleanup restores headroom

- **WHEN** reservation fails but processed retention-eligible segments can be safely deleted
- **THEN** cleanup completes before new admission and recorder continues within quota

#### Scenario: No eligible data

- **WHEN** quota/minimum-free-space reservation fails and no segment is eligible
- **THEN** recorder stops rather than delete protected evidence

#### Scenario: Temporary peak accounting

- **WHEN** cleanup, manifest, incremental output, or final session publication requires temporary bytes on WAL filesystem
- **THEN** synchronized quota calculation includes concurrent peak bytes and never exceeds configured physical bound

#### Scenario: Separate store filesystem

- **WHEN** canonical/store root resides on different filesystem from active WAL
- **THEN** each filesystem enforces independent quota/minimum-free reserve and failure in store domain cannot consume WAL reserve

### Requirement: Lifecycle indexes remain bounded

Recorder SHALL enforce configured maximum byte and entry bounds for `epochs.json`, epoch lineage history, deletion tombstones, and retention metadata. Startup recovery, readiness checks, and status queries MUST NOT scan unbounded historical epochs. An epoch is eligible for compaction only after it is finalized, retained according to policy, cleanup is complete, and all deletion tombstones are persisted.

Eligible history SHALL compact into a deterministic bounded lineage anchor preserving first retained epoch identity, last compacted epoch identity, predecessor relationship, digest-chain root/tip, cleanup summary, and range/count data sufficient to detect missing committed evidence, digest mismatch, lineage fork, incomplete cleanup, or conflicting/incomplete compaction. Each compaction candidate SHALL carry monotonic generation, unique transaction ID, source-index revision/digest, candidate digest, and anchor data; candidate digest SHALL cover anchor, active epochs, tombstone summary, and source metadata. The representation MAY retain active epochs separately:

```text
compacted-lineage-anchor:
  first_retained_epoch
  last_compacted_epoch
  predecessor_epoch
  digest_chain_root
  digest_chain_tip
  cleanup_summary
active_epochs:
  epoch-100001
  epoch-100002
```

Compaction SHALL use private temp-write, file sync, atomic rename, and directory sync. Crash recovery SHALL validate old and candidate indexes independently, install a candidate only when its source revision/digest matches the old authoritative index and its candidate digest/anchor recompute exactly, and otherwise preserve old index with stable contradiction evidence. If protected history cannot fit after legal compaction, recorder SHALL stop new admission/epoch creation with bounded metadata-limit failure evidence; it SHALL not delete protected lineage or exceed configured bounds.

#### Scenario: Multi-year history compaction

- **WHEN** synthetic multi-year finalized epoch history exceeds configured byte or entry limit
- **THEN** compaction reduces index size within hard bounds while preserving required contradiction-detection lineage

#### Scenario: Crash during compaction

- **WHEN** process crashes before or after compaction rename/sync
- **THEN** recovery selects old or complete new index deterministically and never loses required tombstone/lineage proof

#### Scenario: Lineage contradiction after compaction

- **WHEN** compacted summary conflicts with committed evidence, digest, predecessor, or cleanup proof
- **THEN** recovery reports stable contradiction and fails closed without scanning unrelated historical epochs

#### Scenario: Stale compaction candidate

- **WHEN** candidate source revision/digest is stale or candidate digest conflicts with its source index
- **THEN** recovery rejects candidate, preserves old authoritative index, and does not install partial or guessed lineage

### Requirement: Continuous recovery reconciles manifest, files, and checkpoints

Startup recovery SHALL enumerate recognized files deterministically, validate all non-deleted segment headers/envelopes/markers and sealed digests, reconcile manifest revision and deletion transactions, compare ETL checkpoints/publications, and derive exact active/sealed/processed/eligible states. Read-only validation of live active segment SHALL stop at stable recovery-authoritative marker snapshot and SHALL treat concurrently growing/partial suffix as mutable uncertainty, not corruption; only exclusive startup recovery MAY repair/truncate existing allowed final tail.

Manifest rebuild SHALL preserve existing segment ordinals, sequence ranges, commit authority, and retention tombstones. Corruption before or at committed boundary, unexplained missing segment, digest mismatch, impossible overlapping range, or checkpoint beyond authority SHALL fail closed and preserve available bytes.

#### Scenario: Live ETL sees partial suffix

- **WHEN** reader snapshots active segment while writer appends incomplete later frame
- **THEN** reader processes only prior validated marker boundary and neither repairs nor reports committed corruption

#### Scenario: Startup with interrupted cleanup

- **WHEN** manifest and filesystem show valid deleting transaction
- **THEN** exclusive recovery deterministically completes or rolls it back before readiness

#### Scenario: Repeated reconciliation

- **WHEN** recovery runs twice over unchanged state after one allowed repair
- **THEN** second run produces identical manifest/checkpoint states with no additional mutation

### Requirement: WAL lifecycle acceptance

Automated tests SHALL cover size/age rotation, manifest atomicity/rebuild/contradiction, every segment transition, quota reservation, retention eligibility, cleanup crash points, corruption, live snapshot behavior, and deterministic recovery. Privileged acceptance SHALL retain machine-readable manifest, segment/checkpoint digests, quota/cleanup report, and environment/commit identity for real continuous capture across multiple rotations and restart.

#### Scenario: Rotation and recovery proof

- **WHEN** acceptance forces size and age rotation then kills recorder at selected rotation step
- **THEN** restart recovers final authoritative marker, reconciles manifest, and continues without missing or duplicated committed sequence

#### Scenario: Corruption detection proof

- **WHEN** acceptance corrupts sealed segment before cleanup eligibility
- **THEN** recorder quarantines/preserves evidence, blocks cleanup/readiness for affected lineage, and reports stable code
