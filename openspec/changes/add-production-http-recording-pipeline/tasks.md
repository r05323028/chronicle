# Implementation Gates

Gates are dependency ordered and independently reviewable. Capture-specific contracts depend on Gate A. Kernel-independent primitives explicitly marked **parallel-safe before A** MAY proceed while feasibility runs. No gate may claim real eBPF coverage from fixture tests.

## Gate A — Kernel capture feasibility

**Entry dependencies:** archived P0 vertical slice passes; supported Linux 6.1+ x86_64/aarch64 host/VM with cgroup v2, BTF, and required privilege is available.

**Runnable commands to establish:**

```bash
cargo build --manifest-path ebpf-feasibility/Cargo.toml --target bpfel-unknown-none --release
cargo test -p chronicle-capture-ebpf --test privileged_feasibility -- --ignored --nocapture
```

**Acceptance scenario:** dedicated cgroup client performs real IPv4 and IPv6 plaintext HTTP/1.1 exchanges; harness observes request/response payload, stable candidate socket identity, tuple/direction/TCP positions, raw lifecycle state evidence, cumulative loss snapshots, and clean detach.

**Artifacts produced:** checked-in harness, captured ABI probe output, kernel/arch/helper/capability report, GSO/GRO/nonlinear-skb evidence, cgroup subtree evidence, and explicit supported/unsupported matrix proposal.

**Stop/go:** GO only when kernel-derived identity, lifecycle, payload visibility, loss-snapshot timing, and subtree assumptions are proven. STOP and revise/revalidate OpenSpec if any assumption fails. Capture Event v2 kernel fields, connection identity, eBPF/userspace ABI, helper matrix, lifecycle guarantees, and GSO/GRO claims MUST NOT freeze before GO. Generic WAL/recovery/metadata/replay/JSON/filesystem work below may proceed independently.

- [ ] A0 `[ebpf-feasibility/, chronicle-capture-ebpf tests]` Create disposable feasibility-only no-std eBPF package and privileged loader test command above without production feature chain or persisted ABI; verify clean build/run on supported host. This harness may be replaced after Gate A and SHALL NOT freeze production contract.
- [ ] A1 `[ebpf feasibility harness]` Prove candidate Aya connect4/connect6, sock-ops, and cgroup-skb hooks correlate one real client flow across request/response; retain socket cookie/generation, tuple, direction, TCP sequence, and raw `established/state_changed/closed/reset/unknown` evidence; do not require directional half-close; privileged test; depends on A0.
- [ ] A2 `[ebpf feasibility harness]` Prove IPv4/IPv6 normalization, exact selected cgroup node-plus-descendants scope, host PID resolution, stable cgroup inode/ID, namespace ambiguity rejection, and leak-free detach; test root and `/system.slice`/`/user.slice`/`/machine.slice` rejection plus PID movement race; depends on A1.
- [ ] A3 `[ebpf feasibility harness]` Exercise GSO/GRO and nonlinear skb payload visibility and determine whether at-most-four 16 KiB continuations reconstruct ordinary aggregates; amend limits/support matrix rather than guess; depends on A1.
- [ ] A4 `[ebpf feasibility harness]` Force ring reservation loss and prove cumulative per-CPU snapshots can provide previous/current monotonic sample times, drop delta, and loss epoch without claiming connection attribution; depends on A1.
- [ ] A5 `[chronicle-capture-ebpf, docs test artifact]` Record exact kernel, architecture, Aya APIs, helpers, privileges, lifecycle evidence, payload limits, and unsupported cases in machine-readable feasibility report; compare report with spec constants; depends on A1-A4.

**Gate A incremental test:** run commands above and retain one report showing real kernel-derived request/response/lifecycle/loss evidence. Fixture-only output is gate failure.

## Gate B — Durable raw recording

**Entry dependencies:** B1.1-B1.4, B2.2, B3.1-B3.4, B4.1-B4.4, and B5.1-B5.5 are kernel-independent and parallel-safe before Gate A. B2.1/B2.3-B2.4, B4.5, B6, and B7 require Gate A GO or its frozen capture contract. P0 fixture/WAL artifacts remain compatibility fixtures.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-wal
cargo test -p chronicle-application --test recording_lifecycle
chronicle --format json record --source ebpf --cgroup "$CGROUP" --wal-dir "$WAL" --duration-seconds 600 --max-wal-bytes 4294967296
```

**Acceptance scenario:** selected workload streams Capture Event v2 into bounded queue and segmented WAL. Group commit occurs at 4 MiB or 10 ms and at rotation/explicit flush/shutdown. SIGINT/SIGTERM and duration/WAL limits finalize `completed`; capture/WAL failures finalize `failed`; crash leaves recoverable durable prefix and stale metadata for `aborted` recovery.

**Artifacts produced:** Capture Event v2 and `WalRecordEnvelope` contracts, WAL v2 segments, `recording.json`, lock, safe record summary schema/example, recovery report, and rootless fault fixtures.

**Stop/go:** GO when durable watermark and counters exactly distinguish received, queued, written-not-durable, durable, dropped-before-persistence, and ETL-checkpointed; all group-commit fault tests recover only promised durable prefix; recording is bounded. STOP on silent loss, per-record-sync dependency, broad cgroup expansion, or ambiguous status/reason.

### B1. Freeze kernel-independent compatibility and output contracts

- [ ] B1.1 `[chronicle-common, chronicle-wal, chronicle-canonical]` **Parallel-safe before A.** Add version constants and explicit V1/V2/V3 wire-dispatch scaffolding for WAL envelopes, manifests, canonical data, metadata, counters, and unknown-version rejection; preserve P0 WAL v1/canonical v1-v2/manifest v1 fixtures; serde/compatibility tests.
- [ ] B1.2 `[chronicle-replay, chronicle-application, chronicle-cli]` **Parallel-safe before A.** Define `ReplayOutcome`, operation execution state, replayability classification, and report JSON version on existing `ReplayExecution`; compile-time exhaustive match/serde snapshots; no duplicate executor abstraction.
- [ ] B1.3 `[chronicle-application, chronicle-cli]` **Parallel-safe before A.** Add one bounded atomic JSON renderer: serialize complete result before stdout, checked `write_all`/flush, typed serialization versus CLI I/O errors, and no replay retry; shared record/ETL/inspect/replay/doctor fault tests.
- [ ] B1.4 `[chronicle-storage]` **Parallel-safe before A.** Preserve staging, sync, manifest-last, atomic no-replace publication and private modes; expose reusable existing-manifest verification primitive; concurrent/failure tests.

### B2. Define capture and WAL ownership

- [ ] B2.1 `[chronicle-capture]` After A, define Capture Event v2 capture-domain fields only: kernel time, process/stable-cgroup identity, proven connection identity, raw lifecycle evidence, direction/TCP position/payload, truncation, and loss snapshots; no WAL sequence/segment/offset; binary round-trip tests; depends on A5, B1.1.
- [ ] B2.2 `[chronicle-wal]` Define `WalRecordEnvelope` owning recording ID, WAL sequence, segment/offset provenance, schema kind/version, and capture payload; prove in-memory/fixture v2 events need no pre-persistence sequence; depends on B1.1.
- [ ] B2.3 `[chronicle-capture]` Retain fixture v1 decoder/source and its fixture ordering while normalizing to v2 reconstruction input; P0 bytes/outcomes and newer-version rejection tests; depends on B2.1.
- [ ] B2.4 `[chronicle-capture]` Add deterministic v2 fixture source for raw lifecycle, TCP position, truncation, and loss windows; no HTTP/storage coupling; reference/order validation tests; depends on B2.1.

### B3. Implement WAL v2 framing, segmentation, and recovery primitives

- [ ] B3.1 `[chronicle-wal]` **Parallel-safe before A.** Add checksummed segment header codec with recording ID/ordinal/first sequence and bounded metadata; round-trip, wrong magic/version/checksum/identity/truncated-header tests; depends on B1.1.
- [ ] B3.2 `[chronicle-wal]` **Parallel-safe before A.** Add envelope codec and version-dispatch reader yielding segment/offset/WAL-sequence provenance while retaining v1 reader; max-length-before-allocation, CRC, binary payload, unknown kind/version tests; depends on B2.2, B3.1.
- [ ] B3.3 `[chronicle-wal]` **Parallel-safe before A.** Add deterministic numeric segment discovery and temp-file exclusion; missing/duplicate/overlap/order tests; depends on B3.1.
- [ ] B3.4 `[chronicle-wal]` **Parallel-safe before A.** Create segment header under private temp name, file-sync, rename, directory-sync, then append; default 256 MiB/configurable 16 MiB-4 GiB rotation; exact boundary/oversized/crash/permission tests; depends on B3.1-B3.3.

### B4. Implement one bounded group-commit policy

- [ ] B4.1 `[chronicle-wal]` **Parallel-safe before A.** Implement complete-frame writes and group sync at 4 MiB unsynced, 10 ms elapsed with unsynced data, segment rotation, explicit internal flush, or shutdown; no durability CLI mode and no per-record sync default; deterministic fake-clock tests; depends on B3.4.
- [ ] B4.2 `[chronicle-wal, chronicle-application]` **Parallel-safe before A.** Persist/checksum durable watermark after successful data sync and track received/queued/written-not-durable/durable/dropped/checkpointed record+byte counters; watermark-lag behavior MUST conservatively lose, never promote; round-trip/reconciliation tests; depends on B4.1.
- [ ] B4.3 `[chronicle-wal tests]` Inject crash/failure immediately before/after byte threshold, time threshold, rotation sync, explicit flush, shutdown sync, and watermark persistence; assert exact recovered durable prefix, uncertain/lost window, and earlier-prefix readability; depends on B4.1-B4.2.
- [ ] B4.4 `[chronicle-wal]` Enforce total WAL default/hard max 4 GiB over every byte under `segments/` including headers/frames/temp files; exclude metadata/lock/ETL reports; reserve/check header+frame before write; configurable lower >= segment size; boundary/temp/invalid-config tests; depends on B3.4.
- [ ] B4.5 `[chronicle-application, chronicle-wal]` Add bounded 4096-event streaming ingest. Saturation blocks without userspace discard; written leaves queue but remains non-durable until group sync; ordering/capacity/shutdown-drain tests; depends on B2.4, B4.1-B4.2.

### B5. Recovery and recording lifecycle

- [ ] B5.1 `[chronicle-wal]` **Parallel-safe before A.** Implement advisory recording lock, deterministic scan, safe reopen, and authoritative durable watermark handling; concurrent writer/recovery and crash-release tests; depends on B3.1-B3.4 and B4.1-B4.4 only.
- [ ] B5.2 `[chronicle-wal]` Repair only incomplete final record in final segment to verified durable boundary; sync repair; reject truncated segment header, middle/CRC corruption, sequence gap/overlap, identity mismatch without mutation; second-run determinism tests; depends on B5.1.
- [ ] B5.3 `[chronicle-wal, chronicle-application]` Reconcile lagging metadata/counters; discard/report post-watermark complete frames as uncertain rather than durable; stale `starting/recording` -> `aborted` with crash/forced reason; immutable selector mismatch fails; depends on B5.1-B5.2.
- [ ] B5.4 `[chronicle-application]` Persist body-safe atomic `recovery-report.json` with repaired-tail, post-watermark lost/uncertain, corruption, version, sequence, durable record/byte counters; shared JSON renderer tests; depends on B1.3, B5.2-B5.3.
- [ ] B5.5 `[chronicle-wal]` Reopen only after successful recovery at next durable WAL sequence, never overwrite, rotate if required; clean/repaired restart-append tests; depends on B5.2-B5.3.

### B6. Build isolated eBPF adapter

- [ ] B6.1 `[Cargo.toml, chronicle-capture-ebpf, chronicle-application, chronicle-cli, ebpf/]` Add target-specific optional Aya loader, `linux-ebpf` feature chain, Tokio `signal` only, excluded no-std eBPF package, embedded endian-matched object; macOS/rootless builds need no bpf-linker; depends on A5.
- [ ] B6.2 `[CI]` Add unprivileged eBPF compile lane separate from rootless workspace and label privileged runtime not checked; clean build tests; depends on B6.1.
- [ ] B6.3 `[ebpf/]` Implement proven connect/lifecycle/payload programs and 8 MiB ring/per-CPU counters using Gate A ABI; lifecycle emits raw evidence only; no HTTP parsing/raw packet persistence; compile/vector tests; depends on A5, B2.1.
- [ ] B6.4 `[chronicle-capture-ebpf]` Load/configure/attach links atomically, decode ABI defensively, sample typed loss windows, poll into capture source, drain/detach on shutdown, and never silently fall back; mocked verifier/partial-attach/malformed-event/loss/drain tests; depends on B6.1, B6.3, B4.5.

### B7. Harden cgroup selection and recording metadata

- [ ] B7.1 `[chronicle-application, chronicle-capture-ebpf]` Implement exactly-one PID/cgroup selector with canonical host path + inode/ID, exact node-plus-descendants scope, root/shared-root/recorder-cgroup rejection, namespace ambiguity rejection, and no container-ID selector; unit/integration tests; depends on A2, B6.4.
- [ ] B7.2 `[chronicle-application]` Resolve PID during preflight, immediately before attach, and immediately after links attach; require same path/ID across all three and fail/detach on movement/death; race subprocess test; depends on B7.1.
- [ ] B7.3 `[chronicle-application]` Define metadata status `starting/recording/completed/failed/aborted` separate from required shutdown reasons; atomic temp+sync+rename/private modes, unknown-version tests; depends on B1.1, B5.3.
- [ ] B7.4 `[chronicle-application]` Preflight Linux/kernel/arch/BTF/object/hooks/capabilities/WAL path/free space/segment+total/duration bounds before attach; unsupported/unwritable/invalid-bound tests with no successful metadata; depends on B7.1, B7.3.
- [ ] B7.5 `[chronicle-application]` Add duration default 600 seconds/max 3600 and total WAL default/hard max 4 GiB. First limit stops new capture immediately and allows fixed five-second detach/drain/final-sample/group-sync/finalization grace; success completed/exact reason, timeout/failure failed/durable prefix; fake-clock/exact-byte/hung-drain subprocess tests; depends on B4.4-B4.5, B7.3.
- [ ] B7.6 `[chronicle-application]` Orchestrate metadata -> eBPF source -> bounded ingest -> WAL. Source/WAL failure stops as `failed` with capture/wal reason and durable prefix; tests cover all counters; depends on B4.5, B6.4, B7.4-B7.5.
- [ ] B7.7 `[chronicle-application]` Handle SIGINT/SIGTERM as graceful `completed`/exit 0 after successful finalization; second signal may force exit and later recovery marks aborted; subprocess tests for both signals and failure paths; depends on B7.6.
- [ ] B7.8 `[chronicle-cli, chronicle-application]` Wire production record grammar including bounds and safe summary schema through shared atomic renderer; preserve P0 fixture command/outcome; Clap/conflict/JSON/no-secret tests; depends on B1.3, B7.6-B7.7.

**Gate B incremental test:** before HTTP ETL exists, run fake-source/rootless recording and privileged real capture, inspect WAL with recovery scanner, crash recorder, and prove verified durable prefix plus honest aborted/loss metadata.

## Gate C — Recovery and ETL

**Entry dependencies:** Gate B GO. Gate A evidence already frozen into capture ABI. P0 protocol/canonical compatibility fixtures available.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-session -p chronicle-etl -p chronicle-protocol-builtins
chronicle --format json etl --wal-dir "$WAL" --output "$ROOT"
```

**Acceptance scenario:** recover stopped bounded recording, scan only durable prefix, reconstruct connection generations and loss windows, decode supported HTTP, publish one deterministic session, rerun same root as already processed, and intentionally materialize same recording into another root without binding file.

**Artifacts produced:** deterministic canonical v3 session ID, manifest v2/session/payloads, advisory checkpoint, typed ETL/recovery summaries, loss-window provenance, and rootless deterministic fixtures.

**Stop/go:** GO when unchanged WAL always yields byte-identical IDs/content, only verified final tail is repaired, middle corruption fails closed, loss windows degrade only intersecting/uncertain connections, and no operation is silently omitted. STOP on whole-recording loss degradation, cross-root binding, nondeterminism, or unbounded reconstruction.

### C1. Generation-safe reconstruction and loss windows

- [ ] C1.1 `[chronicle-session]` Group production data by feasibility-proven boot/socket/generation identity while retaining fixture `ConnectionKey`; descriptor/tuple/process-restart separation and synthetic-missing-open tests; depends on B2.
- [ ] C1.2 `[chronicle-session]` Implement independent wrap-aware TCP sequence ordering with WAL envelope provenance tie-breaker; direction/wrap/arrival-permutation tests; depends on C1.1.
- [ ] C1.3 `[chronicle-session]` Deduplicate exact retransmitted ranges, detect conflicting overlap, and retain provenance without choosing bytes silently; exact/partial/conflict tests; depends on C1.2.
- [ ] C1.4 `[chronicle-session]` Convert persisted loss snapshots into deterministic windows `(clock_id/origin,previous_time,current_time,delta,epoch)` and compare only matching boot-scoped monotonic clocks with connection activity. Outside remains eligible; intersecting/unknown/mismatched clock degrades; deterministic boundary/adjacency/unknown tests; depends on C1.2.
- [ ] C1.5 `[chronicle-session]` Keep raw lifecycle evidence separate from derived `clean_close/half_close/reset/unknown_termination`; derive half-close only if Gate A proves directional evidence; missing/open/close/reset tests; depends on A5, C1.2.
- [ ] C1.6 `[chronicle-session]` Enforce 1024 active connections, 65,536 chunks/connection, 8 MiB/connection, five-minute event-time idle bound. Byte/chunk/idle limits finalize affected connection; active overflow fails typed; load/bound tests; depends on C1.3-C1.5.
- [ ] C1.7 `[chronicle-protocol, chronicle-etl]` Expose protocol-neutral directional bytes, cross-direction completion order, derived termination, source integrity/loss windows, and WAL ranges; dependency audit and chronology tests; depends on C1.2-C1.6.

### C2. Decode bounded HTTP/1.1 honestly

- [ ] C2.1 `[chronicle-protocol-builtins]` Adapt existing `httparse` decoder to reconstructed/provenance slices while preserving 64 KiB/128-header and P0 fixed-length behavior; depends on C1.7.
- [ ] C2.2 `[chronicle-protocol-builtins]` Decode Content-Length/no-body request/response across arbitrary boundaries with exact consumption/provenance; split/head/body/zero/HEAD/204/304 tests; depends on C2.1.
- [ ] C2.3 `[chronicle-protocol-builtins]` Decode bounded chunked responses/trailers; keep chunked requests/trailers unsupported; binary/split/malformed/missing-terminator tests; depends on C2.1.
- [ ] C2.4 `[chronicle-protocol-builtins]` Decode response-to-close only from trusted derived clean-close and intact source; raw unknown/state-changed/reset/loss cannot complete; tests; depends on C1.5, C2.1.
- [ ] C2.5 `[chronicle-protocol-builtins]` Preserve keep-alive sequential exchanges, FIFO pairing, HEAD method queue, deterministic cross-direction chronology, and pipelining detection; complete prior operations remain; depends on C2.2-C2.4.
- [ ] C2.6 `[chronicle-protocol-builtins]` Represent malformed/incomplete/truncated/unmatched/unsupported/1xx/CONNECT/Upgrade/ambiguous framing/non-HTTP with stable evidence and no panic; enforce 8 MiB body; matrix tests; depends on C2.2-C2.5.

### C3. Canonicalize and publish deterministic session

- [ ] C3.1 `[chronicle-canonical, chronicle-protocol-builtins]` Emit canonical v3 typed completeness, source status/reason, connection/loss-window/WAL-envelope provenance, replay attributes, and operation order; complete outside-window operations remain complete in partial session; binary/provenance tests; depends on C1.4, C2.
- [ ] C3.2 `[chronicle-canonical]` Read v1/v2 with deterministic compatibility defaults, retain `PayloadRef::Object`, reject v4+; checked-in P0 session tests; depends on C3.1.
- [ ] C3.3 `[chronicle-etl]` Acquire stopped recording ownership; process completed/failed/aborted durable prefix; reject active/corrupt/unsupported/contradictory input; integration tests; depends on B5, B7.3.
- [ ] C3.4 `[chronicle-etl]` Read in 4096-record batches, decode v1/v2 envelopes, feed reconstruction, and bound issues to existing 1024; malformed/version/batch determinism tests; depends on C1, C3.3.
- [ ] C3.5 `[chronicle-etl]` Derive deterministic session/connection/operation IDs from domain separators, recording ID, pipeline version, WAL snapshot/provenance, and message order; fixed vectors; depends on C3.1, C3.4.
- [ ] C3.6 `[chronicle-etl]` Build exactly one session per stopped recording. Fail before publication on operation 10,001 or active connection 1025; do not omit. Publish bounded opaque evidence only when durable WAL integrity is trustworthy; tests; depends on C2.6, C3.4-C3.5.
- [ ] C3.7 `[chronicle-etl, chronicle-storage]` Publish deterministic ID via atomic no-replace in requested root without introducing a recording-to-root binding; verify existing manifest/session/provenance/checksums; same root already-processed, mismatch fail, second root allowed; crash/fault/concurrency tests; depends on B1.4, C3.5-C3.6.
- [ ] C3.8 `[chronicle-etl]` Write advisory checkpoint only after publication with durable high-water, exact `wal_snapshot_sha256`, recovery digest, versions, session/manifest/latest-output identity, status/counters. Missing checkpoint repair and cross-root update tests; depends on C3.7.
- [ ] C3.9 `[chronicle-application, chronicle-cli]` Wire `etl --wal-dir --output` and safe human/JSON summary through shared renderer; first/same-root/other-root/crash/corrupt CLI tests; depends on B1.3, C3.3-C3.8.

**Gate C incremental test:** `chronicle etl` over crashed/recovered WAL publishes deterministic session, reruns unchanged in same root without duplicate, publishes into second root intentionally, and loss-window fixture proves outside operation complete while intersecting operation degraded.

## Gate D — Canonical inspect pipeline

**Entry dependencies:** Gate C GO and existing `FilesystemSessionStore` publication semantics.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-storage -p chronicle-application --test inspect
chronicle --format json inspect "$SESSION_ID" --root "$ROOT"
```

**Acceptance scenario:** inspect published session after original WAL is absent/relocated. Report verifies manifest/session/payload metadata integrity, embedded provenance, completeness/loss windows, and fully/partially/not replayable classification without body or arbitrary header values.

**Artifacts produced:** manifest v2 reader/writer, canonical v3 compatibility fixtures, safe inspect human/JSON schemas/examples, and integrity/replayability summaries.

**Stop/go:** GO when P0 manifest/session remain readable, newer versions fail explicitly, inspect is payload-value-safe, and each operation's completeness/replayability is visible. STOP on live-WAL path assumptions, hidden invalid operations, or partial JSON.

- [ ] D1 `[chronicle-storage]` Add manifest v2 recording status/reason, durable high-water/WAL digest, pipeline/session/provenance/loss/completeness/integrity/replayability fields; no checkpoint checksum/live WAL locator; v1 compatibility/v3 rejection/checksum tests; depends on C3.1-C3.2, C3.8.
- [ ] D2 `[chronicle-storage]` Preserve private staging/sync/manifest-last/no-replace publication for v3 and session-qualified SHA-256 payload refs; rerun permission/concurrency/failure tests; depends on D1.
- [ ] D3 `[chronicle-storage]` Inspect manifest/session checksum and payload existence/size without reading bodies; hydration verifies digest before replay. Separate size-vs-same-size-digest tests; depends on D1-D2.
- [ ] D4 `[chronicle-application]` Compute operation/session completeness and `fully_replayable/partially_replayable/not_replayable`; show executable/non-executable counts, loss-window blockers, status/reason, provenance/integrity/warnings; deterministic tests; depends on C3.1, D3.
- [ ] D5 `[chronicle-application]` Human inspect shows safe metadata/method/target/status/size only; seeded Authorization/Cookie/custom-secret/header/body redaction snapshots; depends on D4.
- [ ] D6 `[chronicle-application, chronicle-cli]` Render inspect JSON through shared atomic boundary with stable order/schema and serialization/stdout fault coverage centralized in B1.3; CLI not-found/corruption/P0/P1 tests; depends on B1.3, D4-D5.
- [ ] D7 `[chronicle-application tests]` Delete/relocate source fixture/WAL after publication and prove inspect succeeds solely from embedded provenance without claiming current WAL availability; depends on D6.

**Gate D incremental test:** inspect one mixed-completeness session and prove complete/degraded counts, partial replayability, provenance, integrity, and safe output in human/JSON.

## Gate E — Safe replay and verification

**Entry dependencies:** Gate D GO; existing loopback target parser, policy, HTTP adapter, verifier, and `ReplayExecution` retained.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-replay -p chronicle-application --test replay
chronicle --format json replay "$SESSION_ID" --root "$ROOT" --target http://127.0.0.1:"$PORT" --allow-host 127.0.0.1 --allow-read --allow-write --execute
```

**Acceptance scenario:** mixed session plans every operation. Complete supported operations execute only against authorized loopback target; degraded operations remain non-executable/not-attempted. Middle transport or verification failure preserves prior, current, skipped, and later results. Recorded destination receives no connection.

**Artifacts produced:** extended existing `ReplayExecution`, exact `ReplayOutcome` JSON, operation result/report schemas, mixed-session planner, partial verification report, and target-spy tests.

**Stop/go:** GO when result cardinality/order equals canonical operations for all outcomes and authorization rejection performs zero network. STOP on global rejection caused only by degraded siblings, omitted operation, recorded-target fallback, redirect, retry, or replay after render failure.

- [ ] E1 `[chronicle-replay]` Extend existing `ReplayExecution` with exact outcomes `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, `stopped_verification`; retain one ordered result/operation and no duplicate executor; depends on B1.2.
- [ ] E2 `[chronicle-replay, chronicle-application]` Planner classifies every operation executable/non-executable and session full/partial/not replayable. Incomplete/truncated/malformed/unmatched/unsupported/pipelined/ambiguous-loss are not attempted but do not block complete siblings; matrix tests; depends on D4, E1.
- [ ] E3 `[chronicle-replay, chronicle-application]` Preflight session integrity/capability, explicit loopback target, exact allow-host, execute, and effect gates before network. Authorization denial blocks all executable candidates while preserving non-executable reasons; adapter-connect spy tests; depends on E2.
- [ ] E4 `[chronicle-replay]` Execute candidates sequentially in deterministic plan order, skip non-executable entries, convert first/middle transport failure to failed result, and mark later executable entries stopped-not-attempted; full cardinality/accounting tests; depends on E2-E3.
- [ ] E5 `[chronicle-replay]` Stop on executed non-passing verification while retaining prior/invalid/later results; successful executable subset with invalid siblings -> `completed_with_skips`; tests; depends on E4.
- [ ] E6 `[chronicle-protocol-builtins]` Preserve explicit loopback IP literal, no DNS/proxy/TLS/redirect/retry, one request/connection, five-second timeout, 8 MiB observed body; refusal/timeout/incomplete/overflow/3xx tests; depends on E4.
- [ ] E7 `[chronicle-replay, chronicle-protocol-builtins]` Rebuild Host/Content-Length for target and strip captured Host/auth/cookie/forwarding/hop-by-hop/Connection-token/Transfer-Encoding from authorization influence; malicious-header tests; depends on C3.1, E3.
- [ ] E8 `[chronicle-protocol-builtins]` Preserve exact status, body size/SHA-256, and selected ordered-header verifier with value-safe details; pass/status/header/body/missing/incomplete tests; depends on C3.1.
- [ ] E9 `[chronicle-application]` Build report with plan, replayability, outcome, executable/non-executable/attempted/completed/failed/unattempted and verification counts; aggregate equals entries; depends on E1-E8.
- [ ] E10 `[chronicle-application, chronicle-cli]` Render human report and one final JSON object via shared renderer. Exit 0 dry-run/completed/completed-with-skips; 4 policy/no-executable; 5 transport; 6 executed verification failure; render failure never retries; subprocess tests; depends on B1.3, E9.
- [ ] E11 `[chronicle-application tests]` Instrument authorized target and original recorded destination, including redirect Location escape; assert only override receives requests; depends on E3, E6-E7.
- [ ] E12 `[chronicle-replay]` Reject imported session >10,000 operations before network without truncating report/input silently; bounded test; depends on E2.

**Gate E incremental test:** execute mixed session, force transport failure after success, and verify report contains completed operation, failed operation, all non-executable operations, later unattempted executable operations, aggregate outcome, and zero original-target traffic.

## Gate F — Diagnostics and hardening

**Entry dependencies:** Gates A-E GO. Rootless CI and separate privileged Linux environment available.

**Runnable commands to establish:**

```bash
chronicle --format json doctor --wal-dir "$WAL_PARENT" --output "$ROOT"
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo build --manifest-path ebpf/Cargo.toml --target bpfel-unknown-none --release
./scripts/p1-privileged-acceptance.sh
openspec validate add-production-http-recording-pipeline --strict
```

**Acceptance scenario:** privileged script SHALL run and retain this exact sequence:

1. Start supported Linux host/VM with cgroup v2/eBPF prerequisites.
2. Start dedicated cgroup with production-like client workload.
3. Start Chronicle with bounded duration/WAL size.
4. Generate multiple plaintext HTTP/1.1 exchanges.
5. Verify capture events enter WAL through group commit.
6. Verify received/queued/written-not-durable/durable/dropped/truncated metadata.
7. Abruptly terminate recorder.
8. Recover only verified final tail and mark stale recording aborted.
9. Run ETL and publish deterministic canonical session.
10. Prove loss windows degrade only potentially affected connections.
11. Inspect complete/degraded operations, partial replayability, provenance, integrity.
12. Start separate local replay target.
13. Replay only complete supported operations.
14. Verify degraded operations are non-executable/not-attempted.
15. Cause transport failure after one or more successes.
16. Verify completed/failed/non-executable/later-unattempted results and aggregate outcome.
17. Verify original recorded destination is never contacted.
18. Verify graceful SIGINT/SIGTERM produce completed status/exit 0.
19. Verify duration and WAL limits stop cleanly with exact reasons.
20. Run rootless tests, isolated eBPF compile, privileged acceptance, doctor, and strict OpenSpec validation.

**Artifacts produced:** runnable doctor, operations/security docs, JSON schemas/examples, rootless/eBPF-compile/privileged test profiles, retained acceptance reports, and validated OpenSpec tasks.

**Stop/go:** GO for archive only when every required probe/test passes or is honestly labeled not checked, privileged runtime evidence is retained, docs state bounded-window/non-goals, and independent recovery/replay/security review finds no blocker.

### F1. Operational doctor

- [ ] F1.1 `[chronicle-application]` Define required/optional probe statuses/aggregate precedence and stable code/remediation/order; exact exit 4/4/0/0 tests; depends on B1.1.
- [ ] F1.2 `[chronicle-capture-ebpf]` Probe OS/arch/kernel/cgroup-v2/BTF/hooks/helpers/object/capability/attach non-destructively; cleanup links; distinguish unsupported/not-checked; mocked + privileged smoke; depends on B6.
- [ ] F1.3 `[chronicle-application]` Reuse cgroup selector safety checks for root/broad scope and PID identity race diagnostics with path+ID-safe output; depends on B7.1-B7.2.
- [ ] F1.4 `[chronicle-wal, chronicle-storage]` Probe supplied WAL/output paths for private create/write/space/lock/sync/group-commit prerequisites/no-replace publication. Omitted path -> optional not-checked; no guessed default/overwrite; depends on B4-B5, D2.
- [ ] F1.5 `[chronicle-protocol, chronicle-replay]` Probe protocol capability and replay policy shape without network; missing per-command target informational; depends on E3, E6.
- [ ] F1.6 `[chronicle-application, chronicle-cli]` Compose independent probes, cleanup temp artifacts, wire `doctor [--wal-dir] [--output]`, and render through shared JSON boundary; no environment values; subprocess tests; depends on B1.3, F1.1-F1.5.

### F2. Consolidated integration and acceptance

- [ ] F2.1 `[chronicle-wal tests]` Consolidate framing, envelope ownership, group thresholds/timer/rotation/shutdown, watermark, total limit, final-tail repair, middle corruption, reopen, disk/sync failure, queue, version, deterministic recovery tests; depends on Gate B.
- [ ] F2.2 `[chronicle-session, chronicle-etl tests]` Consolidate generation/order/dedupe/gap/raw-vs-derived lifecycle/loss-window/bounds/deterministic ID/cross-root/checkpoint tests; depends on Gate C.
- [ ] F2.3 `[chronicle-protocol-builtins tests]` Consolidate Content-Length/chunked/no-body/close-derived/keep-alive/sequential/malformed/incomplete/unmatched/upgrade/pipelining/body-bound tests; depends on Gate C.
- [ ] F2.4 `[chronicle-storage, chronicle-application tests]` Consolidate P0/P1 compatibility, atomic publication, inspect integrity/safe output/partial replayability, WAL-absent behavior, and shared JSON failure tests; depends on Gate D.
- [ ] F2.5 `[chronicle-replay, chronicle-application, chronicle-cli tests]` Consolidate full/mixed/not replayable, target/effect denial, first/middle transport, verification stop, all-result accounting, original-target/redirect/timeout/JSON/exit tests; depends on Gate E.
- [ ] F2.6 `[chronicle-application tests]` Rootless fixture v2 -> WAL group commit/recovery -> reconstruction/loss windows -> HTTP -> deterministic session -> inspect -> authorized mixed replay -> verification; label as fixture, not eBPF; depends on F2.1-F2.5.
- [ ] F2.7 `[scripts/p1-privileged-acceptance.sh, tests/e2e]` Add runnable privileged command listed above. Harness creates dedicated cgroup, local upstream/client/recorder/separate replay target and executes retained 20-step sequence; several Content-Length/chunked/sequential exchanges; no Internet/database; depends on Gates A-E.
- [ ] F2.8 `[privileged acceptance]` Verify real events enter WAL via group commit; metadata distinguishes received/queued/written-not-durable/durable/dropped; abruptly terminate, recover final tail, mark aborted, inject separate middle corruption fail-closed case; depends on F2.7.
- [ ] F2.9 `[privileged acceptance]` ETL twice and into second root; assert one deterministic session/store, scoped loss windows, complete/degraded operations, provenance/integrity/partial replayability; depends on F2.8.
- [ ] F2.10 `[privileged acceptance]` Replay executable subset only to explicit local target, force middle transport failure, verify full result accounting and no original-destination contact; depends on F2.9.
- [ ] F2.11 `[privileged acceptance]` Verify successful SIGINT/SIGTERM -> completed/exit 0, duration/WAL limits -> completed/exact reason, source/WAL failures -> failed, second-signal crash -> aborted on recovery; depends on F2.8.
- [ ] F2.12 `[CI/docs]` Publish separate opt-in privileged command/artifacts and mark ordinary CI as rootless fixture + eBPF compile only; no skipped-runtime pass claim; depends on F2.7-F2.11.

### F3. Documentation and final validation

- [ ] F3.1 `[README.md, docs/architecture.md, new operations guide]` Document bounded P1 flow, 10-minute default/60-minute max, 4 GiB WAL ceiling, one group-commit mode, durable watermark/crash window, lifecycle status/reason, cgroup scope safety, and always-on/rotating/incremental deferrals; depends on Gates B-C.
- [ ] F3.2 `[docs/wal-format.md]` Document WAL v1/v2, Capture Event versus `WalRecordEnvelope`, 4 MiB/10 ms group commit, segment/total limits, counters, final-tail-only repair, corruption fail-closed, and cross-root publication without recording-to-root binding; depends on Gates B-C.
- [ ] F3.3 `[docs/canonical-model.md, docs/replay-safety.md]` Document canonical v3 loss/completeness/provenance, full/partial/not replayable, mixed execution, exact outcomes/exits, loopback authorization, no redirects/original target, and sensitive-data boundary; depends on Gates D-E.
- [ ] F3.4 `[docs, CLI help, JSON schema/examples]` Document exact record/ETL/doctor grammar/defaults, shared atomic JSON semantics, remediation, local demo, test-profile labels, and exclusions; depends on F1-F2.
- [ ] F3.5 `[workspace]` Run fmt/check/Clippy/tests commands above and isolated eBPF compile; fix only P1-caused failures; depends on F2-F3.4.
- [ ] F3.6 `[Linux eBPF profile]` Run/retain privileged acceptance with kernel/arch/capabilities/results; do not claim runtime coverage when skipped; depends on F2.7-F2.12, F3.5.
- [ ] F3.7 `[openspec/changes/add-production-http-recording-pipeline]` Run strict validation, stale-language audit, task/spec/implementation comparison, and independent recovery/replay/security review before archive; depends on F3.5-F3.6.

**Gate F incremental test:** execute doctor plus full acceptance and validation commands. Archive only after reports prove all acceptance steps and unsupported/skipped areas are explicit.
