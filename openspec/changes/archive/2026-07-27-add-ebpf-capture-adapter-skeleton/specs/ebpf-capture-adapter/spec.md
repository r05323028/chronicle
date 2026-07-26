## ADDED Requirements

### Requirement: Kernel observations remain behind capture boundary
The system SHALL decode Aya/eBPF ABI values into an adapter-owned `RawKernelObservation` and SHALL convert that value through `CaptureAdapter` before emitting a versioned Capture Event v2. Existing Capture Event v1 fixture bytes and readers SHALL remain compatible. Aya handles, generated eBPF structs, map layouts, ABI padding, and verifier-specific representations MUST NOT appear in `chronicle-capture`, WAL, protocol, application, or CLI domain APIs.

#### Scenario: Kernel ABI does not leak
- **WHEN** a valid Aya ring-buffer item is decoded
- **THEN** only `RawKernelObservation` is passed to `CaptureAdapter`
- **AND** only a `CaptureEvent` or typed adapter failure crosses the adapter boundary

#### Scenario: Production ABI v1 is explicit
- **WHEN** a production ring-buffer item is decoded
- **THEN** it carries `CHRN` magic, little-endian ABI version 1, discriminant, byte-order marker, and declared item size
- **AND** each accepted discriminant has only documented bounded fields
- **AND** retained Gate A `ProbeEvent` bytes are not treated as an unversioned production ABI

#### Scenario: Dependency direction remains stable
- **WHEN** capture adapter dependencies are evaluated
- **THEN** `chronicle-capture-ebpf` MAY depend on `chronicle-capture`
- **AND** `chronicle-capture` MUST NOT depend on Aya or `chronicle-capture-ebpf`

### Requirement: Capture events represent observed evidence only
`CaptureEvent` SHALL represent observed kernel evidence using socket-connect-observed, separately proven socket-connected, socket-close-observed, socket-reset-observed, raw socket-state-changed, payload-fragment, and loss-window-observed kinds. It MUST NOT encode request completion, response completion, replayability, canonical meaning, WAL order, durability, or derived connection completeness.

#### Scenario: Connect evidence maps to domain evidence
- **WHEN** `CaptureAdapter` receives a valid connect observation
- **THEN** it emits `SocketConnectObserved` with preserved identity, timestamp, family, process, and observed cgroup ID evidence
- **AND** it does not claim successful connection establishment

#### Scenario: Application semantics are absent
- **WHEN** payload bytes resemble an HTTP request or response
- **THEN** the adapter emits only payload-fragment evidence
- **AND** it does not emit request, response, operation, replay, or canonical-session meaning

### Requirement: Socket identity resists reuse
The system SHALL define `SocketIdentity` from `socket_cookie`, `first_seen_monotonic_timestamp`, and network namespace identity when available. Boot/clock identity SHALL accompany monotonic timing. PID/TGID and five-tuple values MAY be retained as evidence but MUST NOT identify a connection.

#### Scenario: Same observed connection remains correlated
- **WHEN** connect, lifecycle, and payload observations carry the same socket cookie and first-seen generation in one boot/clock domain
- **THEN** the adapter preserves one `SocketIdentity` across emitted events

#### Scenario: Tuple or PID reuse does not merge connections
- **WHEN** observations reuse a five-tuple or PID but differ in socket cookie, first-seen generation, boot/clock identity, or available network namespace identity
- **THEN** the adapter preserves distinct socket identities

#### Scenario: Required identity is missing
- **WHEN** an observation requiring correlation lacks socket cookie or first-seen monotonic generation
- **THEN** the adapter returns a typed missing-identity failure
- **AND** it does not synthesize identity from PID or five-tuple

### Requirement: Recording scope uses stable cgroup identity
Attributable capture events SHALL include `RecordingScopeIdentity` containing cgroup ID, canonical path, and namespace information. Socket-owner process and observed descendant cgroup ID SHALL originate from connect evidence joined by `SocketIdentity`. Canonical path and namespace information SHALL originate from caller-supplied recording-scope configuration validated before attachment; selector preflight remains outside this slice. Cgroup-skb execution-context identity MUST NOT replace socket-owner identity.

#### Scenario: Stable cgroup scope is preserved
- **WHEN** connect evidence includes a descendant cgroup ID belonging to the caller-supplied validated recording scope
- **THEN** the adapter joins that observed ID with configured canonical path and namespace information
- **AND** later correlated lifecycle and payload events preserve the same `RecordingScopeIdentity`

#### Scenario: Path alone is insufficient
- **WHEN** a canonical cgroup path is present without stable cgroup ID
- **THEN** the adapter rejects required scope attribution as missing identity
- **AND** it does not treat path text alone as stable scope identity

### Requirement: Lifecycle events preserve observation boundaries
The adapter SHALL emit connect-observed evidence for connect4/connect6 attribution without claiming establishment. It SHALL emit connected, close-observed, or reset-observed evidence only when corresponding raw signals are separately proven. Ambiguous sock-ops state changes SHALL remain raw state-change evidence. The capture layer MUST NOT derive clean close, half-close, aborted connection, or directional half-close.

#### Scenario: Active establish is separately proven
- **WHEN** validated sock-ops active-establish evidence correlates with a connect observation
- **THEN** the adapter emits `SocketConnected`
- **AND** connect4/connect6 attribution alone remains `SocketConnectObserved`

#### Scenario: Proven close signal is preserved
- **WHEN** a validated raw hook signal proves a close observation
- **THEN** the adapter emits `SocketClosedObserved`
- **AND** it does not classify clean close or half-close

#### Scenario: Proven reset signal is preserved
- **WHEN** a validated raw hook signal proves a reset observation
- **THEN** the adapter emits `SocketResetObserved`
- **AND** it does not classify replayability or connection completeness

#### Scenario: Ambiguous lifecycle remains ambiguous
- **WHEN** sock-ops state evidence cannot distinguish orderly close, reset, or directional half-close
- **THEN** the adapter emits raw state-change evidence or a typed unsupported-evidence result
- **AND** it does not emit close, reset, or half-close as fact

### Requirement: Payload fragments preserve transport evidence
`PayloadFragment` SHALL contain socket identity, recording-scope identity, network family, direction, kernel monotonic timestamp, transport/continuation sequence information, payload bytes, and truncation metadata. Multiple fragments per connection SHALL be supported and ordering evidence SHALL be preserved. The adapter MUST NOT assume one skb equals one application payload and MUST NOT claim a global total order across CPUs or hooks.

#### Scenario: Multiple fragments remain ordered
- **WHEN** one large payload produces multiple kernel observations or continuations
- **THEN** the adapter emits multiple `PayloadFragment` events with enough sequence and continuation information to preserve observed order

#### Scenario: Truncation remains explicit
- **WHEN** only part of an observed payload is readable or retained
- **THEN** the emitted fragment records captured length, observed length when known, truncation state, and reason or ambiguity
- **AND** missing bytes are not synthesized

#### Scenario: Adapter does not reconstruct TCP
- **WHEN** fragments overlap, contain gaps, or appear retransmitted
- **THEN** the adapter preserves their observed sequence evidence
- **AND** it does not deduplicate, merge, fill gaps, or determine stream completeness

### Requirement: Loss evidence remains temporal and unattributed
The adapter SHALL emit `LossWindowObserved` for positive counter deltas and ambiguous loss-sampling transitions. Each loss event SHALL contain start timestamp, end timestamp, drop delta when trustworthy, loss epoch or generation when available, and ambiguity information. `LossWindowObserved` SHALL remain capture-stream evidence distinct from any future persisted WAL `LossWindow` payload. The adapter MUST NOT identify affected connections or decide completeness or replayability.

#### Scenario: Positive loss delta emits window
- **WHEN** two complete cumulative counter samples on the same monotonic clock produce a positive delta
- **THEN** the adapter emits `LossWindowObserved` bounded by actual sample timestamps with the delta and available generation

#### Scenario: Delayed sampling uses actual interval
- **WHEN** loss-counter sampling occurs later than scheduled
- **THEN** the emitted loss window uses actual previous and current successful sample timestamps
- **AND** it does not fabricate a nominal interval

#### Scenario: Ambiguous sampling stays ambiguous
- **WHEN** counters reset or regress, a map is replaced, a per-CPU read is incomplete, or clock/boot identity changes
- **THEN** the adapter emits ambiguity information or a typed ring-loss failure
- **AND** it does not attribute loss to any socket

#### Scenario: Loss before first successful sample
- **WHEN** the first complete cumulative sample after attachment contains a positive delta
- **THEN** `LossWindowObserved` spans attachment time through the first-sample timestamp
- **AND** marks exact drop timing and affected sockets unknown

#### Scenario: Zero delta does not add evidence event
- **WHEN** a complete loss sample has zero delta
- **THEN** the adapter advances its internal successful-sample boundary
- **AND** it MAY emit no `LossWindowObserved`

#### Scenario: Shutdown final sample
- **WHEN** capture shutdown begins
- **THEN** producer links are detached or quiesced, remaining ring items are drained, and the mandatory final sample occurs while counter maps remain available
- **AND** maps and programs are released only after that sample

#### Scenario: Final sample failure remains visible
- **WHEN** the mandatory final sample fails
- **THEN** ambiguity spans the previous successful sample through shutdown, or attachment time through shutdown when no successful sample exists
- **AND** the adapter returns typed ring-loss or sampling failure instead of clean completion

### Requirement: Capture sources have an ordered lifecycle

The system SHALL define `CaptureSource` lifecycle responsibilities equivalent to `start`, `poll`, `request_shutdown`, `drain`, and `finalize`, with mandatory state order `Created -> Running -> ShutdownRequested -> Draining -> Finalized`. `poll` SHALL consume only currently available normalized capture events and SHALL NOT perform final loss sampling or release resources. `request_shutdown` SHALL be idempotent, stop acceptance of new kernel capture input, preserve already accepted events for draining, and make the final loss sample eligible. `drain` SHALL emit all accepted pending events and final loss evidence without silently discarding buffered ring items. `finalize` SHALL detach producer programs and release maps, ring buffers, and other source resources only after shutdown and drain complete; finalization before drain completion SHALL return a typed lifecycle failure.

CaptureSource owns kernel acquisition, final loss sampling, event drain, and cleanup. It MUST NOT own recording status, shutdown reason, WAL commit behavior, or replay/completeness decisions.

#### Scenario: Graceful shutdown emits final loss evidence
- **WHEN** shutdown is requested during active capture
- **THEN** intake stops, a final `CLOCK_MONOTONIC` counter sample is collected while counter maps remain available, and a final `LossWindowObserved` is emitted when required
- **AND** accepted buffered events drain before source resources release

#### Scenario: Pending events survive shutdown
- **WHEN** the source contains accepted ring events when shutdown is requested
- **THEN** those events remain available through `drain`
- **AND** they are not silently discarded

#### Scenario: Delayed shutdown uses actual observation time
- **WHEN** processing after shutdown request is delayed
- **THEN** the final loss timestamp is the actual `CLOCK_MONOTONIC` counter-observation time
- **AND** the loss window reflects elapsed time rather than assuming request time equals sample time

#### Scenario: Shutdown precedes first successful loss sample
- **WHEN** shutdown occurs before the first complete counter sample
- **THEN** final-sample uncertainty starts at capture attachment/start boundary and ends at the actual final sample time

#### Scenario: Drain failure remains visible and cleanup follows
- **WHEN** drain fails
- **THEN** the source returns a typed failure and diagnostics make missing events visible
- **AND** finalization follows its defined cleanup semantics rather than reporting clean completion

### Requirement: Adapter failures are typed and visible
The adapter SHALL expose typed failures for unsupported kernel capability, eBPF attach failure, verifier rejection, event decode failure, invalid kernel payload, missing identity, ring-buffer loss or incomplete counter evidence, and shutdown cleanup failure. Malformed kernel events MUST NOT silently disappear.

#### Scenario: Invalid ABI item is rejected
- **WHEN** ABI version, size, byte order, discriminant, bounds, or payload metadata is invalid
- **THEN** the adapter returns typed decode or invalid-kernel-payload failure with source context
- **AND** it emits no fabricated `CaptureEvent`

#### Scenario: Attach is atomic
- **WHEN** any required hook fails to load, verify, or attach
- **THEN** adapter startup fails with the corresponding typed capability, verifier, or attach failure
- **AND** already-created links and maps are released

#### Scenario: Cleanup failure is observable
- **WHEN** shutdown cannot detach a hook or release required maps/programs
- **THEN** shutdown returns typed cleanup failure
- **AND** it does not report clean completion

### Requirement: Compatibility evidence distinguishes verified and target environments
Every feasibility report artifact and doctor result SHALL record architecture, kernel version, Linux distribution, BTF availability, and per-hook capability result. Reports SHALL separate `verified_environment` from `target_compatibility_matrix` and SHALL NOT infer compatibility from architecture or kernel family alone.

#### Scenario: Verified environment is labeled exactly
- **WHEN** privileged acceptance passes on Ubuntu 24.04, exact kernel release Linux 6.8.0-136-generic, aarch64, with cgroup v2, BTF, and all required hooks
- **THEN** the evidence artifact labels only that exact environment verified
- **AND** records exact kernel release, optional kernel-family label, and capability results

#### Scenario: x86_64 remains unverified
- **WHEN** the only retained acceptance evidence is from the verified aarch64 environment
- **THEN** x86_64 is labeled `not_verified` in the target compatibility matrix
- **AND** no Linux 6.1+, x86_64, all-architecture, or universal Linux support claim is emitted

#### Scenario: Other environments require separate acceptance
- **WHEN** an x86_64 host, another distribution or kernel version, a cloud-provider kernel, physical NIC, or production offload environment is evaluated
- **THEN** its runtime probe results remain a distinct environment result
- **AND** it becomes verified only after its own privileged acceptance run and retained evidence artifact

### Requirement: Portable conversion tests cover capture boundary
Unit tests SHALL cover `RawKernelObservation` conversion, `CaptureEvent` mapping, socket identity preservation, lifecycle evidence conversion, payload-fragment ordering, loss-event conversion, and invalid-event rejection without requiring privileged kernel access.

#### Scenario: Constructed observations exercise conversion
- **WHEN** portable tests provide valid and invalid constructed raw observations
- **THEN** all required event mappings, identity fields, ordering metadata, loss ambiguity, and typed rejection paths are asserted

### Requirement: Privileged acceptance proves only adapter stream
Privileged Linux tests SHALL be clearly marked and SHALL validate real kernel behavior only on the known verified environment. Acceptance SHALL create a dedicated cgroup, attach the adapter, generate IPv4 and IPv6 plaintext TCP traffic, observe multiple payload fragments and forced loss, stop capture, verify cleanup, and generate compatibility evidence. It MUST NOT write WAL data or invoke ETL, protocol parsing, canonicalization, or replay.

#### Scenario: IPv4 capture stream
- **WHEN** a TCP workload in the dedicated cgroup opens an IPv4 connection on the verified environment
- **THEN** `connect4` evidence maps to a stable socket identity and emitted `CaptureEvent` stream

#### Scenario: IPv6 family is preserved
- **WHEN** the workload opens an IPv6 TCP connection
- **THEN** `connect6` evidence maps to emitted events with IPv6 family preserved

#### Scenario: Large payload produces ordered fragments
- **WHEN** the workload sends payload large enough to produce multiple kernel observations
- **THEN** emitted payload fragments preserve socket identity, timestamps, direction, sequence information, order, and truncation evidence

#### Scenario: Forced ring pressure emits loss evidence
- **WHEN** the acceptance harness forces ring-buffer reservation loss
- **THEN** the adapter emits `LossWindowObserved` with bounded temporal and ambiguity evidence
- **AND** it does not name affected connections

#### Scenario: Shutdown releases kernel resources
- **WHEN** capture stops
- **THEN** producer hooks detach or quiesce, remaining ring items drain, mandatory final counter sampling completes while maps remain, maps and programs are released, no leaked program remains, and an evidence artifact is generated

#### Scenario: Acceptance report stays bounded
- **WHEN** validated-environment acceptance completes
- **THEN** its report identifies Ubuntu 24.04, exact kernel release Linux 6.8.0-136-generic, aarch64 as verified
- **AND** identifies x86_64 and all other unevaluated matrix entries as not verified

### Requirement: Adapter slice excludes recorder behavior
The implementation slice SHALL end at a `CaptureEvent` stream. It MUST NOT add WAL persistence, ETL, TCP reconstruction, HTTP or other application-protocol parsing, TLS handling, canonical sessions, replay, or production daemon lifecycle.

#### Scenario: End-to-end slice stops at stream
- **WHEN** validated kernel observations are converted successfully
- **THEN** the observable result is a `CaptureEvent` stream plus feasibility evidence
- **AND** no WAL writer, ETL pipeline, protocol decoder, canonicalizer, replay component, or always-on daemon is started
