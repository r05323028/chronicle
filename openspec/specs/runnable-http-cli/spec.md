## Purpose

Minimal record, inspect, and replay command contracts plus end-to-end runnable HTTP demonstration.

## Requirements

### Requirement: Minimal record command

CLI record SHALL expose the public forms per `user-intent-cli`: `record -- COMMAND...`, `record --pid PID`, `record --cgroup PATH [--allow-shared-cgroup]`, and `record --retry RECORDING`, with optional `--name` and `--duration`, and global `--data-dir` resolution. Shared acknowledgement SHALL be invalid with `--pid` or no explicit `--cgroup`. Conflicting options SHALL be validated; `record` requires exactly one mode. The removed `--source`/`--wal-dir`/`--segment-bytes`/`--duration-seconds`/`--max-wal-bytes` compatibility forms SHALL NOT be reintroduced. Public selector forms SHALL show effective cgroup path/ID/direct TGID count/descendant cgroup count/selected-subtree scope/acknowledgement before attach, perform preflight, create/finalize metadata, run eBPF plus bounded in-WAL-marker group commit, handle limits/signals, and print safe categorized summary; ETL/finalization runs automatically per `user-intent-cli`. CLI help and diagnostics SHALL use **direct cgroup TGID set** for distinct host-visible TGIDs resolved from numeric PIDs listed directly in selected node's `cgroup.procs`; they SHALL NOT call it a POSIX process group, thread count, container ID, or descendant union.

#### Scenario: Fixture record uses current artifacts

- **WHEN** valid fixture command runs
- **THEN** it uses same Capture Event v1, segmented WAL v1, reconstruction, Canonical Session v1, manifest v1, and report v1 family as production

#### Scenario: Production record happy path

- **WHEN** supported privileged Linux host records valid dedicated workload selector
- **THEN** command writes bounded group-committed WAL/metadata, stops gracefully, and outputs recording ID/status/reason/last valid commit boundary/categorized counters

#### Scenario: Public record finalizes automatically

- **WHEN** the public `record` command completes capture
- **THEN** it finalizes and publishes automatically; no hidden compatibility form exists

#### Scenario: Record argument validation

- **WHEN** selector is missing/multiple, fixture/eBPF options are mixed, or shared acknowledgement lacks explicit cgroup
- **THEN** Clap/application rejects with usage exit 2 and capture does not start

#### Scenario: Explicit shared cgroup acknowledgement

- **WHEN** non-forbidden explicit cgroup has direct TGID count greater than one or descendant cgroup count greater than zero and is selected with `--allow-shared-cgroup`
- **THEN** preflight shows separate direct TGID/descendant counts and selected-subtree warning, then persists acknowledgement before exact-subtree attach

#### Scenario: PID selector direct TGID safety

- **WHEN** selected PID's host-visible TGID is compared with the direct cgroup TGID set
- **THEN** any unrelated direct TGID rejects selection and CLI offers no shared-cgroup override

#### Scenario: Unsupported platform or privilege

- **WHEN** eBPF source is requested on unsupported host or without privilege
- **THEN** command exits non-success with stable safe diagnostic and no successful recording

#### Scenario: Graceful SIGINT

- **WHEN** SIGINT arrives and detach/final sample/capacity-qualified queue handling/final marker sync/finalization succeed
- **THEN** command marks `completed` with `user_interrupt`, prints final summary, preserves ETL-ready WAL, and exits 0

#### Scenario: Graceful SIGTERM

- **WHEN** SIGTERM arrives and detach/final sample/capacity-qualified queue handling/final marker sync/finalization succeed
- **THEN** command marks `completed` with `termination_signal`, prints final summary, and exits 0

#### Scenario: Duration or WAL limit

- **WHEN** duration expires or WAL writer cannot fit next frame while reserving final marker/header
- **THEN** command stops cleanly, uses exact deterministic reason, reports queued discard/post-stop rejection separately, and completes only after final marker/metadata succeed

### Requirement: Minimal inspect command

CLI inspect SHALL expose `inspect <RECORDING>` and `inspect latest` per `user-intent-cli`, resolving recordings from the data directory per `recording-identity` with global `--data-dir`; the removed session-root `inspect SESSION --root ROOT` compatibility form SHALL NOT be reintroduced. It SHALL support global `--format human|json`, default human, and delegate to application/storage service.

#### Scenario: Inspect happy path

- **WHEN** known recording is inspected through the public or compatibility form
- **THEN** command exits 0 with safe summary required by local-session-artifacts spec

#### Scenario: Unknown recording

- **WHEN** recording reference is valid but absent
- **THEN** command exits 3 with stable not-found error

#### Scenario: Invalid recording reference

- **WHEN** recording reference is not a valid ID, `latest`, or known name
- **THEN** command exits 3 without path access outside the data directory

### Requirement: Minimal replay command

CLI replay SHALL expose the public forms per `user-intent-cli` and `safe-local-http-replay`: `replay <RECORDING> -- COMMAND...` (spawned target with inferred local target and authorization) and `replay <RECORDING> --target URL` (explicit target requiring full authorization), mutually exclusive, resolving recordings with global `--data-dir`. Defaults SHALL remain human output, dry-run, no effect authorization, no target default, and no network. The removed session-root `replay SESSION --root ROOT` compatibility form SHALL NOT be reintroduced. Human/JSON output SHALL include exact replay outcome, fully/partially/not replayable classification, aggregate executable/non-executable counts, and one result per operation.

#### Scenario: Replay dry-run

- **WHEN** hidden legacy or public explicit-target replay omits `--execute`
- **THEN** command prints plan/report with dry-run outcome and zero network; command mode instead infers execution intent from `-- COMMAND...`

#### Scenario: Replay denied

- **WHEN** target/effect authorization or wholly invalid recording preflight denies execution
- **THEN** no request is sent, report includes every unattempted/non-executable operation, and execution exits 4

#### Scenario: Mixed-session replay

- **WHEN** partially replayable recording has authorized complete operations and degraded operations
- **THEN** only complete supported operations execute; degraded operations appear not attempted; successful subset yields `completed_with_skips`

#### Scenario: Replay local success

- **WHEN** complete operations are explicitly authorized for loopback target
- **THEN** requests go only to target override, report outcome completed, and exit is 0 if verification passes

#### Scenario: Verification failure

- **WHEN** complete observed response mismatches expected response
- **THEN** report retains mismatch and prior results, marks later operations unattempted, and exit is 6

#### Scenario: Network failure

- **WHEN** transport fails after zero or more completed operations
- **THEN** JSON/human report retains all completed/failed/unattempted entries and exit is 5

### Requirement: Stable CLI exit and output contract

CLI SHALL use exit 0 for successful completed record (including handled SIGINT/SIGTERM and clean limits), ETL/inspect/list, doctor supported/warnings, replay dry-run, `completed`, or `completed_with_skips`; 2 for Clap usage; 3 for recording/WAL/ETL/session/storage/serialization/finalization data errors and recording-resolution failures for inspect/replay/record-retry references; 4 for replay target/effect/no-executable invalid-session denial, unsupported live preflight, or doctor required unsupported/not-checked; 5 for replay transport stop; and 6 for executed verification failure. Unsupported/inconclusive siblings in successful mixed replay SHALL NOT force exit 6. Target application exit after successful exec is factual and not forwarded; bootstrap/hardening/exec failure is a typed Chronicle 3/4 failure. Human success uses stdout and errors stderr; JSON-mode child output is redirected away from Chronicle protocol streams. Deprecated warnings use stderr while success stdout stays atomic. All JSON uses shared atomic rendering.

#### Scenario: Clap usage error

- **WHEN** required argument is missing/invalid
- **THEN** exit is 2 and no application side effect occurs

#### Scenario: JSON application error

- **WHEN** command requested JSON and typed application failure occurs
- **THEN** stderr is valid stable JSON error, stdout has no partial success object, and secrets are absent

#### Scenario: Replay JSON plan visibility

- **WHEN** JSON replay executes network operations
- **THEN** plan is validated before first connection and appears inside one final atomic JSON report rather than pre-execution partial JSON

#### Scenario: Partial replay exit mapping

- **WHEN** replay report outcome is `stopped_transport` after prior success
- **THEN** full report is emitted and process exits 5

#### Scenario: Doctor unresolved required probe

- **WHEN** doctor aggregate is unsupported or required not-checked
- **THEN** full diagnostic report is emitted and process exits 4

#### Scenario: Recording signal exits

- **WHEN** SIGINT or SIGTERM graceful group sync/finalization succeeds
- **THEN** completed summary with distinct shutdown reason is emitted and process exits 0

#### Scenario: Recording finalization failure

- **WHEN** signal shutdown cannot flush WAL or finalize metadata
- **THEN** safe failed summary is emitted and process exits 3

#### Scenario: Child exit not forwarded

- **WHEN** the recorded command exits nonzero or by signal
- **THEN** recording completes, summary reports the child exit status or signal factually, and Chronicle's exit remains 0

#### Scenario: Recording resolution failure

- **WHEN** inspect, replay, or record retry cannot resolve a recording reference
- **THEN** command exits 3 with an actionable stable error and no side effect

#### Scenario: List storage failure

- **WHEN** list encounters unsafe, malformed, contradictory, or oversized catalog/storage input
- **THEN** command exits 3 without treating it as recording-reference resolution

### Requirement: CLI remains outer adapter

HTTP/TCP parsing, canonicalization, capture attachment, WAL recovery, ETL checkpointing, storage, signal lifecycle, safety decisions, network execution, verification, and doctor probes SHALL live outside CLI crate. CLI SHALL parse arguments, map commands, render application results, and map typed outcomes/errors to exit codes only. `etl` and `doctor` SHALL be runnable application services rather than scaffold responses.

#### Scenario: Protocol logic audit

- **WHEN** CLI implementation is reviewed
- **THEN** no protocol decoder, eBPF loader details, WAL scanner, filesystem publisher, replay policy engine, or verifier lives in CLI

#### Scenario: Doctor delegation

- **WHEN** doctor command runs
- **THEN** CLI renders typed application diagnostic results instead of hard-coded success text

### Requirement: Deterministic local HTTP demonstration server

Test support SHALL provide in-process Tokio loopback server on ephemeral port with GET no body, POST body handling, deterministic response headers, non-2xx, binary response, pass mode, and mismatch mode. Core tests SHALL require no Docker, external service, fixed port, root, or Internet.

#### Scenario: GET endpoint

- **WHEN** authorized replay sends fixture GET
- **THEN** server returns deterministic fixed-length response suitable for Passed verification

#### Scenario: POST endpoint

- **WHEN** authorized replay sends binary/fixed body POST
- **THEN** server observes exact method/path/body and returns deterministic response

#### Scenario: Verification mismatch mode

- **WHEN** server is configured to change status/header/body
- **THEN** replay completes and verifier reports Failed

### Requirement: Vertical integration proof

Rootless integration SHALL exercise fixture capture events through recoverable WAL/ETL/canonical publication/inspect/replay/verification without claiming eBPF. Separate privileged Linux acceptance SHALL exercise real workload traffic, recording interruption, WAL recovery, ETL idempotency, inspect, authorized replay, and partial replay failure while proving original destination is not contacted.

#### Scenario: Rootless full pipeline

- **WHEN** ordinary workspace integration suite runs
- **THEN** deterministic capture-event fixtures prove WAL/reconstruction/HTTP/storage/replay boundaries without privileges

#### Scenario: Real Linux eBPF acceptance

- **WHEN** privileged acceptance profile runs on documented supported Linux host
- **THEN** observed real HTTP traffic proves bounded recording, group commit/recovery, loss-window completeness, partial inspect/replay, and retained artifacts/reports

#### Scenario: Coverage labeling

- **WHEN** fixture-only CI passes but privileged profile did not run
- **THEN** documentation/status reports eBPF acceptance not checked, not passed

### Requirement: Required test coverage

Implementation SHALL include WAL framing/rotation/group-commit threshold/time/shutdown/fault/recovery/corruption/reopen/failure/queue tests; stream reconstruction split/order/reuse/gap/truncation/direction/loss-window/lifecycle-derivation tests; HTTP framing/pairing/completeness tests; ETL cross-root idempotency/restart/corruption/publication tests; CLI duration/WAL-limit/cgroup/signal/atomic-JSON/exit tests; and replay mixed-session/full-accounting/first-or-middle-failure/policy/target/redirect/timeout/verification tests. Tests SHALL assert no sensitive values in default output.

#### Scenario: Unit and integration suites

- **WHEN** rootless workspace tests run
- **THEN** all specified non-eBPF cases pass without Docker, root, fixed port, or Internet

#### Scenario: Privileged profile

- **WHEN** privileged Linux profile is explicitly enabled
- **THEN** real eBPF load/attach/capture/loss/shutdown/recovery scenario runs separately

### Requirement: Documentation and validation gates

README, architecture, canonical, replay safety, WAL format, capability matrix, and the operations guide SHALL document supported platform/kernel/privileges, record/WAL/recovery/ETL/inspect/replay workflow, JSON schemas, limits, known exclusions, sensitive-data warning, local demo, CI versus privileged tests, and doctor remediation. OpenSpec and Rust gates SHALL pass without overstating eBPF or production scale.

#### Scenario: Runnable local documentation

- **WHEN** user follows supported Linux example
- **THEN** commands create dedicated workload, record, recover/ETL, inspect, replay only to authorized local target, and show reports

#### Scenario: Repository gates

- **WHEN** implementation is ready for review
- **THEN** fmt, locked workspace check, Clippy all-features, and locked all-features tests pass plus Linux eBPF compile profile

#### Scenario: Strict OpenSpec validation

- **WHEN** change artifacts are complete or implementation tasks are updated
- **THEN** `openspec validate add-production-http-recording-pipeline --strict` passes and checkboxes reflect reality

### Requirement: Shared atomic JSON rendering

Application SHALL serialize complete bounded v1 results before stdout for public record/list/inspect/replay/doctor and hidden compatibility reports. CLI SHALL write bytes through one checked boundary. Serialization failure is typed before stdout; write/flush failure is CLI I/O error. Replay never retries traffic because rendering failed. In JSON mode spawned target stdout/stderr SHALL be connected to the platform null sink before exec and SHALL NOT share Chronicle stdout/stderr or create output files/buffers.

#### Scenario: Serialization failure

- **WHEN** any command result cannot serialize
- **THEN** stdout contains no partial JSON and stderr reports typed data error

#### Scenario: Stdout write failure

- **WHEN** serialization succeeds but checked write/flush fails
- **THEN** CLI returns I/O error without relabeling operation success

#### Scenario: Noisy child in JSON mode

- **WHEN** recorded or replayed target writes arbitrary stdout/stderr while JSON format is selected
- **THEN** Chronicle stdout remains one JSON object, Chronicle stderr remains reserved for its one diagnostic/error object, and child bytes are absent from both

#### Scenario: Replay render failure

- **WHEN** replay completed but final render/write fails
- **THEN** Chronicle does not repeat network operations

### Requirement: Standalone ETL command

CLI SHALL provide `etl --wal-dir DIR --output ROOT` with global human/JSON format. Application SHALL recover/validate recording snapshot through the final recovery-authoritative commit marker, atomically publish deterministic session to requested root, update advisory checkpoint after publication, and output session/checkpoint/completeness/counters. Repeated same-root run SHALL verify/report already processed; another root SHALL be allowed and protected by independent no-replace verification.

#### Scenario: First ETL run

- **WHEN** valid stopped recording WAL is processed
- **THEN** command publishes one session and prints deterministic ID/checkpoint summary

#### Scenario: Second ETL run

- **WHEN** same unchanged WAL/output is processed again
- **THEN** command reports already processed with same ID and no duplicate session

#### Scenario: ETL into another root

- **WHEN** same unchanged recording is intentionally processed into another output root
- **THEN** command publishes/verifies same deterministic session there without `output-binding.json`

#### Scenario: Corrupt WAL ETL

- **WHEN** recovery identifies non-tail corruption or unsupported version
- **THEN** command exits 3, publishes nothing, and emits safe recovery summary

### Requirement: Operational doctor command

Public `doctor` SHALL use resolved data directory and expose no required WAL/output mechanics on its happy-path help. Hidden compatibility/advanced path probes may retain `--wal-dir`/`--output`. For a nonexistent data directory, doctor SHALL inspect the nearest existing ancestor and report prospective creation/private-mode status as supported, unsupported, or `not_checked`; it SHALL NOT claim a conclusive write/private-mode probe without performing one. Platform/protocol/replay-policy probes continue independently. Doctor SHALL create no directory, recording metadata, repair, capture, replay traffic, or persistent probe artifact.

#### Scenario: Prospective path cannot be proved

- **WHEN** target data directory does not exist and ancestor metadata cannot prove creation/private-mode support
- **THEN** doctor reports `not_checked` with remediation rather than claiming writable

#### Scenario: Public doctor help

- **WHEN** user runs `chronicle doctor --help`
- **THEN** default data-directory diagnostics are shown and WAL/output mechanics remain hidden

#### Scenario: Doctor supported

- **WHEN** supported environment passes required probes
- **THEN** command exits 0 with full supported report

#### Scenario: Doctor unsupported

- **WHEN** required capture/storage capability is unsupported or required probe is `not_checked`
- **THEN** command emits full report/remediation and exits 4 while independent probes still run

### Requirement: Versioned command JSON contracts

Documented public `record`, `list`, `inspect`, `replay`, and `doctor` machine-readable outputs that are declared stable SHALL use one deterministic version-1 schema per command contract and SHALL remain compatible within 0.1.x. Public JSON SHALL contain no human prose or captured sensitive values. Hidden `internal` output, advisory diagnostics, and other non-public reports are not compatibility-sensitive unless an active specification explicitly declares them public. Any public schema version increase, field-meaning change, or incompatible output change SHALL follow `public-compatibility-boundary` and an explicit OpenSpec change; no ad hoc v2 dispatch or fallback is permitted.

#### Scenario: New public reports

- **WHEN** a public command emits JSON under a documented stable contract
- **THEN** stdout is one valid deterministic version-1 object matching that command's documented schema

#### Scenario: No ad hoc v2

- **WHEN** implementation adds a public field or changes a public report
- **THEN** it does not introduce v2 or a v1/v2 reader/renderer dispatch without the explicit public compatibility process

#### Scenario: Automation parses outputs

- **WHEN** user or automation runs a public command with JSON format
- **THEN** stdout matches one documented deterministic v1 schema, contains no human prose or sensitive values, and hidden/advisory output is not misrepresented as public

### Requirement: Foreground recorder command

CLI SHALL provide `recorder --config FILE` as foreground long-running service command. It SHALL parse only configuration path and global output/log options, delegate configuration/preflight/recovery/lifecycle/WAL/ETL/store work to application services, remain attached to foreground, and rely on service manager for daemonization/restart. It SHALL NOT accept captured secrets on command line, fork, create authoritative PID file, add custom control socket/server, or run protocol/recovery logic in CLI.

Command SHALL emit one bounded startup result after capture readiness or terminal startup failure, including separate capture-readiness and processing-readiness fields. Normal runtime progress SHALL use safe structured logs/atomic status state rather than unbounded stdout JSON stream. Graceful SIGINT/SIGTERM success SHALL exit 0; startup/runtime/recovery/storage/finalization failure SHALL exit 3; unsupported platform/preflight SHALL exit 4; Clap usage SHALL exit 2.

#### Scenario: Recorder reaches readiness

- **WHEN** valid supported configuration starts and recovery/attachment/ETL resume succeed
- **THEN** command remains foreground, emits safe ready result, and captures continuously

#### Scenario: Invalid configuration

- **WHEN** configuration version/bounds/scope/store are invalid
- **THEN** command exits before capture with usage or typed application error and no partial success output

#### Scenario: Service manager termination

- **WHEN** SIGTERM arrives and graceful drain succeeds
- **THEN** recorder exits 0 with termination-signal state after final WAL/checkpoint/metadata ordering

### Requirement: Non-mutating recorder status command

CLI SHALL provide `recorder-status --state-root DIR` with global human/JSON format. It SHALL delegate to application diagnostics, acquire no writer ownership, perform no repair/cleanup/ETL/capture/replay, and render bounded versioned health, liveness, capture-readiness, processing-readiness, overall-health, lifecycle, watermark, lag, quota, counter, and recovery fields. If live owner cannot be proven or state is stale/contradictory, output SHALL say so without promoting metadata claims.

#### Scenario: Healthy live recorder

- **WHEN** status targets state root with live owner and fresh validated state
- **THEN** output reports healthy/ready plus safe IDs, watermarks, lag, quota, and counters

#### Scenario: Stale running state

- **WHEN** no owner holds lease but persisted lifecycle says running
- **THEN** command reports crash-recovery-required and exits data-error/nonhealthy status without mutation

#### Scenario: JSON status

- **WHEN** user requests JSON
- **THEN** stdout is one valid bounded versioned object through existing atomic renderer

### Requirement: One-shot commands remain available

Existing fixture/production `record`, standalone `etl`, `doctor`, `inspect`, and `replay` commands and their safety/exit contracts SHALL remain supported. `record` and standalone `etl` SHALL reject mutation against a WAL/store filesystem domain owned by a live recorder; read-only `doctor`, `inspect`, and `replay` may continue. `record` SHALL remain bounded and SHALL NOT silently become daemon mode. `etl` SHALL continue to process stopped recording epochs; incremental ETL lifecycle belongs to recorder application service. Replay defaults and authorization SHALL remain unchanged.

#### Scenario: Existing production record

- **WHEN** operator invokes bounded production record syntax
- **THEN** command retains current duration/WAL limits, signal behavior, summary, and no implicit daemon/incremental ETL

#### Scenario: Existing standalone ETL

- **WHEN** stopped epoch is passed to `etl`
- **THEN** one-shot deterministic publication remains available and equivalent to finalized incremental result

### Requirement: Recorder CLI and documentation acceptance

CLI contract tests SHALL cover recorder/status grammar, atomic startup/status JSON, signal exits, stale ownership, failure mapping, safe output, and unchanged one-shot commands. Operations docs SHALL include systemd foreground invocation, private configuration, status, stop/restart, recovery, quota/retention, and supported-host example. Privileged acceptance SHALL invoke recorder under recommended service placement and retain acceptance fingerprint, source provenance, exact command/config digest/output/evidence, and environment compatibility; fixture tests SHALL not claim daemon eBPF acceptance. Exact commit/tree identity is release-only.

#### Scenario: CLI regression suite

- **WHEN** rootless CLI tests run
- **THEN** new commands pass and existing record/etl/inspect/replay/doctor contracts remain unchanged

#### Scenario: Privileged service command

- **WHEN** supported Linux acceptance starts `recorder --config FILE` under systemd
- **THEN** command reaches readiness, handles stop/restart, and retains machine-readable lifecycle evidence tied to commit/environment
