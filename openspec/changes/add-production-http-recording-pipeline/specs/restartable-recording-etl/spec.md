## ADDED Requirements

### Requirement: Recording-scoped ETL input
ETL SHALL accept one recording WAL directory, recover and validate its metadata/segments, and process one stopped recording snapshot. It SHALL refuse actively locked recording, unsupported WAL/capture versions, corrupt durable prefix, contradictory metadata, or WAL changed after prior publication. It MAY process completed, interrupted, or failed recording whose durable prefix is valid and SHALL preserve source status in output.

#### Scenario: Completed recording input
- **WHEN** valid stopped recording is supplied
- **THEN** ETL discovers all ordered segments and processes through recovered high-water mark

#### Scenario: Active recording rejected
- **WHEN** record writer still holds recording lock
- **THEN** ETL fails without reading partial live state or publishing session

#### Scenario: Incomplete recording input
- **WHEN** recording ended interrupted but durable prefix is valid
- **THEN** ETL may publish session marked with interrupted provenance and honest completeness

### Requirement: Generation-safe TCP reconstruction
ETL SHALL group production payload by boot-scoped socket identity and connection generation, validate cgroup/tuple consistency, and maintain independent directional TCP sequence spaces. It SHALL NOT use file descriptor, PID, or tuple alone. Missing lifecycle SHALL create synthetic deterministic generation and incomplete state.

#### Scenario: Descriptor and tuple reuse
- **WHEN** multiple generations reuse descriptor or endpoint tuple
- **THEN** ETL creates separate reconstructed connections

#### Scenario: Missing open event
- **WHEN** payload exists without lifecycle-open event
- **THEN** ETL preserves payload under synthetic identity and marks connection incomplete

### Requirement: Deterministic directional ordering and deduplication
ETL SHALL order bytes within each direction by wrap-aware TCP sequence position, use WAL provenance as deterministic tie-breaker, remove exact retransmitted overlap, and preserve direction. It SHALL also preserve deterministic cross-direction observation order by kernel monotonic timestamp then WAL sequence plus trusted termination/source-integrity state for protocol decoding. Conflicting overlap, missing interval, event-loss marker, or truncation SHALL produce explicit malformed/incomplete/truncated state rather than guessed bytes. Unattributed kernel drop SHALL taint whole recording and every overlapping production connection non-replayable. Fixture v1 events SHALL retain monotonic event ordering.

#### Scenario: Split request and response
- **WHEN** HTTP heads/bodies span multiple capture events in both directions
- **THEN** reconstructed streams contain exact ordered bytes independent of event boundaries

#### Scenario: Exact duplicate retransmission
- **WHEN** same directional byte range is observed twice with identical bytes
- **THEN** reconstructed stream contains one copy and provenance records duplicate observation

#### Scenario: Conflicting overlap
- **WHEN** overlapping TCP positions contain different bytes
- **THEN** affected connection is malformed/non-replayable and bytes are not silently selected

#### Scenario: Missing payload interval
- **WHEN** TCP position gap remains at connection finalization
- **THEN** affected message/connection is incomplete with source gap provenance

#### Scenario: Sequential versus pipelined chronology
- **WHEN** two captures have identical directional request/response bytes but different cross-direction completion interleavings
- **THEN** ETL preserves distinct deterministic completion order so HTTP decoder distinguishes sequential from pipelined case

### Requirement: Bounded reconstruction
ETL SHALL bound active connections, chunks, bytes per connection, idle event-time retention, HTTP body buffering, total operations, issue count, and read batch size as documented. Exceeded limits SHALL create typed skipped/incomplete/truncated/unsupported evidence or fail before unbounded allocation.

#### Scenario: Connection memory limit
- **WHEN** reconstructed connection exceeds 8 MiB bound
- **THEN** ETL stops buffering that connection and records truncated/non-replayable state

#### Scenario: Idle connection eviction
- **WHEN** connection has no event for configured five-minute event-time window
- **THEN** ETL finalizes it incomplete and releases reconstruction memory

#### Scenario: Batch processing
- **WHEN** WAL contains more than 4096 records
- **THEN** ETL processes bounded batches with deterministic final output

### Requirement: Protocol decode and honest canonicalization
ETL SHALL detect protocol from reconstructed streams, invoke registered decoder/canonicalizer, pair HTTP messages, and preserve unsupported/malformed/incomplete/unmatched evidence. It SHALL NOT mark operation complete when source loss, truncation, framing ambiguity, or missing peer affects it. Unsupported non-HTTP traffic SHALL remain bounded opaque non-replayable evidence.

#### Scenario: Complete HTTP operation
- **WHEN** reconstructed request and response are valid supported HTTP/1.1
- **THEN** ETL creates complete canonical operation with typed HTTP data and provenance

#### Scenario: Malformed traffic
- **WHEN** reconstructed bytes contain malformed HTTP
- **THEN** ETL publishes malformed/non-replayable evidence without panic

#### Scenario: Unsupported protocol
- **WHEN** durable bytes are not supported HTTP/1.1
- **THEN** ETL records unknown protocol/opaque evidence and does not claim decoder support

### Requirement: Deterministic one-session publication
P1 ETL SHALL group one stopped recording into one canonical session. Session, connection, and operation identifiers SHALL derive deterministically from recording ID, pipeline version, stable connection provenance, and message order. Publication SHALL use existing filesystem store atomic no-replace boundary.

#### Scenario: First successful run
- **WHEN** valid recording has no prior output
- **THEN** ETL atomically publishes deterministic session and reports ID

#### Scenario: Atomic publication failure
- **WHEN** write/sync/rename fails before publication
- **THEN** final session path is absent, checkpoint does not advance, and rerun is safe

### Requirement: Checkpoint-after-publication idempotency
Before publication, ETL SHALL atomically create recording-local output binding containing recording ID and canonical output-root identity; any different root SHALL be rejected even when post-publication checkpoint is missing. ETL SHALL then own versioned atomic checkpoint containing recording ID, bound output identity, input high-water segment/offset/next sequence, `wal_snapshot_sha256` over exact ordered verified segment bytes through high-water, recovery-report digest/scope, format/pipeline/canonical versions, session ID, published manifest checksum, status, and counters. Manifest SHALL contain high-water, pipeline version, WAL snapshot digest, and session ID but SHALL NOT contain checkpoint-file checksum. Checkpoint SHALL be written only after session publication. Interruption before publication SHALL restart from durable WAL beginning. Repeated run SHALL not create duplicate session or nondeterministically change output.

#### Scenario: Second idempotent run
- **WHEN** same unchanged recording is processed after matching publication/checkpoint
- **THEN** ETL returns `already_processed` with same session ID and no new session

#### Scenario: Resume after interruption
- **WHEN** ETL stops after partially reading segment but before publication
- **THEN** rerun rereads durable input and produces same deterministic session once

#### Scenario: Published session but missing checkpoint
- **WHEN** session exists with matching provenance/checksum but checkpoint write was interrupted
- **THEN** ETL verifies publication and repairs checkpoint without duplicate output

#### Scenario: Same-length WAL mutation
- **WHEN** verified WAL bytes change after publication while high-water coordinates and lengths remain same
- **THEN** recomputed snapshot digest differs and ETL rejects already-processed/checkpoint repair

#### Scenario: Changed output root after publication before checkpoint
- **WHEN** session published to root A but checkpoint write failed and rerun requests root B
- **THEN** recording-local binding rejects root B and no duplicate session is published

#### Scenario: Changed output root with checkpoint
- **WHEN** existing checkpoint is rerun with different canonical output root
- **THEN** ETL rejects change instead of ambiguously reporting already processed

#### Scenario: Conflicting prior publication
- **WHEN** deterministic session path exists with different provenance/checksum
- **THEN** ETL fails and does not overwrite

### Requirement: Corruption and partial-segment behavior
ETL SHALL process through repaired valid final-tail boundary but SHALL stop on corrupted segment/header/record, unsupported version, sequence discontinuity, or checkpoint contradiction. It SHALL NOT skip corrupt middle data or advance checkpoint beyond last trustworthy record.

#### Scenario: Recovered truncated tail
- **WHEN** WAL recovery removed only incomplete final frame
- **THEN** ETL processes durable prefix and session records recovery warning/incomplete provenance

#### Scenario: Corrupted segment
- **WHEN** recovery report identifies non-tail corruption
- **THEN** ETL publishes no session and returns corruption summary

### Requirement: Provenance and counters
Canonical session and ETL summary SHALL include source recording ID/status, connection generation, capture time range, WAL segment/offset/sequence provenance, input/recovery checksums, schema versions, and counts for records processed/skipped, connections, decoded messages, and each completeness class. Default output SHALL omit payload/header values.

#### Scenario: Inspectable provenance
- **WHEN** ETL publishes session from recovered WAL
- **THEN** manifest/canonical data can trace each operation to recording/connection/WAL ranges

#### Scenario: ETL JSON summary
- **WHEN** user requests JSON
- **THEN** output has stable versioned fields, deterministic ordering, counters, checkpoint, and no sensitive values
