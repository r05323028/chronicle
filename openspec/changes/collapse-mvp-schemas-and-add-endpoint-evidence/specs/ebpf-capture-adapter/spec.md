## MODIFIED Requirements

### Requirement: Kernel observations remain behind capture boundary
The system SHALL decode Aya/eBPF ABI v1 values into adapter-owned `RawKernelObservation` and SHALL convert that value through `CaptureAdapter` before emitting sole active Capture Event v1. No earlier Capture Event model or compatibility reader SHALL remain. Aya handles, generated eBPF structs, map layouts, ABI padding, and verifier-specific representations MUST NOT appear in `chronicle-capture`, WAL, protocol, application, or CLI domain APIs.

#### Scenario: Kernel ABI does not leak
- **WHEN** valid Aya ring-buffer item is decoded
- **THEN** only `RawKernelObservation` is passed to `CaptureAdapter`
- **AND** only `CaptureEvent` or typed adapter failure crosses adapter boundary

#### Scenario: Production ABI v1 is explicit
- **WHEN** production ring-buffer item is decoded
- **THEN** it carries `CHRN` magic, little-endian ABI version 1, discriminant, byte-order marker, and declared item size
- **AND** connect-intent item carries bounded remote endpoint and active role while established socket evidence carries bounded local/remote endpoints and active/passive role
- **AND** no ABI v2 dispatch exists

#### Scenario: Dependency direction remains stable
- **WHEN** capture adapter dependencies are evaluated
- **THEN** `chronicle-capture-ebpf` MAY depend on `chronicle-capture`
- **AND** `chronicle-capture` MUST NOT depend on Aya or `chronicle-capture-ebpf`

### Requirement: Capture events represent observed evidence only
`CaptureEvent { schema_version: 1, kind }` SHALL represent observed kernel evidence using socket-connect-observed, separately proven socket-connected, socket-close-observed, socket-reset-observed, raw socket-state-changed, payload-fragment, and loss-window-observed kinds. Socket evidence SHALL preserve local endpoint, remote endpoint, and observed active/passive role. Capture events MUST NOT encode request completion, response completion, replayability, canonical meaning, WAL order, durability, or derived connection completeness.

#### Scenario: Connect evidence maps to domain evidence
- **WHEN** `CaptureAdapter` receives valid pre-autobind connect observation
- **THEN** it emits connect-intent evidence with identity, timestamp, family, process, observed cgroup ID, remote endpoint, and active role
- **AND** it does not claim local endpoint or successful establishment

#### Scenario: Establishment maps complete socket evidence
- **WHEN** `CaptureAdapter` receives active or passive established observation
- **THEN** it emits `SocketConnected` with complete validated local/remote endpoints and role
- **AND** complete socket cache becomes available for dependent payload

#### Scenario: Application semantics are absent
- **WHEN** payload bytes resemble HTTP request or response
- **THEN** adapter emits only payload-fragment evidence
- **AND** it does not emit request, response, operation, replay, or canonical-session meaning

#### Scenario: Capture schema remains v1
- **WHEN** endpoint evidence is serialized
- **THEN** event schema version is 1
- **AND** no `CaptureEventV1`, `CaptureEventV2`, or V1/V2 dispatch enum is used

### Requirement: Payload fragments preserve transport evidence
`PayloadFragment` SHALL contain socket identity, recording-scope identity, network family, direction, kernel monotonic timestamp, transport/continuation sequence information, payload bytes, and truncation metadata. It MUST NOT duplicate local endpoint, remote endpoint, or socket role; those values SHALL be resolved from cached `SocketEvidence`. Multiple fragments per connection SHALL be supported and ordering evidence SHALL be preserved. Adapter MUST NOT assume one skb equals one application payload and MUST NOT claim global total order across CPUs or hooks.

#### Scenario: Multiple fragments remain ordered
- **WHEN** one large payload produces multiple kernel observations or continuations
- **THEN** adapter emits multiple `PayloadFragment` events with enough sequence and continuation information to preserve observed order

#### Scenario: Truncation remains explicit
- **WHEN** only part of observed payload is readable or retained
- **THEN** emitted fragment records captured length, observed length when known, truncation state, and reason or ambiguity
- **AND** missing bytes are not synthesized

#### Scenario: Adapter does not reconstruct TCP
- **WHEN** fragments overlap, contain gaps, or appear retransmitted
- **THEN** adapter preserves observed sequence evidence
- **AND** it does not deduplicate, merge, fill gaps, or determine stream completeness

#### Scenario: Endpoint evidence is not repeated
- **WHEN** payload is emitted for cached socket identity
- **THEN** fragment contains no endpoint tuple or role
- **AND** cached socket evidence remains sole provenance source

### Requirement: Portable conversion tests cover capture boundary
Unit tests SHALL cover `RawKernelObservation` conversion, sole Capture Event v1 mapping, socket identity preservation, pre-autobind connect intent, IPv4/IPv6 established local and remote endpoint preservation, active/passive role, lifecycle evidence conversion, payload-fragment ordering, loss-event conversion, cache correlation, and invalid-event rejection without requiring privileged kernel access.

#### Scenario: Constructed observations exercise conversion
- **WHEN** portable tests provide valid and invalid constructed raw observations
- **THEN** required endpoint, role, identity, ordering, lifecycle, loss ambiguity, cache, and typed rejection paths are asserted
