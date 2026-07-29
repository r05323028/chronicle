## Context

Chronicle P0 is complete and archived. Current runnable flow is:

```text
FixtureCaptureSource
  -> CaptureEvent v1
  -> one fresh WAL v1 segment
  -> WalReader checkpoint
  -> SessionAssembler
  -> built-in HTTP detector/decoder/canonicalizer
  -> Canonical Session v1
  -> FilesystemSessionStore
  -> inspect OR replay planner/executor/verifier
```

Existing seams are sound, but production behavior is deliberately absent: `EbpfCaptureSource` always returns `NotImplemented`; live recording cannot stream because `write_capture_to_wal` buffers every event for a one-segment preflight; the writer cannot reopen; ETL reads one open file and has no durable checkpoint/public command; connection identity is only network namespace plus tuple; chunked and close-delimited HTTP are rejected; `doctor` and the generic application command facade are scaffolds.

P0 already has stronger replay groundwork than the request assumes. `ReplayExecutor::execute` returns `ReplayExecution { results }` and preserves transport-failure entries and later unattempted results. It lacks an explicit aggregate outcome and some non-transport errors still escape as `Err`, so P1 extends this model rather than introducing a parallel executor abstraction.

Crate dependency direction remains:

```text
CLI -> application services -> domain ports/types -> infrastructure adapters
```

Replay continues to depend on canonical/protocol data only. It never reads capture, WAL, ETL, or recorded production destinations.

## Goals / Non-Goals

**Goals:**

- Capture real outbound plaintext TCP traffic and corresponding responses for a dedicated Linux cgroup v2 workload.
- Persist userspace-received events through a bounded, segmented, recoverable WAL with a 4 MiB/10 ms one-sync group-commit window, checksummed in-WAL commit markers, and honest crash-loss accounting.
- Recover a stopped recording and deterministically transform it into one atomic canonical session without duplicate publication.
- Reconstruct directional TCP byte streams using lifecycle identity and TCP sequence positions, then decode bounded HTTP/1.1 messages honestly.
- Preserve provenance, completeness, integrity, and replay readiness through storage and inspect.
- Replay only complete supported operations against an explicitly authorized loopback target and retain partial execution/verification results.
- Provide portable fixture tests and a separate reproducible privileged Linux eBPF acceptance path.

**Non-Goals:**

- TLS plaintext extraction, TLS keys, HTTPS decryption, HTTP/2, HTTP/3, QUIC, or full packet capture.
- PostgreSQL, MySQL/MariaDB, Oracle, MongoDB, Kafka, NATS, S3 protocol semantics, or any new protocol.
- Dynamic plugins, Kubernetes components, distributed capture/ETL, object/database backends, hosted control plane, Web UI, mocks, dependency virtualization, or AI analysis.
- Secret-discovery guarantees, comprehensive redaction, encryption at rest, compliance, tenant isolation, or automatic production-target replay.
- Arbitrary inbound server workload replay, HTTP pipelined replay, production-scale/always-on capture, rotating sessions, incremental ETL, or synchronous ETL during record.
- General-purpose packet reassembly. P1 retains only metadata and bounded TCP application payload required for selected client-initiated flows.

## Decisions

### 0. Prove kernel hook feasibility before freezing capture-specific contracts

First implementation gate is a privileged Linux 6.1 feasibility harness, not production recorder code. It MUST demonstrate one IPv4 and one IPv6 client exchange with stable socket identity across candidate connect/sock-ops/cgroup-skb hooks, raw lifecycle state evidence, normalized tuple/direction, TCP sequence, cumulative ring-loss snapshots, GSO/GRO/nonlinear-skb behavior, and selected-cgroup subtree semantics.

Before this gate passes, implementation MUST NOT freeze production Capture Event v1 kernel-derived fields, connection identity assumptions, eBPF/userspace ABI, supported kernel/helper matrix, lifecycle guarantees, or GSO/GRO payload claims. Kernel-independent work MAY proceed in parallel: generic segmented-WAL framing/group commit/recovery scanner primitives, atomic metadata replacement, replay outcome/result modeling, bounded atomic JSON rendering, and filesystem no-replace publication. If feasibility fails, capture-specific implementation stops and this OpenSpec change is amended/revalidated; generic primitives do not become evidence that live capture works.

### 1. Preserve current component boundaries

```text
Dedicated Linux cgroup v2 workload
               |
               v
Aya eBPF capture adapter
  connect/sock-ops + cgroup skb hooks
               | capture events/loss snapshots
               v
bounded ingest channel (4096 events)
               |
               v
segmented WAL writer + recording metadata
               | durable segments
               v
standalone ETL reader/recovery
               |
               v
connection generation + TCP reconstruction
               |
               v
HTTP/1.1 decode and FIFO pairing
               |
               v
canonical session builder
               |
               v
FilesystemSessionStore atomic publication
               |
               +------> inspect
               |
               +------> replay plan -> authorized HTTP adapter -> verifier
```

`chronicle-capture` owns transport-neutral event contracts; `chronicle-capture-ebpf` owns Linux loading/attachment; `chronicle-wal` owns local durability; `chronicle-session` owns connection/stream reconstruction; `chronicle-etl` owns transformation/checkpoint coordination; existing canonical, storage, replay, application, and CLI layers retain their roles. ETL does not depend on CLI types, and canonical types do not depend on Aya, filesystem, or Tokio. Production composition adds one optional target-specific `chronicle-application -> chronicle-capture-ebpf` edge, propagated by `linux-ebpf` feature from CLI; Tokio adds only `signal` for application shutdown.

### 1a. Implementation gates produce runnable evidence

| Gate | Depends on | Runnable proof | Stop/go |
|---|---|---|---|
| A — Kernel capture feasibility | P0 + supported privileged Linux | feasibility eBPF build/test captures real payload/lifecycle/loss evidence | blocks only capture-specific ABI/contracts |
| B — Durable raw recording | A for capture; generic WAL may precede A | production/fake record -> bounded queue -> group-committed segmented WAL -> crash recovery | requires honest commit boundary/counters/status/bounds |
| C — Recovery and ETL | B | `chronicle etl` -> reconstructed HTTP -> deterministic session, same/cross-root reruns | requires scoped loss windows, deterministic IDs, fail-closed corruption |
| D — Canonical inspect | C | `chronicle inspect` -> integrity/completeness/provenance/partial replayability | requires safe complete operation accounting |
| E — Safe replay/verification | D | authorized mixed replay -> partial report, original target untouched | requires zero-network policy rejection and one result/operation |
| F — Diagnostics/hardening | A-E | doctor + rootless/eBPF compile/privileged 30-step acceptance + strict validation | required before archive |

Each gate's entry dependencies, exact implementation tasks/owners, commands, acceptance scenario, tests, artifacts, and stop/go condition live in `tasks.md`. Gate failure stops dependent gates but does not retroactively block kernel-independent primitives explicitly allowed by decision 0.

### 2. Select a dedicated cgroup v2, with PID as resolver only

P1 accepts exactly one of `--cgroup PATH` or `--pid PID`. Cgroup path resolves on host unified hierarchy under `/sys/fs/cgroup`; attachment covers the selected node plus descendants (the **selected subtree**). Root `/` and known host-wide shared roots (`/system.slice`, `/user.slice`, and `/machine.slice`) SHALL be rejected with no P1 override, including when `--allow-shared-cgroup` is supplied.

The **direct cgroup TGID set** SHALL mean the distinct host-visible thread-group IDs represented by the numeric process IDs listed directly in the selected cgroup node's `cgroup.procs`, after resolving each listed PID to its host-visible TGID. Its cardinality is the **direct TGID count**. This is not a POSIX process group ID (`getpgrp`/PGID), session ID, thread count, container ID, or union of descendant `cgroup.procs` files. Multiple listed PIDs resolving to one TGID count once; two TGIDs sharing one POSIX PGID count twice. `cgroup.procs` supplies direct-node membership only. Descendant cgroup directories are enumerated and reported separately as **descendant cgroup count** because capture scope includes the selected subtree. A listed PID that exits or cannot be safely resolved to a host-visible TGID SHALL NOT be omitted: required record preflight SHALL reject the selector, while a diagnostic probe SHALL report `not_checked` or reject according to its existing required/optional policy.

Preflight SHALL resolve canonical host path/stable cgroup inode/ID, compute the direct cgroup TGID set and descendant cgroup count, and compare Chronicle's own host-visible cgroup identity against the full selected subtree. Any selection containing Chronicle in the selected node or any descendant SHALL be rejected even when Chronicle is absent from the selected node's direct `cgroup.procs`. Inability to obtain required scope counts or identities SHALL fail record preflight rather than assume safety. Output and metadata include canonical host path, stable cgroup inode/ID, direct TGID count, descendant cgroup count, selected-subtree scope, and acknowledgement state, never command lines or environment values.

`--pid` resolves the selected PID's host-visible TGID and host-visible `/proc/<pid>/cgroup` during preflight, resolves again immediately before attachment, and resolves a third time immediately after links attach; all three canonical host paths and inode/IDs MUST match. B7.2 supplies baseline/revalidation API; B7.6 owns production source/link lifetime, invokes attach-time samples, and drops partial links on any failure. PID selector safety SHALL compare that host-visible TGID with the direct cgroup TGID set. Missing PID, namespace ambiguity, movement, identity change, inability to resolve any listed PID, absence of the selected TGID, or presence of any unrelated direct TGID SHALL fail selection and remove partial links. PID selection has no shared-scope override, never widens to an ancestor, and does not infer that a multi-process leaf is a dedicated container from names or shape.

Explicit `--cgroup` uses the exact selected subtree. A leaf with direct TGID count zero or one is accepted without override. Direct TGID count greater than one or any descendant cgroup is treated as shared/broad: preflight warns with non-sensitive separate counts and requires explicit `--allow-shared-cgroup`. The flag is valid only with explicit `--cgroup`; it MUST NOT bypass forbidden roots, Chronicle containment anywhere in the selected subtree, namespace-ambiguous PID resolution, identity instability, or unreadable scope enumeration. Process names and container IDs are not P1 selectors.

Cgroup identity is stable across threads and matches cgroup eBPF attachment. Gate A proved that `connect4/connect6` provide authoritative host-visible TGID, descendant cgroup ID, socket cookie, and candidate generation timestamp. PID-only filtering in packet hooks is unreliable because ingress may execute outside originating task context. `bpf_get_current_pid_tgid` is unavailable to sock-ops and not meaningful for cgroup-skb, while cgroup-skb's current cgroup helper may reflect execution rather than socket ownership. Payload/lifecycle events therefore join process/cgroup identity from connect evidence by socket cookie. Container hints may be recorded from cgroup metadata but never select scope or identify connection.

### 3. Use Aya and payload-only cgroup hooks

Aya fits existing Rust-first policy, avoids libbpf/clang/libelf runtime coupling, and was already documented as deferred until capture had real behavior. Dependencies remain isolated:

- Linux loader/runtime dependencies in `chronicle-capture-ebpf` behind `cfg(target_os = "linux")` and a `linux-ebpf` feature.
- `chronicle-application` and `chronicle-cli` propagate optional `linux-ebpf`; normal Linux builds with feature disabled expose typed unavailable result, while release recording builds enable it explicitly.
- A separate no-std eBPF package under `ebpf/`, explicitly excluded from root workspace and built for `bpfel-unknown-none`/`bpfeb-unknown-none`; loader embeds build artifact so installed CLI has no ambiguous runtime lookup.
- macOS workspace builds compile typed unsupported capture adapter and retain fixture/ETL/inspect/replay development without bpf-linker/eBPF target. Other non-Unix publication remains fail-closed.

P1 hooks are cgroup `connect4/connect6` for workload/socket attribution, cgroup sock-ops for active-establish/raw state-change lifecycle, and cgroup skb ingress/egress for IPv4/IPv6 TCP sequence plus application payload. Gate A's Linux 6.8 aarch64 probe correlated both families by socket cookie, retained veth GSO/GRO metadata (`gso_size` 1448, up to 10 segments) with payload beyond a 52-byte linear head, and separately reconstructed a nonlinear 32 KiB loopback aggregate as two ordered correlation-bearing 16 KiB direct ring-buffer continuations. Hot path filters selected cgroup, parses only required L3/L4 fields, and copies payload as at most four 16 KiB continuation events (64 KiB observed-payload ceiling); larger/non-readable remainder is explicitly truncated. Sock-ops state alone does not distinguish reset from orderly close or prove directional half-close, so production preserves raw state and emits `unknown` unless another proven signal supports a stronger lifecycle value. Hot path updates counters and returns. It performs no HTTP parsing and does not retain raw packet headers.

Only client-initiated connections attributed at connect are canonical replay candidates. Unattributed or mid-connection observations remain inspectable incomplete evidence.

Supported runtime baseline is Linux 6.1+, cgroup v2, readable kernel BTF at `/sys/kernel/btf/vmlinux`, x86_64 or aarch64, and kernel support for required cgroup hooks/ring buffer/socket cookie helpers. Gate A retained proof for Ubuntu 24.04, Linux 6.8.0-136-generic, aarch64, Aya 0.14.0/aya-ebpf 0.2.1; every other kernel/architecture combination remains runtime-probed rather than inferred from that report. Required effective privileges are `CAP_BPF` and `CAP_NET_ADMIN` plus any kernel-specific attach/perf capability reported by `doctor`; older capability models require root/`CAP_SYS_ADMIN` and are unsupported by policy unless run as root. Load/attach failure is explicit and creates no recording marked successful.

Ring buffer capacity is fixed at 8 MiB in P1. Reservation/backend failure increments cumulative per-CPU dropped-event counters. While recording is active, userspace schedules a complete per-CPU sample at least every 100 ms using the same boot-scoped monotonic clock ID/origin as Capture Event kernel timestamps. Capture attachment time is the initial uncertainty boundary; the first successful sample establishes the next observation boundary. Zero delta advances the in-memory boundary without a WAL record. Positive delta emits a typed `LossWindow` record containing actual previous/current successful sample times, aggregated delta, epoch/generation, clock identity, and map identity. A late/stalled poller uses actual elapsed timestamps, never a fabricated 100 ms interval. Counter regression/reset, map replacement, incomplete per-CPU read, boot-ID change, or clock mismatch emits ambiguous loss evidence and cannot support a complete recording claim.

Loss before first successful sample spans attachment time through that sample. After intake stops, shutdown retains the maps, performs a mandatory final sample before WAL finalization, then releases links/maps. If final sampling fails, uncertainty spans previous successful sample through shutdown and recording fails with `capture_failure` while preserving the committed prefix. Counters cannot name exact drop instants or affected connections; ETL degrades only overlapping or temporally/clock-ambiguous connections. Capture events and `LossWindow` records enter one userspace WAL sequencer in observation order; each receives `WalRecordEnvelope` ordering without adding WAL sequence to Capture Event.

### 4. Separate capture-domain evidence from WAL ordering

Capture Event v1 is capture-domain data only. Stable socket identity joins lifecycle and payload evidence. Active connect intent records remote target before autobind; complete `SocketConnected` evidence records validated IPv4/IPv6 local and remote endpoints plus active/passive role at establishment. Payload fragments retain direction/TCP position/payload/truncation without duplicating endpoints or role. Reset/closed values require proven signal; sock-ops state alone remains `state_changed` or `unknown`. `LossWindow` is separate typed persistence payload derived from capture counters.

`WalRecordEnvelope` owns persistence identity and order: recording ID, WAL sequence, segment ordinal/first sequence, byte offset, `RecordKind`/schema version, and encoded payload. `CaptureEvent`, `LossWindow`, and `CommitMarker` record kinds consume same monotonically increasing envelope sequence. Segment/offset become known while writing/recovery, not while capturing. Persistence assigns sequence while wrapping opaque payload, and writer/recovery attaches provenance. Sole WAL v1 reader validates framing, payload length, CRC32C, record schema value 1, and placed provenance without format dispatch. Fixture and production Capture Event v1 use same segmented/group-committed WAL and reconstruction path.

B1.1 establishes mutable-v1 validation for each internal artifact domain. Capture, WAL, canonical, manifest, recording metadata, ETL checkpoint, fixtures, and machine reports accept only value 1 and use unversioned Rust models. No version dispatcher, migration default, dual writer, or repository-history reader remains before explicit compatibility freeze. `recording.json` includes recording ID, optional immutable selector path/cgroup ID, lifecycle status, separate shutdown reason, last-valid-marker boundary, categorized counters, and feasibility-checked capture metadata.

### 4a. Normalize capture evidence before reconstruction

`ReconstructionInput` is small reconstruction-domain contract shared by fixture and production Capture Event v1. Both sources emit same `CaptureEvent` type and enter same conversion without source-version normalization. Contract carries socket identity, complete optional socket evidence, monotonic timestamp, direction, TCP sequence/continuation, raw payload/truncation, raw lifecycle evidence, recording scope identity, and separate loss-window evidence. It contains no WAL sequence/segment/offset, Aya/kernel ABI value, HTTP field, reconstructed message, or fixture-only persistence field.

Reconstruction groups by socket generation, correlates payload to complete evidence, and returns typed missing/conflicting provenance failure. Active maps local→client/remote→server; passive maps remote→client/local→server. Direction controls bytes only. Ordering, deduplication, loss ambiguity, timeout, and HTTP parsing remain later bounded stages.

### 5. WAL v1 is segmented, append-only, bounded, and group-committed

Layout:

```text
<wal-dir>/
  recording.json
  record.lock
  segments/
    00000000000000000001.chwal
    00000000000001048233.chwal
  etl/
    checkpoint.json
    recovery-report.json
```

Each sole-v1 segment starts with exact 64-byte little-endian header. Bytes 0..4 are `CHS1`; 4..6 are format version 1; 6..8 are header schema version 1; 8..12 are header length 64; 12..28 are raw recording UUID bytes; 28..36 are segment ordinal; 36..44 are first WAL sequence; 44..52 are signed Unix-nanosecond creation time; 52..60 are zero reserved bytes; and 60..64 are CRC32C over bytes 0..60. Each `WalRecordEnvelope` frame has exact 48-byte little-endian header. Bytes 0..4 are `CHE1`; 4..6 are envelope version 1; 6..8 are `RecordKind`; 8..10 are flags; 10..12 are record schema version 1; 12..16 are header length 48; 16..20 are payload length; 20..36 are raw recording UUID bytes; 36..44 are WAL sequence; and 44..48 are CRC32C over bytes 0..44 followed by payload. Segment ordinal, segment first sequence, and frame byte offset are derived from validated segment context. Reader rejects wrong magic, any non-v1 declaration, kind, length, checksum, identity, provenance, or sequence before payload allocation; no old framing dispatch remains.

Published segments use exact 20-digit `<first-sequence>.chwal` names. Header-only discovery ignores dot-prefixed temporary files, sorts by numeric filename, validates recording identity and filename/header first-sequence equality, requires unique contiguous 0-based ordinals and strictly increasing first sequences, and rejects other entries. It does not infer frame ranges from headers; exact missing/overlapping envelope sequence validation belongs to B5.1 commit-aware recovery scan.

Default segment size is 256 MiB, configurable from 16 MiB through 4 GiB. Total WAL segment storage defaults to and is hard-capped at 4 GiB; `--max-wal-bytes` may lower that bound but MUST be at least selected segment size. Accounting includes every byte under `segments/` (headers, frames, and temporary publication files) and excludes metadata/lock/ETL reports. Rename-based header publication counts one header allocation; any implementation that duplicates bytes during publication MUST reserve peak temporary plus final bytes. Rotation happens before an append that would exceed segment limit. One format-oversized record is rejected without a partial frame.

P1 adds `RecordKind::CommitMarker` schema v1 as a normal CRC32C-framed `WalRecordEnvelope`. Its fixed 76-byte payload is, in order, little-endian `u16 schema_version = 1`, `u16 reserved = 0`, `u64 durable_through_sequence`, `u64 durable_record_count`, `u64 durable_payload_bytes`, `u64 batch_first_sequence`, `u64 batch_last_sequence`, then 32 raw bytes `batch_sha256`. Counts are cumulative across committed non-marker envelopes; digest covers exact encoded bytes of the contiguous current-segment batch in sequence order. Marker sequence is `batch_last_sequence + 1`; it covers no other marker, never crosses a segment, and makes its own sequence the physical durable boundary. `LossWindow` is a non-marker record and may be covered. Unknown marker versions fail explicitly.

P1 exposes one durability mode. Group commit ordering is: append complete non-marker frames; append complete marker covering them; flush buffered writer; call one `fdatasync` on current segment; only after that call returns success mark the covered batch runtime-durable and send or record its durability acknowledgement; then update in-memory counters and refresh non-authoritative `recording.json` asynchronously or at lifecycle boundaries. No external watermark file, second metadata sync/rename/directory-sync per 10 ms batch, per-record sync, or user-facing durability mode exists.

A commit marker SHALL be **recovery-authoritative** only when its frame is complete; its envelope CRC32C is valid; every referenced frame exists; referenced WAL sequences are contiguous; all referenced frames are non-marker frames in the permitted same-segment batch; `batch_first_sequence` equals the segment first sequence for the first batch in that segment or the previous recovery-authoritative marker sequence plus one for a later batch; `batch_last_sequence` equals the final referenced frame sequence; marker sequence equals `batch_last_sequence + 1`; durable-through and cumulative counts/bytes equal the previous authoritative totals plus the exact current-batch totals; `batch_sha256` matches the exact referenced framed bytes in sequence order; and all relevant segment/header recording identities, ordinals, first sequences, checksums, and versions validate. A complete-looking marker that fails any frame, reference, sequence, segment, identity, version, CRC32C, boundary, cumulative-field, or batch-digest check MUST NOT be authoritative and SHALL fail closed under corruption rules. Frames after the final recovery-authoritative marker remain uncommitted uncertainty and SHALL NOT be promoted.

Recovery authority applies only to persisted WAL durability. Runtime acknowledgement delivery is separate: `fdatasync` may succeed and the process may fail before a caller observes the acknowledgement. Recovery MAY therefore recognize an authoritative committed batch without evidence of acknowledgement delivery, but MUST NOT infer that any caller, queue worker, metadata writer, or CLI renderer observed it before the crash. Acknowledgement-delivery state is not reconstructed unless separately persisted, which P1 does not require. Recording metadata counters may lag and SHALL be reconciled from recovery-authoritative committed WAL without creating an acknowledgement-delivery claim. Pre-sync durability tests therefore reopen a fault-injected durable-media image that excludes unbarriered writes, not merely kill/reopen the same page cache. Empty batches emit no marker. Fixture and production WAL use same current v1 behavior.

Commit triggers remain 4 MiB unsynced non-marker frame bytes, 10 ms since last successful group sync while data is unsynced, rotation, explicit internal flush, and shutdown. Rotation first commits/syncs all current-segment data, finalizes current segment state, creates/syncs/publishes next header, then begins at marker sequence + 1. Shutdown stops intake, applies hard-limit queue policy, appends the final marker for every writable accepted frame, performs one `fdatasync`, then finalizes metadata.

The writer maintains worst-case fixed final-marker reservation before each non-marker append in both current segment and total storage. If frame plus marker cannot fit current segment, it commits current batch and rotates before frame. Remaining total physical capacity MUST cover next complete frame, final marker, and next header/peak temporary publication bytes when rotation is required. If not, it writes no part of that frame, triggers `wal_size_limit`, stops intake, and writes the longest queued prefix whose complete frames still preserve marker reservation. Remaining queued events are counted as `discarded_from_queue_due_to_wal_limit`; post-stop arrivals are counted separately as rejected. Already written frames are committed/synced. Success is `completed` + `wal_size_limit`; marker/sync/metadata failure is `failed` + `wal_failure`. Physical bytes under `segments/` never exceed configured cap.

#### 5a. Terminal WAL-loss is recorder-admission evidence

WAL-cap discard is not a kernel/backend capture loss and SHALL NOT create a new `CaptureEvent` or capture-source event variant. It is recorder/WAL admission evidence: an event was accepted into the bounded ingest path, then could not be persisted after the physical-cap reservation failed. `QueuedWalRecord` therefore carries optional `MonotonicTimestamp { clock: ClockIdentity, nanoseconds }` capture-time metadata independently of its opaque WAL payload. Queueing, flushing, process uptime, and marker persistence timestamps SHALL NOT supply this value.

`RecordKind::TerminalWalLoss` schema v1 is a typed control payload, distinct from kernel-derived `RecordKind::LossWindow`. Its stable fields are `TerminalWalLoss { interval: TerminalWalLossInterval { start: MonotonicTimestamp, end: MonotonicTimestamp }, discarded_records: u64, discarded_payload_bytes: u64, reason: TerminalWalLossReason::WalHardLimit, ambiguity: TerminalWalLossAmbiguity::UnknownDownstreamEffects }`. The interval start/end clocks MUST be identical and `start.nanoseconds <= end.nanoseconds`; `discarded_records` MUST be nonzero. Zero discarded payload bytes are valid only for discarded zero-length payload records. The payload contains no socket identity, kernel/Aya ABI value, queue address, WAL path, arbitrary error text, HTTP field, or inferred affected connection.

The ingest state machine maintains one terminal-discard accumulator per `ClockIdentity`. Each accumulator has exact records/bytes and earliest/latest valid discarded capture timestamp; it emits at most one terminal record per clock for one WAL-limit stop. Clocks are never ordered or merged. A discarded record without valid capture time is retained in a typed metadata-only `TerminalWalLossSummary` entry; when a compatible last persisted capture timestamp exists it supplies its conservative interval start, otherwise the summary records `TimestampUnavailable` ambiguity. Such an entry SHALL NOT manufacture a WAL interval payload. Metadata summaries retain exact total discarded records/bytes, per-clock entries, omission/failure state, and no socket attribution.

Terminal-loss records use a reserved terminal-record path: write only when complete terminal frame plus required final marker fit. They are control evidence, never recursively reclassified as ordinary discarded input, and never increment ordinary WAL-limit discard counters. Successful final marker sync makes a terminal record recovery-authoritative. If no room remains, metadata/final summary carries equivalent typed evidence. If terminal-frame, marker, flush, sync, or final metadata persistence fails, finalization is `failed` + `wal_failure`; prior committed prefix remains authoritative and summary state is `NotPersistedDueToWalFailure`, never a persistence claim. Recovery exposes committed terminal records as typed terminal-loss evidence and excludes them from normal capture-event reconstruction through sole mutable-v1 reader.

Counters/metadata use stable categories `received`, `accepted_into_queue`, `written_not_committed`, `committed`, `discarded_from_queue_due_to_wal_limit`, `kernel_or_backend_dropped`, `rejected_after_stop`, and `etl_checkpointed`, with records/bytes where available. A record leaves queued state when fully written but advances durable counts only after successful group sync. Written-not-durable counters are snapshots, not durability claims.

Recovery scans envelope sequence and marker semantics. The final recovery-authoritative commit marker controls the persisted committed boundary; complete frames after it are uncommitted uncertainty and are not promoted. A partial final marker contributes no commit and may be removed only by verified-final-tail repair. Any complete marker failing reference, sequence, segment, identity, version, CRC32C, boundary, cumulative-field, or batch-digest validation is corruption and fails closed rather than being treated as acknowledgement evidence. Recording metadata may lag and is reconciled from recovery-authoritative committed frames without claiming runtime acknowledgement delivery occurred; missing metadata does not invalidate committed evidence when immutable recording identity is established from validated segment headers. Fixture and production WAL use same current v1 behavior.

Userspace channel is bounded to 4096 events. Producer blocks when full; kernel ring pressure appears through loss samples. WAL write/flush/sync/permission/disk-full errors stop recording, preserve committed prefix, record written-not-durable uncertainty, and finalize `failed`; no continue-on-error policy exists.

### 6. Recovery mutates only a verified partial final tail

Recovery locks recording, validates segment identity/order and every envelope through the final recovery-authoritative commit marker, and treats `recording.json` as last-known lifecycle/selector metadata. Read-only recovery reports complete post-marker suffix without mutation; explicit reopen-for-append discards only that reported uncommitted suffix, syncs changed file/directories, then resumes at marker sequence plus one. It reconciles crash-stale derived metadata but fails immutable recording ID/selector mismatch. A truncated final frame in final segment, including a partial final marker, is truncated to prior verified frame boundary, file-synced, and parent directory synced with recovery warning; that marker contributes no commit. A truncated published segment header, checksum mismatch/malformed frame before the final recovery-authoritative marker, invalid marker reference/digest, sequence gap, segment overlap, or recording-ID mismatch is not skipped/repaired: evidence remains intact and ETL fails by default. Incomplete temporary segment files are reported and ignored because never published.

Low-level WAL reopen-for-append begins only after successful recovery at sequence after last retained envelope and never treats uncommitted complete frames as durable. It exists to satisfy deterministic restart/append durability contract, not to define recording resume. Production record lifecycle never reopens terminal recording without future explicit command. Metadata left `starting` or `recording` after process death is atomically finalized as `aborted` during recovery with `process_crash_recovered` or persisted `forced_termination` shutdown reason. Recovery records any lost/uncertain post-marker range and never relabels crash recovery as graceful completion.

### 7. Recording status describes finalization; shutdown reason describes cause

`record` validates selector/shared-scope acknowledgement, platform, hooks, privileges, BTF, WAL destination, bounds, permissions, and free-space warning before source attachment. `chronicle-capture-ebpf` supplies no-attach object/program-presence and capability-prerequisite probe; B7.6 owns atomic attach failure cleanup and only then transitions metadata to successful recording. Production CLI adds `--duration-seconds` (default 600, inclusive range 1..=3600), `--max-wal-bytes` (default/hard maximum 4 GiB, lower values allowed down to selected segment size), and explicit-cgroup-only `--allow-shared-cgroup`. First duration or total-WAL limit reached detaches/stops new capture immediately, then finalizes within fixed five-second grace. Duration stop processes accepted queue under same frame/marker/header capacity reservation; WAL-limit stop writes only capacity-safe queued prefix and classifies remainder. Timeout/failure becomes failed rather than hanging or claiming completion. No later events are silently ignored.

Recording status transitions are:

```text
starting -> recording -> completed | failed
starting -> failed
starting/recording --recovery-after-crash--> aborted
```

Terminal status is separate from required `shutdown_reason`: `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `wal_size_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`. Graceful SIGINT/SIGTERM, source EOF, and clean configured-limit stop become `completed` only after detach, required final loss sample, capacity-qualified queue handling, final commit-marker sync, segment/metadata finalization succeed. Capture/WAL or required final-sample failure becomes `failed`. Recovered stale active metadata becomes `aborted`. `interrupted` is not a P1 status.

Core metadata v1 is frozen before recovery and includes recording ID, optional immutable selector path/stable cgroup ID, status, separate shutdown reason, last-valid-marker boundary, and categorized counters. It is persisted privately and atomically by temp write, file sync, rename, then directory sync. B7.3 extends this core after Gate A with schema/build/kernel/host identity, selector scope counts/acknowledgement, capabilities, configured/effective bounds, start/end, segment summary, and bounded capture errors without changing core field meanings. Host identity excludes command line/environment and captured payload values. Successfully handled SIGINT and SIGTERM exit 0. Second signal may force immediate exit after best-effort reason persistence, leave a recoverable partial/unsynced tail, and become `aborted` on recovery.

One connection/message spanning a duration or WAL boundary retains committed prefix evidence and is incomplete unless trusted evidence proves completion before the final recovery-authoritative commit marker. ETL consumes exactly the recovered committed boundary. Production `record` never runs ETL implicitly; fixture mode remains portable source using same mutable-v1 artifact path.

### 8. Reconstruct TCP streams from connection generation and TCP positions

Reconstruction groups by boot ID + socket cookie + generation, validates tuple/cgroup consistency, and maintains independent client/server sequence spaces. It converts 32-bit TCP sequence numbers into wrap-aware relative offsets, sorts segments deterministically by relative offset then WAL provenance, removes exact retransmitted overlap, and flags conflicting overlap as malformed. Missing intervals, truncation, lifecycle gaps, eviction, or possible crash-window loss produce incomplete/truncated state; bytes on separate generations are never mixed. ETL derives conservative loss windows from successive persisted counter snapshots and compares them with each connection activity interval. Connections provably outside every loss window remain eligible for complete classification; intersecting connections degrade; unknown/insufficient timing degrades conservatively. Session completeness may be partial while unaffected operations remain complete.

Protocol input preserves byte order within each direction by TCP position and deterministic cross-direction observation order by kernel monotonic timestamp then enclosing WAL sequence. Raw lifecycle evidence stays distinct from derived userspace termination (`clean_close`, `half_close`, `reset`, `unknown_termination`). Half-close is derived only if Gate A proves directional state evidence; otherwise `state_changed` remains unknown. Provenance ranges record segment first sequence, byte offset, WAL sequence, direction, and payload subrange. Current fixture events carry deterministic socket identity, endpoint/role evidence, and ordering through same reconstruction model as production.

### 9. Extend HTTP decoder narrowly

Existing `httparse` remains responsible for bounded start-line/header syntax. Chronicle owns connection decoder, stream buffers, request-method queue, lifecycle-aware finish, and framing. P1 supports request/status lines, ordered headers, exact Content-Length, chunked response transfer coding including bounded trailers as response metadata, no-body responses, valid response-to-close framing only on trusted derived clean-close evidence, keep-alive, and multiple sequential request/response pairs. Chunked requests are unsupported/non-replayable in P1 so trailer semantics cannot bypass request sanitization.

Connection decoder consumes messages in deterministic cross-direction completion order from timestamp/WAL provenance. Requests and final responses pair FIFO; HEAD response rules use queued method. HTTP pipelining is detected when second request completes before first final response; identical directional bytes with sequential versus pipelined observation interleavings therefore remain distinguishable. P1 preserves affected messages/provenance but marks pipelining unsupported/non-replayable. Informational 1xx, CONNECT, Upgrade/WebSocket, non-HTTP/1.1, ambiguous multiple framing, malformed chunks, unmatched messages, and residual bytes become typed malformed/incomplete/unmatched evidence. They are never converted to complete operations.

Canonical MVP v1 adds typed completeness (`complete`, `incomplete`, `truncated`, `malformed`, `unmatched`, `unsupported`) and typed recording/connection/WAL provenance as ordinary append-only fields. `CanonicalSession` owns source provenance, connection/operation completeness maps, and replay attributes directly; completeness maps are authoritative and timeline is sole operation order. Reader accepts only v1 and rejects every other version. P1 writer emits v1.

### 10. ETL restarts from durable input and publishes once

P1 intentionally uses one canonical session per stopped recording. Session ID is deterministic from SHA-256 over a domain separator, recording ID, and canonical pipeline version; UUID variant/version bits are normalized. Connection/operation IDs are derived similarly from stable source provenance and message order. This reuses existing SHA-256 dependency and avoids random IDs on rerun.

ETL sequence is:

```text
recover/discover WAL -> validate versions -> read batches -> reconstruct
-> detect/decode/pair -> canonicalize -> build one session
-> publish atomically -> write checkpoint atomically
```

P1 has no `output-binding.json`. One recording is not product-bound to one filesystem root. Deterministic session ID plus `wal_snapshot_sha256`, manifest provenance/checksum, and existing atomic no-replace publication are sufficient within each requested store. ETL may intentionally materialize unchanged recording into another output root; container/host path aliases and restored recordings therefore need no operator binding reset.

Post-publication checkpoint is ETL-owned advisory JSON containing version, recording ID, last-valid-commit-marker segment/offset/sequence, SHA-256 of exact ordered verified segment bytes through that marker (`wal_snapshot_sha256`), recovery-report digest/scope, pipeline/canonical versions, deterministic session ID, latest published manifest checksum/output identity, status, and counters. It is written only after publication and never rejects a different requested output root. Manifest contains commit-marker boundary, pipeline version, WAL snapshot digest, and deterministic session ID, but never checkpoint-file checksum.

P1 does not persist intermediate reconstruction state: interruption before publication rereads WAL from beginning. For requested output root, existing deterministic session path is accepted only after manifest/session/provenance/checksum verification and reported `already_processed`; mismatch fails without overwrite. Missing checkpoint after publication is repaired from matching manifest. Another root publishes same deterministic session independently. WAL change after publication changes digest and is conflict, not silent mutation.

Unsupported protocol and malformed/incomplete traffic are published as bounded non-replayable evidence when WAL integrity remains trustworthy. WAL corruption/version failure, checkpoint contradiction, or publication failure stops before checkpoint advance. Staging cleanup is best-effort and final session remains absent until rename.

### 11. Extend filesystem inspection, not storage architecture

Existing `FilesystemSessionStore` remains sole P1 canonical store. Mutable manifest v1 is updated in place with source recording ID, capture range, completeness counts, WAL provenance summary/checksum, canonical schema version, integrity, and replay readiness. Full bodies/headers remain persisted by design and can contain secrets. Default human/JSON inspect returns metadata, methods/targets/statuses, sizes, warning codes, and checksums—not arbitrary header values or bodies. `--show-body` or general secret discovery is not added.

Inspect verifies manifest/session checksums and payload existence/size without printing content; replay hydration retains full checksum verification. Publication stays staging + sync + atomic no-replace rename with restrictive permissions.

### 12. Extend existing `ReplayExecution` for mixed sessions

`ReplayExecution` gains `outcome: ReplayOutcome`; no parallel executor model is created. Exact serialized outcomes are `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`. Per-operation result gains explicit transport state (`completed`, `failed`, `not_attempted`) while retaining operation ID, decision, transport category, verification, and stable not-attempted reason. Every canonical operation appears once in plan and result order.

Planner classifies complete/supported operations as executable candidates and incomplete, truncated, malformed, unmatched, unsupported, pipelined, ambiguous-loss, or over-limit operations as non-executable. Non-executable operations remain in plan/report with `not_attempted` and Skipped/Inconclusive/Unsupported reason; they do not by themselves block complete siblings. Session replayability is `fully_replayable` when every operation is executable, `partially_replayable` when at least one is executable and at least one is not, and `not_replayable` when none are executable or session integrity/protocol capability invalidates all.

Before network, application validates session integrity/capabilities, explicit loopback target, allow-host, execute, and effect authorization for every executable candidate. Any target/effect authorization rejection sends zero requests and marks executable candidates policy-not-attempted while preserving non-executable reasons. If authorized, executor runs executable operations in deterministic plan order and skips invalid entries without traffic. Runtime transport/non-passing verification stops later executable operations, but all invalid and later entries remain represented. All executable operations passing yields `completed` or `completed_with_skips`; no executable operations yields `stopped_invalid_session`.

Verification and P0 safety remain unchanged: exact status/body digest/size/selected ordered headers, literal loopback target, no recorded destination fallback, stripped captured credentials/forwarding headers, one request per connection, five-second timeout, no retry/proxy/TLS/redirect. Exit 0 covers dry-run, `completed`, and `completed_with_skips`; 4 covers target/effect policy or no executable operations; 5 transport stop; 6 executed verification stop. Unsupported/inconclusive non-executable entries do not force exit 6 after successful partial execution.

### 13. Doctor reports typed probe results

`doctor` marks probes required or optional and reports `supported`, `supported_with_warnings`, `unsupported`, or `not_checked` with stable code/remediation. Aggregate precedence is: any required unsupported -> unsupported; else any required not-checked -> not-checked; else any warning or optional unsupported/not-checked -> supported-with-warnings; else supported. Required unsupported or not-checked exits 4; supported/warnings exits 0. Probes cover OS/architecture/kernel, cgroup v2, BTF, hooks/helpers, capabilities, eBPF object/backend, WAL path/free space, atomic rename/sync, protocol registry, and replay policy. Temporary private files are removed and values/command lines are never inspected.

### 13a. One bounded atomic JSON rendering boundary

Application services build typed command results; one small shared renderer serializes the complete versioned JSON value into bounded memory before CLI writes stdout with one checked `write_all`. Record/ETL/inspect/replay/doctor reuse this boundary. Serialization failure is typed application/data error before stdout; stdout write/flush failure is CLI I/O error and may leave externally truncated bytes but is never mislabeled as serialization success. Replay execution is never retried because rendering failed. Per-command result bounds, including 10,000 replay operations and bounded doctor/issues arrays, cap renderer memory. Human rendering remains command-specific; no generic reporting framework is introduced.

### 14. Bounds and observable counters are part of correctness

| Boundary | P1 bound/default | Limit behavior |
|---|---:|---|
| recording duration | 600 s default; configurable 1-3600 s | first expiry gracefully stops/finalizes `completed` + `duration_limit` |
| total WAL | 4 GiB default and physical hard max; configurable lower >= segment | reserve final marker/header, write capacity-safe queued prefix, classify remainder, never exceed cap |
| group commit | 4 MiB unsynced or 10 ms; rotation/flush/shutdown | append one marker + one `fdatasync`; failure finalizes `failed` + `wal_failure` |
| loss sampling | every 100 ms while active plus mandatory final sample | actual delayed interval; positive/ambiguous deltas become typed windows |
| eBPF ring | 8 MiB | increment cumulative per-CPU drop counters |
| capture payload/event | 16 KiB; up to 4 continuations/observed skb | truncate above 64 KiB/unreadable remainder |
| userspace ingest queue | 4096 events | block producer; expose ring losses |
| WAL record | existing 16 MiB max | reject/stop recording failed |
| WAL segment | 256 MiB, configurable 16 MiB-4 GiB | rotate and sync before append |
| active connections | existing 1024/session | ETL fails typed limit; no silent omitted connection |
| reconstructed bytes/connection | existing 8 MiB | finalize affected connection truncated |
| chunks/connection | existing 65,536 | finalize affected connection truncated |
| idle connection retention | 5 minutes event time | finalize incomplete |
| HTTP head | existing 64 KiB/128 fields | malformed/unsupported |
| HTTP buffered body | 8 MiB/operation | truncated/non-replayable |
| ETL read batch | 4096 records | continue next batch |
| replay observed body | 8 MiB/operation | typed transport/framing failure |
| canonical operations/report | 10,000/session | ETL fails typed limit before publication; no operation omitted |

Record cannot count HTTP operations because eBPF/WAL hot path intentionally does not parse HTTP; duration/WAL limits bound capture instead. ETL operation/connection overflow therefore fails explicitly rather than silently dropping evidence. Command summaries expose received, accepted/queued, written-not-committed, committed, WAL-limit queued discard, kernel/backend drop, post-stop rejection, ETL-checkpointed, loss-window, recovery, completeness, and replay result counters. P1 adds no metrics server.

## Failure and Recovery Semantics

| Boundary | Default behavior |
|---|---|
| Capture backend startup/feature/privilege failure | fail before status `recording`; no fallback |
| Ring-buffer loss/truncation | persist typed loss window/truncation; degrade intersecting or uncertain connections, not provably outside ones |
| Queue saturation | bounded block; kernel loss remains observable |
| WAL hard limit with queued events | write only complete capacity-safe prefix with marker reservation; count/disclose remainder and terminal loss interval |
| WAL group-sync/permission/disk/write failure | stop capture, finalize failed, preserve committed prefix, report written-not-durable uncertainty |
| Duration/total-WAL limit | bounded graceful stop; completed only after final sample/marker/metadata; spanning or discarded evidence may be incomplete |
| Abrupt termination | OS releases lock; next recovery marks aborted, discards/reports post-marker uncertainty, repairs only verified final tail |
| WAL corruption/version mismatch | stop ETL; preserve bytes; write safe recovery report |
| ETL crash | no checkpoint advance; rerun deterministic from start |
| Malformed/incomplete/unsupported traffic | publish honest non-replayable evidence if WAL integrity is valid |
| Session publication failure | final path absent; no checkpoint advance |
| Mixed-session invalid operation | operation remains non-executable/not-attempted; authorized complete siblings may run |
| Replay target/effect policy rejection | zero network, all operations represented not-attempted |
| Replay refusal/timeout/disconnect | prior results retained; failed and unattempted entries emitted |
| Verification mismatch | retain report, stop later operations, exit 6 |
| JSON encoding failure | safe typed CLI error/exit 3; no replay retry |

## Security Boundaries

- Capture requires explicit Linux privileges and selected dedicated cgroup; P1 does not sandbox a privileged recorder.
- WAL and sessions contain production plaintext, headers, and bodies. Private permissions reduce accidental exposure but are not encryption or tenant isolation.
- Logs, errors, inspect, doctor, and default reports contain only metadata/codes/sizes, never payload or arbitrary captured header values.
- Recorded bytes are untrusted data: parsers are bounded, malformed input returns typed state/error, and no payload is interpreted as command/configuration.
- Captured Host, forwarding, cookie, authorization, connection, and transfer headers cannot grant replay authorization. Replay policy is built only from explicit CLI gates.
- Literal loopback target and no redirects prevent DNS rebinding, authorization expansion, redirect escape, and implicit original-target replay.

## Alternatives Considered

- **HTTP parsing in eBPF:** rejected; verifier limits, kernel safety, parser complexity, and version churn belong in userspace.
- **Direct canonical storage:** rejected; loses recoverable raw evidence and couples capture availability to ETL latency.
- **Single WAL file:** rejected; recovery/rotation/disk management and bounded scan are poorer than existing segmented seam.
- **Return replay transport errors directly:** rejected for expected operational stops because prior results disappear; extend current `ReplayExecution` with outcome.
- **Synchronous ETL during record:** rejected; P0 uses it only for finite fixture convenience, while production needs capture durability and restart independence.
- **General packet capture/pcap:** rejected; needs broader reassembly/privileges and violates P1 payload-only scope.
- **Syscall uprobes/kprobes:** rejected for P1 because read/write/send variants, copied buffers, runtime/library differences, and TLS ambiguity complicate coverage. Cgroup socket hooks match dedicated workload selection and provide TCP positions.
- **PID-only selection:** rejected as primary filter because ingress context and process/thread lifetime are unstable. PID remains a cgroup resolver.
- **libbpf-rs:** viable but rejected for P1 due to clang/libelf/skeleton build requirements and mismatch with Rust-first dependency policy.
- **Random session IDs:** rejected because ETL reruns would duplicate output. Deterministic provenance-derived IDs make no-replace publication an idempotency gate.
- **Persist intermediate ETL state:** deferred; rereading bounded durable WAL is simpler and enough for restart correctness in P1.

## Risks / Trade-offs

- [Kernel/helper semantics differ across supported versions] -> require Linux 6.1+, BTF, startup feature probes, typed doctor output, and privileged acceptance matrix.
- [Cgroup hooks still miss traffic or lose ring events] -> typed conservative loss windows; intersecting/uncertain connections degrade while provably outside connections remain eligible.
- [TCP overlap/wrap/lifecycle ambiguity] -> generation identity, wrap-aware ordering, exact dedupe, conflict warnings, and non-replayable output.
- [Group commit can lose recent writes on crash] -> fixed 4 MiB/10 ms window, in-WAL marker plus one `fdatasync`, durable-media fault injection, separate recovery-authoritative persisted durability versus acknowledgement-delivery semantics, honest aborted/loss metadata.
- [WAL/session disk growth and sensitive data] -> 10-minute default/60-minute max duration, 4 GiB WAL ceiling, free-space warnings/private modes; retention/encryption/redaction remain future work.
- [Corruption blocks later valid records] -> preserve evidence and stop rather than silently skip; operator tooling beyond report is deferred.
- [Aya build path increases CI/package complexity] -> isolate program, keep portable workspace independent, add separate compile and privileged profiles.
- [One session per recording rereads WAL after ETL crash] -> deterministic and simple; incremental state is added only after measured need.
- [Loopback-only replay is narrower than general authorized targets] -> deliberate P0 safety retained; broad networking needs separate threat model/change.
- [Graph report was stale at analysis time] -> decisions above were verified against current source/OpenSpec at HEAD `eb9b50e`, not inferred solely from graph.

## Migration Plan

1. Update each mutable v1 model in place and atomically rewrite repository fixtures, sessions, manifests, metadata, checkpoints, reports, and checksums.
2. Add isolated Linux eBPF build path and portable unsupported adapter before wiring CLI.
3. Introduce recovery/ETL/session deterministic IDs behind new commands; compare rerun output before changing docs/capability claims.
4. Add replay outcome field to machine-report v1 and update every caller/golden in same repository change.
5. Run rootless workspace/OpenSpec gates on every platform lane and privileged Linux acceptance separately.

Rollback reverts code and repository artifacts together. No deployed-data downgrade, old-format reader, or dual write path is promised before compatibility freeze.

## Open Questions

No user decision blocks proposal, but capture-specific implementation is gated. Gate A MUST confirm exact Aya APIs/helper availability, socket identity propagation, raw lifecycle state evidence (including whether directional half-close can be derived), direction/tuple normalization, GSO/GRO/nonlinear-skb handling, stable per-CPU counter/map identity and 100 ms/final-sample behavior, and cgroup subtree/process-enumeration behavior. Failure requires updating/revalidating this change before freezing Capture Event v1/eBPF ABI/support matrix. Generic WAL, recovery, metadata, JSON, replay-result, and filesystem primitives MAY continue independently but MUST NOT be cited as live-capture proof.
