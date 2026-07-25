## Context

Chronicle P0 is complete and archived. Current runnable flow is:

```text
FixtureCaptureSource
  -> CaptureEvent v1
  -> one fresh WAL v1 segment
  -> WalReader checkpoint
  -> SessionAssembler
  -> built-in HTTP detector/decoder/canonicalizer
  -> Canonical Session v2
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
- Persist userspace-received events through a bounded, segmented, recoverable WAL with a 4 MiB/10 ms group-commit window, explicit durable watermark, and honest crash-loss accounting.
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

Before this gate passes, implementation MUST NOT freeze production Capture Event v2 kernel-derived fields, connection identity assumptions, eBPF/userspace ABI, supported kernel/helper matrix, lifecycle guarantees, or GSO/GRO payload claims. Kernel-independent work MAY proceed in parallel: generic segmented-WAL framing/group commit/recovery scanner primitives, atomic metadata replacement, replay outcome/result modeling, bounded atomic JSON rendering, and filesystem no-replace publication. If feasibility fails, capture-specific implementation stops and this OpenSpec change is amended/revalidated; generic primitives do not become evidence that live capture works.

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
| B — Durable raw recording | A for capture; generic WAL may precede A | production/fake record -> bounded queue -> group-committed segmented WAL -> crash recovery | requires honest watermark/counters/status/bounds |
| C — Recovery and ETL | B | `chronicle etl` -> reconstructed HTTP -> deterministic session, same/cross-root reruns | requires scoped loss windows, deterministic IDs, fail-closed corruption |
| D — Canonical inspect | C | `chronicle inspect` -> integrity/completeness/provenance/partial replayability | requires safe complete operation accounting |
| E — Safe replay/verification | D | authorized mixed replay -> partial report, original target untouched | requires zero-network policy rejection and one result/operation |
| F — Diagnostics/hardening | A-E | doctor + rootless/eBPF compile/privileged 20-step acceptance + strict validation | required before archive |

Each gate's entry dependencies, exact implementation tasks/owners, commands, acceptance scenario, tests, artifacts, and stop/go condition live in `tasks.md`. Gate failure stops dependent gates but does not retroactively block kernel-independent primitives explicitly allowed by decision 0.

### 2. Select a dedicated cgroup v2, with PID as resolver only

P1 accepts exactly one of `--cgroup PATH` or `--pid PID`. Cgroup path resolves on host unified hierarchy under `/sys/fs/cgroup`; attachment covers selected node plus descendants. Root `/` and known host-wide shared roots (`/system.slice`, `/user.slice`, and `/machine.slice`) are rejected with no P1 override. Selection containing Chronicle recorder itself is also rejected. Effective host path, stable cgroup inode/ID, and node-plus-descendants scope are shown before attachment and persisted in metadata; path alone is not identity.

`--pid` resolves host-visible `/proc/<pid>/cgroup` during preflight, resolves again immediately before attachment, and resolves a third time immediately after links attach; all three canonical host paths and inode/IDs must match. Missing PID, namespace ambiguity, or movement between cgroups fails before capture and removes partial links. PID resolution never silently widens to a broader ancestor. Explicit `--cgroup` uses exact selected node/subtree. A dedicated cgroup is required; process names and container IDs are not P1 selectors.

Cgroup identity is stable across threads and matches cgroup eBPF attachment. PID-only filtering in packet hooks is unreliable because ingress may execute outside originating task context. Container hints may be recorded from cgroup metadata but never select scope or identify connection.

### 3. Use Aya and payload-only cgroup hooks

Aya fits existing Rust-first policy, avoids libbpf/clang/libelf runtime coupling, and was already documented as deferred until capture had real behavior. Dependencies remain isolated:

- Linux loader/runtime dependencies in `chronicle-capture-ebpf` behind `cfg(target_os = "linux")` and a `linux-ebpf` feature.
- `chronicle-application` and `chronicle-cli` propagate optional `linux-ebpf`; normal Linux builds with feature disabled expose typed unavailable result, while release recording builds enable it explicitly.
- A separate no-std eBPF package under `ebpf/`, explicitly excluded from root workspace and built for `bpfel-unknown-none`/`bpfeb-unknown-none`; loader embeds build artifact so installed CLI has no ambiguous runtime lookup.
- macOS workspace builds compile typed unsupported capture adapter and retain fixture/ETL/inspect/replay development without bpf-linker/eBPF target. Other non-Unix publication remains fail-closed.

P1 hooks are cgroup `connect4/connect6` for workload/socket attribution, cgroup sock-ops for active-establish/close lifecycle, and cgroup skb ingress/egress for IPv4/IPv6 TCP sequence plus application payload. Hot path filters selected cgroup, parses only required L3/L4 fields, and copies payload as at most four 16 KiB continuation events (64 KiB observed-payload ceiling) to handle ordinary GSO/GRO aggregates; larger/non-readable remainder is explicitly truncated. It updates counters and returns. It performs no HTTP parsing and does not retain raw packet headers.

Only client-initiated connections attributed at connect are canonical replay candidates. Unattributed or mid-connection observations remain inspectable incomplete evidence.

Supported runtime baseline is Linux 6.1+, cgroup v2, readable kernel BTF at `/sys/kernel/btf/vmlinux`, x86_64 or aarch64, and kernel support for required cgroup hooks/ring buffer/socket cookie helpers. Required effective privileges are `CAP_BPF` and `CAP_NET_ADMIN` plus any kernel-specific attach/perf capability reported by `doctor`; older capability models require root/`CAP_SYS_ADMIN` and are unsupported by policy unless run as root. Load/attach failure is explicit and creates no recording marked successful.

Ring buffer capacity is fixed at 8 MiB in P1. Reservation failure increments cumulative per-CPU dropped-event counters. Userspace periodically and at shutdown samples those counters and persists typed loss evidence containing boot-scoped monotonic clock ID/origin shared with Capture Event kernel timestamps, previous/current sample times, per-sample drop delta, and cumulative loss epoch/generation. Because counters cannot name lost connections or exact drop instants, ETL treats the interval between observations as a conservative loss window; it never claims precise per-connection attribution. Shutdown drains ring buffer, records a final sample, detaches links, closes ingest, group-syncs WAL, and then finalizes metadata. Missing hooks/features fail startup; there is no silent fallback to fixture capture.

### 4. Separate capture-domain evidence from WAL ordering

Capture Event v2 is capture-domain data only. It represents kernel monotonic timestamp and optional wall-clock correlation; PID/TGID/TID and stable cgroup identity; candidate boot/socket/generation identity plus tuple; raw lifecycle evidence (`established`, `state_changed`, `closed`, `reset`, or `unknown`); direction/TCP position/payload; observed length/truncation/flags; and typed loss snapshots. Exact kernel-derived fields remain feasibility-gated.

`WalRecordEnvelope` owns persistence identity and order: recording ID, WAL sequence, segment ordinal/first sequence, byte offset, record kind/schema version, and encoded capture event payload. Segment/offset become known while writing/recovery, not while capturing. Fixture and in-memory Capture Event v2 values therefore require no WAL sequence. Existing fixture v1 `sequence` remains readable as fixture-source ordering and is normalized without changing P0 bytes; persisted ordering still comes from its WAL v1 record. File descriptor is provenance only. Synthetic connection identity may use recording ID plus first enclosing WAL sequence after persistence and is always marked incomplete.

### 5. WAL v2 is segmented, append-only, bounded, and group-committed

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

Each v2 segment has a checksummed fixed header containing magic, format/header version, recording ID, segment ordinal, first sequence, and creation metadata. Each framed `WalRecordEnvelope` contains kind, flags, WAL sequence, payload length/schema version, and CRC32C over header fields plus payload. Readers retain explicit v1 support for P0 fixture artifacts and reject unknown newer versions before allocation.

Default segment size is 256 MiB, configurable from 16 MiB through 4 GiB. Total WAL segment storage defaults to and is hard-capped at 4 GiB; `--max-wal-bytes` may lower that bound but MUST be at least selected segment size. Accounting includes every byte under `segments/` (headers, frames, temporary segment bytes) and excludes metadata/lock/ETL reports; writer reserves/checks header+frame bytes before writing. Rotation happens before an append that would exceed segment limit. Reaching total WAL bound initiates graceful capture stop before another frame is accepted. One oversized record is rejected. Segment header temp-file sync/rename/directory-sync, metadata atomic replacement, locking, and private modes remain unchanged.

P1 exposes one durability mode: group commit. Writer appends complete frames and starts/maintains an unsynced batch. It flushes and `fdatasync`s when unsynced framed bytes reach 4 MiB, 10 ms elapses since last successful sync while unsynced data exists, segment rotation begins, explicit internal flush boundary is requested, or recording shuts down. Rotation/finalization cannot proceed until preceding data sync succeeds. After each successful data sync, writer atomically persists a checksummed durable watermark before acknowledging batch durable; watermark lag is recovered conservatively and never promotes later bytes. No per-record sync or user-facing durability mode exists.

Counters and metadata distinguish: capture received; accepted into bounded queue; written into WAL page cache but not yet durable; durable through last successful sync watermark; dropped before persistence (kernel/backend loss or rejected before queue); and ETL checkpointed. A record leaves queued state when fully written, but advances durable counts/watermark only after successful `fdatasync`. Written-not-durable counters are snapshots, not durability claims.

Crash may lose any complete frames after last durable watermark and may leave a partial final frame. Recovery MUST treat persisted checksummed durable watermark as sole record-durability authority; synced segment headers establish layout only. Bytes after watermark are reported/discarded as unsynced uncertainty without promotion. Reconciled counters report estimated records/bytes lost from written-not-durable window when known; recording/connection completeness is downgraded when that window may contain capture evidence.

Userspace channel is bounded to 4096 events. Producer blocks when full; kernel ring pressure appears through loss snapshots. WAL write/flush/sync/permission/disk-full errors stop recording, preserve durable prefix, record written-not-durable uncertainty, and finalize `failed`; no continue-on-error policy exists.

### 6. Recovery mutates only a verified partial final tail

Recovery locks recording, treats persisted checksummed durable watermark as sole authority for durable counters/next durable sequence, treats synced segment headers only as discoverable layout, and treats `recording.json` as last-known lifecycle/selector metadata. It reconciles crash-stale derived metadata but fails immutable recording ID/selector mismatch. A truncated final frame in final segment is truncated to last verified boundary, file-synced, and parent directory synced with recovery warning. A truncated published segment header, checksum mismatch, malformed middle record, sequence gap, segment overlap, recording-ID mismatch, or corruption before final tail is not skipped/repaired: evidence remains intact and ETL fails by default. Incomplete temporary segment files are reported and ignored because never published.

Low-level WAL reopen-for-append begins only after successful recovery and uses next expected sequence; it exists to satisfy deterministic restart/append durability contract, not to define recording resume. Production record lifecycle never reopens terminal recording without future explicit command. Metadata left `starting` or `recording` after process death is atomically finalized as `aborted` during recovery with `process_crash_recovered` or persisted `forced_termination` shutdown reason. Recovery records any lost/uncertain post-watermark range and never relabels crash recovery as graceful completion.

### 7. Recording status describes finalization; shutdown reason describes cause

`record` validates selector, platform, hooks, privileges, BTF, WAL destination, bounds, permissions, and free-space warning before attachment. Production CLI adds `--duration-seconds` (default 600, inclusive range 1..=3600) and `--max-wal-bytes` (default/hard maximum 4 GiB, lower values allowed down to selected segment size). First duration or total-WAL limit reached detaches/stops new capture immediately, then drains/group-syncs/finalizes within fixed five-second grace; timeout/failure becomes failed rather than hanging or claiming completion. No later events are silently ignored after stop begins.

Recording status transitions are:

```text
starting -> recording -> completed | failed
starting -> failed
starting/recording --recovery-after-crash--> aborted
```

Terminal status is separate from required `shutdown_reason`: `user_interrupt`, `termination_signal`, `source_completed`, `duration_limit`, `wal_size_limit`, `capture_failure`, `wal_failure`, `process_crash_recovered`, or `forced_termination`. Graceful SIGINT/SIGTERM, source EOF, and clean configured-limit stop become `completed` only after detach, drain, final loss sample, group sync, segment/metadata finalization succeed. Capture/WAL failure becomes `failed`. Recovered stale active metadata becomes `aborted`. `interrupted` is not a P1 status.

Metadata includes schema/build/kernel/host identity, selector path plus stable cgroup ID, capabilities, configured/effective bounds, start/end, WAL version/segments/durable watermark, counters, bounded capture errors, terminal status, and shutdown reason. Host identity excludes command line/environment and captured payload values. Successfully handled SIGINT and SIGTERM exit 0. Second signal may force immediate exit after best-effort reason persistence, leave a recoverable partial/unsynced tail, and become `aborted` on recovery.

One connection/message spanning a duration or WAL boundary retains durable prefix evidence and is incomplete unless trusted evidence proves completion before durable watermark. ETL consumes exactly recovered durable boundary. Production `record` never runs ETL implicitly; fixture mode remains for P0 compatibility.

### 8. Reconstruct TCP streams from connection generation and TCP positions

Reconstruction groups by boot ID + socket cookie + generation, validates tuple/cgroup consistency, and maintains independent client/server sequence spaces. It converts 32-bit TCP sequence numbers into wrap-aware relative offsets, sorts segments deterministically by relative offset then WAL provenance, removes exact retransmitted overlap, and flags conflicting overlap as malformed. Missing intervals, truncation, lifecycle gaps, eviction, or possible crash-window loss produce incomplete/truncated state; bytes on separate generations are never mixed. ETL derives conservative loss windows from successive persisted counter snapshots and compares them with each connection activity interval. Connections provably outside every loss window remain eligible for complete classification; intersecting connections degrade; unknown/insufficient timing degrades conservatively. Session completeness may be partial while unaffected operations remain complete.

Protocol input preserves byte order within each direction by TCP position and deterministic cross-direction observation order by kernel monotonic timestamp then enclosing WAL sequence. Raw lifecycle evidence stays distinct from derived userspace termination (`clean_close`, `half_close`, `reset`, `unknown_termination`). Half-close is derived only if Gate A proves directional state evidence; otherwise `state_changed` remains unknown. Provenance ranges record segment first sequence, byte offset, WAL sequence, direction, and payload subrange. Existing fixture v1 events lacking TCP positions keep fixture ordering and are marked fixture-derived.

### 9. Extend HTTP decoder narrowly

Existing `httparse` remains responsible for bounded start-line/header syntax. Chronicle owns connection decoder, stream buffers, request-method queue, lifecycle-aware finish, and framing. P1 supports request/status lines, ordered headers, exact Content-Length, chunked response transfer coding including bounded trailers as response metadata, no-body responses, valid response-to-close framing only on trusted derived clean-close evidence, keep-alive, and multiple sequential request/response pairs. Chunked requests are unsupported/non-replayable in P1 so trailer semantics cannot bypass request sanitization.

Connection decoder consumes messages in deterministic cross-direction completion order from timestamp/WAL provenance. Requests and final responses pair FIFO; HEAD response rules use queued method. HTTP pipelining is detected when second request completes before first final response; identical directional bytes with sequential versus pipelined observation interleavings therefore remain distinguishable. P1 preserves affected messages/provenance but marks pipelining unsupported/non-replayable. Informational 1xx, CONNECT, Upgrade/WebSocket, non-HTTP/1.1, ambiguous multiple framing, malformed chunks, unmatched messages, and residual bytes become typed malformed/incomplete/unmatched evidence. They are never converted to complete operations.

Canonical v3 adds typed completeness (`complete`, `incomplete`, `truncated`, `malformed`, `unmatched`, `unsupported`) and typed recording/connection/WAL provenance while retaining compatibility defaults for v1/v2 booleans/warnings. Reader accepts v1-v3 and rejects newer versions. P1 writer emits v3.

### 10. ETL restarts from durable input and publishes once

P1 intentionally uses one canonical session per stopped recording. Session ID is deterministic from SHA-256 over a domain separator, recording ID, and canonical pipeline version; UUID variant/version bits are normalized. Connection/operation IDs are derived similarly from stable source provenance and message order. This reuses existing SHA-256 dependency and avoids random IDs on rerun.

ETL sequence is:

```text
recover/discover WAL -> validate versions -> read batches -> reconstruct
-> detect/decode/pair -> canonicalize -> build one session
-> publish atomically -> write checkpoint atomically
```

P1 has no `output-binding.json`. One recording is not product-bound to one filesystem root. Deterministic session ID plus `wal_snapshot_sha256`, manifest provenance/checksum, and existing atomic no-replace publication are sufficient within each requested store. ETL may intentionally materialize unchanged recording into another output root; container/host path aliases and restored recordings therefore need no operator binding reset.

Post-publication checkpoint is ETL-owned advisory JSON containing version, recording ID, input high-water segment/offset/next sequence, SHA-256 of exact ordered verified segment bytes through high-water (`wal_snapshot_sha256`), recovery-report digest/scope, pipeline/canonical versions, deterministic session ID, latest published manifest checksum/output identity, status, and counters. It is written only after publication and never rejects a different requested output root. Manifest contains input high-water, pipeline version, WAL snapshot digest, and deterministic session ID, but never checkpoint-file checksum.

P1 does not persist intermediate reconstruction state: interruption before publication rereads WAL from beginning. For requested output root, existing deterministic session path is accepted only after manifest/session/provenance/checksum verification and reported `already_processed`; mismatch fails without overwrite. Missing checkpoint after publication is repaired from matching manifest. Another root publishes same deterministic session independently. WAL change after publication changes digest and is conflict, not silent mutation.

Unsupported protocol and malformed/incomplete traffic are published as bounded non-replayable evidence when WAL integrity remains trustworthy. WAL corruption/version failure, checkpoint contradiction, or publication failure stops before checkpoint advance. Staging cleanup is best-effort and final session remains absent until rename.

### 11. Extend filesystem inspection, not storage architecture

Existing `FilesystemSessionStore` remains sole P1 canonical store. Manifest version increments to include source recording ID, capture range, completeness counts, WAL provenance summary/checksum, canonical compatibility version, integrity, and replay readiness. Full bodies/headers remain persisted by design and can contain secrets. Default human/JSON inspect returns metadata, methods/targets/statuses, sizes, warning codes, and checksums—not arbitrary header values or bodies. `--show-body` or general secret discovery is not added.

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
| total WAL | 4 GiB default and hard max; configurable lower >= segment | stop before excess append; finalize `completed` + `wal_size_limit` |
| group commit | 4 MiB unsynced or 10 ms; rotation/flush/shutdown | sync batch; failure finalizes `failed` + `wal_failure` |
| eBPF ring | 8 MiB | increment cumulative drop counter/loss epoch |
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

Record cannot count HTTP operations because eBPF/WAL hot path intentionally does not parse HTTP; duration/WAL limits bound capture instead. ETL operation/connection overflow therefore fails explicitly rather than silently dropping evidence. Command summaries expose received, queued, written-not-durable, durable, dropped-before-persistence, ETL-checkpointed, loss-window, recovery, completeness, and replay result counters. P1 adds no metrics server.

## Failure and Recovery Semantics

| Boundary | Default behavior |
|---|---|
| Capture backend startup/feature/privilege failure | fail before status `recording`; no fallback |
| Ring-buffer loss/truncation | persist typed loss window/truncation; degrade intersecting or uncertain connections, not provably outside ones |
| Queue saturation | bounded block; kernel loss remains observable |
| WAL group-sync/permission/disk/write failure | stop capture, finalize failed, preserve durable prefix, report written-not-durable uncertainty |
| Duration/total-WAL limit | bounded graceful stop; completed with exact reason; spanning connection may be incomplete |
| Abrupt termination | OS releases lock; next recovery marks aborted, discards/reports post-watermark uncertainty, repairs only verified final tail |
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
- [Group commit can lose recent writes on crash] -> fixed 4 MiB/10 ms window, durable watermark, fault injection, honest aborted/loss metadata; no silent durability claim.
- [WAL/session disk growth and sensitive data] -> 10-minute default/60-minute max duration, 4 GiB WAL ceiling, free-space warnings/private modes; retention/encryption/redaction remain future work.
- [Corruption blocks later valid records] -> preserve evidence and stop rather than silently skip; operator tooling beyond report is deferred.
- [Aya build path increases CI/package complexity] -> isolate program, keep portable workspace independent, add separate compile and privileged profiles.
- [One session per recording rereads WAL after ETL crash] -> deterministic and simple; incremental state is added only after measured need.
- [Loopback-only replay is narrower than general authorized targets] -> deliberate P0 safety retained; broad networking needs separate threat model/change.
- [Graph report was stale at analysis time] -> decisions above were verified against current source/OpenSpec at HEAD `eb9b50e`, not inferred solely from graph.

## Migration Plan

1. After privileged feasibility gate, add explicit V1/V2/V3 wire DTO decoders that read discriminator first, convert into current domain model, and validate invariants for capture envelopes, WAL payloads, manifests, and canonical sessions; retain P0 fixtures/sessions in tests.
2. Add isolated Linux eBPF build path and portable unsupported adapter before wiring CLI.
3. Introduce recovery/ETL/session deterministic IDs behind new commands; compare rerun output before changing docs/capability claims.
4. Add replay outcome field with temporary caller migration in one change; bump JSON report version rather than changing v1 meaning silently.
5. Run rootless workspace/OpenSpec gates on every platform lane and privileged Linux acceptance separately.

Rollback disables/removes live `record` exposure while preserving WAL v2 readers and canonical v3 readers. Existing P0 fixture record/inspect/replay remains usable. Persisted newer artifacts are never rewritten down-level; older binaries reject unknown versions clearly.

## Open Questions

No user decision blocks proposal, but capture-specific implementation is gated. Gate A MUST confirm exact Aya APIs/helper availability, socket identity propagation, raw lifecycle state evidence (including whether directional half-close can be derived), direction/tuple normalization, GSO/GRO/nonlinear-skb handling, loss-snapshot timing, and cgroup subtree behavior. Failure requires updating/revalidating this change before freezing Capture Event v2/eBPF ABI/support matrix. Generic WAL, recovery, metadata, JSON, replay-result, and filesystem primitives MAY continue independently but MUST NOT be cited as live-capture proof.
