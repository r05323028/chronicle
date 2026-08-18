## Purpose

Supported HTTP/1.1 detection, decoding, correlation, canonical mapping, opaque fallback, and capability registration.

## Requirements

### Requirement: Honest HTTP/1.1 detection

Built-in HTTP detector SHALL inspect bounded assembled socket bytes and report HTTP/1.1 only for plausible valid HTTP/1.1 request or response prefixes. It SHALL distinguish confirmed, probable, need-more-data, rejected, and unknown outcomes and SHALL reject TLS records and HTTP/2 preface.

#### Scenario: Fragmented request line

- **WHEN** client request line is split across socket chunks but completes as valid `METHOD target HTTP/1.1\r\n`
- **THEN** detector identifies `http/1.1` after bounded reassembly

#### Scenario: Incomplete plausible prefix

- **WHEN** available bytes are valid prefix but insufficient for request line
- **THEN** detector reports need-more-data rather than another protocol or malformed completion

#### Scenario: Non-HTTP bytes

- **WHEN** stream begins with TLS record, HTTP/2 preface, or unrelated opaque bytes
- **THEN** detector does not claim supported HTTP/1.1

### Requirement: Stateful fixed-length HTTP decoding

HTTP connection decoder SHALL consume reconstructed directional bytes plus deterministic cross-direction completion order, userspace-derived termination, and source-integrity state. Capture-level lifecycle SHALL remain raw (`established`, `state_changed`, `closed`, `reset`, `unknown`); decoder SHALL use `clean_close`/`half_close` only when userspace can derive it from feasibility-proven evidence. It SHALL parse request/status lines, ordered headers, no-body messages, exact `Content-Length` bodies, chunked response transfer coding, and valid response-to-close bodies across arbitrary capture/TCP chunk boundaries. Chunked requests SHALL be unsupported/non-replayable. Decoder SHALL process persistent keep-alive and multiple sequential exchanges under fixed memory/header/body limits without assuming one capture event equals one message.

#### Scenario: Fragmented request head and body

- **WHEN** valid request head and Content-Length body are split across several reconstructed chunks including head/body boundary
- **THEN** decoder emits exactly one complete request with exact body after enough bytes arrive

#### Scenario: Fragmented response

- **WHEN** valid response head or body is split across directional chunks
- **THEN** decoder emits response only after declared bytes arrive

#### Scenario: Multiple sequential exchanges

- **WHEN** keep-alive connection carries sequential request/response pairs
- **THEN** decoder emits all messages in byte order without mixing bodies

#### Scenario: Coalesced messages

- **WHEN** one reconstructed slice contains end of one message and start of next
- **THEN** decoder consumes exact first frame and continues parsing remainder

#### Scenario: Content-Length zero and request without length

- **WHEN** request declares zero length or supported request has no body framing
- **THEN** decoder emits empty body without consuming next message bytes

#### Scenario: Chunked response

- **WHEN** valid chunked response and terminating zero chunk span multiple reconstructed chunks
- **THEN** decoder emits dechunked exact body and preserves bounded trailer metadata

#### Scenario: Close-delimited response

- **WHEN** response is validly framed by userspace-derived trusted clean close and source integrity is complete
- **THEN** decoder emits body at close with provenance and complete state

#### Scenario: Unproven directional half-close

- **WHEN** capture provides only raw state change/unknown termination not proven by feasibility gate
- **THEN** decoder SHALL NOT infer close-delimited completion or claim half-close support

#### Scenario: Body limit exceeded

- **WHEN** declared or decoded body exceeds configured 8 MiB operation limit
- **THEN** decoder bounds memory and emits truncated/non-replayable evidence rather than complete operation

### Requirement: Header and body fidelity

Canonical HTTP representation SHALL normalize header names to lowercase ASCII while preserving duplicate field order and raw value bytes. Content-Length SHALL be accepted only as exactly one unsigned decimal field; duplicates (even identical), comma-list form, signs, overflow, and invalid values SHALL be malformed. Bodies SHALL remain arbitrary bytes and SHALL NOT require UTF-8.

#### Scenario: Duplicate headers

- **WHEN** request or response contains repeated valid header name
- **THEN** typed HTTP data retains each value in wire order rather than collapsing into map

#### Scenario: Binary body

- **WHEN** fixed-length body contains invalid UTF-8 or zero bytes
- **THEN** body reference preserves exact bytes and size

#### Scenario: Duplicate or invalid Content-Length

- **WHEN** Content-Length is duplicated, comma-listed, signed, overflowing, non-decimal, or otherwise invalid
- **THEN** affected message becomes deterministic malformed/opaque evidence and is non-replayable

### Requirement: Request-response correlation

HTTP connection decoder SHALL correlate final responses to requests FIFO by deterministic cross-direction message-completion order (kernel monotonic timestamp then WAL sequence). HEAD response body rules SHALL use queued request method. Missing peers SHALL remain visible with `unmatched` completeness. More than one simultaneously outstanding request SHALL be treated as pipelining: affected messages SHALL be preserved, marked unsupported/non-replayable, and SHALL NOT be silently treated as ordinary sequential exchanges. Canonicalizer SHALL map completed decoder exchanges rather than rediscover framing/order.

#### Scenario: Sequential FIFO correlation

- **WHEN** two non-pipelined requests and final responses arrive in order on keep-alive connection
- **THEN** canonicalizer produces two ordered complete operations with correct peers

#### Scenario: HEAD response

- **WHEN** queued HEAD request receives response with Content-Length metadata and no body
- **THEN** response completes without consuming following bytes as body

#### Scenario: Missing response

- **WHEN** stream ends with request and no final response
- **THEN** canonicalizer emits unmatched request operation with missing expectation and blocks replay

#### Scenario: Orphan response

- **WHEN** final response appears without queued request
- **THEN** canonicalizer preserves unmatched response evidence and does not invent request

#### Scenario: HTTP pipelining

- **WHEN** second request completes before first final response in cross-direction completion order
- **THEN** affected exchanges are marked unsupported/non-replayable with stable pipelining warning

#### Scenario: Same bytes sequential interleaving

- **WHEN** identical directional bytes complete request1/response1 before request2/response2
- **THEN** decoder treats exchanges as sequential rather than pipelined

### Requirement: Explicit unsupported and malformed behavior

Decoder SHALL NOT silently claim support for chunked requests/request trailers, informational 1xx exchanges, CONNECT, Upgrade/WebSocket, ambiguous Content-Length/Transfer-Encoding combinations, absolute/authority-form targets, non-HTTP/1.1 versions, malformed response chunk framing, pipelining, or over-limit heads/bodies. It SHALL distinguish malformed, incomplete, truncated, unmatched, and unsupported states, preserve bounded affected bytes/provenance as evidence, and block replay. Complete earlier operations SHALL remain.

#### Scenario: Informational response

- **WHEN** stream contains 1xx response sequence
- **THEN** decoder marks affected exchange unsupported without consuming later bytes as ordinary final response

#### Scenario: Truncated message

- **WHEN** loss/truncation or stream end leaves declared head/body/chunk incomplete
- **THEN** decoder emits truncated or incomplete evidence and never complete operation

#### Scenario: Malformed start line or header

- **WHEN** syntax is invalid after enough bytes establish malformed input
- **THEN** decoder emits malformed evidence with stable code and does not panic

#### Scenario: Chunked request

- **WHEN** request uses Transfer-Encoding chunked or request trailers
- **THEN** decoder preserves bounded evidence and marks request unsupported/non-replayable

#### Scenario: Malformed response chunk

- **WHEN** response chunk size, terminator, or trailer framing is invalid
- **THEN** decoder emits malformed/non-replayable evidence and retains bounded provenance

#### Scenario: Unsupported upgrade behavior

- **WHEN** request/response attempts protocol upgrade or WebSocket
- **THEN** decoder marks operation unsupported and does not interpret upgraded bytes as HTTP/1.1

#### Scenario: Pipelined requests

- **WHEN** multiple requests are outstanding before responses
- **THEN** decoder/canonicalizer marks affected operations unsupported and preserves complete prior non-pipelined operations

### Requirement: Typed canonical HTTP operation

Each decoded exchange SHALL become `CanonicalOperation` with protocol `http/1.1`, request kind, classified effect, request/response body refs, offsets, endpoints at connection level, typed completeness, stable warnings, replay attributes, verification expectation, source recording/connection identity, WAL envelope provenance ranges, and intersecting/ambiguous loss-window provenance. Essential HTTP semantics SHALL live in typed versioned HTTP protocol data, not solely string attributes. Incomplete, truncated, malformed, unmatched, unsupported, and ambiguous-loss exchanges SHALL NOT be executable; complete supported operations outside loss windows SHALL remain executable even in partial session.

#### Scenario: Canonical GET

- **WHEN** complete GET exchange is canonicalized
- **THEN** typed method/target/headers/status, complete state, provenance, read effect, and replayable metadata are present

#### Scenario: Canonical POST

- **WHEN** complete POST with binary body is canonicalized
- **THEN** write effect and exact request/response payload refs are present without requiring UTF-8

#### Scenario: Unknown method

- **WHEN** syntactically valid extension method is captured
- **THEN** method bytes are preserved, effect is Unknown, and replay remains denied by default

#### Scenario: Unmatched operation

- **WHEN** only request or only response is decoded
- **THEN** canonical operation records unmatched state, missing peer, source provenance, and replay blocker

#### Scenario: Mixed completeness

- **WHEN** one complete supported operation is provably outside loss window and another operation intersects it
- **THEN** first remains executable, second is non-executable with loss provenance, and session aggregates partial completeness

#### Scenario: Timing conversion

- **WHEN** event timestamps are nondecreasing
- **THEN** operation offsets are deterministic relative to session start and cannot become negative

### Requirement: Capability registration integrity

HTTP registration SHALL mark detection, decoding, canonicalization, replay, and verification Available only when corresponding implementation object is registered. Fake protocol SHALL remain Available. No other protocol status SHALL become Available.

#### Scenario: Registration integrity test

- **WHEN** built-in registry is constructed
- **THEN** each Available status exactly matches non-None implementation for every capability

#### Scenario: HTTP implementation complete

- **WHEN** this change passes acceptance gates
- **THEN** HTTP/1.1 has five Available capabilities and fake remains unchanged

#### Scenario: Other protocol matrix

- **WHEN** documentation/registry is inspected
- **THEN** PostgreSQL, MySQL-family, MySQL, MariaDB, Oracle, MongoDB, Kafka, and NATS retain existing non-Available statuses

### Requirement: Canonical Session v1

Writer SHALL emit sole Canonical Session v1 with backend-neutral Artifact payload refs, structured warnings, typed completeness, endpoint evidence, and typed recording/WAL provenance. Reader SHALL accept only `schema_version == 1` and reject every other value. Canonical Session v1 is a frozen public contract within 0.1.x under `public-compatibility-boundary`; its existing field meanings, integrity/provenance semantics, and replay-relevant completeness behavior SHALL NOT be silently changed. Additive optional metadata may remain version 1 only when it preserves existing field meaning, remains readable when absent, and is covered by the public compatibility review. `PayloadRef::Object` remains supported. WAL, protocol payload, and Session Manifest v1 remain separate contracts.

`CanonicalSession` SHALL own source provenance, connection/operation completeness maps, and replay attributes directly. Completeness maps SHALL be authoritative; connection/operation flags SHALL not duplicate them. Timeline SHALL be sole authoritative operation order.

#### Scenario: Write production HTTP session

- **WHEN** the recording ETL publishes a production session
- **THEN** session uses Canonical Session v1 and typed HTTP/provenance structures

#### Scenario: Non-v1 canonical version

- **WHEN** reader sees schema version zero, two, or newer
- **THEN** it rejects explicitly, performs no replay, and reports unsupported version without mutating the session

#### Scenario: Append compatible optional metadata

- **WHEN** Chronicle adds optional canonical metadata without changing existing field meaning
- **THEN** writer and reader remain Canonical Session v1, existing readers continue to handle the absent field, and the compatibility review records the additive change

### Requirement: Reconstructed stream provenance

HTTP decoder SHALL consume ordered directional stream slices with source WAL ranges and connection generation. Every decoded or residual message SHALL retain sufficient byte-range provenance to identify contributing segment, record sequence, direction, and payload range without requiring raw values in default output.

#### Scenario: Header split across WAL records

- **WHEN** one HTTP header spans multiple capture/WAL records
- **THEN** decoded message references all contributing provenance ranges

#### Scenario: Missing payload provenance

- **WHEN** reconstruction identifies gap between ranges
- **THEN** message is incomplete and provenance identifies gap boundary

### Requirement: HTTP decoder test matrix

Automated tests SHALL cover Content-Length request/response, chunked response, no-body response, valid close-delimited response, keep-alive, sequential exchanges, split head/body boundaries, malformed request/response lines, incomplete heads/bodies, unmatched request/response, upgrade rejection, pipelining rejection, truncation, and deterministic ordering.

#### Scenario: Decoder suite

- **WHEN** rootless workspace tests run
- **THEN** every required framing/completeness/pairing case passes using reconstructed event fixtures
