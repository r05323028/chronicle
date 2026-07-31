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

P1 SHALL represent durable boundaries only with `RecordKind::CommitMarker` schema v1 inside normal `WalRecordEnvelope` framing; it SHALL NOT use an external durable-watermark file. Commit-marker payload SHALL be exactly 76 bytes in this order: little-endian `u16 schema_version = 1`, `u16 reserved = 0`, `u64 durable_through_sequence`, `u64 durable_record_count`, `u64 durable_payload_bytes`, `u64 batch_first_sequence`, `u64 batch_last_sequence`, and 32 raw `batch_sha256` bytes. Marker envelope CRC32C SHALL cover normal envelope header/payload. `batch_sha256` SHALL cover exact encoded framed bytes of every contiguous non-marker envelope in current-segment batch, in sequence order. Durable count/bytes SHALL be cumulative committed non-marker record/payload counts. Marker sequence SHALL equal `batch_last_sequence + 1`; `durable_through_sequence` SHALL equal `batch_last_sequence`. Marker SHALL cover at least one record, cover no other marker, and never reference another segment. Marker sequence itself SHALL be recovered physical durable boundary; next envelope SHALL use marker sequence plus one.

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

Writer SHALL expose one P1 durability policy and no user-facing durability mode. It SHALL trigger group commit when unsynced non-marker framed bytes reach 4 MiB, 10 ms elapses since last successful group sync while data is unsynced, segment rotation begins, explicit internal flush boundary occurs, or graceful shutdown begins. Ordering SHALL be: append complete data/control frames; append complete marker covering them; flush buffered writer; call one `fdatasync` on current segment; only after that call returns success mark covered records runtime-durable and send or record their durability acknowledgement; update in-memory counters and refresh non-authoritative recording metadata asynchronously or at lifecycle boundaries. Marker SHALL be included in the same `fdatasync` as covered records. Runtime MUST NOT mark a batch durable or send/record its durability acknowledgement before successful `fdatasync`.

Recovery authority SHALL apply only to persisted WAL durability and SHALL be determined from recovery-authoritative marker validation, independently of acknowledgement delivery. Process failure MAY occur after `fdatasync` succeeds but before a caller observes the acknowledgement. Recovery MAY recognize that batch as committed but MUST NOT infer that any caller, queue worker, metadata writer, or CLI renderer observed the acknowledgement before crash. Acknowledgement-delivery state SHALL NOT be reconstructed unless separately persisted; P1 does not require such persistence. Pre-sync crash tests SHALL use a fault-injected durable-media image rather than same-kernel page-cache survival. Empty batch SHALL emit no marker. P1 SHALL NOT perform a second watermark-file write/sync/rename/directory-sync per batch and SHALL NOT default to per-record sync.

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

Permission, write, flush, group-sync, marker, or disk-full failure SHALL return typed error, increment WAL failure counter, stop recording, preserve prior committed prefix, retain written-not-durable uncertainty, and finalize `failed` with `wal_failure` where possible. P1 SHALL expose no continue-on-persistence-error mode.

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
