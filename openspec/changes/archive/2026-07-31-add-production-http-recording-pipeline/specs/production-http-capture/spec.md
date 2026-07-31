## ADDED Requirements

### Requirement: Linux cgroup workload selection
Production recording SHALL support Linux 6.1 or newer on x86_64 and aarch64 with host cgroup v2 unified hierarchy, kernel BTF, and required eBPF hooks/helpers. Record SHALL accept exactly one host `--cgroup PATH` or host `--pid PID` and attach to the selected cgroup node plus descendants, called the **selected subtree**.

The **direct cgroup TGID set** SHALL be the distinct host-visible thread-group IDs represented by the numeric process IDs listed directly in the selected cgroup's `cgroup.procs`, after resolving each listed PID to its host-visible TGID. Its cardinality SHALL be reported as **direct TGID count**. This set is not a POSIX process group ID (`getpgrp`/PGID), session ID, thread count, container ID, or union of descendant `cgroup.procs` files. Multiple listed PIDs resolving to one TGID SHALL count once; distinct TGIDs sharing one POSIX PGID SHALL count separately. `cgroup.procs` SHALL provide direct membership of the selected node only. Descendant cgroups SHALL be enumerated and reported separately as **descendant cgroup count** because capture covers the selected subtree. A listed PID that exits or cannot be safely resolved to a host-visible TGID SHALL NOT be silently omitted; record preflight SHALL reject the selector, and diagnostics SHALL report `not_checked` or rejection according to the existing required/optional probe policy.

Preflight SHALL resolve canonical host path/stable inode-ID, direct cgroup TGID set, descendant cgroup count, and Chronicle's own host-visible cgroup identity. Chronicle SHALL compare its identity against the full selected subtree, not only direct `cgroup.procs` membership. Unreadable or namespace-ambiguous required enumeration SHALL fail record preflight. Root `/`, `/system.slice`, `/user.slice`, `/machine.slice`, and a selected subtree containing Chronicle SHALL be rejected with no override.

PID selection SHALL resolve the selected PID's host-visible TGID and host-visible `/proc/<pid>/cgroup` during preflight, immediately before attach, and immediately after links attach; any path/inode/ID change SHALL fail/detach. PID selector safety SHALL compare the selected host-visible TGID with the direct cgroup TGID set. Missing selected TGID, any unrelated direct TGID, PID exit, unsafe resolution, or namespace ambiguity SHALL reject selection, and `--allow-shared-cgroup` SHALL NOT apply.

Explicit `--cgroup` with direct TGID count zero or one and descendant cgroup count zero SHALL be accepted without override. Direct TGID count greater than one or descendant cgroup count greater than zero SHALL be treated shared/broad, produce separate non-sensitive counts, and require `--allow-shared-cgroup`. Flag SHALL be accepted only with explicit `--cgroup` and MUST NOT override forbidden roots, Chronicle containment, unstable identity, namespace-ambiguous PID resolution, or unreadable enumeration. Container IDs SHALL NOT be P1 selectors.

#### Scenario: Dedicated leaf cgroup selected
- **WHEN** explicit cgroup is a stable leaf with direct TGID count zero or one
- **THEN** Chronicle displays safe path/ID/direct TGID count/descendant cgroup count/selected-subtree scope and attaches without shared acknowledgement

#### Scenario: One multithreaded process
- **WHEN** `cgroup.procs` represents one multithreaded process
- **THEN** direct TGID count is one

#### Scenario: Multiple threads
- **WHEN** several listed PIDs resolve to the same host-visible TGID
- **THEN** they contribute one member to the direct cgroup TGID set

#### Scenario: POSIX process group mismatch
- **WHEN** two processes share a POSIX PGID but have different host-visible TGIDs
- **THEN** they count as two direct cgroup TGIDs

#### Scenario: Two unrelated processes
- **WHEN** selected node directly contains two distinct host-visible TGIDs
- **THEN** PID selection is rejected and explicit cgroup selection requires the shared-cgroup acknowledgement policy

#### Scenario: Descendant-only workload
- **WHEN** selected node has no direct TGIDs but has descendant cgroups
- **THEN** direct TGID count is zero, descendant cgroup count is non-zero, and selected-subtree scope is reported separately

#### Scenario: PID alone in dedicated cgroup
- **WHEN** valid PID's host-visible TGID is the sole member of the direct cgroup TGID set
- **THEN** Chronicle verifies the same cgroup identity before preflight/before attach/after attach and records PID plus exact selected subtree

#### Scenario: PID in shared cgroup
- **WHEN** PID-resolved cgroup's direct cgroup TGID set contains an unrelated TGID
- **THEN** Chronicle rejects selection as shared even if acknowledgement flag is supplied

#### Scenario: Resolution failure
- **WHEN** a PID listed by `cgroup.procs` exits or cannot be resolved safely to a host-visible TGID
- **THEN** Chronicle does not omit it and instead reports the probe `not_checked` or rejects the selector according to the required/optional probe policy

#### Scenario: Explicit shared cgroup without acknowledgement
- **WHEN** explicit cgroup has direct TGID count greater than one or descendant cgroup count greater than zero and flag is absent
- **THEN** Chronicle warns with separate non-sensitive counts/scope and rejects before attachment

#### Scenario: Explicit shared cgroup with acknowledgement
- **WHEN** explicit non-forbidden cgroup is shared and user supplies `--allow-shared-cgroup`
- **THEN** Chronicle attaches the exact selected subtree and persists acknowledgement plus direct TGID and descendant cgroup counts

#### Scenario: Forbidden root despite acknowledgement
- **WHEN** selector resolves to `/`, `/system.slice`, `/user.slice`, or `/machine.slice` with acknowledgement
- **THEN** Chronicle rejects selection before attachment

#### Scenario: Recorder in descendant
- **WHEN** Chronicle runs in any descendant of the selected cgroup
- **THEN** selection is rejected even if Chronicle is absent from the selected node's direct `cgroup.procs`

#### Scenario: PID moves during attach
- **WHEN** PID moves/departs or cgroup ID changes between three identity samples
- **THEN** Chronicle fails, removes partial links, and creates no successful recording

#### Scenario: Safe preflight output
- **WHEN** selector preflight reports membership/scope
- **THEN** output contains only canonical path, stable ID, direct TGID count, descendant cgroup count, selected-subtree scope, warnings, and acknowledgement state—not process command lines or environment values

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
Capture backend SHALL observe plaintext IPv4/IPv6 TCP application payload and response payload for selected active or passive workload sockets. Capture Event v1 SHALL contain capture-domain evidence only. Pre-autobind connect intent carries remote endpoint and active role; complete `SocketConnected` evidence carries stable socket identity, validated local/remote endpoints, active/passive role, recording scope, and network family. Payload fragments carry direction, TCP/continuation position, payload, and truncation but MUST NOT duplicate endpoints or role. Typed counter-derived `LossWindow` remains separate. Raw lifecycle values SHALL be `established`, `state_changed`, `closed`, `reset`, or `unknown`; `closed`/`reset` require proven signal. WAL provenance belongs only to `WalRecordEnvelope`. Backend SHALL NOT parse HTTP in eBPF or persist raw packet headers as full packet capture.

#### Scenario: Real plaintext HTTP exchange
- **WHEN** selected workload sends plaintext HTTP/1.1 request and receives response
- **THEN** capture emits complete endpoint/role evidence before endpoint-free directional payload sufficient for userspace reconstruction without embedding WAL sequence

#### Scenario: TLS traffic
- **WHEN** selected workload sends TLS records
- **THEN** capture may preserve opaque TCP payload but SHALL NOT claim decrypted HTTP or TLS plaintext support

#### Scenario: Payload lacks endpoint evidence
- **WHEN** payload references socket without complete cached `SocketEvidence`
- **THEN** adapter/reconstruction returns typed missing-evidence failure and never fabricates endpoint or role

#### Scenario: Packet hook executes outside socket owner task
- **WHEN** sock-ops or cgroup-skb executes without authoritative current PID/TGID or socket-owner cgroup context
- **THEN** Chronicle joins connect-attributed process/cgroup identity by socket cookie and does not use packet-hook current task/cgroup as socket ownership

### Requirement: Stable connection generation
A production connection identity SHALL include boot-scoped socket identity and monotonic connection generation plus tuple/cgroup provenance. File descriptor, PID, or reusable network tuple alone SHALL NOT identify a connection. Separate socket, process restart, descriptor reuse, and later tuple reuse SHALL remain separate generations.

#### Scenario: File descriptor reused
- **WHEN** selected process closes one connection and reuses same descriptor for another
- **THEN** emitted events have distinct connection generations

#### Scenario: Tuple reused later
- **WHEN** same endpoint tuple is used by a later connection
- **THEN** later payload cannot be grouped with earlier connection

### Requirement: Bounded event delivery and 100 ms loss sampling
Each capture event payload SHALL be bounded to 16 KiB and kernel ring capacity SHALL be 8 MiB. One observed skb payload SHALL be split into at most four ordered continuation events (64 KiB ceiling) when readable; remaining bytes SHALL set truncation/original-length indicators. Ring reservation/backend loss SHALL increment cumulative per-CPU counters.

While recording is active, userspace SHALL schedule complete per-CPU samples at least every 100 ms on same boot-scoped monotonic clock ID/origin as Capture Event timestamps. Attachment/start time SHALL be initial boundary; first successful sample SHALL establish observation boundary. Zero delta SHALL update boundary without WAL record. Positive delta SHALL emit typed `LossWindow` with actual previous/current successful sample timestamps, aggregate delta, epoch/generation, map/clock identity. Delayed/stalled poll SHALL use actual elapsed interval, not 100 ms fiction. Reset/regression, map replacement, incomplete per-CPU read, boot-ID change, or clock mismatch SHALL produce ambiguous loss evidence and prevent complete claim. LossWindow and CaptureEvent SHALL be sequenced by `WalRecordEnvelope`; neither SHALL embed WAL order inside Capture Event.

After capture intake stops and before WAL finalization, userspace SHALL retain maps and perform mandatory final sample. Loss before first sample SHALL span attachment through first sample. Final-sample failure SHALL extend uncertainty from previous successful sample through shutdown, remain visible, and finalize `failed` with `capture_failure` while preserving committed prefix. Evidence SHALL NOT claim exact drop timestamp or per-connection attribution.

#### Scenario: Regular 100 ms sampling
- **WHEN** recording poller runs normally
- **THEN** successful per-CPU observations occur no more than 100 ms apart in shared monotonic clock domain

#### Scenario: Zero-delta sample
- **WHEN** consecutive complete samples aggregate same counters
- **THEN** boundary advances but no `LossWindow` WAL record or unnecessary WAL growth occurs

#### Scenario: Positive delta
- **WHEN** aggregate counter increases between samples
- **THEN** one typed window records actual previous/current timestamps, delta, epoch, clock/map identity

#### Scenario: Poller delayed beyond 100 ms
- **WHEN** poll loop runs late
- **THEN** window uses actual expanded elapsed interval and diagnostic exposes missed cadence

#### Scenario: Loss before first sample
- **WHEN** first successful aggregate is positive
- **THEN** uncertainty begins at capture attachment/start time and ends at sample timestamp

#### Scenario: Per-CPU aggregation
- **WHEN** loss increments on multiple CPUs
- **THEN** one complete snapshot aggregates all slots without double counting and preserves map identity

#### Scenario: Counter reset or regression
- **WHEN** any per-CPU value regresses, map identity changes, or clock/boot identity mismatches
- **THEN** Chronicle emits ambiguous loss evidence and cannot claim affected recording complete

#### Scenario: Shutdown final sample
- **WHEN** capture intake stops cleanly
- **THEN** final sample occurs before map release/final marker and positive delta creates final typed window

#### Scenario: Final sample failure
- **WHEN** required counters cannot be read after intake stops
- **THEN** uncertainty extends from prior observation through shutdown and recording fails visibly without complete claim

#### Scenario: GSO or GRO payload observation
- **WHEN** one readable observed TCP payload exceeds 16 KiB but not 64 KiB
- **THEN** Chronicle emits ordered continuation events that reconstruct exact payload without truncation

#### Scenario: Oversized or unreadable payload observation
- **WHEN** one observed TCP payload exceeds 64 KiB or required bytes cannot be read
- **THEN** Chronicle emits bounded available bytes with truncation flag and observed original length

### Requirement: Recording lifecycle metadata
Record service SHALL create unique recording ID and atomically maintain versioned metadata with build, host/kernel, effective cgroup path/ID/scope counts/acknowledgement, capture capabilities, configured bounds, start/end, status, WAL version/segments/last valid commit boundary, categorized counters, bounded errors, and separate shutdown reason. Status SHALL transition `starting -> recording -> completed|failed` or `starting -> failed` when preflight/load/attach fails after metadata creation; recovery SHALL convert stale `starting`/`recording` to `aborted`. Required shutdown reasons SHALL be `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `wal_size_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`. `interrupted` SHALL NOT be P1 status. Metadata SHALL omit command-line secrets, environment values, and payload/header values.

#### Scenario: Successful recording
- **WHEN** capture starts, receives events, and shuts down cleanly
- **THEN** metadata is finalized completed with start/end, segment discovery, and received/persisted/loss counters

#### Scenario: SIGINT shutdown
- **WHEN** user sends SIGINT and detach/capacity-qualified queue handling/final sample/final commit/finalization succeed
- **THEN** Chronicle finalizes `completed` with `user_interrupt` and prints safe summary

#### Scenario: SIGTERM shutdown
- **WHEN** process receives SIGTERM and detach/capacity-qualified queue handling/final sample/final commit/finalization succeed
- **THEN** Chronicle finalizes `completed` with `termination_signal`

#### Scenario: Capture or WAL failure
- **WHEN** capture backend or WAL write/group-sync fails
- **THEN** Chronicle finalizes `failed` with `capture_failure` or `wal_failure` and preserves committed prefix

#### Scenario: Abrupt termination
- **WHEN** process dies before metadata finalization
- **THEN** next recovery finalizes stale active metadata `aborted` with crash/forced reason and never claims graceful completion

### Requirement: Recording bounds, hard-limit shutdown, summary, and counters
Production record SHALL accept `--duration-seconds` default 600 with inclusive range 1 through 3600 and `--max-wal-bytes` default/hard maximum 4 GiB with lower value at least selected segment size. Duration stop SHALL stop intake then process accepted queue under same complete-frame/final-marker/header capacity reservation within five-second grace. WAL-limit stop SHALL stop intake immediately, write only complete queued prefix preserving final-marker/header reservation, classify remainder, and never exceed physical cap. Successful final sample/final marker sync/metadata finalization SHALL yield `completed` with exact reason; failure SHALL yield `failed` with capture/WAL reason and preserve committed prefix. If duration and WAL reservation failure occur same shutdown cycle, `wal_size_limit` SHALL win.

Human/JSON summaries SHALL consistently expose stable categories `received`, `accepted_into_queue`, `written_not_committed`, `committed`, `discarded_from_queue_due_to_wal_limit`, `kernel_or_backend_dropped`, `rejected_after_stop`, and `etl_checkpointed`, with records/bytes where available, plus loss windows, truncations, WAL failures, configured/effective/physical sizes, status, and shutdown reason. Categories SHALL reconcile without overlap. Default output/logs SHALL not print captured payload or arbitrary header values.

#### Scenario: Loss-free summary
- **WHEN** all received events become committed with no capture loss
- **THEN** summary reports matching received/committed counts, zero queued/written-not-committed, and zero drops/discards/rejections

#### Scenario: Failed persistence summary
- **WHEN** WAL persistence fails after capture began
- **THEN** summary reports failed status, last valid commit boundary, queued/written-not-committed uncertainty, categorized loss, and stable error code

#### Scenario: Duration limit
- **WHEN** configured duration expires before WAL reservation failure
- **THEN** recording stops cleanly as `completed` with `duration_limit` after final sample/commit and spanning connection remains incomplete unless complete before commit boundary

#### Scenario: WAL size limit with queue remainder
- **WHEN** only prefix of queued events fits while preserving final marker
- **THEN** fitting prefix commits, remainder is counted WAL-limit discard, terminal loss evidence is emitted or summarized, and successful finalization completes with `wal_size_limit`

#### Scenario: Final commit failure at limit
- **WHEN** final marker write/sync or metadata finalization fails during limit stop
- **THEN** recording is `failed` with `wal_failure`, prior committed prefix remains authoritative, and physical cap remains unexceeded

#### Scenario: Shutdown grace exceeded
- **WHEN** signal/limit stop cannot complete required final sample/queue policy/commit/finalization within five seconds
- **THEN** recording does not hang or claim completed; it finalizes failed where possible and reports committed prefix/uncertain tail

#### Scenario: Physical hard cap
- **WHEN** recording stops at exact or near WAL bound
- **THEN** recursive bytes under `segments/`, including temporary files, never exceed configured maximum

#### Scenario: Invalid bound
- **WHEN** duration exceeds 3600 seconds or WAL limit exceeds 4 GiB/is below segment size
- **THEN** preflight rejects command before capture

### Requirement: Fixture capture remains portable test source
Production capture SHALL reuse existing capture event boundary, and deterministic fixture/in-memory sources SHALL remain available for rootless unit and integration tests. Fixture tests SHALL NOT be represented as real eBPF coverage.

#### Scenario: Non-Linux test run
- **WHEN** workspace tests run without eBPF privileges
- **THEN** fixture capture exercises downstream WAL/ETL behavior while live record reports unsupported clearly
