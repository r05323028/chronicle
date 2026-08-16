## MODIFIED Requirements

### Requirement: Production configuration is versioned and file-based

Recorder SHALL load one private versioned configuration file identifying canonical cgroup selector, state root, filesystem-domain identity and deterministic domain-lock root, bounded epoch duration/size, segment size/age, synchronized global quota/minimum-free reserve, retention mode/age/bytes, ETL bounds/retry/lag policy, filesystem store root, shutdown timeout, and safe logging level. Epoch limits bound each epoch, not parent recording lifetime. The recorder supports one active Chronicle mutating owner per filesystem domain; one parent recording may own a sequence of epochs under that lease. Multiple recorder instances sharing a WAL/store filesystem remain unsupported; independent scopes require isolated filesystem domains. Unknown fields/version, contradictory bounds, unsafe paths, selector containing recorder, or missing quota/retention choice SHALL fail before capture.

Environment MAY supply only non-secret deployment-specific config path and SHALL not override persisted durability/retention/epoch values. Hot reload remains unsupported. Effective normalized configuration digest SHALL be recorded per process attempt and epoch and shown without sensitive values.

#### Scenario: Valid continuous configuration

- **WHEN** versioned config passes semantic/preflight checks with bounded epoch and global quota policy
- **THEN** normalized limits/digest are persisted before parent capture starts

#### Scenario: Invalid epoch/quota policy

- **WHEN** epoch bound cannot be enforced or global quota cannot reserve one valid epoch/transaction
- **THEN** startup fails with remediation and no capture attachment

#### Scenario: Valid configuration

- **WHEN** versioned config passes semantic/preflight checks
- **THEN** normalized effective bounds and digest are persisted before running

#### Scenario: Invalid retention/quota

- **WHEN** cleanup policy could delete before checkpoint or quota cannot reserve one valid epoch/transaction
- **THEN** startup fails with remediation and no capture attachment

### Requirement: Resource expectations and failure containment are documented

Operations guide SHALL document existing 8 MiB eBPF ring, 4096-event ingest queue, 4 MiB/10 ms group commit, configured segment size/age, bounded per-epoch duration/size, ETL connection/body/operation bounds, global per-filesystem quota/minimum-free-space reserve, synchronized co-located writer reservations, expected temporary peak reservation, file-descriptor/thread/task use, and CPU/memory/disk monitoring signals. It SHALL distinguish configured limits from measured recommendations and SHALL not claim production scale without retained evidence.

Recorder failure SHALL affect only configured parent/state root. It SHALL not kill captured workload, widen capture scope, replay traffic, delete unprocessed evidence, or block unrelated recorder domains. Capture/WAL failure SHALL stop the affected parent; ETL/store failure MAY allow successor capture only while quota/headroom and policy permit. Corruption SHALL preserve affected epoch evidence and fail closed; retries/cleanup SHALL not make it authoritative.

#### Scenario: ETL outage with successor headroom

- **WHEN** finalized epoch ETL fails but global WAL quota retains safe headroom
- **THEN** successor capture continues, parent health degrades with per-epoch lag, and predecessor evidence remains protected

#### Scenario: Global quota pressure

- **WHEN** retained/unprocessed epochs consume quota and no eligible cleanup exists
- **THEN** parent capture stops or fails visibly before accepting unaccounted data; unrelated recorder domains continue

#### Scenario: ETL outage with disk headroom

- **WHEN** ETL fails but local WAL remains below quota
- **THEN** capture continues and health degrades with growing lag

#### Scenario: State-root disk pressure

- **WHEN** quota/minimum-free-space cannot be restored by eligible cleanup
- **THEN** affected recorder stops while workload and unrelated recorder roots continue

### Requirement: Readiness, health, and liveness semantics are operationally distinct

Process liveness SHALL mean recorder process is executing. Capture readiness SHALL be true only when filesystem-domain lease, parent/epoch recovery, appendable active epoch, quota reservation, and capture attachment succeed; it SHALL NOT require predecessor ETL catch-up or `ContinuationState::Ready`. Processing readiness SHALL be reported separately and SHALL require per-epoch checkpoint/continuation consistency, output-store availability, and incremental processing health, with `waiting_for_continuation`/`blocked_on_predecessor_continuation` visible when applicable. Overall health SHALL be policy-derived `healthy`, `degraded`, or `failed`; ETL lag/retry, continuation loss, or predecessor retention backlog MAY degrade processing/overall health while durable successor capture remains safe. Failed capture SHALL include lost WAL appendability, unrecoverable epoch/parent corruption, unsafe quota, or capture attachment loss. Failed processing SHALL include checkpoint/continuation lineage contradiction or unavailable output store, but SHALL not relabel valid capture as failed solely because ETL is behind.

Status SHALL expose parent/attempt/current-epoch IDs, finalized epoch summaries, lifecycle/capture-readiness/processing-readiness/overall-health, configuration digest, recovery-authoritative marker per visible epoch, `IncrementalEtlCheckpoint v1` compatibility and `IncrementalEtlCheckpointV2` boundary/lag per epoch where applicable, continuation-in/out state, retained/quota/free bytes, segment/epoch state counts, loss counters, last rollover/recovery/cleanup/publication, and stable failure/remediation codes. `Type=simple` proves liveness only; readiness consumers SHALL poll atomic local status. No unauthenticated network metrics/HTTP server or `sd_notify` integration SHALL be added.

#### Scenario: Parent running with active epoch

- **WHEN** active epoch is appendable and prior epoch proofs are current
- **THEN** liveness and capture readiness are true, status identifies parent/current epoch, and processing readiness reflects each epoch

#### Scenario: Recovery in progress

- **WHEN** process is alive but parent/epoch startup reconciliation has not completed
- **THEN** liveness is true, capture readiness is false, processing readiness is false/unknown, and status says recovering

#### Scenario: Predecessor ETL lag warning

- **WHEN** committed finalized epoch exceeds lag threshold while active successor WAL remains safe
- **THEN** `capture_readiness=ready`, `processing_readiness=degraded`/`waiting_for_continuation`, and `overall_health=degraded` are valid; exact epoch lag/protected bytes are reported and capture is not reported failed

#### Scenario: ETL lag warning

- **WHEN** committed WAL advances beyond warning threshold but quota remains safe
- **THEN** processing readiness and overall health report configured degraded/warning state while capture readiness remains true and committed capture is not reported as failed

### Requirement: Restart and stop policy preserve evidence

Recommended unit SHALL use bounded restart backoff, startup-rate limiting, and stop timeout longer than configured graceful-drain timeout. SIGTERM SHALL first request graceful finalization of active epoch and durable parent/epoch lifecycle metadata. Forced kill SHALL occur only after timeout and SHALL be documented as crash-recovery path. Service restart SHALL not delete state directories or clean any epoch outside validated retention transaction.

Graceful stop SHALL seal active epoch with explicit shutdown reason, persist recovery-authoritative boundary, and make successor creation optional only on a new recorder start; it SHALL not silently turn a stopped parent into an active unbounded session. Restart/recovery SHALL resume parent lineage and never choose epoch by timestamp or directory order.

#### Scenario: Graceful continuous stop

- **WHEN** service receives SIGTERM and drains before unit timeout
- **THEN** active epoch is finalized with stop reason, parent/epoch metadata is durable, ETL may finish independently, and no forced kill occurs

#### Scenario: Forced kill during rollover

- **WHEN** process is killed during epoch transition
- **THEN** startup recovery identifies last durable epoch/marker, reconciles stale active metadata, and either resumes or aborts successor without deleting committed evidence

#### Scenario: Restart storm prevention

- **WHEN** same unrecoverable parent/epoch error repeats
- **THEN** start-limit/backoff leaves service failed for operator action without repeatedly mutating evidence

#### Scenario: Graceful systemd stop

- **WHEN** service receives SIGTERM and drains before unit timeout
- **THEN** unit stops cleanly with retained final metadata/checkpoint and no forced kill

### Requirement: Production deployment acceptance is retained separately

Rootless tests SHALL validate configuration parsing, parent/epoch lifecycle/status schemas, systemd unit text/static safety, rollover and failure-policy decisions, and runbook commands without claiming eBPF. Privileged acceptance SHALL run the acceptance-sensitive source snapshot on supported Ubuntu 24.04 with systemd or faithful service-manager scope, real cgroup v2/eBPF capture, multiple bounded epochs/segments, concurrent finalized-epoch ETL, SIGTERM, forced crash/restart during transition, ETL lag/recovery, disk pressure, and cleanup/leak verification. Evidence SHALL include acceptance fingerprint, source commit/tree provenance, kernel/architecture, capabilities, unit/config digest, parent/epoch lifecycle timeline, artifact/checkpoint checksums, and explicit checked/not-checked claims; exact source identity is release-only.

#### Scenario: Continuous production evidence

- **WHEN** privileged deployment acceptance passes with rollover, ETL restart, and cleanup checks
- **THEN** retained report proves parent/epoch lineage, no duplicate/omitted committed evidence, bounded resource behavior, restart safety, and cleanup for compatible environment
- **AND THEN** release reports additionally prove clean stable source identity

#### Scenario: Production model evidence

- **WHEN** privileged deployment acceptance passes
- **THEN** retained report proves recommended placement, continuous operation, restart, resource/failure behavior, and cleanup for compatible environment without treating OpenSpec validation as runtime proof
- **AND THEN** release reports additionally prove clean stable source identity

#### Scenario: Epoch reaches configured bound

- **WHEN** active epoch reaches configured duration or size bound
- **THEN** recorder durably finalizes its boundary, creates/activates one successor epoch under same parent, and continues capture without requiring user restart

#### Scenario: Explicit stop after many epochs

- **WHEN** user stops parent recording after multiple completed epochs
- **THEN** recorder finalizes only current epoch, preserves all prior epoch metadata, and reports one parent with deterministic ordered epoch list

#### Scenario: Fatal parent stop

- **WHEN** capture/WAL durability or global quota fails
- **THEN** recorder stops parent visibly, preserves every committed epoch prefix and failure reason, and does not claim unbounded success

## ADDED Requirements

### Requirement: Epoch rollover is crash-safe and non-overlapping

Rollover SHALL use one ordered durable capture/topology transition: flush/commit active epoch through a recovery-authoritative marker; persist final epoch status/boundary and admitted-observation outcome digest; persist successor metadata with parent/ordinal/trigger/lineage; acquire successor append authority; publish active-epoch status; then resume capture. No two writers may append one epoch, and no successor may become active before predecessor boundary metadata is durable. ETL continuation generation/consumption is a separate downstream dependency and is not a transition phase or activation predicate. A crash at any transition SHALL recover to predecessor active/finalized or successor active with no invented committed bytes and no skipped epoch ordinal.

#### Scenario: Rollover success

- **WHEN** configured epoch bound is reached and required filesystem/quota reservations are available
- **THEN** transition yields one finalized predecessor and one active successor with linked digests/ordinals, resumes capture, and does not require predecessor ETL catch-up or continuation-out

#### Scenario: Crash before predecessor finalize

- **WHEN** process dies before final boundary commit
- **THEN** recovery resumes/aborts only active epoch per WAL rules and does not expose successor as committed

#### Scenario: Crash after predecessor finalize

- **WHEN** process dies after predecessor marker but before successor activation
- **THEN** recovery preserves finalized predecessor and creates/reconciles one deterministic successor transition without duplicate publication

#### Scenario: Crash after successor activation

- **WHEN** process dies after successor metadata/lease is durable
- **THEN** recovery resumes successor from its own committed marker and retains predecessor finalization/checkpoint lineage

### Requirement: Rollover preserves capture authority and quota accounting

Capture hot path SHALL remain bounded by active epoch limits and SHALL not parse HTTP or wait for ETL publication to decide rollover. Rollover SHALL reserve enough local/global quota for predecessor finalization, successor metadata/first committed batch, recovery report, and configured safety margin before activation. ETL lag SHALL not authorize deletion; if no safe reservation exists, recorder SHALL stop before accepting data it cannot account for. Parent/epoch counters SHALL include rollover attempts, successes, failures, trigger, bytes/records, and visible loss evidence.

#### Scenario: ETL slower than capture

- **WHEN** ETL lags finalized epoch while active successor remains within reserved quota
- **THEN** capture commits successor records, reports lag, and keeps predecessor/source segments protected

#### Scenario: No successor reservation

- **WHEN** epoch bound is reached but required quota/space reservation cannot be acquired
- **THEN** recorder performs policy-defined safe stop/failure at committed boundary and reports exact quota cause rather than delete/drop

### Requirement: Parent lifecycle index is bounded and recoverable

Recorder SHALL persist a versioned parent lifecycle index containing parent identity, epoch ordinals/IDs, predecessor/successor links, status/reason, WAL/recovery/checkpoint/output digests, capture ranges, and retention eligibility. Index updates SHALL be atomic/checksummed and bounded by configured metadata limits; compaction SHALL preserve every epoch identity and digest needed to recover, inspect, replay, and prove deletion. Unknown versions, duplicate ordinals, forks, gaps, digest mismatch, or index/checkpoint disagreement SHALL fail closed.

#### Scenario: Index rebuild after crash

- **WHEN** parent index is missing but epoch metadata/WAL markers are intact
- **THEN** startup rebuilds or reports a deterministic recovery result from validated epoch evidence without choosing by mtime

#### Scenario: Lifecycle fork

- **WHEN** two epochs claim same ordinal or conflicting predecessor digest
- **THEN** recorder refuses readiness/retention and preserves conflicting evidence for operator repair
