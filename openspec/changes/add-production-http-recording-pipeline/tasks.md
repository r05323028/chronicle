# Implementation Gates

Gates are dependency ordered and independently reviewable. Capture-specific contracts depend on Gate A. Kernel-independent primitives explicitly marked **parallel-safe before A** MAY proceed while feasibility runs. No gate may claim real eBPF coverage from fixture tests.

## Gate A — Kernel capture feasibility

**Entry dependencies:** archived P0 vertical slice passes; supported Linux 6.1+ x86_64/aarch64 host/VM with cgroup v2, BTF, and required privilege is available.

**Runnable commands to establish:**

```bash
(cd ebpf-feasibility && cargo build --release)
sudo -E cargo test -p chronicle-capture-ebpf --test privileged_feasibility -- --ignored --nocapture
```

**Acceptance scenario:** dedicated cgroup client performs real IPv4 and IPv6 plaintext HTTP/1.1 exchanges; harness observes request/response payload, stable candidate socket identity, tuple/direction/TCP positions, raw lifecycle state evidence, complete per-CPU loss snapshots on 100 ms monotonic schedule, actual delayed intervals, mandatory final sample, and clean detach.

**Artifacts produced:** checked-in harness, captured ABI probe output, kernel/arch/helper/capability report, GSO/GRO/nonlinear-skb evidence, 100 ms/final/delayed/reset loss-sampling evidence, direct cgroup TGID set/direct TGID count/descendant cgroup count/selected-subtree evidence, and explicit supported/unsupported matrix proposal.

**Stop/go:** GO only when kernel-derived identity, lifecycle, payload visibility, loss-snapshot timing, and subtree assumptions are proven. STOP and revise/revalidate OpenSpec if any assumption fails. Capture Event v1 kernel fields, connection identity, eBPF/userspace ABI, helper matrix, lifecycle guarantees, and GSO/GRO claims MUST NOT freeze before GO. Generic WAL/recovery/metadata/replay/JSON/filesystem work below may proceed independently.

- [x] A0 `[ebpf-feasibility/, chronicle-capture-ebpf tests]` Create disposable feasibility-only no-std eBPF package and privileged loader test command above without production feature chain or persisted ABI; verify clean build/run on supported host. This harness may be replaced after Gate A and SHALL NOT freeze production contract.
- [x] A1 `[ebpf feasibility harness]` Prove candidate Aya connect4/connect6, sock-ops, and cgroup-skb hooks correlate one real client flow across request/response; retain socket cookie/candidate generation, tuple, direction, TCP sequence, and raw established/state-change evidence. Prove sock-ops state alone cannot distinguish reset from orderly close or directional half-close, so preserve `unknown` unless another proven signal exists; privileged test; depends on A0.
- [x] A2 `[ebpf feasibility harness, chronicle-application tests]` Prove IPv4/IPv6 normalization, exact selected-subtree scope, stable cgroup inode/ID, and direct cgroup TGID set parsing from numeric PIDs listed directly in `cgroup.procs` after host-visible TGID resolution; separately count descendant cgroups and compare Chronicle's host-visible cgroup identity against the full subtree. Test one multithreaded process/TGID deduplication, two unrelated TGIDs, two TGIDs sharing one POSIX PGID, descendant-only workload, PID exit/race during enumeration, namespace-ambiguous resolution, recorder in a descendant, shared PID rejection, explicit shared warning/acknowledgement, forbidden roots despite acknowledgement, unreadable enumeration, and leak-free detach; depends on A1.
- [x] A3 `[ebpf feasibility harness]` Exercise GSO/GRO and nonlinear skb payload visibility and determine whether at-most-four 16 KiB continuations reconstruct ordinary aggregates; amend limits/support matrix rather than guess; depends on A1.
- [x] A4 `[ebpf feasibility harness, chronicle-capture-ebpf tests]` Prove complete per-CPU counter aggregation on same boot-monotonic clock at 100 ms schedule and mandatory post-intake final sample. Force ring reservation loss; use fake monotonic clock to test zero-delta no-WAL growth, positive delta, delayed poll expanded interval, pre-first-sample loss, reset/regression/map replacement/partial-read ambiguity, and final-sample failure; never claim exact drop time/connection; depends on A1.
- [x] A5 `[chronicle-capture-ebpf, docs test artifact]` Record exact kernel, architecture, Aya APIs, helpers, privileges, lifecycle evidence, payload limits, and unsupported cases in machine-readable feasibility report; compare report with spec constants; depends on A1-A4.

**Gate A incremental test:** run commands above and retain one report showing real kernel-derived request/response/lifecycle/loss evidence. Fixture-only output is gate failure.

## Gate B — Durable raw recording

**Entry dependencies:** B1.1-B1.4, B2.2, B3.1-B3.4, B4.1-B4.4, and B5.1-B5.5 are kernel-independent and parallel-safe before Gate A. B2.1/B2.3-B2.4, B4.5-B4.6, B6, and B7 require Gate A GO or its frozen capture contract. Repository fixture/WAL artifacts use sole mutable v1 and are rewritten with model changes.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-wal
cargo test -p chronicle-application --test recording_lifecycle
chronicle --format json record --source ebpf --cgroup "$CGROUP" --wal-dir "$WAL" --duration-seconds 600 --max-wal-bytes 4294967296
```

**Acceptance scenario:** selected workload streams Capture Event v1 and 100 ms/actual-delayed LossWindow evidence into bounded queue/segmented WAL. Each 4 MiB/10 ms/rotation/flush/shutdown group appends one in-WAL `CommitMarker` then uses one `fdatasync`; runtime durability acknowledgement occurs only after sync success, while recovery-authoritative marker validation remains distinct from proof of acknowledgement delivery. Hard-cap reservation writes capacity-safe queued prefix, counts remainder, and never exceeds bytes. Shared cgroup uses direct TGID count plus separate descendant cgroup count; PID scope rejects unrelated direct TGIDs. SIGINT/SIGTERM and successful limits finalize `completed`; capture/WAL/final-sample/final-marker failures finalize `failed`; crash leaves marker-recoverable prefix and stale metadata for `aborted` recovery.

**Artifacts produced:** Capture Event v1/LossWindow and `WalRecordEnvelope`/CommitMarker contracts, WAL v1 segments, `recording.json`, lock, categorized safe record summary schema/example, recovery report, and marker/hard-cap/sampling fault fixtures.

**Stop/go:** GO when the final recovery-authoritative commit marker and counters exactly distinguish received, accepted/queued, written-not-committed, committed, WAL-limit queued discard, kernel/backend drop, post-stop rejection, and ETL-checkpointed; all marker/crash/hard-cap/sampling fault tests recover only promised prefix; recording is bounded. STOP on silent loss, second per-group watermark durability cycle, per-record-sync dependency, unacknowledged broad cgroup expansion, physical-cap excess, or ambiguous status/reason.

### B1. Define kernel-independent mutable-v1 and output contracts

- [x] B1.1 `[chronicle-common, chronicle-wal, chronicle-canonical, chronicle-storage, chronicle-application]` **Parallel-safe before A.** Keep one mutable v1 per internal artifact domain with unversioned Rust models and typed non-v1 rejection. Add `RecordingId`; update repository fixtures/goldens atomically. No V1/V2/V3 DTO dispatch, compatibility reader, or speculative next-version constant.
- [x] B1.2 `[chronicle-replay, chronicle-application, chronicle-cli]` **Parallel-safe before A.** Define `ReplayOutcome`, operation execution state, replayability classification, and report JSON version on existing `ReplayExecution`; compile-time exhaustive match/serde snapshots; no duplicate executor abstraction.
- [x] B1.3 `[chronicle-application, chronicle-cli]` **Parallel-safe before A.** Add one bounded atomic JSON renderer: serialize complete result before stdout, checked `write_all`/flush, typed serialization versus CLI I/O errors, and no replay retry; shared record/ETL/inspect/replay/doctor fault tests.
- [x] B1.4 `[chronicle-storage]` **Parallel-safe before A.** Preserve staging, sync, manifest-last, atomic no-replace publication and private modes; expose reusable existing-manifest verification primitive; concurrent/failure tests.
- [x] B1.5 `[chronicle-application]` **Parallel-safe before A.** Define versioned core `recording.json` DTO and private atomic persistence before recovery: recording ID, optional immutable selector path/cgroup ID, status `starting/recording/completed/failed/aborted`, separate shutdown reason, last-valid-marker boundary, and stable categorized record/byte counters. Reject unknown/zero versions discriminator-first; temp write -> file sync -> rename -> directory sync; round-trip/version/mode/fault tests. Capture-specific build/kernel/capability/scope extensions remain B7.3.

### B2. Define capture and WAL ownership

- [x] B2.1 `[chronicle-capture]` After A, define Capture Event v1 capture-domain fields only: kernel time, process/stable-cgroup identity, proven connection identity, raw lifecycle evidence, direction/TCP position/payload/truncation; counter-derived `LossWindow` remains separate; no WAL sequence/segment/offset; binary round-trip tests; depends on A5, B1.1.
- [x] B2.2 `[chronicle-wal]` **Parallel-safe before A.** Define unplaced/placed `WalRecordEnvelope` ownership: recording ID, one monotonic sequence assigned at persistence, optional segment ordinal/first-sequence/offset provenance, `RecordKind`/schema/flags/payload. `CaptureEvent`, `LossWindow`, and `CommitMarker` consume sequence; opaque payload needs no pre-persistence sequence; construction/placement/exhaustive-kind tests; depends on B1.1.
- [x] B2.1a `[chronicle-session, chronicle-capture]` Define one reconstruction-domain input shared directly by fixture and production Capture Event v1. It carries stable socket identity, complete socket endpoint/role evidence, monotonic timestamp, direction, TCP sequence/continuation, raw payload/truncation, raw lifecycle, recording scope, and loss-window evidence; excludes WAL provenance, Aya/kernel ABI types, HTTP fields, and fixture-only persistence data; depends on B2.1.
- [x] B2.3 `[chronicle-session, chronicle-capture]` Update fixture v1 source in place to emit same endpoint-bearing `CaptureEvent` and enter same reconstruction path as production; non-v1 rejection tests; depends on B2.1a.
- [x] B2.4 `[chronicle-capture, chronicle-capture-ebpf]` Add deterministic current fixture source for raw lifecycle/TCP/truncation plus separate LossWindow observations through the shared reconstruction-domain input; fake monotonic 100 ms/delayed/reset/final-sample/no-zero-growth vectors; no HTTP/storage coupling; depends on B2.1a, B2.2-B2.3, A4.

### B3. Implement WAL v1 framing, segmentation, and recovery primitives

- [x] B3.1 `[chronicle-wal]` **Parallel-safe before A.** Implement sole-v1 exact 64-byte little-endian segment header codec: `CHS1`, format/header v1, length, recording UUID, ordinal, first sequence, signed Unix-nanosecond creation time, zero reserved bytes, CRC32C over bytes 0..60. Round-trip/non-v1/wrong magic/length/checksum/identity/reserved/truncated-header tests; depends on B1.1.
- [x] B3.2 `[chronicle-wal]` **Parallel-safe before A.** Implement sole-v1 exact 48-byte little-endian envelope frame: `CHE1`, envelope/record-schema v1, kind/flags, header length 48, payload length, recording UUID, WAL sequence, then CRC32C over bytes 0..44 plus payload. Sole reader derives segment ordinal/first-sequence/byte-offset provenance from validated context, enforces max length before allocation, rejects every non-v1 declaration, and has no compatibility dispatch; depends on B2.2, B3.1.
- [x] B3.3 `[chronicle-wal]` **Parallel-safe before A.** Add header-only deterministic discovery for exact 20-digit `<first-sequence>.chwal` published names; exclude dot-prefixed/temp files; validate recording ID, filename/header first-sequence match, unique contiguous 0-based ordinals, and strictly increasing header first sequences. Test missing/duplicate ordinal, duplicate/non-increasing first sequence, filename mismatch, temp exclusion, and deterministic order. Exact frame-range gap/overlap remains B5.1 commit-aware scan; depends on B3.1.
- [x] B3.4 `[chronicle-wal]` **Parallel-safe before A.** Create segment header under private temp name, file-sync, rename, directory-sync, then append; default 256 MiB/configurable 16 MiB-4 GiB rotation; exact boundary/oversized/crash/permission tests; depends on B3.1-B3.3.

### B4. Implement one bounded group-commit policy

- [x] B4.1 `[chronicle-wal]` **Parallel-safe before A.** Add `RecordKind::CommitMarker` v1 fixed 76-byte codec with version/reserved, durable-through/cumulative counts, batch range, SHA-256, and normal CRC32C envelope. Define recovery authority only for a complete marker whose referenced frames all exist, are contiguous permitted same-segment non-marker frames, match marker boundaries (`batch_first` = segment first or previous authoritative marker + 1; `batch_last` = final referenced sequence; marker = `batch_last + 1`), cumulative fields (previous authoritative totals + exact batch totals), and exact framed-byte digest, and whose envelope plus segment/header identities/checksums/versions validate; one/multi/loss-window/unknown-version/missing/non-contiguous/skipped-prefix/wrong-segment/identity/CRC/boundary/second-batch-cumulative/digest tests; depends on B2.2, B3.2.
- [x] B4.2 `[chronicle-wal]` **Parallel-safe before A.** Implement complete-frame writes and group order data -> marker -> flush -> one segment `fdatasync` -> mark runtime-durable/send or record acknowledgement only after success at 4 MiB, 10 ms, rotation, explicit flush, shutdown; no external watermark durability cycle, per-record sync, or durability CLI mode; fake-clock/threshold/rotation/shutdown/sync-count/acknowledgement-order tests; depends on B3.4, B4.1.
- [x] B4.3 `[chronicle-wal tests]` Inject durable-image crash/failure before marker, during marker, after marker before sync, and after successful `fdatasync` but before caller observes acknowledgement; separately test surviving-marker recovery authority versus acknowledgement-delivery evidence, threshold, rotation, flush, shutdown, complete post-marker uncertainty, and complete-looking markers with corrupt CRC/digest, missing/non-contiguous/skipped-prefix sequence, wrong segment, identity mismatch, boundary mismatch, or second-batch cumulative-total mismatch. Assert final authoritative marker, fail-closed invalid references, no inferred acknowledgement delivery, deterministic rerun, sole-v1 non-version rejection, and no format dispatch; depends on B4.1-B4.2.
- [x] B4.4 `[chronicle-wal]` **Parallel-safe before A.** Enforce physical total default/hard max 4 GiB over headers/frames/temp files under `segments/`; before append reserve complete frame + fixed final marker in current segment and total, plus required next header/peak publication bytes; rotate before data when pair cannot fit; never partial/excess write; exact-cap/rotation/temp/oversized/invalid-config tests; depends on B3.4, B4.1.
- [x] B4.5 `[chronicle-application, chronicle-wal]` Add bounded 4096-event ingest and hard-limit state machine: stop intake, write capacity-safe queued prefix, count exact remaining records/bytes as WAL-limit discard, separate kernel/backend drops and post-stop rejects, final marker sync; empty/full/partial queue, duration tie (`wal_size_limit` wins), final-sync failure tests; depends on B2.4, B4.2-B4.4.
- [x] B4.6 `[chronicle-application, chronicle-wal, chronicle-capture]` Persist typed terminal WAL-loss admission evidence; this is not a kernel/capture-source event and remains incomplete until B4.6.1-B4.6.5 pass; depends on A4, B4.5.
  - [x] B4.6.1 Freeze `TerminalWalLoss` DTO, closed reason/ambiguity enums, per-`ClockIdentity` interval and metadata-only fallback semantics. Include exact record/byte accounting, no socket attribution, no arbitrary error strings, and malformed/contradictory payload rejection; depends on A4, B4.5.
  - [x] B4.6.2 Add optional `MonotonicTimestamp` capture metadata to `QueuedWalRecord`; accumulate earliest/latest discarded timestamps and exact counts separately per clock, use compatible last persisted capture timestamp only as fallback, and reject/represent missing time without fabrication; depends on B4.6.1.
  - [x] B4.6.3 Add `RecordKind::TerminalWalLoss` schema-v1 codec to sole WAL v1 reader/recovery path. Round-trip/non-v1/malformed-interval/zero-or-contradictory-accounting tests; no compatibility dispatch; depends on B4.6.1-B4.6.2.
  - [x] B4.6.4 Integrate reserved terminal-record emission with WAL-cap state machine: one record per discard episode/clock only when terminal frame plus final marker fit; no recursive discard accounting; otherwise persist typed metadata summary; `wal_size_limit` wins duration tie; depends on B4.6.2-B4.6.3.
  - [x] B4.6.5 Add recovery and failure-path tests: single/equal-bound and multi-record intervals, incompatible clocks, exact counts/bytes, emitted once, no-room metadata fallback, final-marker/sync/metadata failure -> `failed` + `wal_failure` + no false persistence claim, recovery typed exposure, and unchanged B4.5 behavior; depends on B4.6.3-B4.6.4.

### B5. Recovery and recording lifecycle

- [x] B5.1 `[chronicle-wal]` **Parallel-safe before A.** Implement advisory lock and deterministic commit-aware scan; final recovery-authoritative marker controls committed boundary/counters only after complete-frame, envelope CRC32C, referenced-frame existence/contiguity/same-segment membership, boundary (`batch_first`/`batch_last`/marker sequence), cumulative-field (previous authoritative totals plus current batch), exact framed-byte SHA-256, and segment/header identity/version validation. Complete invalid markers fail closed; post-marker frames remain uncommitted uncertainty; concurrent writer/crash-release/missing-metadata/P0 v1 tests; depends on B3.1-B3.4, B4.1-B4.4.
- [x] B5.2 `[chronicle-wal]` Repair only incomplete final frame (including partial marker) to prior verified frame boundary and sync repair; ignore that marker as commit; reject truncated header, committed middle/CRC/marker/reference/digest/sequence/identity corruption without mutation; second-run determinism tests; depends on B5.1.
- [x] B5.3 `[chronicle-wal, chronicle-application]` Reconcile lagging/missing core metadata/counters from recovery-authoritative committed WAL without claiming runtime acknowledgement delivery; read-only scan reports complete post-marker frames unchanged; stale `starting/recording` -> `aborted`; immutable recording ID and caller-supplied selector mismatch fail. Explicit suffix discard/reopen remains B5.5. Metadata-lag-after-sync tests; depends on B1.5, B5.1-B5.2.
- [x] B5.4 `[chronicle-application]` Persist body-safe atomic `recovery-report.json` with repaired tail, last marker, post-marker uncertainty, marker corruption/version/reference/digest, committed counts, categorized WAL-limit loss; renderer tests; depends on B1.3, B5.2-B5.3.
- [x] B5.5 `[chronicle-wal]` Reopen only after successful recovery at last marker sequence + 1; never overwrite committed bytes; report then truncate/sync complete uncommitted suffix and allowed partial tail, remove later uncommitted segments, rotate if required; clean/repaired/uncommitted restart-append tests; depends on B5.2-B5.3.

### B6. Build isolated eBPF adapter

- [x] B6.1 `[Cargo.toml, chronicle-capture-ebpf, chronicle-application, chronicle-cli, ebpf/]` Add target-specific optional Aya loader, `linux-ebpf` feature chain, Tokio `signal` only, excluded no-std eBPF package, embedded endian-matched object; macOS/rootless builds need no bpf-linker; depends on A5.
- [x] B6.2 `[CI]` Add unprivileged eBPF compile lane separate from rootless workspace and label privileged runtime not checked; clean build tests; depends on B6.1.
- [x] B6.3 `[ebpf/]` Implement proven connect/lifecycle/payload programs and 8 MiB ring/per-CPU counters using Gate A ABI; lifecycle emits raw evidence only; no HTTP parsing/raw packet persistence; compile/vector tests; depends on A5, B2.1.
- [x] B6.4 `[chronicle-capture-ebpf]` Load/configure/attach atomically, decode ABI, aggregate complete per-CPU counters every 100 ms on shared monotonic clock, emit positive/ambiguous LossWindow observations, advance zero-delta boundary without WAL, stop intake then mandatory final sample before map release, never fallback; fake-clock/delayed-loop/forced-loss/reset/regression/partial-read/final-failure tests; depends on B6.1, B6.3, A4, B4.5-B4.6.

### B7. Harden cgroup selection and recording metadata

- [x] B7.1 `[chronicle-application, chronicle-capture-ebpf]` Implement selector preflight with canonical path/ID; direct cgroup TGID set from host-visible TGID resolution of numeric PIDs listed directly in selected-node `cgroup.procs`; separate direct TGID and descendant cgroup counts; full selected-subtree recorder containment; exact scope; forbidden-root/unreadable/namespace rejection. PID compares selected host-visible TGID with the set and rejects any unrelated TGID; explicit cgroup requires `--allow-shared-cgroup` for direct TGID count greater than one or any descendant; flag never overrides forbidden scope. Test multithreaded deduplication, two unrelated TGIDs, shared POSIX PGID with distinct TGIDs, descendant-only scope/counting, PID exit during enumeration, recorder in descendant, dedicated/shared/ack/no-ack, and safe output; depends on A2, B6.4.
- [x] B7.2 `[chronicle-application]` Provide PID baseline/revalidation API: resolve host-visible TGID and canonical cgroup path/ID at preflight, then fail closed on movement, death, namespace ambiguity, or enumeration race. Production attach-time samples and partial-link rollback belong to B7.6, which owns source/link resources; depends on B7.1.
  - [x] B7.2.1 PID baseline and typed revalidation API: capture canonical path/ID baseline; detect movement, death, namespace ambiguity, and enumeration race fail-closed; does not own eBPF links or detach behavior.
  - [x] B7.2.2 Attach-time revalidation and partial-attachment rollback ownership moved to B7.6, avoiding circular dependency before production probe/link/resource handles exist.
- [x] B7.3 `[chronicle-application]` Extend B1.5 core metadata with feasibility-proven build/kernel/host identity, canonical selector scope/count/acknowledgement, capabilities, configured/effective bounds, bounded capture errors, and live transition orchestration without changing core status/reason/counter meanings; depends on A5, B1.5, B7.2.
- [x] B7.4 `[chronicle-application, chronicle-capture-ebpf]` Define duration default 600/range 1..=3600 and total WAL default/hard max 4 GiB, then preflight selector/shared acknowledgement plus Linux/kernel/arch/BTF/object/hooks/capabilities/WAL path/free space/segment+total bounds before source attach. Capture probe checks object/program presence and capability prerequisites without attaching; B7.6 owns atomic attach failure/cleanup and successful metadata transition. Unsupported/unwritable/invalid-bound/safe-output tests create no successful metadata; depends on B7.1, B7.3.
  - [x] B7.4.1 `[chronicle-capture-ebpf]` Expose a no-attach runtime probe with typed safe results for platform, architecture, cgroup v2, BTF, embedded object/required program presence, and effective `CAP_BPF`/`CAP_NET_ADMIN`; unit-test pure capability/object checks.
  - [x] B7.4.2 `[chronicle-application]` Compose selector and capture probe with WAL/bound validation before source attach; retain no successful metadata on a failed preflight.
- [x] B7.5 `[chronicle-application]` Reuse B7.4 bounds and B4.5-B4.6 bounded-ingest behavior. Live source stop, duration/WAL-limit grace, final sample/marker/metadata outcome, and fake-clock/hung-finalization tests belong to B7.6, which owns production resources; depends on B4.4-B4.6, B7.3.
- [x] B7.6 `[chronicle-application, chronicle-capture-ebpf]` Orchestrate metadata -> eBPF source -> bounded ingest -> WAL. Own duration/WAL-limit stop: stop intake immediately; duration processes accepted queue, WAL limit writes capacity-qualified prefix/discard/final marker within five-second grace; successful final sample/marker/metadata yields completed/exact reason and failure yields failed. For PID selection, revalidate B7.2 baseline immediately before source attach and immediately after links attach through B7.6.1 callback; callback failure drops partial source/links on movement, death, namespace ambiguity, or enumeration race. Reconcile received/accepted/written/committed/WAL-limit-discard/kernel-drop/post-stop-reject/checkpoint counters+bytes. Source/sample/WAL failure stops failed with committed prefix; fake-clock/exact-byte/queued-prefix/hung-finalization/invariant/property tests; depends on B4.5-B4.6, B6.4, B7.2, B7.4-B7.5.
  - [x] B7.6.1 `[chronicle-capture-ebpf]` Add post-attach loader callback. On callback failure drop loaded eBPF before returning so partial links detach; application maps PID revalidation failure to typed safe capture error.
- [x] B7.7 `[chronicle-application]` Handle SIGINT/SIGTERM as graceful `completed`/exit 0 after successful finalization; second signal may force exit and later recovery marks aborted; subprocess tests for both signals and failure paths; depends on B7.6.
- [x] B7.8 `[chronicle-cli, chronicle-application]` Wire production grammar including explicit-cgroup-only `--allow-shared-cgroup`, bounds, commit boundary, categorized counters, physical size, and safe `direct TGID count`/`descendant cgroup count`/`selected subtree` help, warning, and summary through shared atomic renderer; never describe cgroup sharing by POSIX PGID; preserve P0 fixture; Clap/conflict/JSON/no-command-line-or-env-value tests; depends on B1.3, B7.6-B7.7.

**Gate B incremental test:** before HTTP ETL exists, run fake-source/rootless recording and privileged real capture, inspect WAL with recovery scanner, crash recorder, and prove verified commit-marker prefix plus honest aborted/loss metadata.

## Gate C — Recovery and ETL

**Entry dependencies:** Gate B GO. Gate A evidence already frozen into capture ABI. Current mutable-v1 protocol/canonical fixtures available.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-session -p chronicle-etl -p chronicle-protocol-builtins
chronicle --format json etl --wal-dir "$WAL" --output "$ROOT"
```

**Acceptance scenario:** recover stopped bounded recording through the final recovery-authoritative commit marker, ignore complete post-marker frames, reconstruct connection generations plus ring/backend and WAL-limit loss windows, decode supported HTTP, publish one deterministic session, rerun same root as already processed, and intentionally materialize same recording into another root without binding file.

**Artifacts produced:** deterministic canonical MVP-v1 session ID, manifest v1/session/payloads, advisory checkpoint, typed ETL/recovery summaries, loss-window provenance, and rootless deterministic fixtures.

**Stop/go:** GO when unchanged committed marker boundary always yields byte-identical IDs/content, complete post-marker frames are ignored, only verified final tail is repaired, committed corruption fails closed, ring/backend and WAL-limit windows degrade only intersecting/uncertain connections, and no operation is silently omitted. STOP on whole-recording loss degradation, cross-root binding, nondeterminism, or unbounded reconstruction.

### C1. Generation-safe reconstruction and loss windows

- [x] C1.1 `[chronicle-session]` Build on B2.1a shared reconstruction-domain input to group production data by feasibility-proven boot/socket/generation identity while retaining fixture `ConnectionKey`; descriptor/tuple/process-restart separation and synthetic-missing-open tests; depends on B2.3-B2.4.
- [x] C1.2 `[chronicle-session]` Implement independent wrap-aware TCP sequence ordering with WAL envelope provenance tie-breaker; direction/wrap/arrival-permutation tests; depends on C1.1.
- [x] C1.3 `[chronicle-session]` Deduplicate exact retransmitted ranges, detect conflicting overlap, and retain provenance without choosing bytes silently; exact/partial/conflict tests; depends on C1.2.
- [x] C1.4 `[chronicle-session]` Convert ring/backend observations and WAL-limit queue discard into deterministic windows `(clock/map identity, actual previous/current time, delta/epoch/category)`; attachment/final-failure/discard fallbacks apply. Matching-clock outside stays eligible; overlap/unknown/reset/mismatch degrades. Test regular/delayed cadence, pre-first/final failure, terminal loss with/without WAL frame, outside/overlap/unknown; depends on B4.6, C1.2.
- [x] C1.5 `[chronicle-session]` Keep raw lifecycle evidence separate from derived `clean_close/half_close/reset/unknown_termination`; derive half-close only if Gate A proves directional evidence; missing/open/close/reset tests; depends on A5, C1.2.
- [x] C1.6 `[chronicle-session]` Enforce 1024 active connections, 65,536 chunks/connection, 8 MiB/connection, five-minute event-time idle bound. Byte/chunk/idle limits finalize affected connection; active overflow fails typed; load/bound tests; depends on C1.3-C1.5.
- [x] C1.7 `[chronicle-protocol, chronicle-etl]` Expose protocol-neutral directional bytes, cross-direction completion order, derived termination, source integrity/loss windows, and WAL ranges; dependency audit and chronology tests; depends on C1.2-C1.6.

### C2. Decode bounded HTTP/1.1 honestly

- [x] C2.1 `[chronicle-protocol-builtins]` Adapt existing `httparse` decoder to reconstructed/provenance slices while preserving 64 KiB/128-header and P0 fixed-length behavior; depends on C1.7.
- [x] C2.2 `[chronicle-protocol-builtins]` Decode Content-Length/no-body request/response across arbitrary boundaries with exact consumption/provenance; split/head/body/zero/HEAD/204/304 tests; depends on C2.1.
- [x] C2.3 `[chronicle-protocol-builtins]` Decode bounded chunked responses/trailers; keep chunked requests/trailers unsupported; binary/split/malformed/missing-terminator tests; depends on C2.1.
- [x] C2.4 `[chronicle-protocol-builtins]` Decode response-to-close only from trusted derived clean-close and intact source; raw unknown/state-changed/reset/loss cannot complete; tests; depends on C1.5, C2.1.
- [x] C2.5 `[chronicle-protocol-builtins]` Preserve keep-alive sequential exchanges, FIFO pairing, HEAD method queue, deterministic cross-direction chronology, and pipelining detection; complete prior operations remain; depends on C2.2-C2.4.
- [x] C2.6 `[chronicle-protocol-builtins]` Represent malformed/incomplete/truncated/unmatched/unsupported/1xx/CONNECT/Upgrade/ambiguous framing/non-HTTP with stable evidence and no panic; enforce 8 MiB body; matrix tests; depends on C2.2-C2.5.

### C3. Canonicalize and publish deterministic session

- [x] C3.0 `[chronicle-canonical, chronicle-etl, chronicle-application]` Canonical MVP version strategy cleanup only: collapse pre-release canonical schema milestones into one evolving MVP v1; remove obsolete schema constants and dispatchers; flatten former nested metadata; remove duplicated completeness flags; define completeness maps as sole completeness authority and timeline array order as sole session-level operation order; preserve independent WAL, protocol, and manifest version domains.
- [x] C3.1 `[chronicle-canonical, chronicle-protocol-builtins]` Canonical provenance and completeness production: runtime production and integration of typed connection/operation completeness; source status/reason; connection, ring-loss, WAL-limit-loss, WAL-envelope, and commit-marker provenance; replay attributes; correct outside-versus-intersecting-loss-window operation completeness; protocol-builtins and ETL integration; binary and provenance behavior tests. Complete only when all producer and integration behavior is implemented and tested; depends on C1.4, C2.
- [x] C3.2 `[chronicle-canonical, chronicle-storage]` Canonical v1 contract validation: accept canonical schema version 1 only; reject every non-v1 version; preserve `PayloadRef::Object`; validate `CanonicalSession` structural consistency; checked-in current-MVP canonical fixture tests; publication-time fail-closed validation. Structural validation and publication guard complete; fixture coverage remains; depends on C3.1.
- [x] C3.3 `[chronicle-etl]` Acquire stopped recording ownership; process completed/failed/aborted prefix exactly through the final recovery-authoritative commit marker (or unchanged v1 boundary); ignore/report complete post-marker frames; reject active/committed corruption/unsupported/contradictory input; integration tests; depends on B5, B7.3.
- [x] C3.4 `[chronicle-etl]` Read committed v1 WAL envelopes in 4096-record batches, exclude CommitMarker from protocol input, ingest LossWindow/metadata terminal loss, and bound issues to existing 1024; post-marker/marker-version/malformed/batch determinism tests; depends on C1, C3.3.
- [x] C3.5 `[chronicle-etl]` Derive deterministic session/connection/operation IDs from domain separators, recording ID, pipeline version, WAL snapshot/provenance, and message order; fixed vectors; depends on C3.1, C3.4.
- [x] C3.6 `[chronicle-etl]` Build exactly one session per stopped recording. Fail before publication on operation 10,001 or active connection 1025; do not omit. Publish bounded opaque evidence only when durable WAL integrity is trustworthy; tests; depends on C2.6, C3.4-C3.5.
- [x] C3.7 `[chronicle-etl, chronicle-storage]` Publish deterministic ID via atomic no-replace in requested root without introducing a recording-to-root binding; verify existing manifest/session/provenance/checksums; same root already-processed, mismatch fail, second root allowed; crash/fault/concurrency tests; depends on B1.4, C3.5-C3.6.
- [x] C3.8 `[chronicle-etl]` Write advisory checkpoint only after publication with last-valid-commit-marker segment/offset/sequence, exact `wal_snapshot_sha256`, recovery digest, versions, session/manifest/latest-output identity, status/counters. Missing checkpoint repair and cross-root update tests; depends on C3.7.
- [x] C3.9 `[chronicle-application, chronicle-cli]` Wire `etl --wal-dir --output` and safe human/JSON summary through shared renderer; first/same-root/other-root/crash/corrupt CLI tests; depends on B1.3, C3.3-C3.8.

**Gate C incremental test:** `chronicle etl` over crashed/recovered WAL publishes deterministic session, reruns unchanged in same root without duplicate, publishes into second root intentionally, and loss-window fixture proves outside operation complete while intersecting operation degraded.

## Gate D — Canonical inspect pipeline

**Entry dependencies:** Gate C GO and existing `FilesystemSessionStore` publication semantics.

**Runnable commands to establish:**

```bash
cargo test -p chronicle-storage -p chronicle-application --test inspect
chronicle --format json inspect "$SESSION_ID" --root "$ROOT"
```

**Acceptance scenario:** inspect published session after original WAL is absent/relocated. Report verifies manifest/session/payload metadata integrity, embedded provenance, completeness/loss windows, and fully/partially/not replayable classification without body or arbitrary header values.

**Artifacts produced:** manifest v1 reader/writer, canonical MVP-v1 fixtures, safe inspect human/JSON schemas/examples, and integrity/replayability summaries.

**Stop/go:** GO when P0 manifest/session remain readable, newer versions fail explicitly, inspect is payload-value-safe, and each operation's completeness/replayability is visible. STOP on live-WAL path assumptions, hidden invalid operations, or partial JSON.

- [x] D1 `[chronicle-storage]` Update mutable manifest v1 with recording status/reason, last-valid-commit-marker boundary/WAL digest, pipeline/session/provenance/loss/completeness/integrity/replayability fields; no checkpoint checksum/live WAL locator; v1 round-trip/non-v1 rejection/checksum tests; depends on C3.1-C3.2, C3.8.
- [x] D2 `[chronicle-storage]` Preserve private staging/sync/manifest-last/no-replace publication for current canonical v1 and session-qualified SHA-256 payload refs; rerun permission/concurrency/failure tests; depends on D1.
- [x] D3 `[chronicle-storage]` Inspect manifest/session checksum and payload existence/size without reading bodies; hydration verifies digest before replay. Separate size-vs-same-size-digest tests; depends on D1-D2.
- [x] D4 `[chronicle-application]` Compute operation/session completeness and `fully_replayable/partially_replayable/not_replayable`; show executable/non-executable counts, loss-window blockers, status/reason, provenance/integrity/warnings; deterministic tests; depends on C3.1, D3.
- [x] D5 `[chronicle-application]` Human inspect shows safe metadata/method/target/status/size only; seeded Authorization/Cookie/custom-secret/header/body redaction snapshots; depends on D4.
- [x] D6 `[chronicle-application, chronicle-cli]` Render inspect JSON through shared atomic boundary with stable order/schema and serialization/stdout fault coverage centralized in B1.3; CLI not-found/corruption/P0/P1 tests; depends on B1.3, D4-D5.
- [x] D7 `[chronicle-application tests]` Delete/relocate source fixture/WAL after publication and prove inspect succeeds solely from embedded provenance without claiming current WAL availability; depends on D6.

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

- [x] E1 `[chronicle-replay]` Extend existing `ReplayExecution` with exact outcomes `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, `stopped_verification`; retain one ordered result/operation and no duplicate executor; depends on B1.2.
- [x] E2 `[chronicle-replay, chronicle-application]` Planner classifies every operation executable/non-executable and session full/partial/not replayable. Incomplete/truncated/malformed/unmatched/unsupported/pipelined/ambiguous-loss are not attempted but do not block complete siblings; matrix tests; depends on D4, E1.
- [x] E3 `[chronicle-replay, chronicle-application]` Preflight session integrity/capability, explicit loopback target, exact allow-host, execute, and effect gates before network. Authorization denial blocks all executable candidates while preserving non-executable reasons; adapter-connect spy tests; depends on E2.
- [x] E4 `[chronicle-replay]` Execute candidates sequentially in deterministic plan order, skip non-executable entries, convert first/middle transport failure to failed result, and mark later executable entries stopped-not-attempted; full cardinality/accounting tests; depends on E2-E3.
- [x] E5 `[chronicle-replay]` Stop on executed non-passing verification while retaining prior/invalid/later results; successful executable subset with invalid siblings -> `completed_with_skips`; tests; depends on E4.
- [x] E6 `[chronicle-protocol-builtins]` Preserve explicit loopback IP literal, no DNS/proxy/TLS/redirect/retry, one request/connection, five-second timeout, 8 MiB observed body; refusal/timeout/incomplete/overflow/3xx tests; depends on E4.
- [x] E7 `[chronicle-replay, chronicle-protocol-builtins]` Rebuild Host/Content-Length for target and strip captured Host/auth/cookie/forwarding/hop-by-hop/Connection-token/Transfer-Encoding from authorization influence; malicious-header tests; depends on C3.1, E3.
- [x] E8 `[chronicle-protocol-builtins]` Preserve exact status, body size/SHA-256, and selected ordered-header verifier with value-safe details; pass/status/header/body/missing/incomplete tests; depends on C3.1.
- [x] E9 `[chronicle-application]` Build report with plan, replayability, outcome, executable/non-executable/attempted/completed/failed/unattempted and verification counts; aggregate equals entries; depends on E1-E8.
- [x] E10 `[chronicle-application, chronicle-cli]` Render human report and one final JSON object via shared renderer. Exit 0 dry-run/completed/completed-with-skips; 4 policy/no-executable; 5 transport; 6 executed verification failure; render failure never retries; subprocess tests; depends on B1.3, E9.
- [x] E11 `[chronicle-application tests]` Instrument authorized target and original recorded destination, including redirect Location escape; assert only override receives requests; depends on E3, E6-E7.
- [x] E12 `[chronicle-replay]` Reject imported session >10,000 operations before network without truncating report/input silently; bounded test; depends on E2.

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
./scripts/acceptance/p1-privileged.sh
# Separate SDD artifact validation; not privileged runtime evidence.
openspec validate add-production-http-recording-pipeline --strict
```

**Acceptance scenario:** privileged script SHALL run and retain this exact sequence:

1. Start supported Linux host/VM with cgroup v2/eBPF prerequisites.
2. Start dedicated cgroup with production-like client workload.
3. Start Chronicle with bounded duration/WAL size.
4. Generate multiple plaintext HTTP/1.1 exchanges.
5. Verify capture/loss records enter WAL through one-marker/one-`fdatasync` group commit with no watermark-file cycle.
6. Verify received/accepted/written-not-committed/committed/WAL-limit-discard/kernel-drop/post-stop-reject/truncated metadata.
7. Abruptly terminate recorder.
8. Recover only last valid commit-marker prefix, repair only verified final tail, and mark stale recording aborted.
9. Run ETL and publish deterministic canonical session.
10. Prove ring/backend and WAL-limit windows degrade only potentially affected connections.
11. Inspect complete/degraded operations, partial replayability, provenance, integrity.
12. Start separate local replay target.
13. Replay only complete supported operations.
14. Verify degraded operations are non-executable/not-attempted.
15. Cause transport failure after one or more successes.
16. Verify completed/failed/non-executable/later-unattempted results and aggregate outcome.
17. Verify original recorded destination is never contacted.
18. Verify graceful SIGINT/SIGTERM produce completed status/exit 0.
19. Verify duration and WAL limits stop cleanly with exact reasons.
20. Run rootless tests, isolated eBPF compile, privileged acceptance, and doctor. Run strict OpenSpec validation separately as an SDD artifact check; it does not establish runtime completeness.
21. During active recording verify complete per-CPU loss sampling at documented 100 ms cadence.
22. Delay poll loop and prove persisted window uses actual expanded monotonic interval.
23. Trigger group commit and inspect one in-WAL marker covering contiguous batch/sequences.
24. Crash before marker, during marker, after marker before sync, and after successful `fdatasync` but before caller observes durability acknowledgement; test recovery authority separately from acknowledgement-delivery evidence.
25. Prove recovery recognizes only the final marker whose complete frame, envelope CRC32C, referenced frames/sequences/same-segment boundaries, exact framed-byte digest, and segment/header identities/versions validate; reject invalid references fail-closed and leave later complete frames as uncommitted uncertainty.
26. Fill physical WAL hard cap while events remain queued; prove bytes under `segments/` never exceed limit.
27. Prove writable queued prefix commits, remainder counters/bytes classify WAL-limit discard, terminal window records/summarizes, and completion requires final marker/metadata success.
28. Parse direct cgroup TGID sets and separately count descendants; prove multithreaded TGID deduplication, two unrelated TGIDs, shared POSIX PGID does not merge TGIDs, PID exit/race is not omitted, and PID selection rejects unrelated direct TGIDs despite acknowledgement.
29. Attempt explicit shared cgroup without/with acknowledgement; prove reject then exact selected-subtree attach while forbidden roots, namespace ambiguity, and Chronicle in any descendant still reject.
30. Prove operations outside both delayed ring-loss and WAL-limit terminal windows remain complete/replayable.

**Artifacts produced:** runnable doctor, operations/security docs, JSON schemas/examples, rootless/eBPF-compile/privileged test profiles, retained acceptance reports, and validated OpenSpec tasks.

**Stop/go:** GO for archive only when every required probe/test passes or is honestly labeled not checked, privileged runtime evidence is retained, docs state bounded-window/non-goals, and independent recovery/replay/security review finds no blocker.

### F1. Operational doctor

- [x] F1.1 `[chronicle-application]` Define required/optional probe statuses/aggregate precedence and stable code/remediation/order; exact exit 4/4/0/0 tests; depends on B1.1.
- [x] F1.2 `[chronicle-capture-ebpf]` Probe OS/arch/kernel/cgroup-v2/BTF/hooks/helpers/object/capability/attach non-destructively; cleanup links; distinguish unsupported/not-checked; mocked + privileged smoke; depends on B6.
- [x] F1.3 `[chronicle-application]` Reuse selector checks for forbidden roots, Chronicle anywhere in selected subtree, direct cgroup TGID set parsing/deduplication, separate direct TGID/descendant cgroup counts, shared PID rejection, explicit shared acknowledgement, unreadable or PID-exit enumeration, namespace ambiguity, and three-sample PID race; assert POSIX PGID is never used as sharing identity; path+ID+counts only, no command lines/env; depends on B7.1-B7.2.
- [x] F1.4 `[chronicle-wal, chronicle-storage]` Probe supplied WAL/output paths for private create/write/space/lock/one-sync in-WAL-marker commit/physical-cap accounting/no-replace publication. Omitted path -> optional not-checked; no guessed default/overwrite; depends on B4-B5, D2.
- [x] F1.5 `[chronicle-protocol, chronicle-replay]` Probe protocol capability and replay policy shape without network; missing per-command target informational; depends on E3, E6.
- [x] F1.6 `[chronicle-application, chronicle-cli]` Compose independent probes, cleanup temp artifacts, wire `doctor [--wal-dir] [--output]`, and render through shared JSON boundary; no environment values; subprocess tests; depends on B1.3, F1.1-F1.5.

### F2. Consolidated integration and acceptance

- [x] F2.1 `[chronicle-wal tests]` Consolidate v1 framing/envelope ownership, all-record sequence, fixed marker codec, one/multi batch, threshold/timer/rotation/shutdown single-sync counts, runtime acknowledgement only after successful `fdatasync`, crash after sync before acknowledgement observation, recovery-authority tests distinct from delivery-state tests, full marker frame/CRC/reference/sequence/same-segment/skipped-prefix/boundary/second-batch-cumulative/identity/version/digest corruption matrix, metadata-lag reconciliation without delivery claim, post-marker uncertainty, physical hard-cap reservation/queue prefix/discard/final loss/final sync, tail repair, middle corruption, reopen/version/deterministic recovery; depends on Gate B.
- [x] F2.2 `[chronicle-session, chronicle-etl tests]` Consolidate generation/order/dedupe/gap/raw-vs-derived lifecycle, regular/delayed/pre-first/final-failure/reset ring windows, WAL-limit terminal windows from WAL/metadata, outside/overlap/unknown attribution, post-marker exclusion/committed-boundary determinism, bounds/ID/cross-root/checkpoint; depends on Gate C.
- [x] F2.3 `[chronicle-protocol-builtins tests]` Consolidate Content-Length/chunked/no-body/close-derived/keep-alive/sequential/malformed/incomplete/unmatched/upgrade/pipelining/body-bound tests; depends on Gate C.
- [x] F2.4 `[chronicle-storage, chronicle-application tests]` Consolidate mutable-v1 atomic publication, inspect integrity/safe output/partial replayability, WAL-absent behavior, and shared JSON failure tests; depends on Gate D.
- [x] F2.5 `[chronicle-replay, chronicle-application, chronicle-cli tests]` Consolidate full/mixed/not replayable, target/effect denial, first/middle transport, verification stop, all-result accounting, original-target/redirect/timeout/JSON/exit tests; depends on Gate E.
- [x] F2.6 `[chronicle-application tests]` Rootless fixture v1 -> WAL group commit/recovery -> reconstruction/loss windows -> HTTP -> deterministic session -> inspect -> authorized mixed replay -> verification; label as fixture, not eBPF; depends on F2.1-F2.5.
- [x] F2.7 `[scripts/acceptance/p1-privileged.sh, scripts/tests/acceptance/test-p1-privileged-runner.sh, tests/e2e]` Add runnable privileged command listed above. Harness creates dedicated/shared cgroups, local upstream/client/recorder/separate replay target and executes the retained runtime acceptance sequence; several Content-Length/chunked/sequential exchanges; no Internet/database. OpenSpec validation remains a separate SDD artifact check, not runtime evidence; depends on Gates A-E.
- [x] F2.8 `[privileged acceptance]` Verify 100 ms/final/delayed loss sampling; one in-WAL marker + one `fdatasync` and no per-group metadata-watermark cycle; crash after sync before acknowledgement observation; recovery-authoritative marker validation independent of delivery evidence; invalid marker references/digests fail closed; metadata lag reconciles from WAL without delivery claim; recover final authoritative marker/final tail/aborted; separate middle corruption fails closed; depends on F2.7.
- [x] F2.9 `[privileged acceptance]` Fill WAL with queue remainder; assert physical cap, capacity-safe committed prefix, categorized discard, terminal loss record/summary, exact status on success/final-marker failure. ETL twice/second root; prove both loss types scoped and outside operations complete; depends on F2.8.
- [x] F2.10 `[privileged acceptance]` Replay executable subset only to explicit local target, force middle transport failure, verify full result accounting and no original-destination contact; depends on F2.9.
- [x] F2.11 `[privileged acceptance]` Verify direct cgroup TGID set parsing, multithreaded deduplication, two unrelated TGIDs, no POSIX-PGID sharing identity, PID exit/race rejection, separate descendant cgroup counting, dedicated leaf and lone PID acceptance, unrelated-TGID PID rejection, explicit shared rejection without/acceptance with acknowledgement, and forbidden roots/Chronicle in descendant rejection despite flag; successful SIGINT/SIGTERM and clean limits complete, source/sample/marker failures fail, second-signal crash recovers aborted; depends on F2.8-F2.9.
- [x] F2.12 `[CI/docs]` Publish separate opt-in privileged command/artifacts and mark ordinary CI as rootless fixture + eBPF compile only; no skipped-runtime pass claim; depends on F2.7-F2.11.

### F3. Documentation and final validation

- [x] F3.1 `[README.md, docs/architecture.md, new operations guide]` Document bounded P1 flow, 100 ms/actual-delay/final loss sampling, 10-minute default/60-minute max, 4 GiB physical ceiling/queued discard, one-marker/one-sync commit mode/crash window, recovery-authoritative persisted durability versus acknowledgement delivery, lifecycle status/reason, direct cgroup TGID set/direct TGID count/separate descendant cgroup count/selected-subtree shared-cgroup policy and forbidden scopes, and always-on/rotating/incremental deferrals; depends on Gates B-C.
- [x] F3.2 `[docs/wal-format.md]` Document WAL v1, Capture Event/LossWindow/CommitMarker kinds versus `WalRecordEnvelope`, fixed marker bytes/sequence/digest, exact recovery-authority validation, 4 MiB/10 ms one-sync ordering and post-sync runtime acknowledgement, no reconstruction of acknowledgement delivery, rotation, physical limit/reservation/counters, metadata reconciliation, final-tail-only repair/corruption fail-closed, and no external watermark file; depends on Gates B-C.
- [x] F3.3 `[docs/canonical-model.md, docs/replay-safety.md]` Document evolving canonical MVP v1 loss/completeness/provenance, append-only field policy, full/partial/not replayable, mixed execution, exact outcomes/exits, loopback authorization, no redirects/original target, and sensitive-data boundary; depends on Gates D-E.
- [x] F3.4 `[docs, CLI help, JSON schema/examples]` Document exact record/ETL/doctor grammar/defaults including explicit-cgroup-only `--allow-shared-cgroup`, safe scope/count output, stable counter categories, shared atomic JSON semantics, remediation, local demo, test-profile labels, and exclusions; depends on F1-F2.
- [x] F3.5 `[workspace]` Run fmt/check/Clippy/tests commands above and isolated eBPF compile; fix only P1-caused failures; depends on F2-F3.4.
- [x] F3.6 `[Linux eBPF profile]` Run/retain privileged acceptance with kernel/arch/capabilities/results; do not claim runtime coverage when skipped; depends on F2.7-F2.12, F3.5.
- [x] F3.7 `[openspec/changes/add-production-http-recording-pipeline]` Run strict OpenSpec validation, stale-language audit, task/spec/implementation comparison, and independent recovery/replay/security review before archive. OpenSpec validation is specification evidence only and must not be used as runtime completion evidence; depends on F3.5-F3.6.

**Gate F incremental test:** execute doctor plus full runtime acceptance and separate implementation/SDD validation commands. Archive only after reports prove all runtime acceptance steps and unsupported/skipped areas are explicit.
