# production-recorder-operation

## Purpose

Define supported production recorder placement, readiness, operations, failure containment, and retained acceptance evidence.

## Requirements

### Requirement: Supported production placement is systemd-managed node recorder

P2 operations documentation SHALL recommend one foreground Chronicle recorder process managed by systemd for each configured cgroup scope/state root on supported Linux node. Service SHALL be placed outside selected capture subtree. Systemd SHALL own startup, restart, stop timeout, logs, service identity, state directory, and resource controls. Chronicle SHALL not self-fork, write authoritative PID file, or implement supervisor loop.

Application sidecar SHALL be documented as evaluated but not supported P2 default because pod/container churn, duplicate eBPF privilege, ephemeral local storage, recorder-in-selected-subtree risk, and lifecycle coupling conflict with continuous capture. Kubernetes operator/manifests and node-wide multi-tenant agent SHALL remain out of scope.

#### Scenario: Supported service start

- **WHEN** operator starts documented systemd unit on supported host
- **THEN** systemd launches foreground recorder with persistent state root outside captured subtree; `Type=simple` process start is not treated as readiness, and operator/automation polls `recorder-status` until ready

#### Scenario: Recorder exits failed

- **WHEN** recorder exits nonzero after typed recoverable process failure
- **THEN** systemd restart policy applies documented bounded backoff and Chronicle recovery runs before readiness

### Requirement: Production configuration is versioned and file-based

Recorder SHALL load one private versioned configuration file identifying canonical cgroup selector, state root, canonical filesystem-domain identity and deterministic domain-lock root, epoch duration/size, segment size/age, synchronized quota/minimum-free reserve for each filesystem domain, retention mode/age/bytes, ETL bounds/retry/lag policy, filesystem store root, shutdown timeout, and safe logging level. P2 supports one active Chronicle mutating owner per filesystem domain; recorder lease ownership covers the domain, not only the process. Multiple recorder instances sharing a WAL/store filesystem are unsupported; independent cgroup scopes require isolated filesystem domains. Secrets SHALL not be accepted on command line. Unknown fields/version, contradictory bounds, unsafe paths, selector containing recorder, or missing required quota/retention choice SHALL fail before capture.

Environment MAY supply non-secret deployment-specific path to configuration but SHALL not silently override persisted durability/retention values. P2 SHALL not support hot reload. Effective normalized configuration digest SHALL be recorded per process attempt/epoch and shown without sensitive values.

#### Scenario: Valid configuration

- **WHEN** versioned config passes semantic/preflight checks
- **THEN** normalized effective bounds and digest are persisted before running

#### Scenario: Invalid retention/quota

- **WHEN** cleanup policy could delete before checkpoint or quota cannot reserve one valid epoch/transaction
- **THEN** startup fails with remediation and no capture attachment

### Requirement: Service identity and filesystem security are explicit

Supported unit SHALL run under dedicated service identity with least capabilities required by verified eBPF environment, private state/store ownership, state directories mode `0700`, regular artifacts mode no broader than `0600`, restrictive umask, no command-line secrets, and recorder process outside selected cgroup subtree. Capability configuration SHALL be documented and doctor-verified; broad root fallback SHALL be labeled elevated risk rather than implied normal.

WAL, checkpoints, and canonical artifacts SHALL be documented as potentially containing plaintext credentials, bodies, and personal data. Logs, health, metrics/status, journal messages, and default commands SHALL remain payload/header-value safe. P2 SHALL explicitly state no encryption at rest, comprehensive redaction, tenant isolation, or compliance guarantee.

#### Scenario: Unsafe permissions

- **WHEN** state root cannot enforce private ownership/modes
- **THEN** recorder preflight fails rather than capture sensitive data

#### Scenario: Health output

- **WHEN** operator reads status during traffic
- **THEN** output contains IDs/counters/digests/lag/codes but no captured payload, arbitrary header values, command line, or environment value

### Requirement: Resource expectations and failure containment are documented

Operations guide SHALL document existing 8 MiB eBPF ring, 4096-event ingest queue, 4 MiB/10 ms group commit, 256 MiB default segment, per-epoch one-hour/4 GiB maxima, ETL connection/body/operation bounds, configured per-filesystem quota/minimum-free-space reserves, synchronized co-located writer reservations, expected temporary peak reservation, file-descriptor/thread/task use, and CPU/memory/disk monitoring signals. It SHALL distinguish configured limits from measured recommendations and SHALL not claim production scale without retained evidence.

Recorder failure SHALL affect only configured scope/state root. It SHALL not kill captured workload, widen capture scope, replay traffic, delete unprocessed evidence, or block unrelated recorder instances using distinct roots. Capture/WAL failure SHALL stop affected recorder; ETL/store failure MAY allow capture only while quota headroom/policy permits. Corruption SHALL preserve affected evidence in place and fail closed; retries and cleanup SHALL not make it authoritative.

#### Scenario: ETL outage with disk headroom

- **WHEN** ETL fails but local WAL remains below quota
- **THEN** capture continues and health degrades with growing lag

#### Scenario: State-root disk pressure

- **WHEN** quota/minimum-free-space cannot be restored by eligible cleanup
- **THEN** affected recorder stops while workload and unrelated recorder roots continue

### Requirement: Readiness, health, and liveness semantics are operationally distinct

Process liveness SHALL mean recorder process is executing. Capture readiness SHALL be true only when filesystem-domain lease, WAL recovery, appendable epoch, quota reservation, and capture attachment succeed. Processing readiness SHALL be reported separately and SHALL require checkpoint consistency, output-store availability, and incremental processing health. Overall health SHALL be policy-derived `healthy`, `degraded`, or `failed`; ETL lag/retry or retention backlog MAY degrade processing/overall health while durable capture remains safe and capture readiness remains true. Failed capture SHALL include lost WAL appendability, unrecoverable corruption, unsafe quota, or capture attachment loss. Failed processing SHALL include checkpoint lineage contradiction or unavailable output store.

Status SHALL expose recorder/attempt/epoch/segment IDs, lifecycle/capture-readiness/processing-readiness/overall-health, configuration digest, final committed marker, `IncrementalEtlCheckpoint v1` boundary and lag, retained/quota/free bytes, segment state counts, loss counters, last recovery/cleanup/publication, and stable failure/remediation code. P2 systemd unit SHALL use `Type=simple`; systemd active state proves liveness only. Readiness consumers SHALL poll local `recorder-status`/atomic state and SHALL NOT infer readiness from process start. No unauthenticated network metrics/HTTP server or `sd_notify` integration SHALL be added in P2.

#### Scenario: Recovery in progress

- **WHEN** process is alive but startup reconciliation has not completed
- **THEN** liveness is true, capture readiness is false, processing readiness is false or unknown, and health/status says recovering

#### Scenario: ETL lag warning

- **WHEN** committed WAL advances beyond warning threshold but quota remains safe
- **THEN** processing readiness and overall health report configured degraded/warning state while capture readiness remains true and committed capture is not reported as failed

### Requirement: Restart and stop policy preserve evidence

Recommended unit SHALL use bounded restart backoff, startup-rate limiting, and stop timeout longer than configured Chronicle graceful-drain timeout. SIGTERM SHALL be first stop action. Forced kill SHALL occur only after timeout and SHALL be documented as crash-recovery path. Service restart SHALL not delete state directories or run cleanup outside Chronicle's validated retention transaction.

Persistent startup failure from corruption/configuration/permission/quota SHALL not form unbounded restart storm; systemd policy and operator remediation SHALL be documented. Rollback SHALL stop service, preserve state root, and use prior binary only when artifact compatibility policy permits; otherwise operator SHALL retain evidence and start clean state root rather than force old reader.

#### Scenario: Graceful systemd stop

- **WHEN** service receives SIGTERM and drains before unit timeout
- **THEN** unit stops cleanly with retained final metadata/checkpoint and no forced kill

#### Scenario: Restart storm prevention

- **WHEN** same unrecoverable startup error repeats
- **THEN** documented start-limit/backoff leaves service failed for operator action without mutating evidence repeatedly

### Requirement: Operational ownership and runbook are complete

P2 documentation SHALL identify platform/operator owner for host prerequisites/capabilities/cgroup/state disk, application owner for selected workload scope and sensitive-data authorization, and Chronicle operator for configuration, health, retention, upgrades, evidence, and incident response. Runbook SHALL cover install/preflight/start/readiness/status/stop/restart, disk/ETL lag, corruption evidence preservation, forced termination, retention review, upgrade/rollback, evidence export, and leak checks.

Documentation SHALL compare node daemon and sidecar trade-offs, name node daemon recommendation, list unsupported deployment models, and state failure impact/resource expectations. It SHALL not include Kubernetes manifests unless later OpenSpec change adds supported orchestration.

#### Scenario: Operator follows runbook

- **WHEN** supported Ubuntu 24.04 operator follows documented steps
- **THEN** service reaches readiness, captures configured workload, exposes safe status, stops/restarts recoverably, and retains evidence

### Requirement: Production deployment acceptance is retained separately

Rootless tests SHALL validate configuration parsing, state/status schemas, systemd unit text/static safety, failure-policy decisions, and runbook commands without claiming eBPF. Privileged acceptance SHALL run the acceptance-sensitive source snapshot on supported Ubuntu 24.04 with systemd or faithful service-manager scope, real cgroup v2/eBPF capture, multiple epochs/segments, SIGTERM, forced crash/restart, ETL lag/recovery, disk pressure, and cleanup/leak verification. Evidence SHALL include acceptance fingerprint, source commit/tree provenance, kernel/architecture, capabilities, unit/config digest, lifecycle timeline, artifact checksums, and explicit checked/not-checked claims; exact source identity is release-only.

#### Scenario: Production model evidence

- **WHEN** privileged deployment acceptance passes
- **THEN** retained report proves recommended placement, continuous operation, restart, resource/failure behavior, and cleanup for compatible environment without treating OpenSpec validation as runtime proof
- **AND THEN** release reports additionally prove clean stable source identity
