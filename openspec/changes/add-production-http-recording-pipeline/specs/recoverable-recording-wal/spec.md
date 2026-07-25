## ADDED Requirements

### Requirement: Versioned append-only segment format
Production WAL SHALL use ordered segment files with checksummed fixed segment header and checksummed length-framed records. Header SHALL identify format version, recording ID, segment ordinal, and first sequence. Record SHALL identify schema/kind/flags/sequence/payload length and CRC32C. Reader SHALL accept existing P0 WAL v1 where declared, accept P1 v2, reject unknown newer versions before allocation, and cap record length.

#### Scenario: Record framing round trip
- **WHEN** valid capture record is encoded and read from v2 segment
- **THEN** all fields and payload round trip and checksum validates

#### Scenario: Unsupported schema version
- **WHEN** segment or record declares unknown newer format/schema
- **THEN** reader fails with stable unsupported-version error without processing payload

#### Scenario: Checksum mismatch
- **WHEN** framed record bytes differ from checksum
- **THEN** reader reports exact segment/offset corruption and does not return record

### Requirement: Durable segmented append
Writer SHALL append complete frames only, rotate before configured segment limit, and default to flush plus `fdatasync` after every record before marking it durable. Segment size SHALL be configurable within documented 16 MiB through 4 GiB range and default 256 MiB. New segment header SHALL be written/synced under private temporary name, atomically renamed to final name, and parent directory synced before record append. Incomplete temporary segments SHALL never be discovered as final segments. Writer SHALL use restrictive filesystem permissions.

#### Scenario: Segment rotation
- **WHEN** next record would exceed configured segment size
- **THEN** writer syncs current segment, atomically publishes/syncs next valid segment header, then appends record whose sequence matches header

#### Scenario: Crash while creating segment header
- **WHEN** process dies before temporary segment header is published
- **THEN** recovery ignores/reports temporary file and prior final segments remain recoverable

#### Scenario: Graceful close
- **WHEN** writer shuts down normally
- **THEN** final record, segment, and recording metadata are flushed/synced before close succeeds

#### Scenario: Oversized single record
- **WHEN** one framed record exceeds segment or record maximum
- **THEN** writer rejects it explicitly and does not create ambiguous partial record

### Requirement: Bounded ingest and no silent loss
Capture-to-WAL queue SHALL be bounded to 4096 events. Queue saturation SHALL apply backpressure rather than silently discard userspace events. Any resulting kernel-side drop SHALL be observable through capture loss counters. Counters SHALL distinguish received, queued/not durable, durable, dropped before persistence, and ETL-checkpointed states.

#### Scenario: Bounded queue saturation
- **WHEN** producer outruns WAL synchronization and queue fills
- **THEN** queue remains within bound, producer blocks, and any upstream loss is counted

#### Scenario: Dropped-event accounting
- **WHEN** event cannot reach WAL because capture backend reports loss
- **THEN** dropped-before-persistence counter and recording loss state increase

### Requirement: Deterministic recovery scan
Recovery SHALL lock one recording, discover final segments by numeric order, validate headers/recording identity/order, scan every frame, and return last verified checkpoint plus warnings. Verified segment headers/records SHALL be authoritative for segment list, next sequence, and durable counters; recording metadata SHALL be last-known lifecycle/selector state. Recovery SHALL reconcile crash-stale derived metadata and stale `recording` status but reject immutable recording-ID/selector mismatch. Repeated recovery over unchanged bytes SHALL produce same checkpoint, report, and mutation.

#### Scenario: Clean recovery
- **WHEN** all segments and records are valid
- **THEN** recovery reports next expected sequence and performs no data mutation

#### Scenario: Metadata lags durable WAL
- **WHEN** crash leaves valid final segments newer than recording metadata segment list/counters
- **THEN** recovery derives authoritative values from WAL and atomically reconciles metadata as interrupted

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
Permission, write, flush, sync, or disk-full failure SHALL return typed error, increment WAL failure counter, stop recording by default, preserve durable prefix, and finalize failed metadata where possible. P1 SHALL expose no continue-on-persistence-error mode.

#### Scenario: Disk full
- **WHEN** append or sync fails for no space
- **THEN** recording stops, durable prefix remains recoverable, and summary reports disk-full failure/loss

#### Scenario: Permission failure
- **WHEN** WAL directory cannot be privately created/written
- **THEN** recording fails before capture when detected or stops explicitly at first failed write

#### Scenario: Injected disk write failure
- **WHEN** test writer fails after earlier successful records
- **THEN** earlier durable records remain readable and later record is not counted durable

### Requirement: Corruption and recovery observability
Recovery output SHALL include stable codes and counts for repaired partial tails, corrupted headers/records, rejected versions, sequence errors, and recovered durable records/bytes. Logs and reports SHALL not include capture payload values.

#### Scenario: Corruption report JSON
- **WHEN** recovery finds checksum corruption
- **THEN** JSON identifies recording, segment, byte offset, category, and last valid checkpoint without raw payload
