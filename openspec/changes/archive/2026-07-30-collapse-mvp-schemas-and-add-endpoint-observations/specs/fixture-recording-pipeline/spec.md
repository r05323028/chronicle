## MODIFIED Requirements

### Requirement: Versioned deterministic fixture input
System SHALL accept sole mutable fixture schema v1 as bounded JSON containing unique socket definitions and globally ordered capture observations. Validation SHALL enforce total encoded WAL size below configured one-segment limit in addition to per-connection/session limits. Each socket SHALL provide fixture-local ID, deterministic socket identity inputs, optional network namespace, local endpoint, remote endpoint, active/passive role, TCP transport, and modeled process/file-descriptor metadata. Each payload event SHALL provide contiguous sequence, timestamp, socket reference, direction, strict hexadecimal payload, truncation flag, and capture flags. Fixture IDs SHALL only resolve input references and SHALL NOT become canonical persistence format.

#### Scenario: Valid binary fixture
- **WHEN** fixture v1 contains socket evidence plus fragmented HTTP bytes including non-UTF-8 body bytes encoded by `payload_hex`
- **THEN** fixture source emits sole active `CaptureEvent` v1 values in fixture sequence order without UTF-8 conversion

#### Scenario: Unsupported fixture version
- **WHEN** fixture schema version is not 1
- **THEN** record fails before creating WAL or session artifacts with stable unsupported-schema error

#### Scenario: Invalid socket reference or payload
- **WHEN** event references unknown socket, uses invalid hex, non-TCP transport, invalid endpoint, unknown role, duplicate sequence, gap, or decreasing timestamp
- **THEN** record fails before any WAL append and identifies invalid field without printing payload bytes

#### Scenario: Socket identity reuse ambiguity
- **WHEN** two fixture socket IDs resolve to same current `SocketIdentity`
- **THEN** fixture validation rejects input rather than merging distinct socket generations

### Requirement: Fixture source uses capture boundary
Fixture adapter SHALL implement or feed existing `CaptureSource` contract and emit same sole active Capture Event v1 model as production. It SHALL emit socket evidence before dependent payload fragments and SHALL NOT call HTTP, canonical, storage, or replay code. End of fixture SHALL mean finite capture batch completion and SHALL NOT claim TCP FIN or packet reassembly.

#### Scenario: Capture source exhaustion
- **WHEN** final fixture event has been emitted
- **THEN** source returns end-of-stream and application proceeds to WAL flush without synthesizing lifecycle bytes

#### Scenario: Truncation propagation
- **WHEN** fixture payload event has `truncated=true`
- **THEN** emitted `CaptureEvent` carries truncation and downstream connection is never reported complete/replayable

#### Scenario: Source parity
- **WHEN** equivalent fixture and production socket evidence enters capture boundary
- **THEN** both sources produce same `CaptureEvent` Rust type and schema version 1
- **AND** neither source requires version conversion before reconstruction

### Requirement: Bounded ordered session assembly
Session processing SHALL use existing configured limits and `SocketIdentity` generation grouping for both fixture and production capture. Events SHALL be ordered by persisted provenance while retaining observed traffic direction. Duplicate/missing WAL sequences SHALL be rejected; repeated bytes under different sequences SHALL remain distinct. Socket evidence SHALL be cached and used for endpoint/role reconstruction. Silent drops and placeholder endpoints SHALL NOT occur.

#### Scenario: Fragmented bidirectional stream
- **WHEN** request and response are split across interleaved payload fragments after socket evidence
- **THEN** assembled stream contains all fragments in deterministic order with directions mapped relative to socket role
- **AND** canonical endpoints come from cached socket evidence

#### Scenario: Memory bound exceeded
- **WHEN** connection count, bytes per connection, chunks per connection, or socket-evidence cache exceeds configured limits
- **THEN** processing fails with typed limit error and publishes no partial final session

#### Scenario: Missing network bytes
- **WHEN** sequence is contiguous but source flags event truncated
- **THEN** assembler does not invent bytes and propagates truncation to connection and operations
