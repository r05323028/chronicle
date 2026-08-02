## ADDED Requirements

### Requirement: Incremental ETL consumes recovery-authoritative snapshots

Recorder-owned P2 incremental ETL worker/API SHALL consume committed WAL while recorder is running. Existing standalone `etl --wal-dir --output` SHALL retain P1 stopped-recording ownership and SHALL continue to reject a live writer; it MAY process a finalized P2 epoch. Processing input for recorder-owned incremental ETL SHALL be an immutable snapshot descriptor containing recording epoch ID, segment ordinal/digest where sealed, final marker segment/offset/sequence/digest, exact ordered input-byte digest through marker, and capture/clock lineage. ETL SHALL independently validate segment/header/envelope/marker conditions through descriptor boundary before processing. Manifest, metadata, file length, or writer acknowledgement alone SHALL NOT promote input.

Live ETL SHALL use read-only snapshot semantics and SHALL never repair, truncate, reopen, delete, or classify concurrently growing suffix as committed corruption. If active segment changes while snapshot is established, ETL SHALL retry from prior checkpoint or process only stable validated marker. Startup exclusive recovery retains existing repair authority.

#### Scenario: New committed batch

- **WHEN** recorder-owned worker observes recovery-authoritative marker beyond ETL checkpoint
- **THEN** worker validates exact snapshot and processes records after checkpoint through marker

#### Scenario: Standalone ETL sees live writer

- **WHEN** operator runs existing standalone ETL against recording held by live writer
- **THEN** standalone command retains P1 behavior and rejects without reading live state or publishing

#### Scenario: No new marker

- **WHEN** active WAL has only complete or partial frames after last authoritative marker
- **THEN** ETL does not advance input state or output

#### Scenario: Concurrent append

- **WHEN** writer appends while ETL reads active segment
- **THEN** ETL remains bounded by chosen marker and never repairs mutable suffix

### Requirement: FinalSessionCheckpoint v1 and IncrementalEtlCheckpoint v1 are distinct

`FinalSessionCheckpoint v1` SHALL remain the existing P1 stopped-recording checkpoint artifact and SHALL not be extended to represent live incremental state. It owns final-session ETL/publication lifecycle and existing P1 finalization behavior.

`IncrementalEtlCheckpoint v1` SHALL be a separate P2 artifact with a different discriminator, owner, and lifecycle. It SHALL persist enough bounded state to resume without rereading deleted input or changing semantic output. It SHALL include WAL marker lineage; segment digest lineage; decoder reconstruction state; TCP direction state; HTTP correlation state; loss-window state; emitted delta batch references; deterministic ID counters; recorder/epoch/configuration/pipeline/canonical versions; prior checkpoint digest; exact input marker and WAL snapshot digest; sealed segment identities/digests; active connection generation map; directional TCP ranges/chunks/provenance; lifecycle/loss-window state; protocol detector/decoder state including pending request/response correlation; deterministic ID derivation inputs; emitted immutable output identities/digests; source status; bounds/counters; and checkpoint checksum.

`IncrementalEtlCheckpoint v1` SHALL reject unknown/non-v1 before allocation. No version SHALL have dual interpretation. State SHALL obey existing 1024 connections, 65,536 chunks/connection, 8 MiB/connection, five-minute event-time idle, 8 MiB operation body, 10,000 operations/epoch, 1024 issues, and 4096-record read-batch bounds. It MAY contain pending captured bytes and SHALL use private permissions and payload-safe output.

`IncrementalEtlCheckpoint v1` SHALL be written atomically only after every output it references is durably published and verified. It SHALL never advance beyond recovery-authoritative input or rely on wall-clock/process scheduling for deterministic identity.

#### Scenario: Mid-connection checkpoint

- **WHEN** HTTP request or response spans committed snapshots
- **THEN** checkpoint preserves bounded directional/parser/correlation state and resume completes same operation once

#### Scenario: Unsupported checkpoint version

- **WHEN** either checkpoint artifact declares unknown version or a `FinalSessionCheckpoint v1` is presented as incremental state
- **THEN** ETL fails closed before output or retention eligibility

#### Scenario: Checkpoint state exceeds bound

- **WHEN** pending connection/operation state exceeds existing deterministic limit
- **THEN** ETL applies existing typed incomplete/truncated/fail behavior and never emits silent partial checkpoint

### Requirement: Deterministic immutable incremental publication

ETL SHALL publish completed canonical operations/evidence as immutable bounded batches keyed deterministically by recording epoch, pipeline version, exact source WAL provenance, and canonical content. Batch ordering SHALL derive from capture/WAL provenance, not poll interval, worker timing, checkpoint grouping, or restart count. Existing canonical types and completeness/provenance semantics SHALL be reused; P2 SHALL NOT redefine HTTP protocol meaning or weaken Canonical Session v1 validation.

Publication SHALL use `RecordingStore` put-if-absent semantics. Existing same key SHALL be accepted only after size/digest/schema/provenance verification; mismatch SHALL fail without overwrite. Epoch finalization SHALL materialize existing deterministic Canonical Session v1 equivalent to one-shot ETL over same final committed WAL and source status, then use existing atomic session publication semantics.

#### Scenario: Different polling cadence

- **WHEN** same WAL is processed with one large snapshot or many smaller snapshots
- **THEN** final canonical session, operation IDs/order/content, provenance, and completeness are byte-identical

#### Scenario: Existing matching batch

- **WHEN** retry encounters deterministic batch already published with matching digest/provenance
- **THEN** ETL verifies and reuses it without duplicate

#### Scenario: Existing conflicting batch

- **WHEN** deterministic key exists with different bytes or provenance
- **THEN** ETL fails and does not overwrite or advance checkpoint

### Requirement: Publication-before-checkpoint crash safety

Incremental transaction SHALL follow: validate input snapshot; derive outputs and next bounded state deterministically; publish/verify every immutable output; atomically write checkpoint referencing exact input and outputs; then make source segment eligible for processed transition. `processed` SHALL NOT by itself make finalized-epoch WAL retention eligible: final Canonical Session v1 and its manifest MUST also be durably published and verified. Checkpoint SHALL NOT reference unpublished output. Retention SHALL NOT rely on in-memory success.

Crash before publication SHALL replay input. Crash during publication SHALL verify/delete only private staging according to store semantics. Crash after publication but before checkpoint SHALL recompute same deterministic keys, verify outputs, and repair checkpoint. Crash after checkpoint but before manifest processed transition SHALL repair manifest from checkpoint. Every retry over unchanged input SHALL converge without duplicate or omitted canonical operation.

#### Scenario: Crash before output

- **WHEN** ETL stops after input validation/state derivation but before publication
- **THEN** old checkpoint remains and retry deterministically reprocesses input

#### Scenario: Crash after output before checkpoint

- **WHEN** all immutable outputs exist but checkpoint remains old
- **THEN** retry verifies same outputs and advances checkpoint once

#### Scenario: Crash after checkpoint

- **WHEN** checkpoint is durable but WAL manifest still labels segment sealed
- **THEN** reconciliation marks segment processed from verified checkpoint/output proof

### Requirement: Duplicate prevention uses lineage, not event heuristics

ETL SHALL identify already-consumed input by exact epoch/segment/sequence/marker and digest lineage. It SHALL identify already-published output by deterministic key plus content/provenance digest. It SHALL NOT deduplicate operations only by timestamp, HTTP fields, payload equality, socket tuple, or process ID. Same captured operation observed twice at distinct WAL provenance SHALL remain two observations unless existing TCP retransmission rules prove duplicate byte range.

Checkpoint lineage SHALL form one unambiguous chain. Forked checkpoints, predecessor mismatch, checkpoint ahead of WAL, same boundary with different digest, or output set mismatch SHALL fail closed and block retention.

#### Scenario: Same HTTP request twice

- **WHEN** workload legitimately sends identical request at different WAL ranges
- **THEN** ETL emits distinct operations with distinct provenance-derived IDs

#### Scenario: Checkpoint fork

- **WHEN** two checkpoint revisions claim same predecessor with conflicting input/output digest
- **THEN** reconciliation fails without choosing by timestamp

### Requirement: Resume preserves provenance and clock domains

Incremental outputs and final session SHALL preserve recorder ID, recording epoch ID/ordinal, boot/clock identity, connection generation, capture time range, WAL segment/offset/sequence/marker/digest ranges, loss windows, source lifecycle status/reason, ETL checkpoint/output identities, and recovery warnings. Restart time, process-attempt ID, polling cadence, and publication time SHALL be observational metadata only and SHALL NOT affect deterministic canonical identity/order.

Long-lived connection crossing segment snapshots SHALL retain one connection generation and complete only from trusted evidence. Connection crossing recording-epoch boundary SHALL finalize old epoch incomplete unless explicit complete lifecycle evidence precedes boundary; P2 SHALL not carry protocol state across recording IDs or invent cross-epoch session identity.

#### Scenario: Recorder restart in same epoch

- **WHEN** recorder/ETL restart after committed checkpoint and continue same safely reopenable epoch
- **THEN** resumed outputs retain original capture/WAL identity and deterministic IDs

#### Scenario: Connection spans epoch rollover

- **WHEN** TCP connection remains open across bounded recording epoch boundary
- **THEN** each epoch preserves its observed evidence and old epoch does not claim complete operation from future epoch bytes

### Requirement: Incremental lag and backpressure are bounded and visible

ETL queue, worker count, checkpoint state, output batch size, retry count/backoff, and maximum lag thresholds SHALL be configured/documented and bounded. Recorder capture/WAL durability SHALL not wait for ETL publication during normal operation. ETL lag SHALL gate retention and may trigger warning/not-ready according to policy, but SHALL not cause unprocessed WAL deletion. Quota exhaustion caused by ETL lag SHALL stop capture through WAL quota policy with exact counters rather than drop already accepted data silently.

Health/status SHALL expose committed marker, checkpoint marker, lag records/bytes/age, pending/sealed/processed segment counts, last successful publication, retry/failure code, and output bytes without captured values.

#### Scenario: ETL temporarily unavailable

- **WHEN** ETL worker fails while WAL retains headroom
- **THEN** recorder continues durable capture, reports growing lag, and keeps segments ineligible for cleanup

#### Scenario: Lag consumes quota

- **WHEN** unprocessed WAL reaches reserved quota and no segment is eligible
- **THEN** recorder stops/fails visibly rather than delete or skip input

### Requirement: Incremental and one-shot ETL remain equivalent

P1 standalone ETL SHALL remain supported for stopped recordings. For one final committed epoch snapshot, incremental execution and clean one-shot execution SHALL produce same deterministic Canonical Session v1, session/connection/operation IDs, timeline order, payload digests, completeness, replayability, provenance, and issue ordering. Incremental-only batch/checkpoint metadata SHALL not change replay semantics.

#### Scenario: Equivalence fixture

- **WHEN** deterministic WAL fixture is processed incrementally across every commit boundary and separately by one-shot ETL
- **THEN** canonical session bytes and manifest/checksums match exactly

#### Scenario: Crash-resume equivalence

- **WHEN** ETL is killed at every publication/checkpoint boundary then resumed
- **THEN** final output matches uninterrupted run with no duplicate/missing operation

### Requirement: Incremental ETL acceptance

Rootless automated tests SHALL include snapshot concurrency, serialized-state round trip, fragmented long-lived HTTP across segments, loss windows, bounds, publication/checkpoint fault matrix, fork/corruption detection, quota lag, and one-shot equivalence. Privileged acceptance SHALL prove incremental processing of real eBPF WAL during continuous recording, ETL process crash/restart, deterministic final output, and retained checkpoint/publication evidence tied to exact commit/environment.

#### Scenario: Privileged incremental proof

- **WHEN** real workload produces committed records during multi-segment recording and ETL is terminated/restarted
- **THEN** checkpoint resumes from validated lineage, final output is deterministic, and no committed operation is duplicated or omitted

### Requirement: Decoder snapshot and cursor restoration fail closed

The decoder reconstruction snapshot SHALL be a versioned, non-empty bounded representation of active/finalized connection evidence, continuation fragments, loss evidence, and decoder limits. Its metadata SHALL bind decoder kind and implementation version, snapshot schema version, session identity, recording/epoch identity, marker cursor/digest, and serialized length. Restore SHALL reject malformed bytes, unsupported versions or kinds, bound/configuration mismatch, duplicate or inconsistent connection state, session/recording mismatch, cursor mismatch, and segment-lineage fork. A fresh decoder SHALL never replace failed restore.

An incremental retry SHALL require contiguous WAL envelope sequence after its checkpoint cursor and exact matching digest/range for every previously checkpointed segment. Delta publication SHALL retain stable operation fingerprints in checkpoint state and publish only unseen operations; rollback SHALL restore reconstruction, evidence, issue, lineage, fingerprint, and cursor state together.

#### Scenario: Pending continuation restart

- **WHEN** checkpoint is taken with pending fragmented connection bytes and ETL restarts
- **THEN** restore reconstructs same state and uninterrupted/restarted publication is byte-equivalent without duplicate operations

#### Scenario: WAL fork or gap

- **WHEN** resumed snapshot omits a sequence or changes any checkpointed segment digest/range
- **THEN** ETL fails closed before publication and preserves prior checkpoint/output
