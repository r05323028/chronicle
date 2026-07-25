## ADDED Requirements

### Requirement: Linux cgroup workload selection
Production recording SHALL support Linux 6.1 or newer on x86_64 and aarch64 with host cgroup v2 unified hierarchy, kernel BTF, and required eBPF hooks/helpers. Record SHALL accept exactly one dedicated host cgroup path or host PID and attach to selected node plus descendants. Root `/`, `/system.slice`, `/user.slice`, `/machine.slice`, and a selection containing Chronicle recorder SHALL be rejected with no P1 override. Before capture, CLI SHALL show canonical host path, stable cgroup inode/ID, and subtree scope; metadata SHALL persist path plus stable identity. PID selection SHALL resolve through host `/proc`, reject namespace ambiguity, resolve again immediately before attach, and resolve a third time immediately after links attach. Any path/inode/ID change across three samples SHALL fail and detach links before capture. Unsupported platforms, architectures, selectors, kernels, or missing features SHALL fail before capture starts. Container IDs SHALL NOT be P1 selectors.

#### Scenario: Dedicated cgroup selected
- **WHEN** user records a valid dedicated cgroup on a supported Linux host
- **THEN** Chronicle displays and records exact host path plus cgroup inode/ID and attaches only to that node and descendants

#### Scenario: PID resolves to cgroup
- **WHEN** user supplies a valid PID in a dedicated cgroup
- **THEN** Chronicle resolves PID before preflight, immediately before attach, and immediately after attach; verifies unchanged stable cgroup identity; records PID plus effective subtree; and never widens scope

#### Scenario: Root or broad shared cgroup
- **WHEN** selector resolves to `/`, `/system.slice`, `/user.slice`, `/machine.slice`, or Chronicle recorder cgroup
- **THEN** Chronicle rejects selection before attachment with safe broad-scope diagnostic

#### Scenario: PID moves during attach
- **WHEN** PID moves to another cgroup between preflight and attachment verification
- **THEN** Chronicle fails, removes partial links, and creates no successful recording

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
Capture backend SHALL observe client-initiated plaintext IPv4/IPv6 TCP application payload and corresponding response payload for selected workload. Capture Event v2 SHALL contain capture-domain evidence only: kernel timestamp, process/stable-cgroup identity, feasibility-proven connection identity, raw lifecycle evidence, direction, TCP position, payload, truncation, and typed loss evidence. Raw lifecycle values SHALL be `established`, `state_changed`, `closed`, `reset`, or `unknown`; directional half-close SHALL NOT be promised until feasibility proves required hook evidence. WAL recording ID/sequence/segment/offset SHALL belong to `WalRecordEnvelope`, not Capture Event. Backend SHALL NOT parse HTTP in eBPF or persist raw packet headers as full packet capture.

#### Scenario: Real plaintext HTTP exchange
- **WHEN** selected workload sends plaintext HTTP/1.1 request and receives response
- **THEN** capture emits directional payload and raw lifecycle evidence sufficient for userspace reconstruction without embedding WAL sequence

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

### Requirement: Bounded event delivery and typed loss windows
Each capture event payload SHALL be bounded to 16 KiB and kernel ring capacity SHALL be 8 MiB. One observed skb payload SHALL be split into at most four ordered continuation events (64 KiB ceiling) when readable; remaining bytes SHALL set truncation and original-length indicators. Ring reservation/backend loss SHALL increment cumulative counters. Userspace SHALL persist typed loss evidence containing boot-scoped monotonic clock ID/origin, previous/current snapshot times in same clock domain as Capture Event kernel timestamps, drop-count delta, and cumulative loss epoch/generation when loss is observed and at final shutdown sample. Evidence SHALL define a conservative interval, not claim exact drop time or connection attribution.

#### Scenario: GSO or GRO payload observation
- **WHEN** one readable observed TCP payload exceeds 16 KiB but not 64 KiB
- **THEN** Chronicle emits ordered continuation events that reconstruct exact payload without truncation

#### Scenario: Oversized or unreadable payload observation
- **WHEN** one observed TCP payload exceeds 64 KiB or required bytes cannot be read
- **THEN** Chronicle emits bounded available bytes with truncation flag and observed original length

#### Scenario: Ring buffer loss
- **WHEN** kernel cannot reserve capture event space between two userspace counter snapshots
- **THEN** dropped-event counter and loss epoch increase and WAL receives typed evidence for snapshot interval/delta without naming an affected connection

#### Scenario: No observed loss
- **WHEN** consecutive snapshots have zero drop delta
- **THEN** Chronicle persists no false loss window and does not degrade unrelated connection solely from cumulative zero change

### Requirement: Recording lifecycle metadata
Record service SHALL create unique recording ID and atomically maintain versioned metadata with build, host/kernel, effective cgroup path/ID, capture capabilities, configured bounds, start/end, status, WAL version/segments/durable watermark, counters, bounded errors, and separate shutdown reason. Status SHALL transition `starting -> recording -> completed|failed` or `starting -> failed` when preflight/load/attach fails after metadata creation; recovery SHALL convert stale `starting`/`recording` to `aborted`. Required shutdown reasons SHALL be `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `wal_size_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`. `interrupted` SHALL NOT be P1 status. Metadata SHALL omit command-line secrets, environment values, and payload/header values.

#### Scenario: Successful recording
- **WHEN** capture starts, receives events, and shuts down cleanly
- **THEN** metadata is finalized completed with start/end, segment discovery, and received/persisted/loss counters

#### Scenario: SIGINT shutdown
- **WHEN** user sends SIGINT and detach/drain/group-sync/finalization succeed
- **THEN** Chronicle finalizes `completed` with `user_interrupt` and prints safe summary

#### Scenario: SIGTERM shutdown
- **WHEN** process receives SIGTERM and detach/drain/group-sync/finalization succeed
- **THEN** Chronicle finalizes `completed` with `termination_signal`

#### Scenario: Capture or WAL failure
- **WHEN** capture backend or WAL write/group-sync fails
- **THEN** Chronicle finalizes `failed` with `capture_failure` or `wal_failure` and preserves durable prefix

#### Scenario: Abrupt termination
- **WHEN** process dies before metadata finalization
- **THEN** next recovery finalizes stale active metadata `aborted` with crash/forced reason and never claims graceful completion

### Requirement: Recording bounds, summary, and counters
Production record SHALL accept `--duration-seconds` default 600 with inclusive range 1 through 3600 and `--max-wal-bytes` default/hard maximum 4 GiB with lower value at least selected segment size. First reached limit SHALL detach/stop new capture immediately, then drain/group-sync/finalize within fixed five-second shutdown grace. Success SHALL finalize `completed` with exact reason; deadline or drain/WAL failure SHALL finalize `failed` with capture/WAL reason and preserve durable prefix. Human and JSON summaries SHALL consistently expose events/bytes received, accepted into queue, written-not-durable, durable through watermark, dropped before persistence, ETL-checkpointed, capture drops/loss windows, truncations, WAL failures, configured/effective limits, status, and shutdown reason. Default output/logs SHALL not print captured payload or arbitrary header values.

#### Scenario: Loss-free summary
- **WHEN** all received events become durable with no capture loss
- **THEN** summary reports matching received/durable counts, zero queued/written-not-durable, and zero drops

#### Scenario: Failed persistence summary
- **WHEN** WAL persistence fails after capture began
- **THEN** summary reports failed status, durable watermark/prefix, queued and written-not-durable uncertainty, loss counts, and stable error code

#### Scenario: Duration limit
- **WHEN** configured duration expires before WAL size limit
- **THEN** recording stops cleanly as `completed` with `duration_limit` and spanning connection remains incomplete unless complete before durable boundary

#### Scenario: WAL size limit
- **WHEN** next frame would exceed total WAL bound before duration expires
- **THEN** recording stops before excess append, syncs durable data, and completes with `wal_size_limit`

#### Scenario: Shutdown grace exceeded
- **WHEN** signal/limit stop cannot drain/group-sync/finalize within five seconds
- **THEN** recording does not hang indefinitely or claim completed; it finalizes failed where possible and reports durable prefix/uncertain tail

#### Scenario: Invalid bound
- **WHEN** duration exceeds 3600 seconds or WAL limit exceeds 4 GiB/is below segment size
- **THEN** preflight rejects command before capture

### Requirement: Fixture capture remains portable test source
Production capture SHALL reuse existing capture event boundary, and deterministic fixture/in-memory sources SHALL remain available for rootless unit and integration tests. Fixture tests SHALL NOT be represented as real eBPF coverage.

#### Scenario: Non-Linux test run
- **WHEN** workspace tests run without eBPF privileges
- **THEN** fixture capture exercises downstream WAL/ETL behavior while live record reports unsupported clearly
