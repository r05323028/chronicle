## MODIFIED Requirements

### Requirement: Minimal record command

CLI SHALL retain the legacy record forms as hidden deprecated 0.1.x compatibility entrypoints (see `user-intent-cli`): fixture form `record --source fixture --input FILE --root ROOT` and production form `record --source ebpf (--pid PID | --cgroup PATH [--allow-shared-cgroup]) --wal-dir DIR [--segment-bytes N] [--duration-seconds N] [--max-wal-bytes N]`. Shared acknowledgement SHALL be invalid with `--pid`, fixture source, or no explicit `--cgroup`. Duration SHALL default 600 seconds/max 3600; total WAL SHALL default/hard-cap 4 GiB and be at least segment size. Source-specific/conflicting options SHALL be validated. The compatibility production form SHALL show effective cgroup path/ID/direct TGID count/descendant cgroup count/selected-subtree scope/acknowledgement before attach, perform preflight, create/finalize metadata, run eBPF plus bounded in-WAL-marker group commit, handle limits/signals, and print safe categorized summary; it SHALL NOT run ETL implicitly — the public `record` command runs ETL/finalization automatically per `user-intent-cli`. CLI help and diagnostics SHALL use **direct cgroup TGID set** for distinct host-visible TGIDs resolved from numeric PIDs listed directly in selected node's `cgroup.procs`; they SHALL NOT call it a POSIX process group, thread count, container ID, or descendant union.

#### Scenario: Fixture record uses current artifacts

- **WHEN** valid fixture command runs
- **THEN** it uses same Capture Event v1, segmented WAL v1, reconstruction, Canonical Session v1, manifest v1, and report v1 family as production

#### Scenario: Production record happy path

- **WHEN** supported privileged Linux host records valid dedicated workload selector
- **THEN** command writes bounded group-committed WAL/metadata, stops gracefully, and outputs recording ID/status/reason/last valid commit boundary/categorized counters

#### Scenario: Compatibility form does not run ETL implicitly

- **WHEN** the hidden compatibility production form completes capture
- **THEN** it preserves the ETL-ready WAL and does not run ETL, while the public `record` command finalizes and publishes automatically

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

CLI SHALL retain legacy `inspect SESSION_ID --root ROOT` as a hidden deprecated 0.1.x compatibility entrypoint. The public `inspect <RECORDING>` and `inspect latest` SHALL resolve recordings from default or configured storage per `recording-identity` and SHALL NOT require `--root`. It SHALL support global `--format human|json`, default human, and delegate to application/storage service.

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

CLI SHALL retain legacy `replay SESSION_ID --root ROOT --target ORIGIN --allow-host IP [--allow-read] [--allow-write] [--timing asap] [--execute]` as a hidden deprecated 0.1.x compatibility entrypoint whose defaults SHALL remain human output, immediate timing, dry-run, no effect authorization, no target default, and no network. The public forms SHALL be `replay <RECORDING> -- COMMAND...` (spawned target with inferred local target and authorization per `safe-local-http-replay`) and `replay <RECORDING> --target URL` (explicit target requiring full authorization), mutually exclusive per `user-intent-cli`. Human/JSON output SHALL include exact replay outcome, fully/partially/not replayable classification, aggregate executable/non-executable counts, and one result per operation.

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

Until compatibility freeze, each public or hidden command report contract SHALL remain version 1 per `mvp-schema-versioning`; this change introduces no v2 or version dispatch. New public record/list/recording-oriented-inspect contracts are documented v1 objects. Existing hidden legacy success JSON remains byte-compatible through 0.1.x as its documented v1 contract. Any future version increase requires explicit compatibility-freeze change.

#### Scenario: New public reports

- **WHEN** public record, list, or recording-oriented inspect emits JSON
- **THEN** object declares version 1 and matches its one current documented schema

#### Scenario: No ad hoc v2

- **WHEN** implementation adds recording identity fields
- **THEN** it does not introduce v2 or a v1/v2 reader/renderer dispatch

#### Scenario: Automation parses outputs

- **WHEN** command runs with JSON format
- **THEN** stdout matches one documented deterministic v1 schema and contains no human prose
