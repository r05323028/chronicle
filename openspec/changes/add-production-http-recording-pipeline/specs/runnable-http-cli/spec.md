## MODIFIED Requirements

### Requirement: Minimal record command
CLI SHALL preserve fixture form `record --source fixture --input FILE --root ROOT` and add production form `record --source ebpf (--pid PID | --cgroup PATH) --wal-dir DIR [--segment-bytes N]`. Source-specific required/conflicting options SHALL be validated by CLI/application. Production command SHALL perform preflight, create/finalize recording metadata, start eBPF source and bounded WAL writer, handle SIGINT/SIGTERM, and print safe summary; it SHALL NOT run ETL implicitly.

#### Scenario: Fixture record compatibility
- **WHEN** existing valid P0 fixture command runs
- **THEN** it retains fixture -> WAL -> ETL -> publication behavior and output compatibility

#### Scenario: Production record happy path
- **WHEN** supported privileged Linux host records valid dedicated workload selector
- **THEN** command writes durable WAL/metadata, stops gracefully, and outputs recording ID/status/counters

#### Scenario: Record argument validation
- **WHEN** selector is missing/multiple or fixture/eBPF options are mixed
- **THEN** Clap/application rejects with usage exit 2 and capture does not start

#### Scenario: Unsupported platform or privilege
- **WHEN** eBPF source is requested on unsupported host or without privilege
- **THEN** command exits non-success with stable safe diagnostic and no successful recording

#### Scenario: Graceful SIGINT
- **WHEN** SIGINT arrives during active production record
- **THEN** command drains/detaches/flushes, marks interrupted, prints final summary, and preserves ETL-ready WAL

### Requirement: Minimal replay command
CLI SHALL preserve `replay SESSION_ID --root ROOT --target ORIGIN --allow-host IP [--allow-read] [--allow-write] [--timing asap] [--execute]`. Defaults SHALL remain human output, immediate timing, dry-run, no effect authorization, no target default, and no network. Human/JSON output SHALL include explicit replay outcome, aggregate counts, and one result per operation.

#### Scenario: Replay dry-run
- **WHEN** valid session is replayed without execute
- **THEN** command prints plan/report with dry-run outcome and zero network

#### Scenario: Replay denied
- **WHEN** target/effect/session preflight denies execution
- **THEN** no request is sent, report includes every unattempted operation, and execution request exits 4

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
CLI SHALL use exit 0 for successful record/ETL/inspect, doctor supported/warnings, graceful SIGINT with successful flush, replay dry-run, or completed replay with only Passed/Skipped; 2 for Clap usage; 3 for recording/WAL/ETL/session/storage/serialization/finalization data errors; 4 for replay safety/invalid-session denial, unsupported live platform preflight, or doctor required unsupported/not-checked; 5 for replay transport stop; 6 for completed verification Failed/Inconclusive/Unsupported; and 143 for gracefully handled SIGTERM after successful flush. Human success SHALL use stdout and errors stderr. JSON success/error SHALL be valid versioned JSON with stable code/message and no payload/header values. Commands SHALL not emit partial invalid JSON.

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
- **WHEN** replay report outcome is stopped-by-transport after prior success
- **THEN** full report is emitted and process exits 5

#### Scenario: Doctor unresolved required probe
- **WHEN** doctor aggregate is unsupported or required not-checked
- **THEN** full diagnostic report is emitted and process exits 4

#### Scenario: Recording signal exits
- **WHEN** SIGINT or SIGTERM graceful flush succeeds
- **THEN** interrupted summary is emitted and process exits 0 for SIGINT or 143 for SIGTERM

#### Scenario: Recording finalization failure
- **WHEN** signal shutdown cannot flush WAL or finalize metadata
- **THEN** safe failed summary is emitted and process exits 3

### Requirement: CLI remains outer adapter
HTTP/TCP parsing, canonicalization, capture attachment, WAL recovery, ETL checkpointing, storage, signal lifecycle, safety decisions, network execution, verification, and doctor probes SHALL live outside CLI crate. CLI SHALL parse arguments, map commands, render application results, and map typed outcomes/errors to exit codes only. `etl` and `doctor` SHALL be runnable application services rather than scaffold responses.

#### Scenario: Protocol logic audit
- **WHEN** CLI implementation is reviewed
- **THEN** no protocol decoder, eBPF loader details, WAL scanner, filesystem publisher, replay policy engine, or verifier lives in CLI

#### Scenario: Doctor delegation
- **WHEN** doctor command runs
- **THEN** CLI renders typed application diagnostic results instead of hard-coded success text

### Requirement: Vertical integration proof
Rootless integration SHALL exercise fixture capture events through recoverable WAL/ETL/canonical publication/inspect/replay/verification without claiming eBPF. Separate privileged Linux acceptance SHALL exercise real workload traffic, recording interruption, WAL recovery, ETL idempotency, inspect, authorized replay, and partial replay failure while proving original destination is not contacted.

#### Scenario: Rootless full pipeline
- **WHEN** ordinary workspace integration suite runs
- **THEN** deterministic capture-event fixtures prove WAL/reconstruction/HTTP/storage/replay boundaries without privileges

#### Scenario: Real Linux eBPF acceptance
- **WHEN** privileged acceptance profile runs on documented supported Linux host
- **THEN** observed real HTTP traffic passes complete required end-to-end scenario and artifacts/reports are retained

#### Scenario: Coverage labeling
- **WHEN** fixture-only CI passes but privileged profile did not run
- **THEN** documentation/status reports eBPF acceptance not checked, not passed

### Requirement: Required test coverage
Implementation SHALL include WAL framing/rotation/recovery/corruption/reopen/failure/queue tests; stream reconstruction split/order/reuse/gap/truncation/direction tests; HTTP framing/pairing/completeness tests; ETL first/idempotent/restart/corruption/publication tests; CLI record/ETL/inspect/doctor/replay/signal/JSON/exit tests; and replay complete/first-or-middle-failure/policy/target/redirect/timeout/verification tests. Tests SHALL assert no sensitive values in default output.

#### Scenario: Unit and integration suites
- **WHEN** rootless workspace tests run
- **THEN** all specified non-eBPF cases pass without Docker, root, fixed port, or Internet

#### Scenario: Privileged profile
- **WHEN** privileged Linux profile is explicitly enabled
- **THEN** real eBPF load/attach/capture/loss/shutdown/recovery scenario runs separately

### Requirement: Documentation and validation gates
README, architecture, canonical, replay safety, WAL format, capability matrix, and new P1 operations guide SHALL document supported platform/kernel/privileges, record/WAL/recovery/ETL/inspect/replay workflow, JSON schemas, limits, known exclusions, sensitive-data warning, local demo, CI versus privileged tests, and doctor remediation. OpenSpec and Rust gates SHALL pass without overstating eBPF or production scale.

#### Scenario: Runnable local documentation
- **WHEN** user follows supported Linux example
- **THEN** commands create dedicated workload, record, recover/ETL, inspect, replay only to authorized local target, and show reports

#### Scenario: Repository gates
- **WHEN** implementation is ready for review
- **THEN** fmt, locked workspace check, Clippy all-features, and locked all-features tests pass plus Linux eBPF compile profile

#### Scenario: Strict OpenSpec validation
- **WHEN** change artifacts are complete or implementation tasks are updated
- **THEN** `openspec validate add-production-http-recording-pipeline --strict` passes and checkboxes reflect reality

## ADDED Requirements

### Requirement: Standalone ETL command
CLI SHALL provide `etl --wal-dir DIR --output ROOT` with global human/JSON format. Application SHALL recover/validate recording, process deterministic stopped snapshot, atomically publish session, update checkpoint after publication, and output session/checkpoint/completeness/counters. Repeated run SHALL report already processed without duplicate.

#### Scenario: First ETL run
- **WHEN** valid stopped recording WAL is processed
- **THEN** command publishes one session and prints deterministic ID/checkpoint summary

#### Scenario: Second ETL run
- **WHEN** same unchanged WAL/output is processed again
- **THEN** command reports already processed with same ID and no duplicate session

#### Scenario: Corrupt WAL ETL
- **WHEN** recovery identifies non-tail corruption or unsupported version
- **THEN** command exits 3, publishes nothing, and emits safe recovery summary

### Requirement: Operational doctor command
CLI SHALL provide `doctor` with optional WAL/output paths and global human/JSON format. It SHALL render all typed probes and aggregate result, continue independent checks after failures, and perform no recording/replay/repair side effects.

#### Scenario: Doctor supported
- **WHEN** supported environment passes all required probes
- **THEN** command exits 0 with supported report

#### Scenario: Doctor unsupported
- **WHEN** required capture/storage capability is absent
- **THEN** command emits full report, remediation, and exit 4; same exit applies when required probe is not-checked

### Requirement: Versioned command JSON contracts
Production record, ETL, inspect, replay, and doctor success outputs SHALL each use documented versioned JSON object. Arrays SHALL follow deterministic operation/probe/provenance ordering and counters SHALL use stable names. Schema changes SHALL increment output version rather than silently change field meaning.

#### Scenario: Automation parses outputs
- **WHEN** commands run with `--format json`
- **THEN** each stdout object matches documented command schema and contains no human prose outside JSON
