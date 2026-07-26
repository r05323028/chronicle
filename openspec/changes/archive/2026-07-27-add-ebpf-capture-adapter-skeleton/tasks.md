This checklist refines and regroups pending capture ownership from `add-production-http-recording-pipeline`; it does not replace accepted gates or expand into WAL, ETL, protocol, replay, or recorder-lifecycle work. Gate headings preserve requested B6.1-B6.4 names; checkbox IDs use unique `CA*` prefixes to avoid collision with existing P1 task IDs.

| This change | Checkbox prefix | Existing P1 ownership refined |
|---|---|---|
| Gate B6.1 | `CA1` | B2.1, B2.3, B2.4, B6.3-B6.4 |
| Gate B6.2 | `CA2` | B6.1-B6.4 |
| Gate B6.3 | `CA3` | B2.3-B2.4, B6.4 |
| Gate B6.4 | `CA4` | A5, B6.2, B6.4, F1 |

## Gate B6.1 — Capture Domain Boundary

**Deliver:** `RawKernelObservation`, versioned Capture Event v2, `SocketIdentity`, `RecordingScopeIdentity`, and `LossWindowObserved` capture evidence.

**Acceptance:** kernel-specific structures do not leak outside the capture boundary; Capture Event v1 fixture bytes remain compatible.

- [x] CA1.1 `[chronicle-capture]` Define Capture Event v2 evidence kinds and shared timestamp, socket, recording-scope, lifecycle, payload-fragment, sequence, truncation, and loss-ambiguity values; exclude WAL sequence/provenance, durability, protocol, canonical, completeness, and replay fields; refine existing P1 B2.1.
- [x] CA1.2 `[chronicle-capture-ebpf]` Define adapter-private, architecture-aware `RawKernelObservation` kinds and explicit ABI version/size/byte-order validation without exposing Aya or generated eBPF structs; refine existing P1 B6.3-B6.4.
- [x] CA1.3 `[chronicle-capture-ebpf]` Implement `CaptureAdapter` conversion from connect4/connect6 to `SocketConnectObserved`, separately proven active-establish to `SocketConnected`, and only separately proven close/reset signals to corresponding observed events; retain all other lifecycle state as ambiguous raw evidence; map payload fragments and loss windows; keep `LossWindowObserved` distinct from future persisted WAL `LossWindow`.
- [x] CA1.4 `[chronicle-capture, chronicle-capture-ebpf]` Add typed unsupported-capability, attach, verifier, decode, invalid-payload, missing-identity, ring-loss/counter-sampling, and cleanup failures; ensure malformed observations surface with bounded source context and never disappear silently.
- [x] CA1.5 `[chronicle-capture tests]` Add portable unit tests for raw conversion, event mapping, socket/scope identity preservation, lifecycle ambiguity, fragment ordering evidence, loss conversion, invalid ABI/payload rejection, and Capture Event v1 compatibility.
- [x] CA1.6 Run dependency and public-API checks proving `chronicle-common <- chronicle-capture <- chronicle-capture-ebpf` and proving Aya/eBPF ABI types do not appear in downstream domain APIs.

## Gate B6.2 — Aya Userspace Adapter

**Deliver:** Aya loader, ring-buffer reader, event decoder, and `CaptureAdapter` pipeline.

**Acceptance:** the validated privileged Linux environment produces a `CaptureEvent` stream without WAL or downstream processing.

- [x] CA2.1 `[chronicle-capture-ebpf, chronicle-ebpf-capture]` Implement only Gate A-validated connect4/connect6, sock-ops, and cgroup-skb observation programs emitting documented private ABI v1; retained unversioned Gate A `ProbeEvent` remains historical evidence and is never decoded by `CaptureAdapter`; perform no HTTP parsing or raw-packet persistence; refine existing P1 B6.1/B6.3.
- [x] CA2.2 `[chronicle-capture-ebpf]` Load, verify, configure, and attach all required hooks atomically; on any failure detach partial links and release maps/programs before returning the typed failure.
- [x] CA2.3 `[chronicle-capture-ebpf]` Read the ring buffer, decode each ABI item into `RawKernelObservation`, reject malformed sizes/discriminants/bounds, and feed the single `CaptureAdapter` conversion path.
- [x] CA2.4 `[chronicle-capture-ebpf]` Expose the resulting evidence stream through the existing capture-source boundary without buffering for WAL, assigning WAL order, starting ETL, or adding daemon lifecycle.
- [x] CA2.5 `[CI/build]` Add unprivileged architecture-specific eBPF compile checks and rootless workspace checks that clearly state privileged runtime behavior is not proven by compile success; refine existing P1 B6.2.
- [x] CA2.6 `[privileged acceptance]` On Ubuntu 24.04, exact kernel release Linux 6.8.0-136-generic, aarch64, verify one dedicated-cgroup IPv4 flow maps `connect4` to `SocketConnectObserved`, sock-ops active-establish to `SocketConnected`, one stable socket identity, and emitted `CaptureEvent` stream.
- [x] CA2.7 `[privileged acceptance]` On the same verified environment, verify one IPv6 connection maps `connect6` evidence and preserves IPv6 family in emitted events.
- [x] CA2.8 `[privileged acceptance]` Generate a large plaintext TCP payload and verify multiple emitted fragments preserve kernel timestamp, direction, TCP sequence, continuation position, truncation metadata, and per-socket ordering evidence without claiming a global cross-CPU/hook order.

## Gate B6.3 — Lifecycle and Identity

**Deliver:** socket correlation, lifecycle evidence, connect-derived scope attribution, loss-sampling shutdown boundary, and cleanup handling.

**Acceptance:** the same observed connection remains correlated while ambiguous lifecycle evidence stays unclassified.

- [x] CA3.1 `[chronicle-capture-ebpf]` Correlate observations by boot/clock identity, socket cookie, first-seen monotonic generation, and optional network namespace identity; never join by PID/TGID, file descriptor, or five-tuple alone; refine existing P1 B2.3.
- [x] CA3.2 `[chronicle-capture-ebpf]` Accept caller-supplied recording-scope configuration validated before attachment, verify authoritative connect-observed descendant cgroup ID belongs to it, and join process plus full `RecordingScopeIdentity`; do not implement selector preflight or substitute packet-hook execution-context PID/cgroup identity.
- [x] CA3.3 `[chronicle-capture-ebpf]` Keep connect-attribution separate from active-establish; map only separately proven signals to connected, close-observed, or reset-observed events; retain unproven sock-ops state as raw state-change evidence and make no clean-close, half-close, aborted-connection, completeness, or replay claim; refine existing P1 B2.4.
- [x] CA3.4 `[chronicle-capture-ebpf]` Aggregate complete per-CPU loss counters on the Gate A monotonic clock, emit positive or ambiguous `LossWindowObserved` evidence, use actual delayed intervals, and advance zero-delta boundaries without emitting synthetic loss.
- [x] CA3.5 `[chronicle-capture, chronicle-capture-ebpf]` Implement the explicit CaptureSource lifecycle contract: `Created -> Running -> ShutdownRequested -> Draining -> Finalized`; separate start/poll, idempotent shutdown request/intake freeze, drain of accepted events plus final loss evidence, and finalization. Enforce shutdown ordering: stop intake, take mandatory actual-time `CLOCK_MONOTONIC` final sample while counter maps remain, emit required final loss evidence, drain pending events, then detach/release resources. Return typed lifecycle, final-sample, drain, or cleanup failure; do not add WAL/ETL/replay/recording lifecycle behavior.
- [x] CA3.6 `[portable tests]` Cover PID reuse, tuple reuse, socket-cookie generation reuse, optional namespace identity, connect-derived cgroup attribution, lifecycle ambiguity, counter reset/regression, map replacement, incomplete per-CPU read, delayed sample, initial uncertainty, and final-sample failure. Add lifecycle state-transition, idempotent request-shutdown, drain-after-shutdown, finalize-before-drain rejection, final-loss ordering, pending-event drain, delayed-final-sample, and drain-failure/cleanup tests.
- [x] CA3.7 `[privileged acceptance]` Verify running capture -> shutdown request -> final loss sample -> drain -> finalize ordering, then verify no attached Chronicle links, loaded Chronicle programs, retained maps, or dedicated cgroup resources using explicit before/after bpftool and cgroup inspection.

## Gate B6.4 — Compatibility Evidence

**Deliver:** feasibility report artifact, doctor capability output, and mandatory environment metadata.

**Acceptance:** reports identify verified Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64 evidence separately from unverified target-matrix entries, including x86_64.

- [x] CA4.1 `[chronicle-capture-ebpf evidence schema]` Add mandatory architecture, exact kernel version, Linux distribution, cgroup v2 status, BTF availability, and per-hook capability result fields; add explicit `verified_environment` and `target_compatibility_matrix` sections.
- [x] CA4.2 `[Gate A/B6 evidence]` Regenerate the retained verified-environment report so distribution is machine-readable, preserve exact Ubuntu 24.04/Linux 6.8.0-136-generic/aarch64 observations, and label x86_64 plus every unevaluated environment `not_verified`.
- [x] CA4.3 `[doctor/reporting boundary]` Reuse the existing typed doctor/report boundary to render environment and hook capability results without adding record commands, daemon management, WAL probes, ETL, protocol, canonical, or replay behavior to this slice.
- [x] CA4.4 `[privileged acceptance]` Start the validated environment, create a dedicated cgroup, launch a plaintext TCP workload, attach `CaptureAdapter`, generate IPv4 and IPv6 traffic, observe and convert kernel events, verify socket identity/timestamps/direction/fragments/loss evidence, detach or quiesce producers, drain the ring, take the mandatory final counter sample while maps remain, release maps/programs, generate the evidence artifact, and assert the report contains no unsupported architecture claim.
- [x] CA4.5 `[loss acceptance]` Force ring pressure on the verified environment and verify `LossWindowObserved` includes actual start/end timestamps, positive drop delta, available epoch/generation, and ambiguity fields without affected-connection attribution.
- [x] CA4.6 `[separate matrix acceptance]` Keep future x86_64 validation as a separate privileged environment, test run, and retained report; no aarch64 result may satisfy or imply x86_64 acceptance.
- [x] CA4.7 `[documentation]` Document verified versus target matrix and list unverified distributions, kernel minor versions, cloud-provider kernels, physical NIC behavior, and production offload characteristics without claiming Linux 6.1+ universal support.
- [x] CA4.8 Run `openspec validate add-ebpf-capture-adapter-skeleton --strict`, focused unit/build checks, the marked privileged acceptance command on the verified environment, contradiction grep for forbidden WAL/ETL/protocol/replay behavior, and `git diff --check`; retain commands and results with the feasibility evidence.
