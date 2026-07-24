## ADDED Requirements

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
HTTP decoder SHALL independently buffer each direction and parse request/status lines, ordered headers, no-body messages, and exact `Content-Length` bodies across arbitrary chunk boundaries. It SHALL process multiple complete messages in one chunk and multiple sequential exchanges on one connection under fixed memory/header limits.

#### Scenario: Fragmented request head and body
- **WHEN** request line, headers, and fixed-length body span several client-to-server chunks
- **THEN** decoder emits one complete request only after declared bytes arrive

#### Scenario: Fragmented response
- **WHEN** response status, headers, and body span several server-to-client chunks
- **THEN** decoder emits one complete response with exact status, headers, and body bytes

#### Scenario: Multiple sequential exchanges
- **WHEN** connection carries request/response followed by second request/response
- **THEN** canonicalizer emits two operations ordered by request sequence

#### Scenario: Coalesced messages
- **WHEN** one socket chunk contains end of one message and all or part of next
- **THEN** decoder consumes exact first boundary and continues parsing remainder

#### Scenario: Content-Length zero and request without length
- **WHEN** message has `Content-Length: 0` or request omits both Content-Length and Transfer-Encoding
- **THEN** message completes with empty body

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
Canonicalizer SHALL correlate final responses to requests FIFO by completed HTTP message order. HEAD response body rules SHALL use queued request method. Missing peers SHALL remain visible and incomplete.

#### Scenario: Sequential FIFO correlation
- **WHEN** two complete request/response pairs are observed sequentially
- **THEN** each response attaches to matching oldest request

#### Scenario: HEAD response
- **WHEN** HEAD request receives final response with Content-Length header but no body bytes
- **THEN** response completes without waiting for body and recorded body is empty

#### Scenario: Missing response
- **WHEN** fixture ends after complete request without final response
- **THEN** operation has no recorded response, warning, incomplete non-executable state, and Inconclusive verification expectation

#### Scenario: Orphan response
- **WHEN** complete response has no pending request
- **THEN** system preserves server bytes as opaque non-replayable evidence

### Requirement: Explicit unsupported and malformed behavior
Decoder SHALL NOT silently claim support for chunked transfer coding, ordinary close-delimited response bodies, informational 1xx exchanges, CONNECT, Upgrade/WebSocket, absolute/authority-form targets, non-HTTP/1.1 versions, or over-limit heads. It SHALL preserve affected bytes as opaque evidence with stable warning code and block replay. Complete earlier operations SHALL remain.

#### Scenario: Chunked message
- **WHEN** message declares Transfer-Encoding including chunked
- **THEN** operation/connection reports `unsupported_transfer_encoding`, preserves opaque evidence, and is not replayable

#### Scenario: Close-delimited response
- **WHEN** ordinary final response permits body but has neither supported Content-Length nor no-body status/method rule
- **THEN** response reports `unsupported_close_delimited_body` and is not treated as empty success

#### Scenario: Informational response
- **WHEN** 1xx response appears before final response
- **THEN** affected exchange reports `unsupported_informational_response` and does not silently shift FIFO correlation

#### Scenario: Truncated message
- **WHEN** decoder finishes with partial head or fewer body bytes than Content-Length
- **THEN** it emits `truncated_message` opaque/incomplete evidence without panic or byte loss

#### Scenario: Malformed start line or header
- **WHEN** HTTP-like stream contains complete invalid syntax
- **THEN** it emits stable malformed warning and preserves directional opaque bytes

#### Scenario: Pipelined requests
- **WHEN** more than one request completes before oldest final response
- **THEN** decoder may correlate FIFO for inspection but records pipeline depth and marks connection non-replayable

### Requirement: Typed canonical HTTP operation
Each supported exchange SHALL become `CanonicalOperation` with protocol `http/1.1`, request kind, classified effect, request/response body refs, offsets, endpoints at connection level, truncation/incomplete state, stable warnings, replay attributes, and verification expectation. Essential HTTP semantics SHALL live in typed versioned `HttpOperationDataV1`, not solely string attributes.

#### Scenario: Canonical GET
- **WHEN** complete GET exchange is decoded
- **THEN** operation effect is Read and typed data preserves method, target, ordered headers, status, message sequences, and verification fields

#### Scenario: Canonical POST
- **WHEN** complete POST exchange is decoded
- **THEN** operation effect is Write and request body ref contains exact body bytes

#### Scenario: Unknown method
- **WHEN** syntactically valid unsupported method is decoded
- **THEN** operation effect is Unknown and replay policy denies it by default

#### Scenario: Timing conversion
- **WHEN** fixture events contain wall timestamps
- **THEN** request/response offsets are deterministic relative to session start; missing timestamps produce warning rather than fabricated precision

### Requirement: Canonical schema compatibility
Writer SHALL emit Canonical Session v2 with backend-neutral Artifact payload refs and structured canonical warnings. Reader SHALL accept v1 and v2, default missing v1 warning lists to empty, retain existing S3 Object variant, and reject unknown newer schema.

#### Scenario: Read existing v1 session
- **WHEN** valid schema v1 session is loaded
- **THEN** it remains readable with empty new warning fields

#### Scenario: Write HTTP session
- **WHEN** HTTP session is persisted
- **THEN** schema version is 2 and HTTP protocol-data media type/version identifies `HttpOperationDataV1`

#### Scenario: Unknown canonical version
- **WHEN** filesystem manifest/session declares unsupported newer canonical schema
- **THEN** inspect/replay fail typed compatibility error without partial interpretation

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
