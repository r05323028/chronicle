## ADDED Requirements

### Requirement: Versioned append-only segment format
Production WAL SHALL use ordered segment files with checksummed fixed segment header and checksummed length-framed `WalRecordEnvelope` records. Header SHALL identify format version, recording ID, segment ordinal, and first sequence. Envelope SHALL own recording ID, WAL sequence, segment/offset provenance, schema kind/version, flags, payload length, capture event payload, and CRC32C. Capture Event SHALL NOT require WAL sequence before append. Reader SHALL accept existing P0 WAL v1 where declared, accept P1 v2, reject unknown newer versions before allocation, and cap record length.

#### Scenario: Record framing round trip
- **WHEN** valid capture record is encoded and read from v2 segment
- **THEN** all fields and payload round trip and checksum validates

#### Scenario: Unsupported schema version
- **WHEN** segment or record declares unknown newer format/schema
- **THEN** reader fails with stable unsupported-version error without processing payload

#### Scenario: Checksum mismatch
- **WHEN** framed record bytes differ from checksum
- **THEN** reader reports exact segment/offset corruption and does not return record

### Requirement: Durable segmented group commit
Writer SHALL append complete frames only and expose one P1 durability policy. It SHALL flush and `fdatasync` unsynced frames when framed unsynced bytes reach 4 MiB, 10 ms elapses since last successful sync while data is unsynced, segment rotation begins, explicit internal flush boundary occurs, or graceful shutdown begins. After successful data sync, writer SHALL atomically persist checksummed durable watermark before acknowledging batch durable. Durable counters SHALL advance only through persisted watermark; lag/failure SHALL recover conservatively and SHALL NOT promote later bytes. No per-record default or user-selectable weaker/stronger mode SHALL exist. Segment size SHALL default 256 MiB and be configurable 16 MiB through 4 GiB. Total WAL segment storage SHALL default/hard-cap at 4 GiB and configurable lower bound SHALL be at least selected segment size. Bound SHALL count every byte under `segments/`, including segment headers, framed envelopes, and temporary segment bytes; metadata/lock/ETL reports outside `segments/` SHALL be excluded. Writer SHALL reserve/check header plus frame bytes before creating/writing them. New segment header publication/private permissions remain atomic and restrictive.

#### Scenario: Byte threshold commit
- **WHEN** unsynced framed data reaches 4 MiB
- **THEN** writer flushes/syncs batch and advances durable watermark through final synced envelope

#### Scenario: Time threshold commit
- **WHEN** unsynced data exists for 10 ms since last successful sync
- **THEN** writer group-syncs without waiting for another record

#### Scenario: Watermark persistence failure
- **WHEN** data sync succeeds but durable-watermark persistence fails
- **THEN** recording fails safely, batch is not acknowledged durable, and recovery uses prior persisted watermark

#### Scenario: Segment rotation
- **WHEN** next record would exceed configured segment size
- **THEN** writer group-syncs current segment, atomically publishes/syncs next valid segment header, then appends record whose WAL sequence matches header

#### Scenario: Crash while creating segment header
- **WHEN** process dies before temporary segment header is published
- **THEN** recovery ignores/reports temporary file and prior final segments remain recoverable

#### Scenario: Graceful close
- **WHEN** writer shuts down with less than 4 MiB unsynced and before 10 ms expires
- **THEN** shutdown boundary group-syncs final batch and metadata before close succeeds

#### Scenario: Total WAL limit
- **WHEN** next segment header or envelope would make bytes under `segments/` exceed configured total WAL bytes
- **THEN** writer creates/writes neither excess bytes nor ambiguous temp file and returns typed clean-limit signal so recording drains/finalizes without silent drop

#### Scenario: Oversized single record
- **WHEN** one framed record exceeds segment or record maximum
- **THEN** writer rejects it explicitly and does not create ambiguous partial record

### Requirement: Bounded ingest and no silent loss
Capture-to-WAL queue SHALL be bounded to 4096 events. Queue saturation SHALL apply backpressure rather than silently discard userspace events. Any resulting kernel-side drop SHALL be observable through typed loss evidence. Counters SHALL distinguish received, accepted/queued, written-not-durable, durable through watermark, dropped before persistence, and ETL-checkpointed records/bytes.

#### Scenario: Bounded queue saturation
- **WHEN** producer outruns WAL synchronization and queue fills
- **THEN** queue remains within bound, producer blocks, and any upstream loss is counted without promoting queued/written records to durable

#### Scenario: Dropped-event accounting
- **WHEN** event cannot reach WAL because capture backend reports loss
- **THEN** dropped-before-persistence counter and recording loss state increase

### Requirement: Deterministic recovery scan
Recovery SHALL lock one recording, discover final segments by numeric order, validate headers/recording identity/order, scan every frame, and return last verified durable checkpoint plus warnings. Persisted checksummed durable watermark SHALL be sole authority for durable record/byte counters and next durable sequence; synced segment headers establish discoverable layout only. Verified record bytes after watermark SHALL NOT be silently promoted and SHALL be truncated/discarded to durable boundary or reported uncertain under final-tail rules. Recording metadata SHALL supply last-known lifecycle/selector state. Recovery SHALL reconcile counters/lost unsynced window and convert stale `starting`/`recording` to `aborted` with crash/forced reason while rejecting immutable identity/selector mismatch. Repeated recovery over unchanged bytes SHALL produce same checkpoint, report, and mutation.

#### Scenario: Clean recovery
- **WHEN** all segments and records are valid
- **THEN** recovery reports next expected sequence and performs no data mutation

#### Scenario: Metadata lags durable WAL
- **WHEN** crash leaves valid final segments newer than recording metadata segment list/counters
- **THEN** recovery uses last persisted watermark as authoritative, reports/discards later valid frames as written-after-watermark uncertainty, and atomically reconciles metadata as aborted

#### Scenario: Crash after write before group sync
- **WHEN** complete envelopes exist after last durable watermark because process crashed before group sync
- **THEN** recovery does not claim them durable, reconciles loss counters, and degrades affected recording/connection completeness

#### Scenario: Deterministic recovery rerun
- **WHEN** recovery is run twice after one repair
- **THEN** second run reports same valid boundary with no additional truncation

### Requirement: Verified partial-tail repair
Recovery SHALL truncate only an incomplete final record in final segment back to last verified record boundary, sync repaired segment, and persist recovery warning. It SHALL NOT repair truncated segment header, middle-record corruption, checksum mismatch, sequence gap, overlap, or recording-ID mismatch.

#### Scenario: Truncated final record
- **WHEN** final segment ends during final record header or payload
- **THEN** recovery truncates to prior verified boundary and records warning/counter

#### Scenario: Truncated segment header
- **WHEN** segment header itself is incomplete
- **THEN** recovery marks segment corrupt, leaves bytes intact, and refuses ETL

#### Scenario: Corrupted middle record
- **WHEN** non-final or middle record is malformed/checksum-invalid
- **THEN** recovery preserves segment, reports corruption, and does not skip ahead

### Requirement: Safe reopen and append
Writer SHALL reopen only after successful recovery and advisory lock acquisition. It SHALL continue with next expected sequence, never overwrite existing bytes, and create next segment when final valid segment cannot accept record. Concurrent writer attempts SHALL fail.

#### Scenario: Restart and append
- **WHEN** process restarts after clean or repaired tail and reopens WAL
- **THEN** new record appends at recovered next sequence and full WAL reads contiguously

#### Scenario: Concurrent writer
- **WHEN** second writer opens active recording
- **THEN** second open fails without modifying segments

### Requirement: Explicit persistence failure policy
Permission, write, flush, group-sync, or disk-full failure SHALL return typed error, increment WAL failure counter, stop recording, preserve durable prefix, retain written-not-durable uncertainty, and finalize `failed` with `wal_failure` where possible. P1 SHALL expose no continue-on-persistence-error mode.

#### Scenario: Disk full
- **WHEN** append or sync fails for no space
- **THEN** recording stops, durable prefix remains recoverable, and summary reports disk-full failure/loss

#### Scenario: Permission failure
- **WHEN** WAL directory cannot be privately created/written
- **THEN** recording fails before capture when detected or stops explicitly at first failed write

#### Scenario: Injected disk write failure
- **WHEN** test writer fails after earlier successful records
- **THEN** earlier durable batch remains readable and failed/current unsynced batch is never counted durable

### Requirement: Corruption, durability-window, and recovery observability
Recovery output SHALL include stable codes and counts for repaired partial tails, discarded/uncertain post-watermark records/bytes, corrupted headers/records, rejected versions, sequence errors, and recovered durable records/bytes. Logs/reports SHALL not include capture payload values.

#### Scenario: Group-commit fault injection
- **WHEN** tests crash writer before/after byte threshold, time threshold, rotation sync, explicit flush, and shutdown sync
- **THEN** recovered durable prefix and lost-window counters match last successful sync exactly

#### Scenario: Corruption report JSON
- **WHEN** recovery finds checksum corruption
- **THEN** JSON identifies recording, segment, byte offset, category, and last valid checkpoint without raw payload
