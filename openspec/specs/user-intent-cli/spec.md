## Purpose

Public CLI designed around user intent: `record`, `replay`, `list`, `inspect`, `doctor`; record and replay execution models; deprecation and compatibility behavior; human/JSON output and exit contracts.

## Requirements

### Requirement: Public command hierarchy

CLI SHALL expose exactly five public top-level commands: `record`, `replay`, `list`, `inspect`, and `doctor`; hidden `internal` entrypoints do not count as public. `record` and `replay` accept a trailing command after `--`; `record --retry RECORDING` is a mutually exclusive recovery/finalization mode. Global `--format human|json` and `--data-dir` remain public global options, default human. Help SHALL describe intent and hide WAL/segment/ETL/daemon mechanics. Replay help SHALL show that command mode infers Read authorization but still accepts/requires `--allow-write` when executable Write operations exist.

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

`chronicle record [--name NAME] [--duration DURATION] -- COMMAND...` SHALL run the command in a supervised cgroup-v2 scope and produce one parent recording that may contain multiple bounded epochs. Target-independent platform/build/capability/cgroup/data-path preflight SHALL complete before durable parent ID/sidecar/WAL/catalog or target creation. After exact domain-lock acquisition and mutable revalidation, Chronicle creates the scope, persists one caller-allocated parent identity, blocks a bootstrap before direct target exec, attaches capture, then drops to trusted invoking UID/GID with existing capability, descriptor, and signal hardening before release. Bootstrap/hardening/exec failure is Chronicle failure, not child exit 127.

No target-executable instruction SHALL run before successful attachment. Chronicle captures observed traffic for the process and descendants remaining in scope; scope is lifecycle/capture accounting, not hostile-code containment. Human mode may pass through child output. JSON mode SHALL connect child stdout/stderr to the platform null sink so Chronicle output remains atomic and secret-free. On child completion, user stop, explicit whole-run deadline, or fatal failure, Chronicle runs bounded scope cleanup, finalizes the active epoch and parent lineage, drains required epoch ETL/publication, and updates the parent catalog under one domain lock.

`--duration` is optional and is a whole-recording deadline. When omitted, there is no implicit 600-second or one-hour recording stop. Epoch age/byte thresholds only roll to a successor. The explicit duration parser SHALL accept checked positive values such as `10m` and `24h`, reject zero/overflow, and SHALL not apply the epoch maximum as a recording-wide parser cap. The recording's epoch and segment bounds remain configured operational limits.

#### Scenario: Command runs until child completion

- **WHEN** user runs `chronicle record -- ./my-app` and the app exits before any deadline
- **THEN** Chronicle stops with `source_completed`, finalizes all epochs, publishes the parent aggregate, prints the completion summary, and exits 0

#### Scenario: Command crosses epochs

- **WHEN** the child remains alive across one or more epoch age/byte thresholds
- **THEN** Chronicle keeps the child and capture source running, rolls epochs, preserves one parent ID, and does not stop because an epoch bound was reached

#### Scenario: Explicit whole-run duration

- **WHEN** user runs `chronicle record --duration 10m -- ./my-app` and the child outlives ten minutes
- **THEN** Chronicle stops the whole run with deterministic `duration_limit`, performs supervised cleanup, finalizes the current epoch and parent lineage, and does not interpret the deadline as an epoch limit

#### Scenario: Ctrl-C stop

- **WHEN** user presses Ctrl-C while the target runs
- **THEN** Chronicle drains once with `user_interrupt`, performs existing bounded TERM/KILL cleanup for the owned scope, finalizes the parent, and exits according to handled-signal success semantics

#### Scenario: ETL or publication failure

- **WHEN** recovery-authoritative epoch WAL exists but an epoch ETL, parent publication, or catalog completion fails
- **THEN** Chronicle retains the parent/epoch IDs and protected evidence, marks the run recoverable, exits nonzero, and points to the no-recapture retry path

#### Scenario: Retry does not recapture

- **WHEN** user retries a recoverable parent recording
- **THEN** Chronicle reacquires the exact domain lock, repairs/retries epoch and parent ETL/publication idempotently for the same parent ID, never starts target/capture, and creates no duplicate session or epoch

#### Scenario: Retry conflict

- **WHEN** `--retry` is combined with command, PID, cgroup, name, or duration capture options
- **THEN** Clap rejects with exit 2 and no mutation occurs

#### Scenario: Privilege check before capture

- **WHEN** host/build/privileges/cgroup or secure data path cannot support capture
- **THEN** Chronicle fails before scope, parent ID, sidecar, WAL, catalog entry, or target creation

#### Scenario: Target launch failure

- **WHEN** credential hardening or direct exec fails before target code
- **THEN** Chronicle cleans the owned scope, reports Chronicle failure with exit 3 or 4 by typed cause, and does not report child exit 127

#### Scenario: Record happy path

- **WHEN** user runs `chronicle record -- ./my-app` and the app exits normally
- **THEN** Chronicle captures the app and descendants, finalizes, publishes, updates the catalog, prints the completion summary, and exits 0

#### Scenario: Graceful interruption

- **WHEN** user presses Ctrl+C while the target runs
- **THEN** Chronicle gracefully shuts down the scope, finalizes WAL/ETL/publication, prints the summary with interrupt reason, and exits 0

#### Scenario: Duration bound

- **WHEN** user passes `--duration 5m` and the app outlives it
- **THEN** Chronicle stops at the bound with the deterministic duration reason and completes finalization

### Requirement: Record advanced selectors

`chronicle record --pid PID` and `chronicle record --cgroup PATH` SHALL record an already-running workload through the same parent/epoch/segment orchestration as command mode. These selector modes SHALL be mutually exclusive with each other and with command execution. Without `--duration`, they SHALL remain capable of running indefinitely until explicit stop, source/capture failure, unrecoverable storage/quota failure, or administrative end. With `--duration`, the deadline applies to the whole parent run and is not reset at epoch rollover.

Selector modes SHALL attach without terminating the workload, use the same WAL -> per-epoch ETL -> immutable publication -> parent catalog workflow as command mode, and differ only in not owning or terminating the workload. On normal stop, duration, or failure Chronicle SHALL detach/stop capture and perform bounded finalization but SHALL never send TERM/KILL to the selected PID or any process in the selected cgroup subtree. Existing shared-scope acknowledgement and PID direct-TGID safety checks remain unchanged.

#### Scenario: PID selector without duration

- **WHEN** user runs `chronicle record --pid 1234` without `--duration`
- **THEN** Chronicle attaches to the resolved subtree, rolls bounded epochs as needed, continues until explicit stop or failure, and leaves PID 1234 and its workload alive after detaching

#### Scenario: Cgroup selector without duration

- **WHEN** user runs `chronicle record --cgroup /workload` without `--duration`
- **THEN** Chronicle records indefinitely through the same parent lineage until explicit stop or failure, and does not terminate any member during finalization

#### Scenario: Selector deadline

- **WHEN** user runs `chronicle record --duration 24h --pid 1234` or the equivalent cgroup command
- **THEN** Chronicle uses one whole-run deadline, may create many epochs, detaches at the deadline, and leaves the selected workload alive

#### Scenario: Selector conflict

- **WHEN** user passes a command after `--` together with `--pid` or `--cgroup`
- **THEN** Clap/application rejects with usage exit 2 and no capture starts

#### Scenario: PID selector

- **WHEN** user runs `chronicle record --pid 1234`
- **THEN** Chronicle attaches to the resolved cgroup, records until bound, detaches without terminating the workload, and returns the recording ID

#### Scenario: Cgroup selector

- **WHEN** user runs `chronicle record --cgroup /workload`
- **THEN** Chronicle attaches to the exact subtree, records until bound without terminating the workload, then finalizes, publishes, and updates the catalog like command mode

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

`chronicle list` SHALL enumerate at most the catalog hard limit and return one row per stable parent recording, including in-progress/recoverable/failed/inconsistent entries with STATUS. Human output SHALL retain ID, NAME, CREATED, DURATION, SESSIONS, OPERATIONS, and STATUS, and SHALL add bounded epoch visibility such as EPOCHS, ACTIVE_EPOCH, and PUBLISHED_EPOCHS without exposing WAL paths by default. JSON SHALL retain the existing versioned object and fields and add deterministic nullable/number fields for `epoch_count`, `active_epoch`, and `published_epoch_count`; consumers must not infer a new recording from each epoch.

SESSIONS and OPERATIONS SHALL aggregate only verified published canonical sessions and operations across epochs. DURATION SHALL be the final parent duration when ended; for an active parent it SHALL be null or a separately named elapsed field, never a false final duration. CREATED SHALL use the parent allocation timestamp. Ordering remains parent creation time newest first, tie-broken by parent ID. Empty storage remains exit 0 with a helpful hint.

#### Scenario: List multi-epoch recording

- **WHEN** storage contains one active parent with two published epochs and one active epoch
- **THEN** human/JSON list returns one parent row with stable ID, aggregate counts, epoch count, active epoch, and in-progress status

#### Scenario: List published aggregate

- **WHEN** a stopped parent has three published epoch sessions
- **THEN** SESSIONS equals three, OPERATIONS equals the sum of epoch operations, and no epoch appears as a separate public recording

#### Scenario: List JSON

- **WHEN** user requests `--format json list`
- **THEN** stdout is one valid versioned JSON object with deterministic parent ordering and safe epoch summary fields

#### Scenario: List while recorder owns domain

- **WHEN** a live recorder is rolling or ETL is processing a previous epoch
- **THEN** list returns bounded read-only reconciled state and does not mutate catalog/epoch files or block capture

#### Scenario: List populated

- **WHEN** storage contains published recordings
- **THEN** human output shows one row per recording with ID/NAME/CREATED/DURATION/SESSIONS/OPERATIONS/STATUS and no WAL paths or segment details

#### Scenario: List empty

- **WHEN** storage contains no recordings
- **THEN** human output shows an empty-state message and a hint to run `chronicle record -- ...`, and exits 0

### Requirement: Inspect command

`chronicle inspect <RECORDING>` and `chronicle inspect latest` SHALL resolve the stable parent recording and SHALL not require `--root`. Human and JSON output SHALL prioritize parent metadata, final/elapsed duration, aggregate operation/session counts, lifecycle/stop reason, replay eligibility, and an ordered bounded epoch summary. Each epoch summary SHALL expose public ordinal, epoch ID, state, committed range/bytes, ETL/checkpoint/publication state, retention/deletion state, completeness, and safe warnings. Inspect SHALL not expose raw WAL paths or payload values by default and SHALL not scan unbounded historical epoch data.

Unknown parent recording SHALL exit 3 with a stable not-found error. A live active epoch may be reported as incomplete/not yet published; inspect must distinguish that mutable tail from a missing, failed, or contradictory middle epoch. Read-only inspect may operate while the exact domain lock is held by the recorder.

#### Scenario: Inspect stable parent

- **WHEN** user inspects a multi-epoch parent by ID, name, or `latest`
- **THEN** output shows one parent identity and deterministic ordered epoch details, not one unrelated recording per rollover

#### Scenario: Inspect active recording

- **WHEN** a recording is still running with finalized published epochs and one active epoch
- **THEN** inspect reports the active tail and published prefix separately without treating the run as fully final or mutating state

#### Scenario: Inspect unknown

- **WHEN** parent recording reference does not resolve
- **THEN** command exits 3 with a stable not-found error naming the safe unresolved reference

#### Scenario: Inspect by ID

- **WHEN** user runs `chronicle inspect rec_00000000-0000-0000-0000-000000000001`
- **THEN** command prints the prioritized human summary and exits 0

#### Scenario: Inspect latest

- **WHEN** user runs `chronicle inspect latest` and at least one published recording exists
- **THEN** command resolves and inspects the newest published recording and exits 0

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

### Requirement: Hidden internal operational entrypoints only

Exactly five public top-level commands exist: `record`, `replay`, `list`, `inspect`, and `doctor`. The hidden `internal` namespace (`internal recorder`, `internal recorder-status`, `internal etl`, `internal record-fixture`, `internal bootstrap`) is the current operational surface for the continuous recorder, recorder status, standalone ETL, and deterministic fixture recording; it carries no deprecation machinery. No hidden pre-release CLI compatibility surface exists: top-level legacy `recorder`/`recorder-status`/`etl`, `record --source ...`, and session-root `--root` forms were removed before 0.1.0 and SHALL NOT be reintroduced without an explicit OpenSpec change.

#### Scenario: Legacy forms are rejected

- **WHEN** a script invokes `record --source fixture`, top-level `recorder`, or `inspect SESSION --root ROOT`
- **THEN** the CLI rejects the invocation as a usage error (exit 2) and no deprecation diagnostic exists

#### Scenario: Internal forms remain operational

- **WHEN** an operator invokes `internal recorder --config FILE`, `internal recorder-status --state-root DIR`, `internal etl --wal-dir WAL --output ROOT`, or `internal record-fixture --input FILE --root ROOT`
- **THEN** the command runs with no warning and emits the documented machine-readable output

### Requirement: Human and JSON output contract

Human output SHALL remain concise and user-oriented. Successful public record SHALL print one stable parent ID, final/elapsed duration, aggregate operations/dropped counts, epoch count or rollover summary, and safe next commands. JSON public record v1 SHALL include stable parent `recording_id`, nullable name, status/reason, duration, aggregate operation/drop totals, existing categorized counters, and deterministic epoch/published-epoch summary fields. Epoch IDs and rollover failures may be included only as bounded safe metadata; payloads, command lines, environment, and arbitrary headers remain excluded.

List/inspect/replay reports SHALL render parent and epoch state deterministically and atomically. Serialization failure SHALL occur before any output. Existing human child pass-through and JSON child null-sink rules remain.

#### Scenario: Parent completion output

- **WHEN** a public recording completes after multiple epochs
- **THEN** human/JSON output contains one parent ID and aggregate counts, with no per-epoch ID substituted as the recording ID

#### Scenario: Rollover failure output

- **WHEN** a successor cannot be durably opened
- **THEN** output reports typed recording failure and rollover-loss counters without claiming a clean duration/WAL-limit stop

#### Scenario: Atomic JSON error

- **WHEN** a JSON command hits a typed application error
- **THEN** stdout has no partial object and stderr carries one versioned safe error object

#### Scenario: Record human output

- **WHEN** record completes in human mode
- **THEN** stdout contains the id, duration, operations, and dropped lines and the Try block, with no internal paths

#### Scenario: Replay human output

- **WHEN** replay completes in human mode
- **THEN** stdout contains the operation counts, pass/fail lines, and final Replay passed./failed. line

### Requirement: Exit and child-process result contract

CLI SHALL preserve the existing exit-code family: 0 for success including handled signal/source/deadline stops and successful parent publication; 2 for usage; 3 for recording/WAL/ETL/session/storage/serialization/finalization/resolution data errors; 4 for replay target/effect/no-executable denial, unsupported live preflight, or required doctor failure; 5 for replay transport stop; and 6 for executed verification failure. Epoch rollover SHALL not itself map to a terminal exit. A quota/lineage/rollover failure SHALL map to the typed recording failure family. A selected PID/cgroup remains outside Chronicle cleanup and is never treated as a child exit.

#### Scenario: Child exit reported, not forwarded

- **WHEN** a supervised child exits nonzero or by signal after successful exec
- **THEN** Chronicle reports the child result factually, completes parent finalization, and does not forward the child status as Chronicle's exit code

#### Scenario: Handled deadline or signal

- **WHEN** a command or selector stops because of explicit duration or first handled signal
- **THEN** the result records the reason, applies mode-specific cleanup, and exits according to the preserved success/failure contract

#### Scenario: Target remains alive

- **WHEN** a PID/cgroup recording ends because of stop, deadline, or Chronicle failure
- **THEN** Chronicle detaches and reports its own result without terminating or claiming a child result for the target

#### Scenario: New command exit mapping

- **WHEN** `list`, `inspect`, or `doctor` fails in a typed way
- **THEN** the failure maps to the preserved 3/4 family and no internal exit code leaks
