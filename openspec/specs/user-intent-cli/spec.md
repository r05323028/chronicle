## Purpose

Public CLI designed around user intent: `record`, `replay`, `list`, `inspect`, `doctor`; record and replay execution models; deprecation and compatibility behavior; human/JSON output and exit contracts.

## Requirements

### Requirement: Public command hierarchy

CLI SHALL expose exactly five public top-level commands: `record`, `replay`, `list`, `inspect`, and `doctor`; hidden `internal` and deprecated aliases do not count as public. `record` and `replay` accept a trailing command after `--`; `record --retry RECORDING` is a mutually exclusive recovery/finalization mode. Global `--format human|json` and `--data-dir` remain public global options, default human. Help SHALL describe intent and hide WAL/segment/ETL/daemon mechanics. Replay help SHALL show that command mode infers Read authorization but still accepts/requires `--allow-write` when executable Write operations exist.

#### Scenario: Happy-path record help

- **WHEN** user runs `chronicle record --help`
- **THEN** help shows command execution, `--retry`, `--name`, `--duration`, `--pid`, `--cgroup`, and global options, and omits `--source`, `--wal-dir`, `--root`, and `--segment-bytes`

#### Scenario: Happy-path replay help

- **WHEN** user runs `chronicle replay --help`
- **THEN** help shows `replay RECORDING [--allow-write] -- COMMAND...` and explicit `--target` mode; command mode omits target/host/read/execute gates but explains Write remains explicit

#### Scenario: Global options

- **WHEN** user passes `--format json` or `--data-dir DIR` with any public command
- **THEN** output format and storage root resolve accordingly

### Requirement: Record command execution

`chronicle record [--name NAME] [--duration DURATION] -- COMMAND...` SHALL run the command in a supervised cgroup-v2 scope and produce one published recording. Target-independent platform/build/capability/cgroup/data-path preflight SHALL complete before durable ID/sidecar/WAL/catalog or target creation. After exact domain-lock acquisition and mutable revalidation, Chronicle creates the scope, persists caller-allocated identity, blocks a bootstrap before direct target exec, attaches capture, then drops to trusted invoking UID/GID with all capability sets cleared, `no_new_privs`, Chronicle descriptors closed, and signals reset before release. Bootstrap/hardening/exec failure is Chronicle failure, not child exit 127.

No target-executable instruction SHALL run before successful attachment. Chronicle captures observed traffic for the process and descendants remaining in scope; scope is lifecycle/capture accounting, not hostile-code containment, and output SHALL NOT claim otherwise. Human mode may pass through child output. JSON mode SHALL connect child stdout/stderr to the platform null sink so Chronicle stdout/stderr remain atomic and secret-free; discarded child bytes are not retained. On normal exit or handled signal Chronicle runs bounded scope cleanup, capture finalization, ETL, publication, and catalog update, releasing domain lock last. Defaults remain duration 600/max 3600 seconds and WAL default/hard cap 4 GiB with WAL at least segment size.

#### Scenario: Record happy path

- **WHEN** user runs `chronicle record -- ./my-app` and the app exits normally
- **THEN** Chronicle captures the app and descendants, finalizes, publishes, updates the catalog, prints the completion summary, and exits 0

#### Scenario: Graceful interruption

- **WHEN** user presses Ctrl+C while the target runs
- **THEN** Chronicle gracefully shuts down the scope, finalizes WAL/ETL/publication, prints the summary with interrupt reason, and exits 0

#### Scenario: Duration bound

- **WHEN** user passes `--duration 5m` and the app outlives it
- **THEN** Chronicle stops at the bound with the deterministic duration reason and completes finalization

#### Scenario: ETL or publication failure

- **WHEN** recovery-authoritative WAL exists but ETL, publication, or catalog completion fails
- **THEN** Chronicle retains the ID/WAL, marks `recoverable`, exits nonzero, and points to `chronicle record --retry <id>`

#### Scenario: Retry does not recapture

- **WHEN** user runs `chronicle record --retry RECORDING` for a recoverable entry
- **THEN** Chronicle reacquires the exact domain lock, retries recovery/finalization/publication idempotently for the same ID, never starts target/capture, and creates no duplicate session

#### Scenario: Retry conflict

- **WHEN** `--retry` is combined with command, PID, cgroup, name, or duration capture options
- **THEN** Clap rejects with exit 2 and no mutation

#### Scenario: Privilege check before capture

- **WHEN** host/build/privileges/cgroup or secure data path cannot support capture
- **THEN** Chronicle fails before scope, target, ID, sidecar, WAL, catalog entry, or name reservation exists

#### Scenario: Target launch failure

- **WHEN** credential hardening or direct exec fails before target code
- **THEN** Chronicle cleans the owned scope, reports Chronicle failure with exit 3 or 4 by typed cause, and does not report child exit 127

### Requirement: Record advanced selectors

`chronicle record --pid PID` and `chronicle record --cgroup PATH` SHALL record an already-running workload. These selector modes SHALL be mutually exclusive with each other and with command execution. They SHALL attach without terminating the workload and SHALL run the same internal WAL → ETL → publish → catalog workflow as command mode, differing only by not owning or terminating the workload. They SHALL preserve the existing shared-scope acknowledgement requirement for explicit cgroups with multiple direct TGIDs or descendants, and the existing PID direct-TGID safety check. `--cgroup PATH` with a shared scope SHALL still require explicit acknowledgement through the advanced/internal option.

#### Scenario: PID selector

- **WHEN** user runs `chronicle record --pid 1234`
- **THEN** Chronicle attaches to the resolved cgroup, records until bound, detaches without terminating the workload, and returns the recording ID

#### Scenario: Cgroup selector

- **WHEN** user runs `chronicle record --cgroup /workload`
- **THEN** Chronicle attaches to the exact subtree, records until bound without terminating the workload, then finalizes, publishes, and updates the catalog like command mode

#### Scenario: Selector conflict

- **WHEN** user passes a command after `--` together with `--pid` or `--cgroup`
- **THEN** Clap/application rejects with usage exit 2 and no capture starts

### Requirement: Replay command execution

`chronicle replay <RECORDING> [--allow-write] -- COMMAND...` SHALL first resolve/hydrate and run every target-independent integrity/replayability/effect gate; predictable denial returns a complete zero-traffic report without spawning target code. Remaining command mode is Linux-only and requires a supervisor-controlled cgroup-v2 scope whose membership controls are unavailable to dropped target credentials; otherwise preflight directs user to portable `--target` mode. Scope is lifecycle/ownership evidence, not a general sandbox.

Discovery SHALL join stable process start-time + FD socket-inode evidence from current scope members against `/proc/net/tcp{,6}`, keep exact loopback endpoints matching recorded protocol/port evidence, and require one unique endpoint. `/proc/<pid>/net/tcp*` alone is insufficient. Evidence SHALL be revalidated immediately before connection; mutation restarts bounded discovery or fails with zero traffic. Exact address family SHALL be preserved, including bracketed IPv6; IPv6 evidence never authorizes IPv4. Chronicle uses existing planner/executor/verifier only and bounded-terminates/reaps the scope on every normal outcome. JSON child output isolation matches record mode; pre-existing `--target` workloads are never terminated.

#### Scenario: Replay happy path

- **WHEN** user runs `chronicle replay rec_00000000-0000-0000-0000-000000000001 -- ./my-app` and the app starts and listens on the recorded loopback port
- **THEN** Chronicle spawns the app, discovers the unique loopback listener, replays, verifies, prints the PASS/FAIL summary, and exits per the replay exit contract

#### Scenario: No listener found

- **WHEN** the spawned app does not open a matching listener within the readiness window
- **THEN** Chronicle fails preflight with an actionable message and sends zero replay traffic

#### Scenario: Ambiguous listeners

- **WHEN** the spawned scope owns more than one listener matching recorded protocol/port evidence
- **THEN** Chronicle fails preflight with an actionable ambiguity message and sends zero replay traffic

### Requirement: Replay target mode

`chronicle replay <RECORDING> --target URL` SHALL replay against an already-running application. Target mode SHALL be mutually exclusive with command mode. Explicit target replay SHALL continue to require the full existing authorization gates with the exact syntax `chronicle replay REC --target http://127.0.0.1:8080 --execute --allow-host 127.0.0.1 --allow-read` for read-only replay, with `--allow-write` for write replay. Effect flags SHALL be independent: a read-only recording SHALL require `--allow-read`, a write-only recording SHALL require `--allow-write`, and a recording containing both Read and Write operations SHALL require both flags; any missing required flag SHALL stop replay with `stopped_policy` and zero traffic. Explicit loopback target, exact host authorization, effect authorization, and explicit execution acknowledgement SHALL all be required. Authentication, publication, and unknown effects SHALL remain denied. Recorded destinations SHALL remain forbidden in both modes, and pre-existing workloads targeted via `--target` SHALL never be terminated by Chronicle.

#### Scenario: Target mode replay

- **WHEN** user runs `chronicle replay rec_00000000-0000-0000-0000-000000000001 --target http://127.0.0.1:8080` with explicit authorization flags
- **THEN** Chronicle replays only to the supplied loopback target and reports the outcome

#### Scenario: Mode conflict

- **WHEN** user passes both `-- COMMAND...` and `--target URL`
- **THEN** Clap/application rejects with usage exit 2 and no replay traffic occurs

### Requirement: List command

`chronicle list` SHALL enumerate at most the catalog hard limit (10,000) and fail safely on oversized/unsafe input, including in-progress/recoverable/failed/inconsistent entries with STATUS, and SHALL NOT expose WAL/checkpoint internals by default. Human output SHALL be a readable table with columns ID, NAME, CREATED, DURATION, SESSIONS, OPERATIONS, and STATUS, ordered by creation time with newest first, tie-broken by recording ID. SESSIONS SHALL be the number of published canonical sessions for the recording (one for the 0.1.x one-recording/one-session model); OPERATIONS SHALL be the total canonical operation count. For unpublished entries human output SHALL render DURATION as `—`, SESSIONS as `0`, and OPERATIONS as `0`, while JSON SHALL emit `duration: null`, `sessions: 0`, and `operations: 0` and SHALL never emit the dash character as a JSON value; CREATED SHALL always use the persisted allocation timestamp. Machine-readable output SHALL use the global `--format json` contract with one versioned object containing the same fields. Empty storage SHALL produce an empty list with exit 0 and a helpful hint in human mode.

#### Scenario: List populated

- **WHEN** storage contains published recordings
- **THEN** human output shows one row per recording with ID/NAME/CREATED/DURATION/SESSIONS/OPERATIONS/STATUS and no WAL paths or segment details

#### Scenario: List empty

- **WHEN** storage contains no recordings
- **THEN** human output shows an empty-state message and a hint to run `chronicle record -- ...`, and exits 0

#### Scenario: List JSON

- **WHEN** user requests `--format json list`
- **THEN** stdout is one valid versioned JSON object with deterministic ordering

### Requirement: Inspect command

`chronicle inspect <RECORDING>` and `chronicle inspect latest` SHALL resolve the recording from default or configured storage and SHALL NOT require `--root`. Human output SHALL prioritize recording metadata, duration, operation/session counts, protocols, endpoints, loss/incompleteness warnings, and replay eligibility. JSON output SHALL use the existing versioned inspect JSON contract extended with recording identity fields. Unknown recording SHALL exit 3 with a stable not-found error; invalid recording syntax SHALL exit 3 without path access.

#### Scenario: Inspect by ID

- **WHEN** user runs `chronicle inspect rec_00000000-0000-0000-0000-000000000001`
- **THEN** command prints the prioritized human summary and exits 0

#### Scenario: Inspect latest

- **WHEN** user runs `chronicle inspect latest` and at least one published recording exists
- **THEN** command resolves and inspects the newest published recording and exits 0

#### Scenario: Inspect unknown

- **WHEN** recording ID does not resolve
- **THEN** command exits 3 with a stable not-found error naming the unresolved reference

### Requirement: Bounded spawned-target supervision

Record attachment readiness and replay listener readiness SHALL each have a 30-second hard deadline. Replay discovery SHALL poll at most every 100 ms and accept at most five unstable snapshot retries within that deadline. Normal scope shutdown SHALL send TERM, wait at most 5 seconds, send KILL when still populated, then wait at most 5 seconds; cleanup bookkeeping SHALL stop after a 15-second absolute deadline. Readiness timeout exits 4 with zero target traffic; cleanup timeout exits 3, preserves recording/replay result evidence, reports possible orphan state, and never claims cleanup success.

In JSON mode child stdout/stderr SHALL be connected directly to the platform null sink before exec; Chronicle SHALL create no child-output file, buffer, or counter and child bytes SHALL never enter Chronicle stdout/stderr. Human pass-through remains controlled by caller terminal/pipe.

#### Scenario: Listener deadline

- **WHEN** replay target never opens a matching listener or continually destabilizes discovery
- **THEN** readiness decision occurs within 30 seconds, followed by at most 15 seconds of cleanup (45 seconds total), with exit 4 if cleanup succeeds, zero replay traffic, and bounded snapshot attempts

#### Scenario: Resistant child cleanup

- **WHEN** spawned target ignores TERM
- **THEN** Chronicle escalates to KILL, waits within stated bounds, and reports cleanup failure/orphan risk with exit 3 if scope remains populated

#### Scenario: Noisy JSON child

- **WHEN** child writes arbitrary-volume output in JSON mode
- **THEN** bytes go to null sink, Chronicle emits one atomic JSON result, and no child-output file or unbounded buffer exists

### Requirement: Deprecated and internal entrypoints

Legacy forms SHALL remain available during 0.1.x as hidden deprecated compatibility entrypoints: top-level `recorder`, `recorder-status`, and `etl` as aliases of the explicit internal namespace `chronicle internal recorder`, `internal recorder-status`, `internal etl`, and `internal record-fixture`; legacy `record --source fixture|ebpf` with `--wal-dir`, `--root`, `--segment-bytes`, and related flags; legacy `replay SESSION --root ROOT --target ... --allow-host ... [--allow-read] [--allow-write] [--timing] [--execute]`; and legacy `inspect SESSION --root ROOT`. They SHALL be omitted from normal help, SHALL route into the same application services as the new commands (no duplicate business logic), SHALL emit a deprecation warning, and SHALL remain non-breaking for existing scripts and acceptance flows through 0.1.x. In human mode the warning SHALL be a stderr line; in JSON mode a successful legacy command SHALL keep its stdout object unchanged and atomic and emit exactly one versioned structured deprecation diagnostic object on stderr, while a failed legacy command SHALL emit the normal single error JSON on stderr whose message includes the replacement hint and no separate warning object. The deprecation diagnostic SHALL be `{"version":1,"level":"warning","code":"deprecated_cli","invocation":"<fixed-form-id>","replacement":"<safe-syntax-template>"}`; neither field may contain raw argv, credentials, target URLs with userinfo, or child command text. Removal SHALL target the 0.2 boundary after documentation, systemd/deployment scripts, and acceptance flows migrate. Errors from compatibility entrypoints SHALL point users toward the new syntax.

#### Scenario: Legacy record with warning

- **WHEN** a script invokes legacy `chronicle record --source ebpf --cgroup /w --wal-dir /x`
- **THEN** the command still works, prints a deprecation warning to stderr in human mode, and continues to produce the same recording semantics

#### Scenario: Legacy JSON stays atomic

- **WHEN** a script invokes a successful legacy command with `--format json`
- **THEN** stdout remains one unchanged valid JSON object and exactly one versioned structured deprecation diagnostic appears on stderr

#### Scenario: Legacy JSON failure with hint

- **WHEN** a legacy command with `--format json` fails
- **THEN** stderr carries the single normal error JSON whose message includes the new-syntax hint, and no separate warning object is emitted

#### Scenario: Legacy help hidden

- **WHEN** user inspects `chronicle --help`
- **THEN** deprecated entrypoints do not appear in normal help

#### Scenario: New syntax error hint

- **WHEN** a compatibility entrypoint receives an invalid legacy invocation
- **THEN** error output points to the equivalent new syntax

### Requirement: Human and JSON output contract

Human output SHALL be concise, readable, and oriented around user actions. Successful record SHALL print this shape (values illustrative; raw argv never substituted):

```text
Recording complete.
  id: rec_00000000-0000-0000-0000-000000000001
  duration: 1m 2s
  operations: 3
  dropped: 0
Try:
  chronicle inspect rec_00000000-0000-0000-0000-000000000001
  chronicle replay rec_00000000-0000-0000-0000-000000000001 -- COMMAND...
```

Replay SHALL print operation counts, `✓ passed`/`✗ failed`, and final `Replay passed.`/`Replay failed.`. Untrusted names/references SHALL be control-escaped in human output. Mutable-v1 policy remains authoritative: public record, public recording-oriented inspect, list, replay, doctor, and hidden legacy reports each use their documented v1 command contract; this change introduces no v2/version dispatch/freeze. Public record v1 includes canonical `recording_id`, nullable `name`, stable status/reason, `duration_ms`, operation/drop totals, existing categorized counters, and nullable structured child result. JSON mode reserves stdout/stderr for one Chronicle object/diagnostic and redirects child output; serialization completes before output.

#### Scenario: Record human output

- **WHEN** record completes in human mode
- **THEN** stdout contains the id, duration, operations, and dropped lines and the Try block, with no internal paths

#### Scenario: Replay human output

- **WHEN** replay completes in human mode
- **THEN** stdout contains the operation counts, pass/fail lines, and final Replay passed./failed. line

#### Scenario: Atomic JSON error

- **WHEN** a JSON command hits a typed application error
- **THEN** stdout has no partial object and stderr carries the versioned JSON error

### Requirement: Exit and child-process result contract

CLI SHALL preserve the existing exit-code family: 0 for success including handled signal stops and clean limits, doctor supported/warnings, replay dry-run, `completed`, and `completed_with_skips`; 2 for Clap usage; 3 for recording/WAL/ETL/session/storage/serialization/finalization/resolution data errors; 4 for replay target/effect/no-executable invalid-session denial, unsupported live preflight, or doctor required unsupported/not-checked; 5 for replay transport stop; 6 for executed verification failure. A target application exit status/signal after successful exec SHALL be reported factually and not forwarded. Bootstrap, credential-drop, descriptor/signal setup, or `execve` failure SHALL be a typed Chronicle failure, never synthesized as child exit 127. New commands SHALL reuse this family: `list` and `inspect` resolution failures SHALL exit 3.

#### Scenario: Child exit reported, not forwarded

- **WHEN** the recorded command exits nonzero or by signal
- **THEN** Chronicle still completes recording and exits 0, and the summary reports the child's exit status or signal factually

#### Scenario: New command exit mapping

- **WHEN** `list`, `inspect`, or `doctor` fails in a typed way
- **THEN** the failure maps to the preserved 3/4 family and no internal exit code leaks
