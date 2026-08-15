## MODIFIED Requirements

### Requirement: Size-and-age segment rotation

The recorder WAL writer SHALL preserve existing size rotation and rotate a non-empty active segment when configured maximum age expires. Rotation bounds apply inside one epoch; a segment rotation SHALL never end the parent recording or allocate a new epoch. Default size SHALL remain 256 MiB with the existing 16 MiB through 4 GiB range. Default age SHALL remain the documented bounded value; age triggers SHALL use monotonic elapsed time. Rotation checks SHALL run on append and periodic writer tick so low-traffic segments eventually seal. Empty segments SHALL NOT rotate only because age elapsed.

Age/size rotation SHALL use existing order: commit/sync current-segment data with recovery-authoritative marker, seal the segment, persist integrity/lifecycle metadata, create/sync/publish the next header, then append. It SHALL not alter 64-byte segment headers, 48-byte envelope frames, 76-byte commit markers, one-sync authority, sequence order, or acknowledgement semantics.

#### Scenario: Size rotation remains compatible

- **WHEN** the next complete frame plus final-marker reservation exceeds configured segment size inside an active epoch
- **THEN** existing size-triggered commit/sync/rotation behavior occurs unchanged and the parent/epoch continue

#### Scenario: Age rotation under low traffic

- **WHEN** a non-empty segment reaches configured age without another event
- **THEN** a periodic writer tick commits pending data, seals that segment, and opens the next segment in the same epoch

#### Scenario: Empty segment age

- **WHEN** an empty active segment reaches its age threshold
- **THEN** the writer does not create endless empty segments or epochs

#### Scenario: Crash during segment rotation

- **WHEN** the process fails at an age/size rotation write, sync, or publish step
- **THEN** recovery recognizes only existing validated WAL authority and resumes or fails by existing rules without ending the parent solely for segment age

#### Scenario: Crash during age rotation

- **WHEN** process fails at any age-rotation write/sync/publish step
- **THEN** recovery recognizes only existing validated commit/segment authority and resumes or fails by existing rules

### Requirement: Physical total-WAL hard cap and reservation

`max_wal_bytes` SHALL remain a physical hard cap for one epoch, defaulting to and never exceeding 4 GiB; a configured lower value SHALL be at least the selected segment size. It covers every byte under that epoch's `segments/`, including headers, envelopes, and temporary publication files, while metadata/lock/ETL reports outside `segments/` remain separate. Before every non-marker frame, the writer SHALL reserve/check current-segment and epoch capacity for the complete frame plus final commit-marker frame, and rotation SHALL reserve the next segment header and peak temporary bytes. Writer SHALL never write a partial frame or exceed the epoch cap.

Reaching this cap SHALL trigger epoch rollover when the parent run is still active and a successor reservation/creation can succeed. It SHALL not emit a public recording-wide `wal_size_limit` stop. If successor resources cannot be reserved or created, the coordinator SHALL stop intake, preserve the old authoritative prefix, record typed quota/rollover failure and exact admitted-observation loss evidence, and fail the parent run. A legacy hidden one-shot adapter may retain old `wal_size_limit` behavior only under its compatibility contract.

#### Scenario: Epoch cap reached with successor capacity

- **WHEN** the next complete frame cannot fit in the active epoch while a successor can be reserved
- **THEN** the active epoch is finalized and one successor is linked; the parent continues without discarding accepted observations as a normal WAL-limit stop

#### Scenario: Limit reached with empty queue

- **WHEN** an epoch capacity check reaches its cap with no admitted event remaining
- **THEN** the epoch is sealed and rollover is attempted without creating an empty successor until new evidence or an operational transition requires one

#### Scenario: Queue partially writable at boundary

- **WHEN** only a prefix of admitted observations fits before the old epoch marker
- **THEN** the fitting prefix is committed, every remaining ordinal is assigned to the successor after its marker or recorded once as `epoch_rollover_failure`, and the parent does not claim silent loss

#### Scenario: No successor capacity

- **WHEN** the successor reservation fails because quota/minimum-free headroom is unavailable
- **THEN** intake stops, protected evidence remains, typed quota/rollover failure is persisted, and no empty-epoch loop occurs

#### Scenario: Exact epoch hard-cap boundary

- **WHEN** the final old-epoch marker ends exactly at its configured physical limit
- **THEN** that epoch remains valid, measured bytes never exceed its cap, and the parent may continue in a separately reserved successor

#### Scenario: Oversized single record

- **WHEN** one framed record exceeds format or segment maximum independent of remaining epoch capacity
- **THEN** the writer returns typed invalid-record failure and creates no ambiguous partial frame

#### Scenario: Final commit sync failure

- **WHEN** old-epoch final marker write/flush/`fdatasync` fails during rollover
- **THEN** the old committed prefix remains authoritative, transition recovery preserves evidence, and the parent fails visibly without pretending rollover succeeded

#### Scenario: Writable queued prefix

- **WHEN** all currently queued frames fit while preserving final marker
- **THEN** writer writes and commits that prefix without exceeding cap

#### Scenario: Queue partially writable

- **WHEN** only prefix of accepted queue fits with marker reservation
- **THEN** writer commits fitting prefix and counts exact remainder records/bytes as WAL-limit queue discard

#### Scenario: Rotation required at limit

- **WHEN** next data frame plus same-segment final marker requires new segment
- **THEN** current batch commits first and total reservation includes next header/peak temporary bytes plus data frame plus marker before creating temporary file

#### Scenario: No room for terminal loss marker

- **WHEN** remaining capacity fits required commit marker but not optional `TerminalWalLoss` frame plus marker
- **THEN** writer commits written prefix, emits no partial terminal frame, and metadata/final summary preserve exact typed discard evidence

#### Scenario: No room for another data record

- **WHEN** next complete frame plus marker reservation exceeds remaining bytes
- **THEN** writer writes none of frame and initiates WAL-limit stop

#### Scenario: Exact hard-cap boundary

- **WHEN** final marker ends exactly at configured physical byte limit
- **THEN** finalization succeeds and measured bytes under `segments/` equal but never exceed limit

#### Scenario: Concurrent duration and WAL limits

- **WHEN** duration expiry and writer reservation failure are observed in same shutdown cycle
- **THEN** `wal_size_limit` takes precedence because physical-cap enforcement caused accepted queue discard

### Requirement: Retention policy is explicit and conservative

Recorder configuration SHALL declare retention mode and bounds for epoch-local raw WAL and parent/derived artifacts. The recorder SHALL support `retain` and `delete_after`; implicit deletion SHALL NOT occur. `delete_after` minimum age and optional maximum retained bytes remain subordinate to checkpoint-gated eligibility and parent lineage proof. Policy changes SHALL be recorded by configuration digest and SHALL not retroactively authorize cleanup before reconciliation.

Canonical epoch sessions, parent lineage, checkpoints, manifests, tombstones, and immutable outputs SHALL have separate retention classes from source WAL. Deleting raw epoch WAL SHALL preserve parent/epoch provenance and state that raw evidence was intentionally expired; it SHALL not claim raw evidence remains available. Failed, corrupt, unprocessed, uncheckpointed, uncertain, active, or policy-unexpired epochs SHALL not be deleted automatically.

#### Scenario: Retain policy under sustained pressure

- **WHEN** finalized epochs are processed but `retain` policy leaves no eligible raw data to delete
- **THEN** the recorder preserves epochs and may stop/fail new admission at quota pressure; it does not silently lose lineage

#### Scenario: Delete-after epoch

- **WHEN** an epoch is finalized, its committed WAL and digest are verified, ETL checkpoint/output and final epoch session are durable, policy age/bytes permit deletion, and cleanup proof passes
- **THEN** cleanup may delete eligible raw segments through the existing crash-safe transaction and retain tombstone/provenance

#### Scenario: Failed epoch

- **WHEN** an epoch ends failed with a valid committed prefix and unresolved uncertain tail
- **THEN** automatic cleanup preserves its WAL and parent lineage until operator resolution or an explicit safe policy covers the evidence

#### Scenario: Retain policy

- **WHEN** segment is processed and quota pressure exists under retain mode
- **THEN** recorder does not delete it and may stop capture at quota boundary

#### Scenario: Delete-after policy

- **WHEN** segment is processed, unambiguous, older than configured minimum, and cleanup proof passes
- **THEN** recorder may perform cleanup transaction and retain tombstone

### Requirement: Global quota and minimum-free-space protection

Continuous recording SHALL enforce configured physical quota per filesystem while retaining the existing per-epoch hard maximum. One synchronized reservation authority SHALL account for the active epoch, sealed epochs awaiting ETL/retention, successor header/first-marker capacity, parent/epoch manifests and checkpoints, RecordingStore outputs, final-session staging, temporary files, and cleanup trash peak. Separate filesystem roots SHALL maintain independent quota/minimum-free reserves and cannot borrow one another's reserve.

Admission and every rollover SHALL reserve complete next frames/markers, segment/epoch headers, transition/catalog writes, concurrent ETL/session publication peaks, and temporary cleanup bytes before mutation. At pressure, the recorder SHALL first run only already-eligible cleanup. It SHALL never delete unprocessed/unverified/unexpired data to make space. If reservation still fails, it SHALL stop intake, persist typed quota-pressure/rollover-failure evidence, finalize the current authoritative prefix where possible, and fail/not-ready under controlled policy. It SHALL not loop-roll empty epochs or continue capture without durable capacity.

#### Scenario: Eligible cleanup restores headroom

- **WHEN** a successor or ETL reservation fails but finalized processed epochs are safely retention-eligible
- **THEN** eligible cleanup completes, quota is rebuilt, and one successor/reservation may proceed without changing parent identity

#### Scenario: No eligible data

- **WHEN** quota/minimum-free reservation fails and no epoch/segment is eligible
- **THEN** the recorder stops new admission rather than delete protected evidence or invent a new recording

#### Scenario: Temporary peak accounting

- **WHEN** rollover, manifest, incremental output, final session, or cleanup requires temporary bytes
- **THEN** synchronized quota calculation includes the concurrent peak and never exceeds configured physical bounds

#### Scenario: ETL lag consumes quota

- **WHEN** finalized epochs cannot be processed while the active successor continues and protected WAL consumes headroom
- **THEN** capture health reports lag and stops/fails before unsafe deletion or silent loss

#### Scenario: Separate store filesystem

- **WHEN** canonical/store root resides on a different filesystem from active WAL
- **THEN** each filesystem enforces its own reserve and store pressure cannot consume the WAL reserve

### Requirement: Lifecycle indexes remain bounded

Recorder SHALL enforce configured maximum byte and entry bounds for the parent epoch catalog, epoch lineage history, deletion tombstones, and retention metadata. Startup recovery, readiness, list, inspect, and status MUST NOT scan unbounded historical epochs. An epoch is eligible for compaction only after finalization, required ETL/checkpoint/session proof, policy reconciliation, cleanup completion, and durable tombstones.

Eligible history SHALL compact into a deterministic bounded lineage anchor preserving parent ID, first retained epoch identity/ordinal, last compacted epoch identity/ordinal, predecessor relationship, digest-chain root/tip, cleanup summary, aggregate counters, and range/count data sufficient to detect missing committed evidence, digest mismatch, lineage fork, incomplete cleanup, or conflicting compaction. Candidate installation SHALL use private temp-write, file sync, atomic rename, directory sync, source revision/digest, and candidate digest validation. If protected history cannot fit after legal compaction, stop new admission/epoch creation with bounded metadata-limit failure; never delete protected lineage or exceed limits.

#### Scenario: Multi-year history compaction

- **WHEN** finalized epoch history exceeds configured byte or entry limits
- **THEN** compaction reduces the index within bounds while preserving parent/epoch range, predecessor/digest proof, cleanup summary, and active tail

#### Scenario: Crash during compaction

- **WHEN** the process crashes before or after compaction rename/sync
- **THEN** recovery selects the old or complete digest-matching index deterministically and never loses tombstone/lineage proof

#### Scenario: Lineage contradiction after compaction

- **WHEN** a compacted summary conflicts with committed evidence, parent/epoch digest, predecessor, or cleanup proof
- **THEN** recovery reports stable contradiction and fails closed without scanning unrelated history

#### Scenario: Protected history exhausts bound

- **WHEN** failed, unprocessed, uncertain, or retained history cannot fit after legal compaction
- **THEN** recorder stops new admission/epoch creation with stable metadata-limit failure and does not delete protected lineage

#### Scenario: Stale compaction candidate

- **WHEN** candidate source revision/digest is stale or candidate digest conflicts with its source index
- **THEN** recovery rejects candidate, preserves old authoritative index, and does not install partial or guessed lineage

### Requirement: Continuous recovery reconciles manifest, files, and checkpoints

Startup recovery SHALL enumerate recognized files deterministically for every epoch, validate all non-deleted segment headers/envelopes/markers and sealed digests, reconcile parent/epoch manifest revisions and deletion transactions, compare per-epoch ETL checkpoints/publications, and derive exact active/sealed/processed/eligible states. Read-only validation of a live active segment SHALL stop at a stable recovery-authoritative marker snapshot and treat a concurrent partial suffix as mutable uncertainty, not corruption; only exclusive recovery MAY repair/truncate the allowed final tail.

Manifest rebuild SHALL preserve parent/epoch identity, epoch-local segment ordinals, sequence ranges, commit authority, retention tombstones, and predecessor lineage. Corruption before or at a committed boundary, unexplained missing segment/epoch, digest mismatch, impossible overlap, parent/epoch fork, or checkpoint beyond authority SHALL fail closed and preserve available bytes.

#### Scenario: Live ETL sees partial suffix

- **WHEN** ETL snapshots an active successor while the writer appends an incomplete later frame
- **THEN** ETL processes only the prior validated marker boundary and neither repairs nor reports committed corruption

#### Scenario: Startup with interrupted cleanup

- **WHEN** parent/epoch manifest and filesystem show a valid deleting transaction
- **THEN** exclusive recovery deterministically completes or rolls it back before readiness

#### Scenario: Repeated reconciliation

- **WHEN** recovery runs twice over unchanged parent/epoch state after one allowed repair
- **THEN** the second run produces identical manifest/checkpoint/lineage state with no additional mutation

### Requirement: WAL lifecycle acceptance

Automated tests SHALL cover segment size/age rotation, epoch byte/age rollover, manifest atomicity/rebuild/contradiction, every transition phase, quota reservation and pressure, retention eligibility, cleanup crash points, corruption, live snapshots, parent/epoch identity, repeated rollover, and deterministic recovery. Privileged acceptance SHALL retain machine-readable parent/epoch manifests, segment/checkpoint digests, quota/cleanup report, acceptance fingerprint, compatible environment, and source provenance for real continuous capture across multiple epochs and restarts. Exact commit/tree identity is release-only.

#### Scenario: Rotation and recovery proof

- **WHEN** acceptance forces segment and epoch size/age rotation then kills the recorder at selected transition steps
- **THEN** restart recovers the final authoritative marker, reconciles parent/epoch lineage, and continues without missing or duplicated committed sequence

#### Scenario: Retention/pressure proof

- **WHEN** acceptance applies retain/delete-after and sustained quota pressure while ETL is delayed
- **THEN** only proof-eligible data is deleted and recorder stops visibly when protected headroom is exhausted

#### Scenario: Corruption detection proof

- **WHEN** acceptance corrupts a sealed segment or parent/epoch lineage before cleanup eligibility
- **THEN** evidence remains preserved, affected readiness/cleanup is blocked, and stable diagnostics are retained

## ADDED Requirements

### Requirement: Epoch-local WAL boundaries

The WAL API SHALL make its total-byte, segment-sequence, marker, recovery, and retention boundaries explicit as epoch-local. Parent recording lifetime and aggregate identity SHALL be supplied by the application/ETL layer and SHALL not alter WAL v1 bytes or commit authority.

#### Scenario: Same parent across WAL directories

- **WHEN** one parent recording owns two epoch WAL directories with independent sequence ranges
- **THEN** each directory validates its own epoch WAL identity/markers and the parent lineage links them without merging sequences

#### Scenario: Segment limit is not run stop

- **WHEN** a segment or epoch reaches its configured bound and successor resources are valid
- **THEN** WAL returns a rollover-safe boundary to the application rather than a public recording termination
