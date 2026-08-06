## Purpose

Restartable recording-scoped ETL, deterministic reconstruction, publication, and checkpoint recovery.

## Requirements

### Requirement: Recording-scoped ETL input

ETL SHALL accept one recording WAL directory, recover and validate metadata/segments, and process exactly recovered committed snapshot through last valid v1 `CommitMarker` (or unchanged declared P0 v1 boundary). It SHALL process `completed`, `failed`, and recovered `aborted` recordings when committed prefix is valid, preserving source status/reason. A live `starting`/`recording` writer holding the advisory lock SHALL be rejected without reading partial live state; stale `starting`/`recording` metadata after lock release SHALL be reconciled to `aborted` with `process_crash_recovered` before processing. Unsupported versions, corrupt committed prefix/marker reference, contradictory metadata, or WAL digest conflict SHALL be rejected. Complete frames after final valid marker SHALL remain written-not-durable uncertainty and SHALL NOT be promoted.

#### Scenario: Completed recording input

- **WHEN** valid stopped recording is supplied
- **THEN** ETL discovers all ordered segments and processes exactly through recovered commit-marker boundary

#### Scenario: Live active recording rejected

- **WHEN** record writer still holds recording lock
- **THEN** ETL fails without reading partial live state or publishing session

#### Scenario: Stale active recording recovered

- **WHEN** `starting`/`recording` metadata remains after writer lock release and committed prefix is valid
- **THEN** ETL atomically marks recording `aborted` with `process_crash_recovered`, persists recovery report, and publishes prefix with aborted provenance

#### Scenario: Aborted recording input

- **WHEN** recovery finalized stale active recording as aborted and commit-marker prefix is valid
- **THEN** ETL SHALL publish deterministic session with aborted/crash provenance and honest durability-window completeness

#### Scenario: Failed recording input

- **WHEN** capture/WAL failure left valid commit-marker prefix
- **THEN** ETL SHALL process prefix and preserve failed status/reason without claiming full source completion

### Requirement: Generation-safe TCP reconstruction

ETL SHALL group production payload by boot-scoped socket identity and connection generation, validate cgroup/tuple consistency, and maintain independent directional TCP sequence spaces. It SHALL NOT use file descriptor, PID, or tuple alone. Missing lifecycle SHALL create synthetic deterministic generation and incomplete state.

#### Scenario: Descriptor and tuple reuse

- **WHEN** multiple generations reuse descriptor or endpoint tuple
- **THEN** ETL creates separate reconstructed connections

#### Scenario: Missing open event

- **WHEN** payload exists without lifecycle-open event
- **THEN** ETL preserves payload under synthetic identity and marks connection incomplete

### Requirement: Deterministic directional ordering and deduplication

ETL SHALL order bytes within each direction by wrap-aware TCP sequence position, use enclosing WAL provenance as deterministic tie-breaker, remove exact retransmitted overlap, and preserve direction. It SHALL preserve deterministic cross-direction observation order by kernel monotonic timestamp then WAL sequence plus derived termination/source-integrity state. It SHALL consume both ring/backend `LossWindow` records and terminal WAL-limit loss evidence from committed WAL or recording metadata. Window bounds SHALL use actual previous/current successful sample times (attachment time before first sample, shutdown after final-sample failure) or earliest discarded queued-event time through shutdown; delayed poll SHALL retain expanded interval. ETL SHALL compare only matching boot-scoped monotonic clocks with connection activity, retain complete eligibility for provably outside connections, degrade overlap, and conservatively degrade unknown/mismatched/reset evidence. It SHALL NOT claim precise lost-connection attribution. Conflicting overlap, missing interval, truncation, uncommitted tail, or durability uncertainty SHALL produce explicit malformed/incomplete/truncated state. Fixture v1 events SHALL retain fixture ordering.

#### Scenario: Split request and response

- **WHEN** HTTP heads/bodies span multiple capture events in both directions
- **THEN** reconstructed streams contain exact ordered bytes independent of event boundaries

#### Scenario: Exact duplicate retransmission

- **WHEN** same directional byte range is observed twice with identical bytes
- **THEN** reconstructed stream contains one copy and provenance records duplicate observation

#### Scenario: Conflicting overlap

- **WHEN** overlapping TCP positions contain different bytes
- **THEN** affected connection is malformed/non-replayable and bytes are not silently selected

#### Scenario: Missing payload interval

- **WHEN** TCP position gap remains at connection finalization
- **THEN** affected message/connection is incomplete with source gap provenance

#### Scenario: Connection outside loss window

- **WHEN** connection activity interval ends before one loss window begins or starts after it ends
- **THEN** that window SHALL NOT alone degrade connection/operations

#### Scenario: Connection intersects loss window

- **WHEN** connection activity overlaps conservative loss interval
- **THEN** connection and affected operations become incomplete/uncertain and non-executable

#### Scenario: Delayed-poller window

- **WHEN** positive drop delta spans actual sample timestamps more than 100 ms apart
- **THEN** ETL uses expanded actual interval deterministically rather than nominal cadence

#### Scenario: WAL-limit terminal window

- **WHEN** queued events were discarded at physical hard cap
- **THEN** ETL uses committed loss record or equivalent metadata interval/counters and degrades only overlapping/ambiguous activity

#### Scenario: Loss relationship unknown

- **WHEN** lifecycle/timing evidence lacks matching clock identity/origin, has counter reset/regression, or cannot prove connection outside loss interval
- **THEN** ETL SHALL conservatively degrade connection and retain explicit ambiguous-loss provenance

#### Scenario: Sequential versus pipelined chronology

- **WHEN** two captures have identical directional request/response bytes but different cross-direction completion interleavings
- **THEN** ETL preserves distinct deterministic completion order so HTTP decoder distinguishes sequential from pipelined case

### Requirement: Bounded reconstruction

ETL SHALL bound active connections to 1024, chunks per connection to 65,536, bytes per connection to 8 MiB, idle event-time retention to five minutes, HTTP body buffering to 8 MiB/operation, total canonical operations to 10,000, issue count to existing 1024, and read batch to 4096 records. Per-connection byte/chunk/idle limits SHALL finalize affected connection incomplete/truncated. Active-connection or total-operation overflow SHALL fail typed before publication rather than omit connections/operations. Recording hot path SHALL NOT parse HTTP to enforce operation count; duration/WAL limits bound input instead.

#### Scenario: Connection memory limit

- **WHEN** reconstructed connection exceeds 8 MiB bound
- **THEN** ETL stops buffering that connection and records truncated/non-replayable state

#### Scenario: Idle connection eviction

- **WHEN** connection has no event for configured five-minute event-time window
- **THEN** ETL finalizes it incomplete and releases reconstruction memory

#### Scenario: Operation limit

- **WHEN** decoding would produce operation 10,001
- **THEN** ETL fails with exact limit/counter, publishes no partial session, and omits no operation silently

#### Scenario: Active connection limit

- **WHEN** input requires more than 1024 simultaneously active connections
- **THEN** ETL fails bounded before publication with exact limit evidence

#### Scenario: Batch processing

- **WHEN** WAL contains more than 4096 records
- **THEN** ETL processes bounded batches with deterministic final output

### Requirement: Protocol decode and honest canonicalization

ETL SHALL detect protocol from reconstructed streams, invoke registered decoder/canonicalizer, pair HTTP messages, and preserve unsupported/malformed/incomplete/unmatched evidence. It SHALL NOT mark operation complete when source loss, truncation, framing ambiguity, or missing peer affects it. Unsupported non-HTTP traffic SHALL remain bounded opaque non-replayable evidence.

#### Scenario: Complete HTTP operation

- **WHEN** reconstructed request and response are valid supported HTTP/1.1
- **THEN** ETL creates complete canonical operation with typed HTTP data and provenance

#### Scenario: Malformed traffic

- **WHEN** reconstructed bytes contain malformed HTTP
- **THEN** ETL publishes malformed/non-replayable evidence without panic

#### Scenario: Unsupported protocol

- **WHEN** durable bytes are not supported HTTP/1.1
- **THEN** ETL records unknown protocol/opaque evidence and does not claim decoder support

### Requirement: Deterministic one-session publication

P1 ETL SHALL group one stopped recording into one canonical session. Session, connection, and operation identifiers SHALL derive deterministically from recording ID, pipeline version, stable connection provenance, and message order. Publication SHALL use existing filesystem store atomic no-replace boundary.

#### Scenario: First successful run

- **WHEN** valid recording has no prior output
- **THEN** ETL atomically publishes deterministic session and reports ID

#### Scenario: Atomic publication failure

- **WHEN** write/sync/rename fails before publication
- **THEN** final session path is absent, checkpoint does not advance, and rerun is safe

### Requirement: Checkpoint-after-publication idempotency without output binding

ETL SHALL derive deterministic session ID and `wal_snapshot_sha256` over exact ordered verified segment bytes through last valid commit marker before publication. In each requested output root it SHALL use existing atomic no-replace publication and verify any existing deterministic manifest/session/provenance/checksums before reporting `already_processed`; mismatch SHALL fail without overwrite. Recording-local `output-binding.json` SHALL NOT be created or required. Versioned atomic checkpoint SHALL contain recording ID, commit-marker segment/offset/sequence, WAL/recovery digests, format/pipeline/canonical versions, session ID, latest manifest checksum/output identity, status, and counters, but SHALL be advisory across roots and written only after publication. ETL SHALL permit same recording to materialize intentionally into another output root. Interruption before publication SHALL reread committed WAL; missing checkpoint after publication SHALL be repaired from verified requested-store manifest.

#### Scenario: Second idempotent run

- **WHEN** same unchanged recording is processed after matching publication/checkpoint
- **THEN** ETL returns `already_processed` with same session ID and no new session

#### Scenario: Resume after interruption

- **WHEN** ETL stops after partially reading segment but before publication
- **THEN** rerun rereads durable input and produces same deterministic session once

#### Scenario: Published session but missing checkpoint

- **WHEN** session exists with matching provenance/checksum but checkpoint write was interrupted
- **THEN** ETL verifies publication and repairs checkpoint without duplicate output

#### Scenario: Same-length WAL mutation

- **WHEN** verified WAL bytes change after publication while commit-marker boundary and lengths remain same
- **THEN** recomputed snapshot digest differs and ETL rejects already-processed/checkpoint repair

#### Scenario: Different output root after publication before checkpoint

- **WHEN** session published to root A, checkpoint write failed, and operator requests root B
- **THEN** ETL SHALL publish same deterministic session to absent root-B path and SHALL verify/no-replace independently in each store

#### Scenario: Different output root with checkpoint

- **WHEN** existing checkpoint names root A and operator requests root B
- **THEN** checkpoint SHALL NOT reject root B; ETL verifies/publishes deterministic session in requested store and updates advisory checkpoint after success

#### Scenario: Conflicting prior publication

- **WHEN** deterministic session path exists with different provenance/checksum
- **THEN** ETL fails and does not overwrite

### Requirement: Corruption and partial-segment behavior

ETL SHALL process through last valid commit-marker boundary after allowed verified-final-tail repair but SHALL stop on corrupted segment/header/committed record/commit marker, unsupported version, sequence/reference/digest discontinuity, or checkpoint contradiction. It SHALL ignore/report complete uncommitted frames after final valid marker and SHALL NOT skip corrupt committed middle data or advance checkpoint beyond marker.

#### Scenario: Recovered truncated tail

- **WHEN** WAL recovery removed only incomplete final frame
- **THEN** ETL processes committed prefix and session records recovery warning/incomplete provenance

#### Scenario: Corrupted segment

- **WHEN** recovery report identifies non-tail corruption
- **THEN** ETL publishes no session and returns corruption summary

### Requirement: Provenance and counters

Canonical session and ETL summary SHALL include source recording ID/status/shutdown reason, connection generation, capture time range, WAL segment/offset/sequence/last-valid-marker provenance, written-not-durable post-marker range, typed ring/backend and WAL-limit loss windows/epochs/deltas, categorized discard counters, input/recovery checksums, schema versions, and counts for records processed/skipped, connections, decoded messages, and each completeness/replayability class. Default output SHALL omit payload/header values.

#### Scenario: Inspectable provenance

- **WHEN** ETL publishes session from recovered WAL
- **THEN** manifest/canonical data can trace each operation to recording/connection/WAL ranges

#### Scenario: ETL JSON summary

- **WHEN** user requests JSON
- **THEN** output has stable versioned fields, deterministic ordering, counters, checkpoint, and no sensitive values

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

Rootless automated tests SHALL include snapshot concurrency, serialized-state round trip, fragmented long-lived HTTP across segments, loss windows, bounds, publication/checkpoint fault matrix, fork/corruption detection, quota lag, and one-shot equivalence. Privileged acceptance SHALL prove incremental processing of real eBPF WAL during continuous recording, ETL process crash/restart, deterministic final output, and retained checkpoint/publication evidence tied to acceptance fingerprint, compatible environment, and source provenance. Exact commit/tree identity is release-only.

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
