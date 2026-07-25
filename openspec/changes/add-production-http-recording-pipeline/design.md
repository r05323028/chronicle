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
- Persist every userspace-received event through a bounded, segmented, recoverable WAL with explicit durability and loss accounting.
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
- Arbitrary inbound server workload replay, HTTP pipelined replay, production-scale throughput claims, or synchronous ETL during record.
- General-purpose packet reassembly. P1 retains only metadata and bounded TCP application payload required for selected client-initiated flows.

## Decisions

### 0. Prove kernel hook feasibility before freezing persisted contracts

First implementation gate is a privileged Linux 6.1 feasibility harness, not production code. It MUST demonstrate one IPv4 and one IPv6 client exchange with stable socket cookie across connect/sock-ops/cgroup-skb, lifecycle close callback, normalized tuple/direction, TCP sequence, ring-loss counter, GSO/GRO/nonlinear-skb behavior, and selected-cgroup subtree semantics. If gate fails, implementation stops and this design/spec is amended before capture event v2, WAL v2 payload schema, or support matrix is frozen.

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

### 2. Select a dedicated cgroup v2, with PID as resolver only

P1 accepts exactly one of `--cgroup PATH` or `--pid PID`. Cgroup path is resolved on host unified hierarchy under `/sys/fs/cgroup`, and attachment covers selected node plus descendants. `--pid` resolves host-visible `/proc/<pid>/cgroup` once, verifies resulting cgroup inode/ID, and records both PID and resolved host path; ambiguous cgroup-namespace/container PID resolution is rejected with instruction to pass explicit host cgroup path. CLI reports that node plus descendants are effective scope. A dedicated cgroup is required for deterministic isolation and privileged acceptance harness. Process names and container IDs are not separate P1 selectors.

Cgroup identity is stable across threads, supports container workloads, and matches cgroup eBPF attachment. PID-only filtering in packet hooks is unreliable because ingress may execute outside originating task context. Container ID is recorded when discoverable from cgroup metadata but is not trusted as connection identity.

### 3. Use Aya and payload-only cgroup hooks

Aya fits existing Rust-first policy, avoids libbpf/clang/libelf runtime coupling, and was already documented as deferred until capture had real behavior. Dependencies remain isolated:

- Linux loader/runtime dependencies in `chronicle-capture-ebpf` behind `cfg(target_os = "linux")` and a `linux-ebpf` feature.
- `chronicle-application` and `chronicle-cli` propagate optional `linux-ebpf`; normal Linux builds with feature disabled expose typed unavailable result, while release recording builds enable it explicitly.
- A separate no-std eBPF package under `ebpf/`, explicitly excluded from root workspace and built for `bpfel-unknown-none`/`bpfeb-unknown-none`; loader embeds build artifact so installed CLI has no ambiguous runtime lookup.
- macOS workspace builds compile typed unsupported capture adapter and retain fixture/ETL/inspect/replay development without bpf-linker/eBPF target. Other non-Unix publication remains fail-closed.

P1 hooks are cgroup `connect4/connect6` for workload/socket attribution, cgroup sock-ops for active-establish/close lifecycle, and cgroup skb ingress/egress for IPv4/IPv6 TCP sequence plus application payload. Hot path filters selected cgroup, parses only required L3/L4 fields, and copies payload as at most four 16 KiB continuation events (64 KiB observed-payload ceiling) to handle ordinary GSO/GRO aggregates; larger/non-readable remainder is explicitly truncated. It updates counters and returns. It performs no HTTP parsing and does not retain raw packet headers.

Only client-initiated connections attributed at connect are canonical replay candidates. Unattributed or mid-connection observations remain inspectable incomplete evidence.

Supported runtime baseline is Linux 6.1+, cgroup v2, readable kernel BTF at `/sys/kernel/btf/vmlinux`, x86_64 or aarch64, and kernel support for required cgroup hooks/ring buffer/socket cookie helpers. Required effective privileges are `CAP_BPF` and `CAP_NET_ADMIN` plus any kernel-specific attach/perf capability reported by `doctor`; older capability models require root/`CAP_SYS_ADMIN` and are unsupported by policy unless run as root. Load/attach failure is explicit and creates no recording marked successful.

Ring buffer capacity is fixed at 8 MiB in P1. Reservation failure increments a per-CPU dropped-event map. Because lost reservation has no connection identity, any increment taints whole recording and every production connection overlapping recording as incomplete/non-replayable. Userspace snapshots and emits recording-level loss markers; shutdown drains ring buffer, detaches links, closes ingest, flushes WAL, and then finalizes metadata. Missing hooks/features fail startup; there is no silent fallback to fixture capture.

### 4. Introduce transport capture event v2 without deleting v1

`CaptureEvent` becomes a versioned envelope capable of representing:

- recording-local WAL sequence assigned in userspace;
- kernel monotonic timestamp and optional wall-clock correlation;
- PID/TGID, thread ID when available, cgroup ID/path hash, optional container hint;
- boot-scoped connection identity: host boot ID, socket cookie, monotonic connection generation, network namespace when available, and tuple;
- lifecycle kind (`opened`, `payload`, `half_closed`, `closed`, `loss_marker`);
- direction, TCP sequence position, payload bytes, observed payload length, truncation, capture flags, and cumulative loss snapshot.

Existing fixture event v1 remains readable and is normalized into the reconstruction input. File descriptor is provenance only and never part of identity. A connection missing lifecycle metadata receives deterministic synthetic generation from recording ID, socket cookie/tuple, and first WAL sequence, and is marked incomplete.

### 5. WAL v2 is segmented, append-only, and sync-on-record by default

Layout:

```text
<wal-dir>/
  recording.json
  record.lock
  segments/
    00000000000000000001.chwal
    00000000000001048233.chwal
  etl/
    output-binding.json
    checkpoint.json
    recovery-report.json
```

Each v2 segment has a checksummed fixed header containing magic, format/header version, recording ID, segment ordinal, first sequence, and creation metadata. Records retain fixed framing with kind, flags, sequence, payload length, schema version, and CRC32C over header fields plus payload. Readers retain explicit v1 support for P0 fixture artifacts and reject unknown newer versions before allocation.

Default segment size is 256 MiB, configurable from 16 MiB through 4 GiB. Rotation happens before an append that would exceed limit; one oversized record is rejected. New segment header is written to private temporary file, file-synced, atomically renamed to final segment name, and `segments/` directory-synced before any record append; incomplete temp files are reported/ignored by recovery. Initial recording metadata and every metadata replacement use same temp-file sync, rename, parent-directory sync order. `rustix` advisory locking prevents concurrent writers. New segments and metadata use `0700` directories/`0600` files on Unix.

P1 default durability policy is sync-on-record: write full frame, flush, `fdatasync`, then increment durable counters and acknowledge dequeue. This is intentionally slower but makes the durability distinction precise. No weaker fsync mode is exposed in P1. In-memory counters distinguish received, queued/not-yet-durable, durable, dropped before persistence, and ETL-checkpointed records.

Userspace channel is bounded to 4096 events. Producer blocks when full; kernel ring pressure then appears through eBPF dropped-event counters. Chronicle never silently drops from the userspace queue. WAL write/permission/disk-full errors stop recording, retain recoverable files, increment failure/loss counters, and finalize status `failed`; no continue-on-error policy is offered in P1.

### 6. Recovery mutates only a verified partial final tail

Recovery locks recording, treats verified final segment headers/records as authority for segment list, next sequence, and durable counters, and treats `recording.json` as last-known lifecycle/selector metadata. It reconciles crash-stale derived metadata but fails immutable recording ID/selector mismatch. A truncated final frame in final segment is truncated to last verified boundary, file-synced, and parent directory synced with recovery warning. A truncated published segment header, checksum mismatch, malformed middle record, sequence gap, segment overlap, recording-ID mismatch, or corruption before final tail is not skipped/repaired: evidence remains intact and ETL fails by default. Incomplete temporary segment files are reported and ignored because never published.

Low-level WAL reopen-for-append begins only after successful recovery and uses next expected sequence; it exists to satisfy deterministic restart/append durability contract, not to define recording resume. Production record lifecycle never reopens terminal recording without future explicit command. Metadata left `recording` after process death is atomically finalized as `interrupted` during recovery.

### 7. Recording lifecycle is an application service

`record` validates selector, platform, hooks, privileges, BTF, WAL destination, permissions, and free-space warning before attachment. It creates UUID recording ID and atomically persists metadata status transitions:

```text
starting -> recording -> completed | interrupted | failed
```

Metadata includes schema/build/kernel/host identity, selector and effective cgroup, capabilities, start/end, WAL version/segments, counters, bounded capture errors, and shutdown reason. Host identity excludes command line/environment and captured payload values. SIGINT produces `interrupted` with exit success only if shutdown/flush succeeds; SIGTERM produces `interrupted` and a distinct summary reason. Startup/load and persistence failures are non-success.

Signal handling lives in application/platform adapter code. Second termination signal may force exit after best-effort metadata update and is documented as possible partial tail. ETL is never run implicitly by production `record`; fixture mode remains available for portable tests and P0 compatibility.

### 8. Reconstruct TCP streams from connection generation and TCP positions

Reconstruction groups by boot ID + socket cookie + generation, validates tuple/cgroup consistency, and maintains independent client/server sequence spaces. It converts 32-bit TCP sequence numbers into wrap-aware relative offsets, sorts segments deterministically by relative offset then WAL provenance, removes exact retransmitted overlap, and flags conflicting overlap as malformed. Missing intervals, recording-level loss taint, truncation, lifecycle gaps, or eviction produce incomplete/truncated state; bytes on separate generations are never mixed.

Protocol input preserves two orders: byte order within each direction by TCP position, and deterministic cross-direction observation order by kernel monotonic timestamp then WAL sequence. It also carries trusted termination (`clean_close`, `half_close`, `unknown`) and source-integrity state. Provenance ranges record segment first sequence, byte offset, WAL record sequence, direction, and payload subrange. Existing fixture v1 events lacking TCP positions keep monotonic event ordering and are marked fixture-derived; production v2 never assumes one event equals one HTTP message.

### 9. Extend HTTP decoder narrowly

Existing `httparse` remains responsible for bounded start-line/header syntax. Chronicle owns connection decoder, stream buffers, request-method queue, lifecycle-aware finish, and framing. P1 supports request/status lines, ordered headers, exact Content-Length, chunked response transfer coding including bounded trailers as response metadata, no-body responses, valid response-to-close framing only on trusted close, keep-alive, and multiple sequential request/response pairs. Chunked requests are unsupported/non-replayable in P1 so trailer semantics cannot bypass request sanitization.

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

Before any publication, ETL atomically creates `output-binding.json` in recording directory with recording ID and canonicalized output-root identity; existing different binding is rejected even when checkpoint is absent. This closes publish-before-checkpoint crash window. Relocation requires copying recording plus bound output together or clearing binding/checkpoint under documented operator procedure.

Post-publication checkpoint is ETL-owned JSON containing version, recording ID, bound output identity, input high-water segment/offset/next sequence, SHA-256 of exact ordered verified segment bytes through high-water (`wal_snapshot_sha256`), recovery-report digest/scope, pipeline/canonical versions, deterministic session ID, published manifest checksum, status, and counters. Manifest contains input high-water, pipeline version, WAL snapshot digest, and deterministic session ID, but never checkpoint-file checksum, avoiding circular hashes.

P1 does not persist intermediate reconstruction state: interruption before publication safely rereads WAL from beginning. Checkpoint advances only after atomic session publication. Before `already_processed` or checkpoint repair, ETL recomputes WAL snapshot digest so same-length valid mutation is detected. Matching existing session reports `already_processed`; mismatch fails without overwrite. WAL change after publication is conflict, not silent mutation.

Unsupported protocol and malformed/incomplete traffic are published as bounded non-replayable evidence when WAL integrity remains trustworthy. WAL corruption/version failure, checkpoint contradiction, or publication failure stops before checkpoint advance. Staging cleanup is best-effort and final session remains absent until rename.

### 11. Extend filesystem inspection, not storage architecture

Existing `FilesystemSessionStore` remains sole P1 canonical store. Manifest version increments to include source recording ID, capture range, completeness counts, WAL provenance summary/checksum, canonical compatibility version, integrity, and replay readiness. Full bodies/headers remain persisted by design and can contain secrets. Default human/JSON inspect returns metadata, methods/targets/statuses, sizes, warning codes, and checksums—not arbitrary header values or bodies. `--show-body` or general secret discovery is not added.

Inspect verifies manifest/session checksums and payload existence/size without printing content; replay hydration retains full checksum verification. Publication stays staging + sync + atomic no-replace rename with restrictive permissions.

### 12. Add aggregate replay outcome to existing result model

`ReplayExecution` gains `outcome: ReplayOutcome`. Per-operation result gains explicit transport state (`completed`, `failed`, `not_attempted`) while retaining operation ID, transport category, and optional verification. Outcomes cover completed, dry-run, stopped-policy, stopped-invalid-session, stopped-transport, and stopped-verification. Every planned operation has exactly one result. Verification mismatch means transport state completed plus verification Failed; transport failure means state failed; later entries are not attempted. Outcome precedence is `dry_run`, then `stopped_invalid_session`, then `stopped_policy`, then runtime transport/verification, then completed. Expected operational stops return `ReplayExecution`; invariant/program/configuration failures remain `ReplayError`.

Planner/application preflight target policy, canonical validity, and protocol registry/capabilities before any connection. Missing protocol registration/capability is stopped-invalid-session for persisted session data; internal registry construction failure remains configuration error. Any preflight denial sends zero requests. Runtime transport or non-passing verification stops later operations. Current loopback-only literal-IP target grammar, exact `--allow-host`, explicit effect gates, `--execute`, stripped captured credentials, one-request-per-connection, five-second timeout, and no redirects remain unchanged. Recorded endpoints and resolved/captured headers never influence authorization.

Verification remains exact status + body size/SHA-256 + ordered end-to-end headers except current fixed ignore set. No heuristic or user-defined comparator is added. Missing/incomplete expected responses are inconclusive and non-executable. Human mode shows plan before network then final report. JSON mode constructs/validates plan before network but emits one atomic final JSON object containing plan, outcome/counts, and every result; this avoids partial invalid JSON. Exit 0 means dry-run or all completed with Passed/Skipped, 4 policy/invalid-session rejection, 5 transport stop, and 6 completed verification mismatch/unsupported/inconclusive. JSON serialization failure is exit 3 after retaining in-memory execution result; it does not trigger retry.

### 13. Doctor reports typed probe results

`doctor` marks probes required or optional and reports `supported`, `supported_with_warnings`, `unsupported`, or `not_checked` with stable code/remediation. Aggregate precedence is: any required unsupported -> unsupported; else any required not-checked -> not-checked; else any warning or optional unsupported/not-checked -> supported-with-warnings; else supported. Required unsupported or not-checked exits 4; supported/warnings exits 0. Probes cover OS/architecture/kernel, cgroup v2, BTF, hooks/helpers, capabilities, eBPF object/backend, WAL path/free space, atomic rename/sync, protocol registry, and replay policy. Temporary private files are removed and values/command lines are never inspected.

### 14. Bounds and observable counters are part of correctness

| Boundary | P1 bound/default | Limit behavior |
|---|---:|---|
| eBPF ring | 8 MiB | increment kernel drop counter |
| capture payload/event | 16 KiB; up to 4 continuations/observed skb | truncate above 64 KiB/unreadable remainder |
| userspace ingest queue | 4096 events | block producer; expose ring losses |
| WAL record | existing 16 MiB max | reject/stop recording |
| WAL segment | 256 MiB, configurable 16 MiB-4 GiB | rotate before append |
| active connections | existing 1024/session | mark excess skipped/incomplete |
| reconstructed bytes/connection | existing 8 MiB | evict/mark truncated |
| chunks/connection | existing 65,536 | mark truncated |
| idle connection retention | 5 minutes event time | finalize incomplete |
| HTTP head | existing 64 KiB/128 fields | malformed/unsupported |
| HTTP buffered body | 8 MiB/operation | truncated/non-replayable |
| ETL read batch | 4096 records | continue next batch |
| replay observed body | 8 MiB/operation | typed transport/framing failure |
| operations/report | 10,000/session | remaining operations unsupported |

Command summaries and structured logs expose capture events/bytes received, persisted events/bytes, queued-not-durable, capture drops/truncations, WAL failures/recovery/corruption, ETL processed/skipped, reconstructed connections, decoded messages and completeness classes, replay attempted/completed/failed/unattempted, and aggregate verification. P1 adds no metrics server.

## Failure and Recovery Semantics

| Boundary | Default behavior |
|---|---|
| Capture backend startup/feature/privilege failure | fail before status `recording`; no fallback |
| Ring-buffer loss/truncation | persist counter/marker; unattributed drop taints whole recording and overlapping production connections non-replayable |
| Queue saturation | bounded block; kernel loss remains observable |
| WAL permission/disk/write/sync failure | stop capture, finalize failed, preserve durable prefix |
| Abrupt termination | OS releases lock; next recovery marks interrupted and repairs only verified final tail |
| WAL corruption/version mismatch | stop ETL; preserve bytes; write safe recovery report |
| ETL crash | no checkpoint advance; rerun deterministic from start |
| Malformed/incomplete/unsupported traffic | publish honest non-replayable evidence if WAL integrity is valid |
| Session publication failure | final path absent; no checkpoint advance |
| Replay policy rejection | zero network, all operations represented not-attempted |
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
- [Cgroup hooks still miss traffic or lose ring events] -> explicit kernel counters/loss markers, incomplete canonical states, no completeness claim after loss.
- [TCP overlap/wrap/lifecycle ambiguity] -> generation identity, wrap-aware ordering, exact dedupe, conflict warnings, and non-replayable output.
- [Sync-on-record throughput is low] -> correctness-first P1 default; benchmark counters document ceiling before any weaker policy is designed.
- [WAL/session disk growth and sensitive data] -> rotation/bounds/free-space warnings/private modes; retention/encryption/redaction remain explicit future work.
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

No user decision blocks proposal, but implementation is gated. Privileged feasibility decision 0 MUST confirm exact Aya APIs, helper availability, cookie/lifecycle propagation, direction/tuple normalization, GSO/GRO handling, and cgroup subtree scope before persisted contracts are frozen. Failure requires updating this OpenSpec change and revalidation; implementation MUST NOT guess or silently substitute another capture semantic.
