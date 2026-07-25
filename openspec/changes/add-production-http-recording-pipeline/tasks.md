## 0. Privileged kernel feasibility gate

- [ ] 0.1 `[ebpf feasibility harness]` On supported Linux 6.1 host, prove Aya program types/helpers can propagate same socket cookie/generation across connect4/connect6, sock-ops lifecycle, and cgroup-skb ingress/egress for one real client flow; retain evidence for request, response, close, tuple, direction, TCP sequence, and loss counter; no persisted format work may start before pass.
- [ ] 0.2 `[ebpf feasibility harness]` Prove IPv4/IPv6 normalization, selected host cgroup node-plus-descendants scope, host PID resolution, cgroup namespace ambiguity rejection, and cleanup after detach; document exact kernel/arch/capabilities; depends on 0.1.
- [ ] 0.3 `[ebpf feasibility harness]` Exercise GSO/GRO and nonlinear skb payload visibility; confirm at-most-four 16 KiB continuation strategy reconstructs ordinary aggregates or amend OpenSpec/support matrix and re-run strict validation before task 1; depends on 0.1.

## 1. Freeze P1 contracts and compatibility

- [ ] 1.1 `[chronicle-common, chronicle-capture, chronicle-wal, chronicle-canonical]` After gate 0 passes, add version constants and typed IDs/enums for recording, capture event v2, WAL v2, manifest v2, canonical v3, completeness, lifecycle, provenance, and counters; preserve older artifacts and reject unknown newer versions; add serde/version unit tests; depends on 0; changes persisted/public types.
- [ ] 1.2 `[chronicle-capture, chronicle-session]` Replace tuple/fd-only production identity with boot ID + socket cookie + generation contract while retaining fixture v1 normalization; verify descriptor/tuple/process-restart separation tests; depends on 1.1; changes capture/session public types.
- [ ] 1.3 `[chronicle-replay, chronicle-application, chronicle-cli]` Define stable replay outcome, operation execution state, report JSON v2, and exit mapping before implementation; verify compile-time exhaustive matching and serde snapshots; depends on 1.1; changes replay/application/CLI API.
- [ ] 1.4 `[workspace, docs]` Record P1 fixed bounds (16 KiB/event, four continuations/64 KiB observed payload, 8 MiB ring, 4096 queue/batch, 256 MiB segment, existing 8 MiB connection, 64 KiB/128 HTTP head, 8 MiB body, 10,000 operations) as owning-crate constants; assert design/spec values match tests; depends on 1.1.
- [ ] 1.5 `[chronicle-capture, chronicle-canonical, chronicle-storage]` Implement explicit version-discriminated V1/V2/V3 wire DTO decode -> domain conversion -> invariant validation for capture envelopes, canonical sessions, and manifests instead of serde-defaulting one current struct; fixed compatibility/unknown-version tests; depends on 1.1.

## 2. Isolate Linux eBPF build and dependencies

- [ ] 2.1 `[Cargo.toml, crates/chronicle-capture-ebpf, chronicle-application, chronicle-cli, ebpf/]` Add target-specific optional Aya loader dependencies, explicit `linux-ebpf` feature chain CLI -> application -> adapter, only Tokio `signal` feature, and separate no-std eBPF package explicitly excluded from root workspace; prove macOS workspace needs no eBPF target/bpf-linker and feature-disabled Linux reports unavailable; depends on 1.1; changes dependency graph.
- [ ] 2.2 `[ebpf/, build tooling]` Add explicit reproducible little-/big-endian eBPF object build command and loader embedding contract without general task framework/runtime path ambiguity; verify clean Linux release binary includes matching object; depends on 2.1.
- [ ] 2.3 `[CI]` Add unprivileged Linux eBPF compile lane separate from ordinary locked workspace gates and label privileged runtime acceptance not checked; verify normal CI remains rootless; depends on 2.2.
- [ ] 2.4 `[chronicle-capture-ebpf]` Replace constructible runtime `cfg!` scaffold with cfg-gated supported loader and typed non-Linux adapter; verify unsupported-platform/load-error tests and no direct unit-struct bypass; depends on 2.1.

## 3. Extend capture event contracts and fixture sources

- [ ] 3.1 `[chronicle-capture]` Implement capture event v2 fields for kernel time, PID/TGID/TID, cgroup/container hint, boot/socket/generation identity, tuple, lifecycle, TCP sequence, original payload length, truncation, flags, loss snapshot, and provenance-safe encoding; round-trip binary/non-UTF-8 tests; depends on 1.1-1.2.
- [ ] 3.2 `[chronicle-capture]` Keep v1 fixture decoder/source and normalize events into reconstruction input without claiming TCP lifecycle/position; add fixture-v1 compatibility and newer-version rejection tests; depends on 3.1.
- [ ] 3.3 `[chronicle-capture]` Add deterministic capture-event fixture source capable of lifecycle/TCP sequence/loss/truncation cases for non-eBPF tests; validate every event sequence/reference and ensure no HTTP/storage coupling; depends on 3.1.
- [ ] 3.4 `[chronicle-capture]` Add capture counters snapshot/merge types with received bytes/events, dropped, truncated, and loss marker semantics; verify monotonic/saturating behavior; depends on 3.1.

## 4. Implement WAL v2 framing

- [ ] 4.1 `[chronicle-wal]` Add fixed checksummed segment header codec with format/header version, recording ID, ordinal, first sequence, and bounded metadata; test round trip, truncated header, wrong magic, checksum mismatch, recording mismatch, and unsupported version; depends on 1.1.
- [ ] 4.2 `[chronicle-wal]` Extend record codec for declared capture schema/version while preserving WAL v1 reader; test record framing round trip, CRC mismatch, kind/version rejection, max-length rejection before allocation, and binary payload; depends on 4.1.
- [ ] 4.3 `[chronicle-wal]` Add numeric segment discovery/order validation and deterministic path naming without scanning unrelated files; test missing/duplicate/overlap/out-of-order segment cases; depends on 4.1.
- [ ] 4.4 `[chronicle-wal]` Add version-dispatch reader yielding segment/offset/sequence provenance and stable corruption errors; test P0 v1 read compatibility and P1 v2 multi-segment read; depends on 4.1-4.3.

## 5. Build durable WAL writer and bounded ingest

- [ ] 5.1 `[chronicle-wal]` Refactor writer to create header in private temp file, file-sync, atomic rename, directory-sync, then append; rotate before configurable 16 MiB-4 GiB limit and use 0700/0600 modes; test rotation, crash before/after header publish, ignored temp, exact-boundary/oversized record, permissions; depends on 4.
- [ ] 5.2 `[chronicle-wal]` Implement write-full + flush + `fdatasync` per record and durable acknowledgement only after sync; test graceful close and injected write/flush/sync failure preserving earlier durable prefix; depends on 5.1.
- [ ] 5.3 `[chronicle-application, chronicle-wal]` Add bounded 4096-event ingest worker/channel that streams rather than buffering full source; queue saturation blocks and shutdown drains/joins; test bounded capacity, ordering, no userspace drop, and queued-versus-durable counters; depends on 3.4, 5.2.
- [ ] 5.4 `[chronicle-application]` Map permission/disk-full/write/sync failure to stop-recording policy, failed metadata, observable loss/failure counters, and safe error; add fault-injection integration tests; depends on 5.3.
- [ ] 5.5 `[chronicle-application]` Retain existing finite `write_capture_to_wal` behavior only for fixture compatibility or route fixture through streaming writer without changing P0 CLI outcome; regression-test checked-in fixture vertical slice; depends on 5.3.

## 6. Add WAL recovery and reopen

- [ ] 6.1 `[chronicle-wal]` Implement advisory recording lock with existing `rustix`, exclusive writer/recovery ownership, and crash-release semantics; test concurrent writer/recovery rejection and lock release; depends on 5.1.
- [ ] 6.2 `[chronicle-wal, chronicle-application]` Implement deterministic recovery where verified final segment headers/records authoritatively derive segment list/next sequence/durable counters and recording metadata supplies immutable identity/selector plus last-known lifecycle; reconcile stale derived metadata/status, reject immutable mismatch, test metadata lag and repeated recovery; depends on 4.4, 6.1.
- [ ] 6.3 `[chronicle-wal]` Repair only incomplete final record in final segment by truncating/syncing to verified boundary; test truncated fixed header, truncated payload/checksum, empty tail, and second recovery no-op; depends on 6.2.
- [ ] 6.4 `[chronicle-wal]` Refuse/memoize truncated segment header, corrupted middle record, checksum mismatch, sequence gap, overlap, and recording mismatch without skipping/mutating evidence; test each stable segment/offset error and recovery report data; depends on 6.2.
- [ ] 6.5 `[chronicle-wal]` Implement reopen-after-recovery append at next sequence with rotate-if-needed and no overwrite; test restart-and-append after clean and repaired WAL; depends on 6.3-6.4.
- [ ] 6.6 `[chronicle-application]` Persist atomic body-safe `recovery-report.json` and surface repaired/corrupt/version counters; verify report serialization and no payload bytes; depends on 6.2-6.5.

## 7. Implement Linux eBPF capture program

- [ ] 7.1 `[ebpf/]` Add cgroup connect4/connect6 programs that filter effective cgroup, assign/store client socket attribution and generation, and emit bounded open evidence; verify map/key layout against userspace ABI and kernel compile; depends on 2.2, 3.1.
- [ ] 7.2 `[ebpf/]` Add sock-ops lifecycle handling for active established, half-close, and close with boot/socket/generation identity and tuple consistency; verify deterministic synthetic test vectors/ABI decoding; depends on 7.1.
- [ ] 7.3 `[ebpf/]` Add cgroup skb ingress/egress IPv4/IPv6 TCP parsing that emits only application payload as up to four ordered 16 KiB continuations (64 KiB ceiling), direction, TCP sequence/timestamps/truncation; exclude raw headers/HTTP parsing; verify options, empty/malformed, GSO/GRO/nonlinear skb, continuation and truncation vectors; depends on 7.1 and gate 0.
- [ ] 7.4 `[ebpf/]` Add 8 MiB ring buffer and per-CPU received/bytes/drop/truncation counters; increment reservation failures and expose loss snapshot path; validate map capacity and counter ABI; depends on 7.3.
- [ ] 7.5 `[chronicle-capture-ebpf]` Load/relocate eBPF object with kernel BTF, configure selected cgroup/maps, attach required links atomically, and detach all on partial/startup failure; test loader with mocked feature/attach failures and leak-free cleanup; depends on 2.4, 7.1-7.4.
- [ ] 7.6 `[chronicle-capture-ebpf]` Poll ring buffer into `CaptureSource`/ingest events, decode ABI defensively, snapshot loss counters, drain on shutdown, and never fall back silently; add malformed-event, loss-marker, shutdown-drain, and unsupported-feature tests; depends on 7.5, 5.3.

## 8. Add recording metadata and lifecycle service

- [ ] 8.1 `[chronicle-application]` Define versioned recording metadata/status/capability/selector/shutdown/counter models and atomic temp+sync+rename persistence with private modes; test transitions, serialization, interrupted stale-state recovery, and unknown version; depends on 1.1, 6.6.
- [ ] 8.2 `[chronicle-application, chronicle-capture-ebpf]` Implement exactly-one host PID/cgroup selector, resolve/verify host unified cgroup inode/ID, attach selected node plus descendants, reject namespace ambiguity, and record effective scope; test missing/dead PID, ambiguous container PID, invalid path, conflicting selectors; depends on 7.5, 8.1.
- [ ] 8.3 `[chronicle-application]` Implement preflight for Linux/kernel/architecture/BTF/object/hooks/capabilities/WAL destination/free-space warning before attach; test unsupported platform, insufficient privileges, unwritable WAL, and load failure with no successful recording; depends on 7.5, 8.1.
- [ ] 8.4 `[chronicle-application]` Orchestrate metadata -> eBPF source -> bounded ingest -> WAL writer and stop on source/WAL failure; verify received/queued/durable/drop/truncation counters and failed durable prefix; depends on 5.3-5.4, 7.6, 8.3.
- [ ] 8.5 `[chronicle-application]` Add SIGINT/SIGTERM adapter and one-shot graceful shutdown ordering (detach, drain, close queue, sync WAL, finalize metadata); subprocess-test both signals and second-signal best-effort behavior; depends on 8.4.
- [ ] 8.6 `[chronicle-application]` Add stable body-safe human/JSON recording summaries plus checked-in JSON schema/example covering version, ID/status/reason/segments/counter names/order/errors; test no captured payload/header values; depends on 8.4-8.5.

## 9. Implement generation-safe TCP reconstruction

- [ ] 9.1 `[chronicle-session]` Extend assembler to group by production connection generation while retaining fixture `ConnectionKey` compatibility and documented 1024 connection/65,536 chunk limits; test fd reuse, tuple reuse, process restart, and separate generations; depends on 1.2, 3.2.
- [ ] 9.2 `[chronicle-session]` Implement independent directional wrap-aware TCP sequence ordering with WAL provenance tie-breaker; test deterministic order, direction handling, sequence wrap, and event arrival permutations; depends on 9.1.
- [ ] 9.3 `[chronicle-session]` Deduplicate exact retransmitted ranges and detect conflicting overlap without choosing bytes silently; test exact duplicate, partial identical overlap, conflicting overlap, and provenance retention; depends on 9.2.
- [ ] 9.4 `[chronicle-session]` Detect missing intervals/lifecycle, clean/half/unknown termination, truncation, and recording-level unattributed loss; any kernel drop taints every overlapping production connection non-replayable; test missing open/payload, close, global loss, truncation; depends on 9.2-9.3.
- [ ] 9.5 `[chronicle-session]` Enforce existing 8 MiB per-connection and five-minute event-time idle retention with explicit incomplete eviction; add memory-bound/idle/load-oriented tests proving no unbounded growth; depends on 9.4.
- [ ] 9.6 `[chronicle-protocol, chronicle-etl]` Expose protocol-neutral input with per-direction TCP byte order, cross-direction completion order (kernel monotonic timestamp + WAL sequence), trusted termination/source-integrity, and WAL ranges; compile dependency audit plus identical-bytes/different-interleaving test; depends on 9.2-9.5.

## 10. Extend HTTP/1.1 streaming decode

- [ ] 10.1 `[chronicle-protocol-builtins]` Adapt HTTP decoder input to reconstructed/provenance slices while preserving existing `httparse` 64 KiB/128-header bounds and P0 fixed-length tests; depends on 9.6.
- [ ] 10.2 `[chronicle-protocol-builtins]` Implement Content-Length request/response and no-body response across arbitrary head/body/capture boundaries with exact byte/provenance consumption; test split request, split response, header boundary, body boundary, zero length, HEAD/204/304; depends on 10.1.
- [ ] 10.3 `[chronicle-protocol-builtins]` Implement bounded chunked response decoding (extensions syntax only, exact chunks, zero terminator, bounded response trailers) and classify any chunked request/request trailer unsupported; test split response chunks, binary body/trailers/malformed/missing terminator and malicious chunked request; depends on 10.1.
- [ ] 10.4 `[chronicle-protocol-builtins]` Implement valid response-to-close framing only when lifecycle close is trustworthy; test complete close-delimited response, missing close, loss/truncation before close; depends on 10.1, 9.4.
- [ ] 10.5 `[chronicle-protocol-builtins]` Preserve multiple sequential keep-alive messages and independent directions without treating one capture event as message; test coalesced/split messages and more than one exchange on connection; depends on 10.2-10.4.
- [ ] 10.6 `[chronicle-protocol-builtins]` Enforce 8 MiB body limit and return typed truncated/unsupported evidence without panic/allocation growth; test declared/chunked/close body overflow; depends on 10.2-10.4.

## 11. Pair HTTP messages and canonicalize completeness/provenance

- [ ] 11.1 `[chronicle-protocol-builtins]` Keep method queue, HEAD/close framing, message completion chronology, FIFO final-response pairing, and pipelining detection inside connection decoder; canonicalizer maps completed exchanges; test unmatched peers and complete prior exchanges; depends on 10.5.
- [ ] 11.2 `[chronicle-protocol-builtins]` Detect second request completed before first final response as pipelining and mark affected operations unsupported/non-replayable; test identical directional bytes with sequential versus pipelined WAL/timestamp interleavings plus complete prior exchange; depends on 11.1.
- [ ] 11.3 `[chronicle-protocol-builtins]` Represent malformed request/status line, incomplete head/body, malformed chunks, 1xx, CONNECT, Upgrade/WebSocket, ambiguous framing, and non-HTTP/1.1 with stable completeness/warnings; test each and no panic; depends on 10, 11.1.
- [ ] 11.4 `[chronicle-canonical, chronicle-protocol-builtins]` Emit canonical v3 typed completeness and recording/connection/WAL provenance for complete and degraded operations; test binary body refs, source ranges, operation order, and non-replayability; depends on 1.1, 11.1-11.3.
- [ ] 11.5 `[chronicle-canonical]` Add v1/v2 compatibility mapping into v3 completeness/provenance defaults and reject v4+; round-trip checked-in P0 sessions/fixtures; depends on 11.4.
- [ ] 11.6 `[chronicle-protocol-builtins]` Update HTTP capability registration integrity only after detector/decoder/canonicalizer/replay/verifier remain wired; assert no other protocol becomes Available; depends on 11.4.

## 12. Build recording-scoped ETL discovery and deterministic IDs

- [ ] 12.1 `[chronicle-etl]` Add recording-directory entry point that acquires read/recovery ownership, validates stopped metadata, discovers all segments, and rejects active/corrupt/unsupported inputs before publication; integration-test completed/interrupted/failed/active recordings; depends on 6, 8.1.
- [ ] 12.2 `[chronicle-etl]` Read recovered WAL in 4096-record batches, decode v1/v2 events, feed reconstruction, and accumulate bounded safe issues/counters/provenance; test partially processed segment, unsupported event version, malformed event, and batch determinism; depends on 4.4, 9, 12.1.
- [ ] 12.3 `[chronicle-etl]` Derive deterministic session/connection/operation UUIDs from SHA-256 domain separators, recording ID, pipeline version, provenance, and message order; add fixed vectors and collision-domain tests; depends on 11.4, 12.2.
- [ ] 12.4 `[chronicle-etl]` Build exactly one canonical session per stopped recording with source status/time/counters and honest incomplete/malformed/unmatched/unsupported aggregation; test complete and incomplete recording outputs; depends on 11, 12.2-12.3.
- [ ] 12.5 `[chronicle-etl]` Stop before output on WAL corruption/version/sequence/checkpoint contradiction while publishing bounded opaque evidence for trustworthy unsupported protocol/malformed traffic; test both fatal and publishable issue classes; depends on 12.1-12.4.

## 13. Add ETL checkpointing and atomic idempotency

- [ ] 13.1 `[chronicle-etl]` Define atomic pre-publication `output-binding.json` (recording + canonical output root) and post-publication checkpoint v1 with binding, high-water, exact verified-byte `wal_snapshot_sha256`, recovery digest/scope, pipeline/canonical/session, manifest checksum/status/counters; test atomic create/conflict, round trip, digest vector, torn temp, safe JSON; depends on 12.
- [ ] 13.2 `[chronicle-application, chronicle-storage]` Persist output binding before deterministic `FilesystemSessionStore` publication and checkpoint only after publication; inject failures at binding/payload/session/manifest/sync/rename/checkpoint and assert binding/final/checkpoint order; depends on 13.1.
- [ ] 13.3 `[chronicle-etl, chronicle-storage]` Recompute WAL snapshot before matching publication/checkpoint -> already processed or repair; reject checksum/provenance, same-length valid WAL mutation, changed root with/without checkpoint, including crash after root-A publication before checkpoint then root-B rerun; prove no overwrite/duplicate; depends on 13.2.
- [ ] 13.4 `[chronicle-etl]` Implement restart after ETL interruption by deterministic reread from WAL start, not intermediate state; kill/fault after batches and prove identical single publication; depends on 13.2-13.3.
- [ ] 13.5 `[chronicle-application]` Add safe ETL human/JSON renderer plus checked-in JSON schema/examples covering version, output identity, counters/checkpoint/completeness/order/error; test serialization failure/no payload values; depends on 13.1-13.4.

## 14. Persist provenance and extend inspect

- [ ] 14.1 `[chronicle-storage]` Add manifest v2 recording/status/time/WAL high-water/snapshot digest/pipeline/session/provenance/completeness/integrity fields, explicitly no checkpoint-file checksum; retain manifest v1 reader/unknown-version rejection; P0/P1 and one-way checksum tests; depends on 11.5, 13.1.
- [ ] 14.2 `[chronicle-storage]` Keep staging + sync + manifest-last + atomic no-replace publication and 0700/0600 permissions for v3/provenance data; rerun concurrent/failure/private-permission tests; depends on 14.1.
- [ ] 14.3 `[chronicle-storage, chronicle-application]` Extend inspect to verify manifest/session checksums and payload existence/size, expose recording/WAL provenance availability, schema/integrity/completeness/replay readiness, and never read bodies for summary; tests with WAL present/removed and corrupt artifacts; depends on 14.1.
- [ ] 14.4 `[chronicle-application]` Update human inspect output with operation order, time range, completeness/malformed/unmatched counts, provenance/integrity/warnings; snapshot no secret/body/header values; depends on 14.3.
- [ ] 14.5 `[chronicle-application, chronicle-cli]` Bump inspect JSON schema with stable deterministic structure and typed serialization failure/no partial stdout; CLI contract tests and schema docs fixture; depends on 14.3.

## 15. Complete replay partial-execution model

- [ ] 15.1 `[chronicle-replay]` Extend existing `ReplayExecution` with `ReplayOutcome` and explicit operation state while retaining one result per planned operation; no duplicate executor type; unit-test exhaustive outcome/result accounting; depends on 1.3.
- [ ] 15.2 `[chronicle-replay]` Convert expected connect/execute transport failures into failed result + stopped-transport outcome and populate all later unattempted results; test first and middle failure with prior results preserved; depends on 15.1.
- [ ] 15.3 `[chronicle-replay, chronicle-application]` Preflight target/effect/session and registry capabilities before connection; implement precedence dry-run -> invalid-session/capability -> policy -> runtime; missing persisted capability yields invalid outcome, registry construction remains error; all zero-network outcomes include every result; depends on 15.1.
- [ ] 15.4 `[chronicle-replay]` Stop on non-passing verification with stopped-verification outcome and explicit later unattempted results; test Passed then Failed/Inconclusive/Unsupported; depends on 15.1.
- [ ] 15.5 `[chronicle-application]` Migrate replay callers from result-vector assumptions to outcome/result model, retain full report after partial execution, and reserve `ReplayError` for invariants/configuration; application tests; depends on 15.2-15.4.
- [ ] 15.6 `[chronicle-cli]` Human replay prints plan before network then report; JSON validates plan before network and emits one final object containing plan/outcome/results; map completed/dry-run/policy/invalid/transport/verification exits and test no partial JSON; depends on 15.5.

## 16. Preserve controlled replay safety and bounds

- [ ] 16.1 `[chronicle-replay]` Re-run/extend target parser tests for explicit loopback IP literal, exact allow-host, no DNS/userinfo/path/query/fragment/HTTPS/wildcard/original fallback; depends on 15.1; no policy broadening.
- [ ] 16.2 `[chronicle-replay, chronicle-protocol-builtins]` Ensure captured Host/authorization/cookie/forwarding/hop-by-hop/Connection-token/Transfer-Encoding fields cannot affect target/policy and target Host/Content-Length are rebuilt; test malicious recorded headers; depends on 11.4.
- [ ] 16.3 `[chronicle-protocol-builtins]` Preserve one-request-per-connection, five-second timeout, no retry/proxy/TLS/redirect behavior and bound observed response body to 8 MiB; test redirect does not follow, refusal, timeout, incomplete response, oversized response; depends on 15.2.
- [ ] 16.4 `[chronicle-replay]` Reject incomplete/truncated/malformed/unmatched/unsupported/pipelined operations before any network, while complete siblings remain represented; test mixed session preflight blocks all; depends on 11.4, 15.3.
- [ ] 16.5 `[chronicle-application/tests]` Add target spy proving authorized override receives requests and original recorded destination never receives/connects, including redirect Location escape attempt; depends on 16.1-16.4.

## 17. Produce structured verification reports

- [ ] 17.1 `[chronicle-protocol-builtins]` Preserve exact HTTP status, ordered selected headers with fixed ignore set, and body SHA-256/size comparison; add pass/status/header/body/missing expected/incomplete expected tests with value-safe details; depends on 11.4.
- [ ] 17.2 `[chronicle-application]` Build replay report JSON v2 with outcome, aggregate attempted/completed/failed/unattempted and verification counts, and every operation result; verify aggregation equals entries after partial failure; depends on 15.5, 17.1.
- [ ] 17.3 `[chronicle-application]` Update human report with same metadata-safe categories and no response/request body or arbitrary header values; snapshot all outcome classes; depends on 17.2.
- [ ] 17.4 `[chronicle-application, chronicle-cli]` Handle final replay JSON serialization failure as exit 3 without pre-execution partial JSON, network retry, or loss of in-memory execution result; fault-injection/subprocess test; depends on 17.2.
- [ ] 17.5 `[chronicle-cli]` Document and test exit 0/4/5/6 aggregation for dry-run/pass, policy/invalid, transport, and verification outcomes; depends on 15.6, 17.2.

## 18. Implement doctor probes

- [ ] 18.1 `[chronicle-application]` Define required/optional diagnostic probes and aggregate precedence: required unsupported, required not-checked, warnings/optional failures, supported; stable codes/remediation/order and exact exit 4/4/0/0 tests; depends on 1.1.
- [ ] 18.2 `[chronicle-capture-ebpf]` Implement non-destructive OS/arch/kernel/cgroup-v2/BTF/hook/helper/object/capability/attach probes with cleanup and safe distinction between unsupported and not-checkable; mocked tests plus privileged smoke; depends on 7.5, 18.1.
- [ ] 18.3 `[chronicle-wal, chronicle-storage]` Implement private temp WAL path writability/space warning/lock/sync and atomic no-replace publication probes without overwriting user files; tests on supported and injected-unsupported filesystems; depends on 6.1, 14.2, 18.1.
- [ ] 18.4 `[chronicle-protocol, chronicle-replay]` Probe protocol capability registry and replay policy shape without network; report HTTP available, other actual statuses, loopback/no-redirect policy, and missing per-command target as informational; tests; depends on 11.6, 16.1, 18.1.
- [ ] 18.5 `[chronicle-application]` Compose independent probes, continue after failures, remove temporary artifacts/links, and emit body-safe human/JSON output; test no side effects and no environment credential values; depends on 18.2-18.4.

## 19. Wire application services and CLI

- [ ] 19.1 `[chronicle-application]` Replace generic `ChronicleApplication` record/ETL/inspect/replay/doctor scaffold branches with typed service entry points or remove unused facade without duplicating already-runnable functions; compile caller migration and scaffold regression removal; depends on 8, 13, 14, 15, 18.
- [ ] 19.2 `[chronicle-cli]` Add production record arguments `--source ebpf (--pid|--cgroup) --wal-dir [--segment-bytes]` while preserving fixture command; Clap parse/conflict/help tests; depends on 8.6.
- [ ] 19.3 `[chronicle-cli]` Make `etl --wal-dir --output` runnable with safe human/JSON summaries and exit 0/3; first/idempotent/corrupt CLI tests; depends on 13.5.
- [ ] 19.4 `[chronicle-cli]` Make `doctor` runnable with optional paths/full human/JSON probes; exit 4 for required unsupported/not-checked and 0 for supported/warnings including optional not-checked; subprocess tests; depends on 18.5.
- [ ] 19.5 `[chronicle-cli]` Preserve inspect/replay syntax and global format while wiring schema-v2 summaries/outcomes; dry-run/authorization/override/report/serialization tests; depends on 14.5, 15.6, 17.
- [ ] 19.6 `[chronicle-cli/tests]` Add production-record signal harness with fake capture: successful SIGINT -> interrupted/exit 0, successful SIGTERM -> interrupted/exit 143, flush/finalization failure -> failed/exit 3; assert summary/metadata; depends on 8.5, 19.2.

## 20. Standardize observability and safe output

- [ ] 20.1 `[chronicle-application, owning crates]` Define shared summary field names and structured tracing events for required capture/WAL/ETL/HTTP/replay counters without adding metrics server; compile/render tests; depends on 3.4, 6.6, 12.4, 17.2.
- [ ] 20.2 `[chronicle-application]` Ensure every fatal/partial boundary emits stable code, recording/session/checkpoint identity, and counters while excluding payload/header/environment values; snapshot redaction tests with seeded secrets; depends on 20.1.
- [ ] 20.3 `[chronicle-session, chronicle-etl, chronicle-replay]` Add load-oriented bounded tests for queue, reconstruction memory/idle eviction, ETL batch, HTTP body, replay response, and report operation limits; assert explicit state/error and bounded allocation proxies, not throughput claims; depends on 5.3, 9.5, 10.6, 12.2, 16.3.

## 21. Complete recovery and pipeline integration tests

- [ ] 21.1 `[chronicle-wal tests]` Consolidate required WAL matrix: framing, checksum, rotation, close, truncated final record/header, checksum/middle corruption, restart append, disk failure, bounded queue, drops, version rejection, deterministic recovery; ensure each uses production v2 path; depends on 4-6.
- [ ] 21.2 `[chronicle-session tests]` Consolidate required reconstruction matrix: split request/response/head/body, multiple requests, close, fd/tuple reuse, missing lifecycle/payload, truncation, direction, duplication, deterministic ordering; depends on 9.
- [ ] 21.3 `[chronicle-protocol-builtins tests]` Consolidate required HTTP matrix: Content-Length request/response, chunked, no-body, keep-alive, sequential exchanges, malformed lines, incomplete head/body, unmatched peers, upgrade, pipelining rejection; depends on 10-11.
- [ ] 21.4 `[chronicle-etl, chronicle-storage tests]` Consolidate first run, second idempotent run, interruption restart, partial segment, corrupt/version input, incomplete recording, deterministic IDs, atomic publication, checkpoint repair, and no duplicates; depends on 12-14.
- [ ] 21.5 `[chronicle-replay, chronicle-application, chronicle-cli tests]` Consolidate all-complete, first/middle transport failure, prior/failed/unattempted retention, mismatch, invalid operation, policy, original target, redirect, timeout, JSON, and exit behavior; depends on 15-19.
- [ ] 21.6 `[chronicle-application/tests]` Add rootless full fixture-event v2 -> WAL recovery -> reconstruction -> HTTP -> deterministic session -> inspect -> authorized replay -> verification test; delete source fixture/WAL only after publication boundary assertions; depends on 21.1-21.5.

## 22. Add real privileged Linux acceptance harness

- [ ] 22.1 `[tests/e2e or scripts/]` Build reproducible supported-Linux harness that creates dedicated cgroup, starts local HTTP upstream, launches production-like plaintext HTTP client workload, recorder, and separate replay target; no external Internet/database; depends on 7-8, 19.
- [ ] 22.2 `[privileged acceptance]` Generate multiple headers/request bodies/responses, persistent sequential connection use, Content-Length, and chunked response; assert kernel eBPF events (not fixture source) and durable WAL counters/bytes; depends on 22.1.
- [ ] 22.3 `[privileged acceptance]` Abruptly kill recorder during traffic, append truncated tail fixture only after kill if needed for deterministic boundary, restart recovery, assert verified-tail repair and separately inject checksum corruption detection/no silent skip; depends on 22.2, 6.
- [ ] 22.4 `[privileged acceptance]` Run ETL twice and after injected interruption; assert deterministic one session, reconstructed chunks/messages/pairs, honest incomplete evidence, provenance, atomic publication, and no duplicates; depends on 22.3, 12-14.
- [ ] 22.5 `[privileged acceptance]` Run inspect human/JSON then replay only to explicit local target; simulate middle transport failure and assert completed/failed/unattempted/outcome/verification report; instrument original destination to prove no replay contact; depends on 22.4, 15-17.
- [ ] 22.6 `[CI/docs]` Publish separate opt-in privileged profile command/artifact retention and mark standard CI as fixture + eBPF compile only; document kernel/capability prerequisites and cleanup; depends on 22.1-22.5.

## 23. Documentation and final validation

- [ ] 23.1 `[README.md, docs/architecture.md]` Document P1 scope, exact cgroup/PID semantics, Linux 6.1+/x86_64/aarch64/cgroup-v2/BTF/capabilities, Aya build isolation, crate flow, limits, and explicit TLS/protocol/platform exclusions; depends on implementation.
- [ ] 23.2 `[docs/wal-format.md, new operations guide]` Document WAL v1/v2 layout, atomic segment-header publication/fsync order, permissions, sync-on-record, rotation/counters/failures/recovery, output-bound checkpoint/digests/idempotency, record/ETL JSON schemas/error examples, and runnable record-through-ETL example; depends on 4-13.
- [ ] 23.3 `[docs/canonical-model.md, docs/replay-safety.md]` Document canonical v3 completeness/provenance/migration, stored sensitive-data warning/non-guarantees, inspect schemas, loopback authorization, no redirects/original target, replay outcome/report JSON v2 and exits; depends on 14-17.
- [ ] 23.4 `[docs, CLI help]` Document `doctor` statuses/remediation, troubleshooting, local end-to-end demo, CI fixture/eBPF-compile versus privileged runtime acceptance, and known limitations; depends on 18-22.
- [ ] 23.5 `[workspace]` Run `cargo fmt --all --check`, `cargo check --workspace --all-targets --locked`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, and `cargo test --workspace --all-features --locked`; fix only P1-caused failures; depends on all code/tests/docs.
- [ ] 23.6 `[Linux eBPF profile]` Run isolated eBPF compile and privileged acceptance on supported kernel, record kernel/arch/capabilities/results, and do not claim runtime coverage when skipped; depends on 22, 23.5.
- [ ] 23.7 `[openspec/changes/add-production-http-recording-pipeline]` Run `openspec validate add-production-http-recording-pipeline --strict`, compare implementation/tasks/specs, and perform independent recovery/replay/security review before archive; depends on 23.5-23.6.
