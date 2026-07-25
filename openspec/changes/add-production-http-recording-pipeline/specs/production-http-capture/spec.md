## ADDED Requirements

### Requirement: Linux cgroup workload selection
Production recording SHALL support Linux 6.1 or newer on x86_64 and aarch64 with host cgroup v2 unified hierarchy, kernel BTF, and required eBPF hooks/helpers. Record SHALL accept exactly one dedicated host cgroup path or host PID; attachment SHALL cover selected node plus descendants. PID selection SHALL resolve once through host `/proc` to host `/sys/fs/cgroup`, verify cgroup identity, and clearly report effective subtree. Ambiguous namespace-relative resolution SHALL be rejected in favor of explicit host cgroup path. Unsupported platforms, architectures, selectors, kernels, or missing features SHALL fail before capture starts.

#### Scenario: Dedicated cgroup selected
- **WHEN** user records a valid dedicated cgroup on a supported Linux host
- **THEN** Chronicle attaches capture only to that cgroup and records effective selector in metadata

#### Scenario: PID resolves to cgroup
- **WHEN** user supplies a valid PID in a dedicated cgroup
- **THEN** Chronicle resolves and records PID plus cgroup and warns that cgroup is effective capture scope

#### Scenario: Unsupported host
- **WHEN** live recording is requested on macOS, non-cgroup-v2 Linux, unsupported architecture, or kernel below minimum
- **THEN** Chronicle fails with stable unsupported code and writes no successful recording metadata

### Requirement: Explicit privilege and backend preflight
Record service SHALL check BTF, capture object availability, required eBPF capabilities, attach permissions, and hook/helper availability before status becomes `recording`. It SHALL NOT silently fall back to fixture capture or a weaker capture mode when any required check fails.

#### Scenario: Insufficient privileges
- **WHEN** user lacks required effective capabilities
- **THEN** recording fails before attachment with actionable capability remediation and no payload values

#### Scenario: eBPF verifier rejection
- **WHEN** kernel rejects program load or attachment
- **THEN** recording status is failed with stable backend error and no claim that traffic was captured

### Requirement: Payload-only transport capture
Capture backend SHALL observe client-initiated plaintext IPv4/IPv6 TCP application payload and corresponding response payload for selected workload. It SHALL emit connection lifecycle, tuple, direction, TCP ordering position, timestamp, process/cgroup attribution, truncation, and loss evidence through versioned transport-neutral capture events. It SHALL NOT parse HTTP in eBPF or persist raw packet headers as full packet capture.

#### Scenario: Real plaintext HTTP exchange
- **WHEN** selected workload sends plaintext HTTP/1.1 request and receives response
- **THEN** capture emits ordered directional payload/lifecycle evidence sufficient for userspace reconstruction

#### Scenario: TLS traffic
- **WHEN** selected workload sends TLS records
- **THEN** capture may preserve opaque TCP payload but SHALL NOT claim decrypted HTTP or TLS plaintext support

#### Scenario: Mid-connection observation
- **WHEN** payload is observed without required connection-open attribution
- **THEN** event remains inspectable with synthetic identity marker and affected connection is not complete/replay-ready

### Requirement: Stable connection generation
A production connection identity SHALL include boot-scoped socket identity and monotonic connection generation plus tuple/cgroup provenance. File descriptor, PID, or reusable network tuple alone SHALL NOT identify a connection. Separate socket, process restart, descriptor reuse, and later tuple reuse SHALL remain separate generations.

#### Scenario: File descriptor reused
- **WHEN** selected process closes one connection and reuses same descriptor for another
- **THEN** emitted events have distinct connection generations

#### Scenario: Tuple reused later
- **WHEN** same endpoint tuple is used by a later connection
- **THEN** later payload cannot be grouped with earlier connection

### Requirement: Bounded event delivery and truncation
Each capture event payload SHALL be bounded to 16 KiB and kernel ring capacity SHALL be bounded. One observed skb payload SHALL be split into at most four ordered continuation events (64 KiB ceiling) when readable; remaining bytes SHALL set truncation and original-length indicators. Ring reservation/backend loss SHALL increment observable counters and emit recording-level loss evidence when userspace resumes. Because lost reservation lacks connection identity, any kernel drop SHALL taint whole recording and every overlapping production connection as incomplete/non-replayable.

#### Scenario: GSO or GRO payload observation
- **WHEN** one readable observed TCP payload exceeds 16 KiB but not 64 KiB
- **THEN** Chronicle emits ordered continuation events that reconstruct exact payload without truncation

#### Scenario: Oversized or unreadable payload observation
- **WHEN** one observed TCP payload exceeds 64 KiB or required bytes cannot be read
- **THEN** Chronicle emits bounded available bytes with truncation flag and observed original length

#### Scenario: Ring buffer loss
- **WHEN** kernel cannot reserve capture event space
- **THEN** dropped-event counter increases and whole recording plus overlapping production connections become incomplete/non-replayable

### Requirement: Recording lifecycle metadata
Record service SHALL create unique recording ID and atomically maintain versioned metadata with build, host/kernel, effective selector, capture capabilities, start/end, status, WAL version/segments, counts, bounded errors, and shutdown reason. Status SHALL transition from `starting` to `recording` and then exactly one of `completed`, `interrupted`, or `failed`. Metadata SHALL omit command-line secrets, environment values, and payload/header values.

#### Scenario: Successful recording
- **WHEN** capture starts, receives events, and shuts down cleanly
- **THEN** metadata is finalized completed with start/end, segment discovery, and received/persisted/loss counters

#### Scenario: SIGINT shutdown
- **WHEN** user sends SIGINT during recording
- **THEN** Chronicle detaches, drains, flushes durable state, finalizes interrupted metadata, and prints safe summary

#### Scenario: SIGTERM shutdown
- **WHEN** process receives SIGTERM
- **THEN** Chronicle performs same bounded graceful shutdown with distinct termination reason

#### Scenario: Abrupt termination
- **WHEN** process dies before metadata finalization
- **THEN** next recovery recognizes stale recording state as interrupted rather than completed

### Requirement: Recording summary and counters
Human and JSON record summaries SHALL consistently expose events/bytes received, queued but not durable at shutdown, events/bytes durably persisted, capture drops, truncations, WAL failures, recording status, and shutdown reason. Default output and logs SHALL not print captured payload or arbitrary header values.

#### Scenario: Loss-free summary
- **WHEN** all received events become durable with no capture loss
- **THEN** summary reports matching received/persisted counts and zero drops

#### Scenario: Failed persistence summary
- **WHEN** WAL persistence fails after capture began
- **THEN** summary reports failed status, durable prefix counts, non-durable/loss counts, and stable error code

### Requirement: Fixture capture remains portable test source
Production capture SHALL reuse existing capture event boundary, and deterministic fixture/in-memory sources SHALL remain available for rootless unit and integration tests. Fixture tests SHALL NOT be represented as real eBPF coverage.

#### Scenario: Non-Linux test run
- **WHEN** workspace tests run without eBPF privileges
- **THEN** fixture capture exercises downstream WAL/ETL behavior while live record reports unsupported clearly
