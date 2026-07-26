## Why

Gate A proved required eBPF hooks on one specific host matrix, but Chronicle still lacks a defined boundary between kernel observations and capture-domain evidence. This change defines that next narrow implementation slice without extending into recording, persistence, reconstruction, protocol semantics, or replay.

## What Changes

- Define an architecture-aware `RawKernelObservation -> CaptureAdapter -> CaptureEvent` boundary.
- Define evidence-only capture events that distinguish connect attribution from separately proven active establishment, close, reset, payload-fragment, and loss-window facts.
- Define boot/clock-scoped socket identity using socket cookie, first-seen monotonic timestamp, and network namespace identity when available; never equate a five-tuple or PID with a connection.
- Define stable recording-scope identity using cgroup ID, canonical path, and namespace information.
- Define typed adapter failures and explicit rejection of malformed kernel observations.
- Define an explicit `CaptureSource` lifecycle (`start`, `poll`, `request_shutdown`, `drain`, `finalize`) so final loss evidence and accepted kernel events are emitted before resource release; this boundary owns acquisition and cleanup only, never recording status, shutdown reason, WAL commits, or replay decisions.
- Define unit and privileged acceptance coverage for IPv4, IPv6, ordered payload fragments, loss evidence, and cleanup.
- Require feasibility evidence and doctor output to report architecture, kernel version, distribution, BTF availability, and hook capability results.
- Treat Ubuntu 24.04, exact kernel release Linux 6.8.0-136-generic (Linux 6.8 family), aarch64 as the only verified environment. Treat x86_64 and every other distribution, kernel version, cloud kernel, physical NIC, and offload environment as unverified target-matrix entries requiring separate acceptance evidence.
- Exclude WAL writing, ETL, TCP reconstruction, HTTP or other protocol parsing, canonical sessions, replay, and an always-on production daemon.

## Capabilities

### New Capabilities

- `ebpf-capture-adapter`: Kernel-observation decoding, evidence-only capture-domain normalization, socket/scope identity preservation, loss evidence, typed failures, compatibility reporting, and bounded privileged acceptance.

### Modified Capabilities

None.

## Impact

- Future implementation is confined primarily to `chronicle-capture` domain contracts and the `chronicle-capture-ebpf` Linux adapter, preserving `chronicle-common <- chronicle-capture <- chronicle-capture-ebpf` dependency direction.
- Existing `chronicle-wal`, `chronicle-protocol`, application, replay, and CLI recording behavior remains unchanged by this slice; compatibility evidence may consume existing doctor/reporting boundaries without adding recorder lifecycle.
- Gate A feasibility harness and retained report remain evidence inputs, not a universal support declaration.
- This change creates planning artifacts only; no production code is implemented.
