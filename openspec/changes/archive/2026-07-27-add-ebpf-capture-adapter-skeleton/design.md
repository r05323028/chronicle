## Context

Gate A is complete. Its retained evidence proves `cgroup/connect4`, `cgroup/connect6`, sock-ops, and cgroup-skb ingress/egress behavior on Ubuntu 24.04, exact kernel release Linux 6.8.0-136-generic, aarch64, with cgroup v2 and BTF. It also proves socket-cookie correlation, connect-derived process/cgroup attribution, payload visibility including bounded continuations, cumulative per-CPU loss evidence, and clean detach in that environment.

That evidence is not proof for x86_64, other distributions or kernel versions, cloud-provider kernels, physical NICs, or production offload behavior. Existing P1 text that names a broader target matrix remains a target, not verified compatibility. This slice must make that distinction explicit wherever feasibility is reported or accepted.

Current dependency direction is:

```text
chronicle-common
       ^
       |
chronicle-capture
       ^
       |
chronicle-capture-ebpf
```

`chronicle-capture` owns protocol-neutral evidence contracts. `chronicle-capture-ebpf` owns Linux/Aya loading, attachment, ABI decoding, and normalization. `chronicle-wal`, `chronicle-protocol`, application services, and CLI consumers stay downstream or outside this slice. Existing Capture Event v1 fixtures remain intact.

Target slice:

```text
validated Aya hooks
        |
        v
private eBPF ABI values
        |
        v
RawKernelObservation
        |
        v
CaptureAdapter
        |
        v
CaptureEvent stream
```

## Goals / Non-Goals

**Goals:**

- Isolate Aya and eBPF ABI structures inside `chronicle-capture-ebpf`.
- Normalize decoded kernel evidence into versioned, protocol-neutral `CaptureEvent` values owned by `chronicle-capture`.
- Preserve socket identity, recording-scope identity, monotonic timing, direction, family, transport sequence, fragment order, truncation, lifecycle evidence, and loss ambiguity.
- Reject malformed or incomplete observations explicitly with typed failures.
- Produce reproducible feasibility evidence and doctor output that separate verified environment from target compatibility matrix.
- Prove one bounded `CaptureEvent` stream on the validated privileged environment.

**Non-Goals:**

- WAL writing, WAL ordering, durability, group commit, or recording metadata lifecycle.
- ETL, TCP stream reconstruction, retransmission handling, or loss attribution.
- HTTP, TLS, database, Kafka, NATS, or any application-protocol semantics.
- Canonical sessions, completeness, replayability, replay, or verification decisions.
- Production recorder composition, long-running daemon behavior, or always-on capture.
- Directional half-close claims.
- x86_64 or universal Linux compatibility claims.

## Decisions

### 1. Keep kernel ABI and raw observations adapter-private

`chronicle-capture-ebpf` will own three stages:

```text
Aya ring item / map sample
        -> ABI decoder
        -> RawKernelObservation
        -> CaptureAdapter
        -> chronicle_capture::CaptureEvent
```

`RawKernelObservation` is an adapter-internal, architecture-aware decoded form. It may carry hook source, kernel monotonic timestamp, socket cookie, cgroup/process evidence, network family and tuple metadata, direction, TCP sequence, skb/continuation metadata, payload bytes, and cumulative loss counters. Aya handles, generated eBPF structs, padding, byte order, verifier details, and map layouts never cross into `chronicle-capture`.

`CaptureAdapter` is the only conversion path from decoded kernel observations to capture-domain events. `chronicle-capture-ebpf` may depend on `chronicle-capture`; the reverse dependency is forbidden. Capture Event v1 fixture bytes and readers remain compatible; this slice introduces the versioned evidence model as Capture Event v2 rather than reinterpreting v1 fields.

**Alternative considered:** expose generated ABI structs and let consumers convert them. Rejected because it couples domain code to kernel layout, architecture, Aya, and verifier-driven representation.

### 1a. Freeze production private ABI v1 before BPF migration

The retained Gate A `ProbeEvent` layout is feasibility evidence only. It has no version, byte-order marker, or declared item size, so `CaptureAdapter` MUST NOT decode it. The production `chronicle-ebpf-capture` package will emit private ABI v1 and then rerun Gate A acceptance; the retained report remains historical evidence, not proof that ABI v1 behaves identically.

ABI v1 is little-endian and starts with exactly 12 bytes:

```text
0..4   magic: ASCII `CHRN`
4..6   version: u16 = 1
6      discriminant: u8
7      byte_order: u8 = 1 (little-endian)
8..12  declared_item_size: u32
```

The declared size MUST equal ring-item size. The body has one of these forms:

```text
connect4/connect6: common_socket (48 bytes)
sockops:           common_socket + signal:u8 + raw_state:u32
cgroup-skb:        common_socket + family:u8 + tcp_sequence:u32 + continuation:u32
                   + observed_length:u32 + captured_length:u32 + truncated:u8 + payload bytes
loss-counters:     timestamp:u64 + generation:u32 + one-or-more per_cpu_counter:u64

common_socket: timestamp:u64 + socket_cookie:u64 + first_seen_monotonic:u64
               + network_namespace:u64 (zero means unavailable) + cgroup_id:u64
               + pid:u32 + tid:u32
```

Only validated connect4/connect6, sock-ops, and cgroup-skb hooks may emit these discriminants. ABI v1 contains no Aya handles, generated Rust structs, HTTP meaning, persistence fields, or raw-packet storage. Decoder rejects any unrecognized discriminant, version, byte order, declared size, truncated integer, missing required socket identity, or payload bound mismatch with typed bounded-context failure.

**Alternative considered:** decode retained `ProbeEvent` as legacy ABI v0. Rejected because it would preserve an unversioned production boundary and obscure whether current BPF programs satisfy ABI v1 validation.

### 2. Model observed evidence, not reconstructed meaning

`CaptureEvent` will be an evidence-only versioned model with these event kinds:

- `SocketConnectObserved` for connect-attribution evidence
- `SocketConnected` only for separately proven active-establish evidence
- `SocketClosedObserved`
- `SocketResetObserved`
- `SocketStateChangedObserved` for raw state evidence that does not prove close, reset, or half-close
- `PayloadFragment`
- `LossWindowObserved`

`SocketConnectObserved` preserves connect4/connect6 attribution without claiming successful establishment. `SocketConnected` requires Gate A's separately proven sock-ops active-establish signal. Close and reset kinds remain reserved unless implementation evidence identifies separately proven raw signals; ambiguous state MUST NOT populate them. Gate A showed other sock-ops state changes alone cannot distinguish orderly close, reset, or directional half-close, so ambiguous state remains `SocketStateChangedObserved`; it is never upgraded by the adapter.

The event model contains no request/response completion, application operation, connection completeness, replayability, WAL sequence, durable offset, or canonical meaning.

**Alternative considered:** emit derived connection lifecycle or HTTP-shaped events. Rejected because those decisions require cross-event reconstruction owned by future ETL/session/protocol layers.

### 3. Use socket cookie plus generation evidence for identity

`SocketIdentity` consists of:

```text
socket_cookie
first_seen_monotonic_timestamp
network_namespace_identity: Option<_>
```

The first-seen timestamp is the connect-derived generation candidate proven by Gate A. Network namespace identity is included only when the adapter can obtain it without guessing. Boot/clock identity accompanies monotonic timestamps so evidence from different boots is not merged.

PID/TGID and normalized tuple remain attributes. Neither is connection identity. Five-tuple reuse, PID reuse, and socket-cookie lifecycle reuse therefore cannot silently merge observations. Missing required cookie or first-seen generation is a typed `MissingIdentity` failure rather than a synthetic identity.

**Alternative considered:** use five-tuple or PID. Rejected because both are reusable and Gate A validated socket-cookie correlation instead.

### 4. Preserve connect-derived cgroup scope identity

Each attributable event carries:

```text
RecordingScopeIdentity {
    cgroup_id,
    canonical_path,
    namespace_information,
}
```

Connect evidence is authoritative for socket-owner process and observed descendant cgroup ID. Canonical path and namespace information come from caller-supplied recording-scope configuration validated before attachment; this slice consumes that identity but does not implement selector preflight. `CaptureAdapter` verifies the connect-observed cgroup ID belongs to the configured scope before joining full `RecordingScopeIdentity` through `SocketIdentity`. Later payload and lifecycle observations reuse that joined identity. The adapter does not substitute cgroup-skb execution-context identity for socket-owner identity. Path strings are descriptive; stable cgroup ID remains required.

**Alternative considered:** identify scope by canonical path alone. Rejected because paths can be renamed or reused and Gate A safety decisions require stable identity plus path/context.

### 5. Keep payload fragments minimal and ordered

`PayloadFragment` carries:

```text
socket_identity
recording_scope_identity
network_family
direction
kernel_timestamp
sequence_information
payload_bytes
truncation_metadata
```

`sequence_information` preserves transport sequence plus continuation position needed to order multiple observations without reconstructing a stream. `truncation_metadata` preserves captured length, observed length when known, truncation state, and reason/ambiguity. One kernel observation may emit multiple ordered fragments; one skb is never treated as one application message or as a complete stream segment.

The adapter preserves kernel timestamps, TCP sequence, and continuation positions as ordering evidence but does not claim a global total order across CPUs or hooks. It does not deduplicate retransmissions, resolve overlap, fill gaps, or parse payload.

**Alternative considered:** coalesce fragments in the adapter. Rejected because coalescing introduces reconstruction and can hide loss or truncation evidence.

### 6. Emit loss windows without attribution

Userspace samples cumulative per-CPU loss counters using the Gate A monotonic-clock model. Positive deltas and ambiguous sampling transitions produce:

```text
LossWindowObserved {
    start_timestamp,
    end_timestamp,
    drop_delta,
    loss_epoch_or_generation: Option<_>,
    ambiguity_information,
}
```

Counter reset/regression, map replacement, partial reads, missing initial/final samples, clock mismatch, and delayed sampling remain explicit ambiguity. A clean zero-delta sample advances the internal boundary without requiring an event.

The adapter never names affected sockets, marks traffic incomplete, or decides replayability. `LossWindowObserved` is capture-stream evidence, not the future persisted WAL `LossWindow` payload. Future persistence may translate it, and future ETL owns temporal attribution.

**Alternative considered:** attach each loss delta to active sockets. Rejected because Gate A counters cannot identify exact drop time or affected connection.

### 7. Fail typed and never silently discard malformed observations

Adapter errors cover:

- unsupported kernel capability
- eBPF attach failure
- verifier rejection
- event decode failure
- invalid kernel payload
- missing identity
- ring-buffer loss or incomplete loss-counter sampling
- shutdown cleanup failure

Decode/validation failures are surfaced through the adapter stream or terminal adapter result with hook/source context that excludes payload contents. Observed ring reservation loss is normally represented by `LossWindowObserved`; inability to form trustworthy loss evidence is also surfaced as a typed failure/ambiguous window. No malformed item is dropped only to keep the stream running.

### 8. Separate verified evidence from target compatibility

Every feasibility artifact and doctor result records:

```text
architecture
kernel version
Linux distribution
BTF availability
per-hook capability result
```

Reports have separate `verified_environment` and `target_compatibility_matrix` sections. This change accepts only:

```text
verified_environment:
  architecture: aarch64
  distribution: Ubuntu 24.04
  kernel_release: Linux 6.8.0-136-generic
  kernel_family: Linux 6.8
  cgroup_v2: true
  BTF: available
```

The retained report MUST record exact kernel release; family-level `Linux 6.8` labeling is additional context, never a substitute for exact evidence identity. `x86_64` is explicitly `not_verified` and requires a separate privileged acceptance environment and retained report. Other distributions, kernel minor versions, cloud kernels, physical NICs, and production offload conditions are likewise unverified.

A runtime capability probe may report an unverified host's observed results, but it cannot promote that host or architecture to verified compatibility without its own acceptance run and evidence artifact.

### 9. Keep acceptance privileged, bounded, and non-production

Portable unit tests use constructed `RawKernelObservation` values to cover conversion, identity, lifecycle, fragment order, loss windows, and invalid input. Privileged tests are clearly ignored/marked and run only with explicit root/capability setup.

Validated-environment acceptance creates a dedicated cgroup, attaches the adapter, generates IPv4 and IPv6 plaintext TCP traffic, observes multiple payload fragments and forced ring pressure, detaches or quiesces producer links, drains remaining ring items, takes the mandatory final counter sample while maps remain, verifies links/maps/programs are released, and writes an evidence report. It proves only kernel-to-`CaptureEvent` streaming and cleanup. It does not start a daemon or write WAL data.

### 6a. Give CaptureSource an explicit shutdown lifecycle

`CaptureSource` currently exposes polling only, so it cannot guarantee a final loss observation and ring drain before resources disappear. Future implementation defines this mandatory sequence:

```text
Created -> Running -> ShutdownRequested -> Draining -> Finalized
```

Repository-specific API names may differ, but responsibilities do not: `start` enters `Running`; `poll` consumes currently available normalized events without final sampling or release; `request_shutdown` stops accepting new kernel input, freezes the boundary, and makes final sampling eligible; `drain` emits every event accepted before shutdown plus final loss evidence; and `finalize` detaches programs and releases maps/ring resources only after shutdown and drain complete. `request_shutdown` is idempotent. `finalize` before successful drain is a typed lifecycle failure.

Shutdown ordering is fixed: (1) request shutdown, (2) stop intake, (3) observe final per-CPU counters on `CLOCK_MONOTONIC`, (4) create final `LossWindowObserved` when required, (5) drain accepted ring events and final diagnostics in source ordering, then (6) release resources. Final observation time is measured when counters are actually read, not assumed equal to shutdown request time. A final sample before any successful normal sample uses attachment/capture-start as its uncertain window start. A drain failure is typed and visible; cleanup still follows its defined release path and reports missing events diagnostically.

The source owns kernel acquisition, final loss sampling, draining, and cleanup. Higher layers own recording status, shutdown reason, WAL commit behavior, and replay/completeness decisions. This adds no WAL, ETL, protocol, canonical, replay, or daemon behavior.

**Alternative considered:** one ambiguous `shutdown()` method. Rejected because it cannot express intake freeze, drain completion, and resource finalization ordering or their distinct failures.

## Risks / Trade-offs

- [Risk] Gate A report lacks a machine-readable distribution field even though retained documentation identifies Ubuntu 24.04. → [Mitigation] Gate B6.4 evidence schema makes distribution mandatory and regenerates evidence on the verified environment.
- [Risk] Socket cookie may be reused over time. → [Mitigation] Pair it with first-seen monotonic generation and optional network namespace identity; never join by cookie alone across generations.
- [Risk] Lifecycle signals remain ambiguous. → [Mitigation] Preserve raw state-change evidence and forbid inferred half-close/clean-close semantics.
- [Risk] Ring loss cannot be attributed to connections. → [Mitigation] Emit bounded temporal/clock ambiguity only; defer attribution to ETL.
- [Risk] Architecture-dependent ABI layout may decode incorrectly. → [Mitigation] Keep ABI private, embed explicit version/size/byte-order checks, reject mismatch, and record architecture in evidence.
- [Risk] Requested B6.1-B6.4 labels overlap existing pending B2.1/B2.3/B2.4 and B6.1-B6.4 ownership. → [Mitigation] Treat this change's four gates as implementation-ready refinements/regrouping of those pending capture tasks, not replacements; do not modify accepted WAL, ETL, protocol, replay, or daemon decisions.

## Migration Plan

No production deployment or data migration occurs in this planning change. Future implementation proceeds gate-by-gate: add capture-domain evidence types while preserving Capture Event v1 fixtures, add the private Aya adapter behind existing target-specific boundaries, run unit tests, then run privileged acceptance on the verified environment and retain compatibility evidence. Rollback removes the new adapter path and leaves fixture capture and all downstream modules unchanged.

## Open Questions

- Which additional kernel signal, if any, will separately prove close or reset beyond ambiguous sock-ops state changes? Until proven, adapter emits only raw state-change evidence.
- When an x86_64 environment becomes available, which exact distribution/kernel pair will become its separate acceptance matrix entry? No x86_64 support claim is made by this change.
