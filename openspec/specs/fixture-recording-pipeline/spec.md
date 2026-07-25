## Purpose

Versioned fixture ingestion through WAL, ordered session assembly, checkpoint reporting, and deterministic incomplete-input behavior.

## Requirements

### Requirement: Versioned deterministic fixture input
The system SHALL accept fixture schema v1 as bounded JSON containing unique connection definitions and globally ordered socket-byte events. Validation SHALL enforce total encoded WAL size below configured one-segment limit in addition to per-connection/session limits. Each connection SHALL provide a fixture-local ID, optional network namespace, client/server endpoints, TCP transport, and modeled process/file-descriptor metadata. Each event SHALL provide contiguous sequence, timestamp, connection reference, direction, strict hexadecimal payload, truncation flag, and capture flags. Fixture IDs SHALL only resolve input references and SHALL NOT become a canonical persistence format.

#### Scenario: Valid binary fixture
- **WHEN** fixture v1 contains fragmented HTTP bytes including non-UTF-8 body bytes encoded by `payload_hex`
- **THEN** fixture source emits equivalent `CaptureEvent` values in fixture sequence order without UTF-8 conversion

#### Scenario: Unsupported fixture version
- **WHEN** fixture schema version is not 1
- **THEN** record fails before creating WAL or session artifacts with stable unsupported-schema error

#### Scenario: Invalid connection reference or payload
- **WHEN** event references unknown connection, uses invalid hex, non-TCP transport, zero endpoint port, duplicate sequence, gap, or decreasing timestamp
- **THEN** record fails before any WAL append and identifies invalid field without printing payload bytes

#### Scenario: Tuple reuse ambiguity
- **WHEN** two fixture connection IDs resolve to same current `ConnectionKey`
- **THEN** fixture validation rejects input because current assembler cannot distinguish reused socket tuples

### Requirement: Fixture source uses capture boundary
The fixture adapter SHALL implement or feed existing `CaptureSource` contract and emit ordinary versioned `CaptureEvent` socket-byte chunks. It SHALL NOT call HTTP, canonical, storage, or replay code. End of fixture SHALL mean finite capture batch completion and SHALL NOT claim TCP FIN or packet reassembly.

#### Scenario: Capture source exhaustion
- **WHEN** final fixture event has been emitted
- **THEN** source returns end-of-stream and application proceeds to WAL flush without synthesizing lifecycle bytes

#### Scenario: Truncation propagation
- **WHEN** fixture event has `truncated=true`
- **THEN** emitted `CaptureEvent` carries truncation and downstream connection is never reported complete/replayable

### Requirement: Every event crosses WAL durability boundary
Record service SHALL encode each emitted `CaptureEvent` using current capture envelope and append one existing WAL v1 `CaptureEvent` record with matching sequence. It SHALL flush WAL before ETL reads any record. Fixture bytes SHALL never be passed directly to session assembler or HTTP decoder.

#### Scenario: Successful append and readback
- **WHEN** valid N-event fixture is recorded
- **THEN** WAL contains N complete CRC32C-valid records and ETL receives events only by decoding those records

#### Scenario: WAL write failure
- **WHEN** append or flush fails
- **THEN** record fails, no final session directory is published, and error identifies WAL stage without payload content

#### Scenario: Oversized one-segment capture
- **WHEN** encoded fixture WAL would exceed configured segment size
- **THEN** P0 record fails during preflight before append and reports one-segment limit

### Requirement: WAL ordering and checkpoint behavior
ETL SHALL read complete WAL records in contiguous sequence order and SHALL preserve reader checkpoint after last valid record. Manifest SHALL store segment first sequence, byte offset, and next expected sequence used for this processing run.

#### Scenario: Complete WAL
- **WHEN** reader reaches clean end after complete records
- **THEN** checkpoint points immediately after last record and session may be complete if protocol processing also completes

#### Scenario: Partial tail
- **WHEN** reader encounters partial header or payload
- **THEN** it stops at partial record start, does not advance checkpoint, records bounded `partial_wal_tail` issue, and marks persisted session non-replayable

#### Scenario: Corrupt WAL
- **WHEN** record has invalid magic, version, CRC, or sequence
- **THEN** ETL fails without publishing canonical session

#### Scenario: Restart request
- **WHEN** record is invoked for session ID whose WAL or final session destination already exists under a shared root
- **THEN** P0 fails clearly rather than repairing, truncating, resuming, or overwriting that session; unrelated existing sessions in root remain valid

### Requirement: Bounded ordered session assembly
Session processing SHALL use existing `SessionAssembler` limits and `ConnectionKey` grouping. Chunks SHALL be sorted by global monotonic sequence while retaining direction. Duplicate/missing WAL sequences SHALL be rejected; repeated bytes under different sequences SHALL remain distinct. Silent drops SHALL NOT occur.

#### Scenario: Fragmented bidirectional stream
- **WHEN** request and response are split across interleaved directional chunks
- **THEN** assembled stream contains all chunks in global sequence order with original directions

#### Scenario: Memory bound exceeded
- **WHEN** connection count, bytes per connection, or chunks per connection exceed configured limits
- **THEN** processing fails with typed limit error and publishes no partial final session

#### Scenario: Missing network bytes
- **WHEN** sequence is contiguous but source flags event truncated
- **THEN** assembler does not invent bytes and propagates truncation to connection and operations

### Requirement: Deterministic session completion
Finite fixture EOF SHALL trigger assembly finish. Session/connection SHALL be incomplete when WAL tail is partial, any chunk is truncated, HTTP processing leaves residual bytes, request lacks response, or unsupported framing is preserved. Complete operations before incomplete evidence SHALL remain inspectable.

#### Scenario: Complete fixture exchange
- **WHEN** fixture ends after complete supported request and final response
- **THEN** resulting connection and operation are complete unless another issue exists

#### Scenario: Fixture ends mid-message
- **WHEN** fixture ends before declared HTTP head/body length is complete
- **THEN** result contains deterministic truncated warning and opaque/incomplete evidence rather than fabricated completion
