## Purpose

Recoverable v1 segmented WAL, commit markers, bounded admission, persistence failure, and recovery behavior.

## Requirements

### Requirement: Sole WAL v1 append-only segment format

Production and fixture WAL SHALL use sole mutable v1 ordered segment files with checksummed fixed segment headers and checksummed length-framed `WalRecordEnvelope` records. Segment header SHALL be exactly 64 bytes, little-endian: `CHS1` magic at bytes 0..4, format version 1 at 4..6, header version 1 at 6..8, header length 64 at 8..12, raw recording UUID at 12..28, segment ordinal at 28..36, first sequence at 36..44, signed Unix-nanosecond creation time at 44..52, zero reserved bytes at 52..60, and CRC32C over bytes 0..60 at 60..64. Envelope SHALL own recording ID, WAL sequence, segment/offset provenance, `RecordKind`, schema version, flags, payload length, payload, and CRC32C. Envelope frame SHALL have exact 48-byte little-endian header: `CHE1` magic at bytes 0..4, envelope version 1 at 4..6, `RecordKind` at 6..8, flags at 8..10, record schema version 1 at 10..12, header length 48 at 12..16, payload length at 16..20, raw recording UUID at 20..36, WAL sequence at 36..44, and CRC32C over bytes 0..44 followed by payload at 44..48; payload SHALL immediately follow. Segment ordinal, segment first sequence, and frame byte offset SHALL be derived provenance. Every record kind consumes one monotonically increasing sequence. Reader SHALL reject every non-v1 segment/envelope/record schema before allocation and cap record length; no earlier framing or format dispatch remains.

#### Scenario: Record framing round trip

- **WHEN** valid capture, loss-window, and commit-marker records are encoded and read from WAL v1 segment
- **THEN** every envelope field/payload round trips, each CRC32C validates, and sequence advances across all record kinds

#### Scenario: Capture event has no persistence sequence

- **WHEN** Capture Event v1 reaches writer without WAL identity
- **THEN** writer assigns next envelope sequence while Capture Event bytes remain capture-domain data

#### Scenario: Unsupported schema version

- **WHEN** segment, envelope, or record declares unknown newer format/schema
- **THEN** reader fails with stable unsupported-version error without processing payload

#### Scenario: Earlier framing is rejected

- **WHEN** bytes use old single-file magic or any non-v1 declaration
- **THEN** sole WAL v1 reader returns typed error without compatibility dispatch

### Requirement: Fixed in-WAL commit-marker format

The WAL SHALL represent durable boundaries only with `RecordKind::CommitMarker` schema v1 inside normal `WalRecordEnvelope` framing; it SHALL NOT use an external durable-watermark file. Commit-marker payload SHALL be exactly 76 bytes in this order: little-endian `u16 schema_version = 1`, `u16 reserved = 0`, `u64 durable_through_sequence`, `u64 durable_record_count`, `u64 durable_payload_bytes`, `u64 batch_first_sequence`, `u64 batch_last_sequence`, and 32 raw `batch_sha256` bytes. Marker envelope CRC32C SHALL cover normal envelope header/payload. `batch_sha256` SHALL cover exact encoded framed bytes of every contiguous non-marker envelope in current-segment batch, in sequence order. Durable count/bytes SHALL be cumulative committed non-marker record/payload counts. Marker sequence SHALL equal `batch_last_sequence + 1`; `durable_through_sequence` SHALL equal `batch_last_sequence`. Marker SHALL cover at least one record, cover no other marker, and never reference another segment. Marker sequence itself SHALL be recovered physical durable boundary; next envelope SHALL use marker sequence plus one.

A marker SHALL be recovery-authoritative only when all of these validate: marker frame completeness; marker envelope CRC32C; presence of every referenced frame; contiguous referenced WAL sequences; permitted same-segment non-marker batch membership; `batch_first_sequence` equal to segment first sequence for the first batch in that segment or previous recovery-authoritative marker sequence plus one for a later batch; `batch_last_sequence` equal to the final referenced frame sequence; marker sequence equal to `batch_last_sequence + 1`; durable-through and cumulative counts/bytes equal to previous authoritative totals plus exact current-batch totals; SHA-256 over the exact referenced framed bytes; and all relevant segment/header checksums, recording identities, ordinals, first sequences, and versions. A complete-looking marker failing any reference, sequence, segment, identity, version, CRC32C, boundary, cumulative-field, or batch-digest check MUST NOT be authoritative and SHALL fail closed according to corruption rules. Frames after the final recovery-authoritative marker SHALL remain uncommitted uncertainty.

#### Scenario: One record plus marker

- **WHEN** one data record forms group-commit batch
- **THEN** following marker covers exactly that sequence, carries cumulative counts/digest, and consumes next sequence

#### Scenario: Multiple records committed together

- **WHEN** capture and loss-window records form one batch
- **THEN** one following marker covers complete contiguous range and digest of both record kinds

#### Scenario: Marker crosses segment boundary

- **WHEN** marker references record outside marker's segment or non-contiguous sequence
- **THEN** reader reports corruption and does not use marker as durable boundary

#### Scenario: Unknown commit-marker version

- **WHEN** marker payload declares version newer than 1 or nonzero reserved field
- **THEN** recovery fails explicitly as unsupported/invalid without treating marker as commit

### Requirement: One-sync segmented group commit

Writer SHALL expose one durability policy and no user-facing durability mode. It SHALL trigger group commit when unsynced non-marker framed bytes reach 4 MiB, 10 ms elapses since last successful group sync while data is unsynced, segment rotation begins, explicit internal flush boundary occurs, or graceful shutdown begins. Ordering SHALL be: append complete data/control frames; append complete marker covering them; flush buffered writer; call one `fdatasync` on current segment; only after that call returns success mark covered records runtime-durable and send or record their durability acknowledgement; update in-memory counters and refresh non-authoritative recording metadata asynchronously or at lifecycle boundaries. Marker SHALL be included in the same `fdatasync` as covered records. Runtime MUST NOT mark a batch durable or send/record its durability acknowledgement before successful `fdatasync`.

Recovery authority SHALL apply only to persisted WAL durability and SHALL be determined from recovery-authoritative marker validation, independently of acknowledgement delivery. Process failure MAY occur after `fdatasync` succeeds but before a caller observes the acknowledgement. Recovery MAY recognize that batch as committed but MUST NOT infer that any caller, queue worker, metadata writer, or CLI renderer observed the acknowledgement before crash. Acknowledgement-delivery state SHALL NOT be reconstructed unless separately persisted; The WAL does not require such persistence. Pre-sync crash tests SHALL use a fault-injected durable-media image rather than same-kernel page-cache survival. Empty batch SHALL emit no marker. The WAL SHALL NOT perform a second watermark-file write/sync/rename/directory-sync per batch and SHALL NOT default to per-record sync.

#### Scenario: Byte threshold commit

- **WHEN** unsynced non-marker frame bytes reach 4 MiB
- **THEN** writer appends one marker, flushes, performs one segment `fdatasync`, then acknowledges covered records

#### Scenario: Time threshold commit

- **WHEN** unsynced data exists for 10 ms since last successful group sync
- **THEN** writer commits without waiting for another record and uses one segment `fdatasync`

#### Scenario: Segment rotation

- **WHEN** next record requires rotation
- **THEN** writer commits/syncs all current-segment data, finalizes current segment, creates/syncs/publishes next header, and begins next segment at prior marker sequence plus one

#### Scenario: Graceful close

- **WHEN** writer shuts down with unsynced writable records below thresholds
- **THEN** writer appends final marker, flushes, `fdatasync`s once, then finalizes recording metadata

#### Scenario: Crash before marker write

- **WHEN** crash durable image contains complete batch records but no following marker
- **THEN** records remain written-not-durable uncertainty after the previous recovery-authoritative marker

#### Scenario: Crash during marker frame

- **WHEN** final marker frame is partial
- **THEN** marker contributes no commit and verified-final-tail rule may truncate it to prior frame boundary

#### Scenario: Crash after marker write before sync

- **WHEN** failure injector discards unsynced durable-media state after complete marker write but before `fdatasync`
- **THEN** recovery finds only the prior recovery-authoritative marker and writer never reported the new batch runtime-durable

#### Scenario: Surviving marker without acknowledgement evidence

- **WHEN** recovery observes a complete marker after process crash but acknowledgement delivery was not separately persisted
- **THEN** recovery decides persisted WAL authority only by full marker/reference/identity/digest validation and does not claim acknowledgement delivery occurred

#### Scenario: Crash after sync but before acknowledgement

- **WHEN** data frames and valid commit marker were `fdatasync`-completed but process crashed before durability acknowledgement was observed
- **THEN** recovery recognizes the batch as committed but does not claim acknowledgement delivery occurred

#### Scenario: Metadata lag after sync

- **WHEN** `fdatasync`-completed authoritative WAL is newer than recording metadata counters
- **THEN** recovery uses the authoritative batch and reconciles metadata without reconstructing acknowledgement delivery

### Requirement: Physical total-WAL hard cap and reservation

`max_wal_bytes` SHALL default to and be hard-capped at 4 GiB; configurable lower value SHALL be at least selected segment size. It SHALL be a physical hard limit over every byte under `segments/`, including headers, envelopes, and temporary publication files; metadata/lock/ETL reports outside `segments/` SHALL be excluded. Before writing a non-marker frame, writer SHALL reserve/check both current-segment and total capacity for complete frame plus fixed final commit-marker frame; if pair cannot fit current segment, writer SHALL commit current batch and rotate before data frame. Total-capacity check SHALL also include next segment header plus peak temporary publication bytes when rotation is required. Rename-only publication counts one header allocation; implementations that duplicate bytes SHALL reserve peak duplicate storage. Writer SHALL never write partial frame or exceed configured cap.

When next complete frame cannot fit while preserving reservation, writer SHALL return typed clean-limit signal, set `shutdown_reason = wal_size_limit`, stop capture intake immediately, and write only longest queued prefix whose complete frames preserve final-marker reservation. It SHALL classify remaining accepted queue records/bytes as `discarded_from_queue_due_to_wal_limit`; classify post-stop arrivals separately; append final marker for every already written writable frame; and `fdatasync`. Successful marker sync/metadata finalization SHALL produce `completed`; marker/sync/metadata failure SHALL produce `failed` with `wal_failure`. One format-oversized record SHALL remain typed invalid-record failure rather than partial write.

#### Scenario: Limit reached with empty queue

- **WHEN** reservation check reaches hard cap with no queued event remaining
- **THEN** intake stops, any unsynced written batch is marker-committed, and successful finalization completes with `wal_size_limit`

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

#### Scenario: Oversized single record

- **WHEN** one framed record exceeds format or segment maximum independent of remaining total capacity
- **THEN** writer rejects typed invalid record and creates no ambiguous partial frame

#### Scenario: Concurrent duration and WAL limits

- **WHEN** duration expiry and writer reservation failure are observed in same shutdown cycle
- **THEN** `wal_size_limit` takes precedence because physical-cap enforcement caused accepted queue discard

#### Scenario: Final commit sync failure

- **WHEN** final marker write/flush/`fdatasync` fails during WAL-limit stop
- **THEN** recording is `failed` with `wal_failure`, prior committed prefix remains authoritative, and hard cap remains unexceeded

### Requirement: Bounded ingest and categorized no-silent-loss counters

Capture-to-WAL queue SHALL be bounded to 4096 events. Queue saturation SHALL apply backpressure rather than silently discard userspace events; resulting kernel-side drop SHALL be observable through typed loss evidence. Counters SHALL expose stable categories `received`, `accepted_into_queue`, `written_not_committed`, `committed`, `discarded_from_queue_due_to_wal_limit`, `kernel_or_backend_dropped`, `rejected_after_stop`, and `etl_checkpointed`, with records and bytes where available. Summary and metadata SHALL reconcile categories without counting one event in multiple loss categories.

#### Scenario: Bounded queue saturation

- **WHEN** producer outruns WAL synchronization and queue fills before stop
- **THEN** queue remains bounded, producer blocks, and upstream loss is counted without promoting queued/written records

#### Scenario: WAL-limit queue discard accounting

- **WHEN** accepted queued events cannot fit after capacity-qualified prefix
- **THEN** exact discarded records/bytes leave queued count, enter only WAL-limit discard category, and never enter written/durable counts

#### Scenario: Post-stop incoming event

- **WHEN** event arrives after intake stop begins
- **THEN** it is rejected/counts post-stop rather than accepted into queue or confused with WAL-limit queued discard

#### Scenario: Summary consistency

- **WHEN** final metadata and summary are rendered
- **THEN** accepted equals written plus still queued plus categorized accepted-queue discard, and durable does not exceed written

### Requirement: Typed terminal WAL-loss evidence

WAL-cap discard SHALL be modeled as recorder/WAL admission loss, not kernel/backend capture loss. It SHALL NOT add a `CaptureEvent` or capture-source event variant. A discarded event is one already accepted into bounded ingest but denied persistence by WAL physical-cap reservation; it remains exclusively in `discarded_from_queue_due_to_wal_limit` accounting.

`QueuedWalRecord` SHALL carry optional typed capture-time metadata using existing `MonotonicTimestamp { clock: ClockIdentity, nanoseconds }`, independent of its opaque WAL payload. Queueing wall time, flush time, terminal-marker time, and unrelated process uptime SHALL NOT substitute for capture time. The ingest state machine SHALL accumulate discarded record/byte counts and earliest/latest valid timestamps separately for each `ClockIdentity`; it SHALL NOT compare or merge incompatible clocks.

`RecordKind::TerminalWalLoss` schema version 1 SHALL encode `TerminalWalLoss { interval: TerminalWalLossInterval { start: MonotonicTimestamp, end: MonotonicTimestamp }, discarded_records: u64, discarded_payload_bytes: u64, reason: TerminalWalLossReason::WalHardLimit, ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects }`. `start.clock` and `end.clock` SHALL match, `start.nanoseconds <= end.nanoseconds`, and `discarded_records > 0`. Zero payload bytes are valid only when every attributed discarded record has zero payload; no decoder SHALL infer otherwise. Reason and ambiguity are closed typed enums; payload SHALL contain no Aya/kernel ABI values, queue addresses, WAL paths, arbitrary error strings, HTTP/reconstruction data, or guessed socket identities.

At most one terminal WAL-loss record per clock identity SHALL be emitted for one WAL-limit stop. A complete terminal frame plus required final marker MUST fit before write. Terminal control records SHALL not be recursively treated as ordinary queue input or added to ordinary discard counters. Only a successful final marker sync makes terminal evidence persisted. If no terminal frame plus marker fits, metadata/final summary SHALL retain a typed per-clock `TerminalWalLossSummary` with exact counts and `MetadataOnly` persistence state; ETL SHALL consume equivalent conservative evidence. A discarded event with no trusted timestamp SHALL be represented in a separate typed summary entry: use compatible last persisted capture timestamp as conservative start only when available; otherwise use `TimestampUnavailable` ambiguity and emit no interval-bearing WAL payload. Marker/frame/flush/sync/metadata failure SHALL finalize `failed` with `wal_failure`, preserve prior committed prefix, and report `NotPersistedDueToWalFailure` rather than claim a terminal record was persisted.

Reader/recovery SHALL decode recovery-authoritative `TerminalWalLoss` envelopes as typed control evidence, expose them separately from ordinary CaptureEvent input, and reject non-v1 terminal-loss schema values plus malformed/contradictory payloads without compatibility dispatch. Operations provably outside a known matching-clock interval may remain complete; overlapping or clock/timing-ambiguous operations SHALL degrade. Policy SHALL NOT claim exact drop time or per-connection attribution.

#### Scenario: Single discarded record

- **WHEN** one timestamped record is discarded at WAL cap
- **THEN** its terminal interval has equal start/end timestamps, exact count/bytes, `WalHardLimit` reason, and `UnknownDownstreamEffects` ambiguity

#### Scenario: Multiple discarded records

- **WHEN** several discarded records share one clock
- **THEN** interval start/end are their earliest/latest capture timestamps and record/byte totals are exact

#### Scenario: Incompatible clocks

- **WHEN** discarded records have different `ClockIdentity` values
- **THEN** writer emits separate per-clock terminal records/summaries and never compares or merges their timestamps

#### Scenario: Terminal codec validation

- **WHEN** valid terminal-loss payload is encoded then decoded
- **THEN** every typed field round trips; unsupported schema, unequal clocks, reversed interval, zero records, invalid enum discriminant, or contradictory accounting is rejected

#### Scenario: Terminal loss record fits

- **WHEN** one or more per-clock terminal frames plus their required final marker fit
- **THEN** each is emitted once, marker-committed, recovery exposes typed evidence, and terminal records are not recursively counted as discard

#### Scenario: No room for terminal loss marker

- **WHEN** only required marker fits after queue discard
- **THEN** writer emits no partial terminal frame, metadata/final summary has exact typed `MetadataOnly` evidence, and ETL receives equivalent conservative evidence

#### Scenario: Terminal finalization failure

- **WHEN** terminal frame, final marker, flush, sync, or metadata persistence fails
- **THEN** recording is `failed` with `wal_failure`, prior committed prefix remains authoritative, and summary reports `NotPersistedDueToWalFailure`

#### Scenario: Missing timestamp

- **WHEN** a discarded record has no trusted timestamp
- **THEN** summary uses compatible last persisted capture timestamp only as conservative fallback or records `TimestampUnavailable`; writer never fabricates an interval-bearing terminal WAL payload

#### Scenario: Sole-v1 preservation

- **WHEN** current repository WAL v1 artifacts are read
- **THEN** capture, loss, marker, and terminal-loss records use one current dispatch and every non-v1 declaration is rejected

### Requirement: Commit-aware deterministic recovery scan

Recovery SHALL lock recording, discover final segments numerically, validate every relevant segment header checksum/version/recording identity/ordinal/first sequence and envelope version/order/CRC32C, scan every envelope, and select the final recovery-authoritative commit marker as committed boundary/counters. Authority SHALL require every marker condition defined by the fixed commit-marker requirement; recovery MUST NOT infer authority from marker appearance alone. Complete frames after the final authoritative marker SHALL be written-not-durable uncertainty and SHALL NOT be promoted. Read-only recovery SHALL report them without mutation; reopen-for-append SHALL first report then truncate current segment to marker end/remove later uncommitted segments, sync changed file/directories, and continue at marker sequence plus one. A complete marker with missing, non-contiguous, skipped-prefix, wrong-segment, identity-mismatched, checksum/CRC-mismatched, boundary-mismatched, cumulative-total-mismatched, or batch-SHA-256-mismatched references SHALL be rejected as authoritative and SHALL fail closed according to corruption rules. Recording metadata may lag and SHALL be reconciled from authoritative committed records without claiming runtime acknowledgement delivery occurred; missing metadata SHALL NOT invalidate committed WAL when immutable recording identity is established from validated segment headers. Unknown newer marker version SHALL fail explicitly. Repeated recovery over unchanged bytes SHALL produce the same checkpoint/report/mutation.

#### Scenario: Clean recovery

- **WHEN** all validations pass and final envelope is a recovery-authoritative marker
- **THEN** recovery reports marker sequence plus one as next expected sequence and performs no data mutation

#### Scenario: Metadata lags committed WAL

- **WHEN** WAL contains a recovery-authoritative committed batch but recording metadata counters lag
- **THEN** recovery derives committed counters from WAL and atomically reconciles metadata as aborted without claiming runtime acknowledgement delivery

#### Scenario: Complete uncommitted frames

- **WHEN** complete envelopes exist after the final recovery-authoritative marker
- **THEN** read-only recovery does not claim/mutate them, reports exact range/bytes as uncertainty, and degrades potentially affected completeness

#### Scenario: Reopen after uncommitted suffix

- **WHEN** append reopen is explicitly requested after reported complete post-marker frames
- **THEN** recovery discards only entire uncommitted suffix with file/directory sync and writer resumes at marker sequence plus one

#### Scenario: Corrupt marker checksum

- **WHEN** final marker CRC32C or batch SHA-256 mismatches
- **THEN** marker is not a commit; corruption before/at claimed committed boundary fails closed rather than skipping ahead

#### Scenario: Invalid marker references

- **WHEN** a complete marker references missing, non-contiguous, skipped-prefix, wrong-segment, identity-mismatched, CRC-mismatched, boundary-mismatched, cumulative-total-mismatched, or digest-mismatched frames
- **THEN** recovery rejects it as authoritative and fails closed according to corruption rules

#### Scenario: Deterministic recovery rerun

- **WHEN** recovery runs twice after one allowed repair
- **THEN** second run reports identical committed boundary/counters with no additional mutation

### Requirement: Verified partial-tail repair

Recovery SHALL truncate only incomplete final frame in final segment back to prior verified frame boundary, sync repaired segment, and persist recovery warning. Partial final `CommitMarker` SHALL be ignored as commit before truncation. Recovery SHALL NOT repair truncated segment header, middle-record corruption, checksum mismatch, sequence gap, overlap, invalid marker references, or recording-ID mismatch.

#### Scenario: Truncated final data frame

- **WHEN** final segment ends during final data envelope header or payload
- **THEN** recovery truncates to prior verified frame boundary and retains prior recovery-authoritative marker boundary

#### Scenario: Truncated final marker

- **WHEN** final segment ends during commit-marker envelope/payload
- **THEN** marker contributes no commit and recovery truncates only that verified partial final tail

#### Scenario: Corrupted middle record

- **WHEN** non-final or middle record is malformed/checksum-invalid
- **THEN** recovery preserves segment, reports corruption, and does not skip ahead

### Requirement: Safe reopen and append

Writer SHALL reopen only after successful recovery and advisory lock acquisition. It SHALL never overwrite committed bytes, SHALL remove/discard only reported uncommitted suffix/allowed partial final tail with required sync, and SHALL continue at final recovery-authoritative marker sequence plus one or create next segment when required. Concurrent writer attempts SHALL fail.

#### Scenario: Restart and append

- **WHEN** process reopens after clean or repaired tail
- **THEN** new envelope appends at recovered next sequence and a later group receives a new recovery-authoritative marker after successful sync

#### Scenario: Concurrent writer

- **WHEN** second writer opens active recording
- **THEN** second open fails without modifying segments

### Requirement: Explicit persistence failure policy

Permission, write, flush, group-sync, marker, or disk-full failure SHALL return typed error, increment WAL failure counter, stop recording, preserve prior committed prefix, retain written-not-durable uncertainty, and finalize `failed` with `wal_failure` where possible. The WAL SHALL expose no continue-on-persistence-error mode.

#### Scenario: Disk full

- **WHEN** append, marker, or sync fails for no space outside clean reservation path
- **THEN** recording fails, prior committed prefix remains recoverable, and summary reports disk-full/loss

#### Scenario: Injected write failure

- **WHEN** test writer fails after earlier successful marker
- **THEN** earlier committed batch remains readable and failed/current batch is never counted durable

### Requirement: Corruption, commit-window, and recovery observability

Recovery output SHALL include stable codes/counts for repaired partial tails, uncommitted post-marker records/bytes, corrupted headers/records/markers, rejected versions, sequence/reference/digest errors, recovered committed records/bytes, WAL-limit queue discard, and terminal loss evidence. Logs/reports SHALL not include capture payload values.

#### Scenario: Group-commit fault matrix

- **WHEN** tests crash before marker, during marker, after marker before sync, after sync, at threshold, rotation, explicit flush, and shutdown
- **THEN** recovered boundary/counters match the final recovery-authoritative marker and prior committed prefix remains readable

#### Scenario: Physical-size assertion

- **WHEN** every hard-limit/failure-injection case completes
- **THEN** recursive physical bytes under `segments/` never exceed configured maximum

### Requirement: Size-and-age segment rotation

The recorder WAL writer SHALL preserve existing size rotation and additionally rotate a non-empty active segment when configured maximum age expires. Default size SHALL remain 256 MiB with existing 16 MiB through 4 GiB range. Default age SHALL be five minutes with documented bounded configuration; age SHALL use monotonic elapsed time for trigger and persisted wall time only for observability. Rotation check SHALL run on append and periodic writer tick so low-traffic segments eventually seal. Empty segment SHALL NOT rotate only because age elapsed.

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

Segment lifecycle SHALL be `active -> sealed -> processed -> retention_eligible -> deleting -> deleted`; corruption or contradiction is a terminal evidence-preservation state. Exactly one segment MAY be active per epoch. Only recovery-authoritative marker ending at or before sealed length MAY establish sealed range. Processed transition SHALL require durable ETL checkpoint that names exact segment digest/range and every required immutable publication. Retention eligibility SHALL additionally require configured retention policy. No transition SHALL regress or skip proof requirements.

Deleted entries SHALL remain bounded tombstones in manifest/epoch index until entire epoch retention record expires. Tombstone SHALL retain identity, range, digest, checkpoint/publication references, deletion reason/time, and cleanup transaction ID but no captured bytes. Evidence-preservation segments SHALL never be auto-deleted.

#### Scenario: Segment processed

- **WHEN** ETL checkpoint durably references sealed segment digest/range and required outputs
- **THEN** manifest may transition sealed segment to processed

#### Scenario: Retention age alone expires

- **WHEN** retention time expires before ETL/output proof
- **THEN** segment remains sealed and cannot become retention eligible

#### Scenario: Corrupt sealed segment

- **WHEN** sealed segment digest or committed content fails validation
- **THEN** segment remains preserved in place, is non-authoritative, and automatic cleanup stops for affected epoch

### Requirement: Checkpoint-gated cleanup transaction

Cleanup SHALL delete only sealed segments from stopped/finalized epochs or prefixes explicitly supported by durable incremental ETL state; The recorder default SHALL clean finalized epochs and SHALL NOT require active-epoch prefix deletion. Eligibility SHALL require: validated recovery-authoritative committed range; matching sealed digest; durable deterministic incremental output publication; durable ETL checkpoint naming input/output lineage; for finalized epoch, verified existing Canonical Session v1 plus manifest/payload integrity from `FilesystemSessionStore`; no unresolved corruption/uncertainty; retention policy authorization; and quota safety reserve for transaction metadata.

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

Recorder configuration SHALL declare retention mode and bounds. The recorder SHALL support `retain` and `delete_after` policy for local WAL; implicit deletion SHALL NOT occur. `delete_after` SHALL specify minimum age and MAY specify maximum retained bytes, but both remain subordinate to checkpoint-gated eligibility. Policy changes SHALL be recorded by configuration digest and SHALL NOT retroactively authorize cleanup until next successful reconciliation.

Canonical outputs, checkpoints, manifests, and tombstones SHALL have separate retention classes from source WAL. Deleting source WAL SHALL preserve provenance that source was intentionally expired and SHALL not claim raw evidence remains available. Failed, corrupted, unprocessed, uncheckpointed, or policy-unexpired segments SHALL not be deleted automatically.

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

Continuous recorder SHALL enforce configured physical quota domain per filesystem while retaining existing per-epoch 4 GiB hard maximum. The recorder supports one active Chronicle mutating owner per filesystem domain. Recorder lease ownership SHALL cover the domain, not only the recorder process; multiple recorder instances sharing the same WAL/store filesystem are unsupported. Standalone mutating commands SHALL reject execution when a live recorder owns the domain, while read-only commands may continue. Different cgroup scopes requiring independent recorder instances SHALL use isolated filesystem domains.

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

Automated tests SHALL cover size/age rotation, manifest atomicity/rebuild/contradiction, every segment transition, quota reservation, retention eligibility, cleanup crash points, corruption, live snapshot behavior, and deterministic recovery. Privileged acceptance SHALL retain machine-readable manifest, segment/checkpoint digests, quota/cleanup report, acceptance fingerprint, compatible environment, and source provenance for real continuous capture across multiple rotations and restart. Exact commit/tree identity is release-only.

#### Scenario: Rotation and recovery proof

- **WHEN** acceptance forces size and age rotation then kills recorder at selected rotation step
- **THEN** restart recovers final authoritative marker, reconciles manifest, and continues without missing or duplicated committed sequence

#### Scenario: Corruption detection proof

- **WHEN** acceptance corrupts sealed segment before cleanup eligibility
- **THEN** recorder preserves evidence in place, blocks cleanup/readiness for affected lineage, and reports stable code
