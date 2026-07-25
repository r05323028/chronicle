## Why

P0 proves Chronicle's fixture-driven HTTP path, but production capture, crash-recoverable WAL operation, standalone ETL, operational diagnostics, and complete partial-replay outcomes remain scaffolds or explicit deferrals. P1 is needed to prove the same existing boundaries with real plaintext HTTP/1.1 traffic from a selected Linux workload without weakening replay safety or claiming broader protocol/platform support.

## What Changes

- Add Linux-only eBPF capture for a dedicated cgroup v2 workload, with `--pid` resolution as a convenience, emitting bounded transport-neutral capture events through the existing capture seam; userspace retains TCP/HTTP reconstruction.
- Replace the one-segment, buffer-all fixture recording path for live capture with bounded capture-to-WAL flow, durable segmented WAL v2, explicit loss/failure counters, recording metadata, crash recovery, and safe reopen.
- Add standalone, restartable, idempotent ETL over recovered WAL segments, including connection-generation identity, TCP ordering/deduplication, provenance, deterministic session identity, and atomic checkpoint/publication ordering.
- Extend HTTP/1.1 decoding for reconstructed streams with chunked and valid close-delimited bodies, explicit completeness states, persistent sequential exchanges, and safe rejection of pipelining/upgrades and ambiguous traffic.
- Extend canonical filesystem sessions and inspect summaries with recording/WAL provenance, integrity, completeness, replay readiness, stable JSON, and body/header-value-safe defaults.
- Extend existing replay execution rather than replacing it: add an explicit overall outcome while retaining completed, failed, and unattempted operation results after transport or policy stops; update verification reports and exit mapping.
- Add runnable `record`, `etl`, and `doctor` application/CLI paths while preserving CLI-as-adapter layering, fixture capture for portable tests, dry-run replay defaults, explicit loopback target authorization, and no recorded-target fallback.
- Add rootless fixture/recovery tests plus a separate privileged Linux eBPF acceptance profile proving record interruption, WAL recovery, ETL, inspect, authorized replay, and partial-execution reporting.
- Document Linux/kernel/BTF/capability requirements, WAL/session sensitive-data risks, recovery, limits, JSON contracts, and known P1 exclusions.

## Capabilities

### New Capabilities
- `production-http-capture`: Linux cgroup-selected eBPF capture, recording lifecycle/metadata, bounded ingest, loss visibility, shutdown, and platform/privilege behavior.
- `recoverable-recording-wal`: Durable segmented capture WAL, framing/versioning, sync policy, rotation, recovery, corruption handling, backpressure, and persistence counters.
- `restartable-recording-etl`: WAL discovery/checkpointing, connection reconstruction, deterministic HTTP session publication, idempotency, restart, and ETL operational summaries.
- `recording-diagnostics`: `doctor` environment probes and stable supported/warning/unsupported/not-checked results for recording, storage, and replay prerequisites.

### Modified Capabilities
- `http11-operations`: Decode reconstructed multi-event streams, add required framing/completeness semantics, and explicitly reject unsafe pipelining/upgrades.
- `local-session-artifacts`: Persist recording/connection/WAL provenance and completeness, extend integrity/replay-readiness inspection, and retain safe output defaults.
- `safe-local-http-replay`: Add explicit aggregate replay outcome and durable partial verification reporting without weakening current loopback-only authorization.
- `runnable-http-cli`: Add production `record`, standalone `etl`, operational `doctor`, revised summaries/JSON contracts, signal handling, and partial-replay exit behavior while retaining fixture mode.

## Impact

- **Code:** `chronicle-common`, `chronicle-capture`, `chronicle-capture-ebpf`, `chronicle-wal`, `chronicle-session`, `chronicle-etl`, `chronicle-canonical`, `chronicle-protocol-builtins`, `chronicle-storage`, `chronicle-replay`, `chronicle-application`, and `chronicle-cli`; layering remains CLI -> application -> ports/adapters, with one explicit optional Linux-only `chronicle-application -> chronicle-capture-ebpf` composition edge.
- **Formats/APIs:** New capture event and WAL segment versions, recording metadata/checkpoint files, canonical completeness/provenance fields, replay outcome, and versioned JSON summaries require backward-compatible readers or explicit rejection of unknown newer versions. Existing P0 fixture/canonical/WAL artifacts remain readable.
- **Dependencies:** Aya-family dependencies are isolated to Linux capture program/loader build paths; `chronicle-application`/`chronicle-cli` propagate an optional `linux-ebpf` feature and Tokio adds only its signal feature. Existing `httparse`, Tokio TCP, CRC32C, SHA-256, serde, and filesystem adapters are reused. No database, broker, web framework, TLS stack, or dynamic plugin system is added.
- **Platform/operations:** Recording requires Linux 6.1+, cgroup v2, BTF, supported x86_64/aarch64, and documented eBPF capabilities/root fallback. macOS retains fixture/ETL/inspect/replay development; other unsupported platforms may compile but filesystem publication remains fail-closed and live recording fails clearly.
- **Security:** WAL and canonical files can contain sensitive production headers/bodies; P1 uses restrictive permissions and safe default output but does not promise encryption, secret discovery, or comprehensive redaction. Replay remains dry-run and loopback-only unless explicit target, allow-host, effect, and execute gates pass.
- **Migration/conflict:** Repository already has `ReplayExecution { results }`, contrary to the prompt's possible older `Result<Vec<_>>` shape; P1 extends it with `ReplayOutcome` instead of creating a duplicate executor model. P0 also intentionally excludes production capture, WAL restart, chunked/close-delimited HTTP, runnable ETL, and real doctor probes; these become explicit deltas while all wider protocols/backends remain excluded.
- **Risk:** Kernel/hook compatibility, capture loss, TCP ambiguity, disk exhaustion/corruption, sensitive-data persistence, eBPF CI privilege limits, and format migration are primary review risks.
