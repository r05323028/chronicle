## ADDED Requirements

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

### Requirement: P1 one-shot commands remain available

Existing fixture/production `record`, standalone `etl`, `doctor`, `inspect`, and `replay` commands and their safety/exit contracts SHALL remain supported. `record` and standalone `etl` SHALL reject mutation against a WAL/store filesystem domain owned by a live recorder; read-only `doctor`, `inspect`, and `replay` may continue. `record` SHALL remain bounded and SHALL NOT silently become daemon mode. `etl` SHALL continue to process stopped P1/P2 epochs; incremental ETL lifecycle belongs to recorder application service. Replay defaults and authorization SHALL remain unchanged.

#### Scenario: Existing production record

- **WHEN** operator invokes bounded P1 production record syntax
- **THEN** command retains current duration/WAL limits, signal behavior, summary, and no implicit daemon/incremental ETL

#### Scenario: Existing standalone ETL

- **WHEN** stopped epoch is passed to `etl`
- **THEN** one-shot deterministic publication remains available and equivalent to finalized incremental result

### Requirement: P2 CLI and documentation acceptance

CLI contract tests SHALL cover recorder/status grammar, atomic startup/status JSON, signal exits, stale ownership, failure mapping, safe output, and unchanged P1 commands. Operations docs SHALL include systemd foreground invocation, private configuration, status, stop/restart, recovery, quota/retention, and supported-host example. Privileged acceptance SHALL invoke recorder under recommended service placement and retain exact command/config digest/output/evidence; fixture tests SHALL not claim daemon eBPF acceptance.

#### Scenario: CLI regression suite

- **WHEN** rootless CLI tests run
- **THEN** new commands pass and existing record/etl/inspect/replay/doctor contracts remain unchanged

#### Scenario: Privileged service command

- **WHEN** supported Linux acceptance starts `recorder --config FILE` under systemd
- **THEN** command reaches readiness, handles stop/restart, and retains machine-readable lifecycle evidence tied to commit/environment
