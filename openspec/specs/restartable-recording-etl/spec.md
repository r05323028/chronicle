## Purpose

Restartable recording-scoped ETL, deterministic reconstruction, publication, and checkpoint recovery.

## Requirements

### Requirement: Recording-scoped ETL input

ETL SHALL accept one epoch WAL directory plus its parent/epoch lineage descriptor and optional immutable continuation-in descriptor, recover and validate exactly through the last valid v1 `CommitMarker`, and process only that epoch's committed snapshot. The continuation-in descriptor SHALL be accepted only when it names the unique predecessor in authoritative `epochs.json`, matches predecessor/successor IDs and ordinal, and verifies checksum, decoder/schema/pipeline version, marker/segment lineage, and bounded state. ETL SHALL process finalized, failed, and crash-recovered epochs when their committed prefix is valid, preserving epoch and parent status/reason. A live active epoch writer holding its WAL lock SHALL be rejected by standalone ETL; recorder-owned incremental ETL MAY consume a read-only committed snapshot while the writer is live. Unsupported versions, corrupt committed prefix/marker reference, contradictory parent/epoch metadata, WAL digest conflict, invalid continuation, or ambiguous predecessor SHALL be rejected. Complete frames after the final valid marker remain written-not-durable uncertainty and SHALL not be promoted.

#### Scenario: Finalized epoch input

- **WHEN** a valid finalized epoch is supplied with parent/epoch metadata
- **THEN** ETL discovers ordered segments, validates lineage, and processes exactly through that epoch's recovery-authoritative marker

#### Scenario: Recorder-owned live snapshot

- **WHEN** the long-lived recorder owns the active successor and exposes a stable committed snapshot
- **THEN** incremental ETL processes only that snapshot without repair or mutation and leaves the active suffix for a later snapshot

#### Scenario: Standalone ETL sees live writer

- **WHEN** an operator invokes standalone ETL against an epoch held by a live writer
- **THEN** standalone ETL rejects without reading partial live state or publishing a session

#### Scenario: Failed epoch input

- **WHEN** capture/WAL failure leaves a valid committed epoch prefix
- **THEN** ETL publishes/reports that epoch with failed/aborted provenance and does not claim parent completion

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

Recording ETL SHALL publish one deterministic Canonical Session v1 per finalized epoch, plus immutable continuation references when a decoder has bounded state crossing into a successor. Epoch session identity SHALL derive from stable parent `RecordingId`, `EpochId`/ordinal, pipeline version, and the verified epoch input/continuation seed. A continuation-only predecessor session remains inspectable and independently immutable but SHALL not contain a replayable duplicate operation that a successor may complete. The parent aggregate references verified epoch manifests/checkpoints and terminal operations without replacing session-store authority or merging mutable decoder state.

#### Scenario: First epoch publication

- **WHEN** a valid finalized epoch has no prior output
- **THEN** ETL atomically publishes one deterministic epoch session and records its parent/epoch association

#### Scenario: Multiple epoch publication

- **WHEN** three epochs of one parent finalize
- **THEN** ETL publishes three independently valid sessions whose parent association and ordinal order are deterministic

#### Scenario: Atomic publication failure

- **WHEN** write/sync/rename fails before an epoch session is published
- **THEN** its final session path is absent, its checkpoint does not advance, and retry is safe without affecting successor capture

#### Scenario: First successful run

- **WHEN** valid recording has no prior output
- **THEN** ETL atomically publishes deterministic session and reports ID

### Requirement: Checkpoint-after-publication idempotency without output binding

ETL SHALL derive deterministic epoch session identity and WAL snapshot digest over exact ordered verified epoch bytes through the last valid marker before publication. In each requested output root it SHALL use existing atomic no-replace publication and verify any existing deterministic epoch session/manifest/provenance/checksums before reporting `already_processed`; mismatch SHALL fail without overwrite. `IncrementalEtlCheckpoint v1` SHALL bind stable parent ID, epoch ID/ordinal, epoch WAL identity, marker/segment lineage, decoder state, output references, source status, and configuration/pipeline digests. A parent aggregate SHALL advance only after every referenced epoch output/checkpoint is verified.

#### Scenario: Second idempotent epoch run

- **WHEN** the same unchanged epoch is processed after matching publication/checkpoint
- **THEN** ETL returns the same session/output identity without duplicate session or delta publication

#### Scenario: Published output but missing checkpoint

- **WHEN** epoch output is durable but checkpoint write was interrupted
- **THEN** retry verifies/adopts the matching output and repairs that epoch checkpoint once

#### Scenario: Conflicting publication

- **WHEN** deterministic epoch output exists with different provenance/checksum
- **THEN** ETL fails closed, preserves existing output, and does not advance checkpoint or parent aggregation

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

Recorder-owned incremental ETL SHALL consume committed WAL snapshots for the active epoch while capture continues. Each snapshot descriptor SHALL include parent recording ID, epoch ID/ordinal, epoch WAL identity, sealed segment identity/digest where applicable, final marker segment/offset/sequence/digest, exact ordered input-byte digest, and capture/clock lineage. ETL SHALL validate segment/header/envelope/marker conditions through the descriptor boundary before processing. Manifest, metadata, file length, or writer acknowledgement alone SHALL not promote input.

Live ETL SHALL be read-only and SHALL never repair, truncate, reopen, delete, or classify a concurrent suffix as committed corruption. If the active segment changes while a snapshot is established, ETL SHALL retry from the prior per-epoch checkpoint or process only the stable marker boundary. Startup exclusive recovery retains repair authority.

#### Scenario: New committed batch

- **WHEN** the active successor exposes a marker beyond its epoch checkpoint
- **THEN** ETL validates exact lineage and processes records after that checkpoint through the marker

#### Scenario: No new marker

- **WHEN** active WAL has only complete/partial frames after the last authoritative marker
- **THEN** ETL does not advance input state or output

#### Scenario: Concurrent append

- **WHEN** writer appends while ETL reads the active epoch
- **THEN** ETL remains bounded by its chosen marker and never repairs the mutable suffix

#### Scenario: Standalone ETL sees live writer

- **WHEN** operator runs existing standalone ETL against recording held by live writer
- **THEN** standalone command retains existing behavior and rejects without reading live state or publishing

### Requirement: FinalSessionCheckpoint v1 and IncrementalEtlCheckpoint v1 are distinct

`FinalSessionCheckpoint v1` SHALL remain the existing stopped-recording final-session artifact. `IncrementalEtlCheckpointV1` SHALL remain a distinct read-only compatibility artifact for legacy epoch-local processing; its serialized `recording_id` is a legacy WAL/epoch identity and MUST NOT be interpreted as the parent. New parent-aware multi-epoch processing SHALL use `IncrementalEtlCheckpointV2` with a distinct discriminator and owner/lifecycle contract. V2 SHALL bind typed parent `RecordingId`, current `EpochId`/ordinal, optional unique predecessor/successor IDs, exact marker/segment lineage, continuation-in/out references and digests, decoder/schema/pipeline versions, bounded decoder state, emitted operation fingerprints, immutable output identities, source status, and checksum. `EpochContinuationCheckpointV1` SHALL be a separate bounded handoff artifact whose identity binds parent, predecessor, successor, marker/segment lineage, decoder/schema/pipeline versions, and checksum.

V2 restore SHALL reject unknown versions, v1/v2 discriminator confusion, parent/epoch/predecessor/successor mismatch, marker/cursor mismatch, lineage fork/gap, checksum failure, unsupported decoder kind/version, duplicate/inconsistent state, and bound/configuration overflow before output or retention eligibility. A fresh decoder SHALL never replace failed continuation restore. Legacy v1 artifacts may be materialized only through an explicit one-epoch compatibility adapter.

#### Scenario: Mid-connection checkpoint

- **WHEN** an epoch's HTTP request/response spans committed snapshots
- **THEN** its checkpoint preserves bounded parser/correlation state and resume completes that epoch operation once

#### Scenario: Unsupported or wrong-epoch checkpoint

- **WHEN** checkpoint declares an unknown version or belongs to another parent/epoch
- **THEN** ETL fails closed before output or retention eligibility

#### Scenario: Pending continuation restart

- **WHEN** ETL restarts with pending fragmented connection bytes in an epoch checkpoint
- **THEN** restored state and uninterrupted state produce byte-equivalent epoch output without duplicate operations

#### Scenario: Unsupported checkpoint version

- **WHEN** either checkpoint artifact declares unknown version or a `FinalSessionCheckpoint v1` is presented as incremental state
- **THEN** ETL fails closed before output or retention eligibility

#### Scenario: Checkpoint state exceeds bound

- **WHEN** pending connection/operation state exceeds existing deterministic limit
- **THEN** ETL applies existing typed incomplete/truncated/fail behavior and never emits silent partial checkpoint

### Requirement: Deterministic immutable incremental publication

ETL SHALL publish completed canonical operations/evidence as immutable bounded batches keyed deterministically by stable parent ID, epoch ID/ordinal, pipeline version, exact source WAL provenance, and canonical content. Batch ordering SHALL derive from capture/WAL provenance, not poll interval, worker timing, checkpoint grouping, or restart count. Existing canonical types and completeness/provenance semantics SHALL be reused; Incremental ETL SHALL not redefine HTTP meaning or weaken Canonical Session v1 validation.

#### Scenario: Different polling cadence

- **WHEN** the same epoch WAL is processed through one large snapshot or many smaller snapshots
- **THEN** epoch session bytes, operation IDs/order/content, provenance, completeness, and parent aggregate association are identical

#### Scenario: Existing matching batch

- **WHEN** retry encounters a deterministic epoch batch already published with matching digest/provenance
- **THEN** ETL verifies and reuses it without duplicate

#### Scenario: Existing conflicting batch

- **WHEN** a deterministic key exists with different bytes or provenance
- **THEN** ETL fails and does not overwrite or advance checkpoint

### Requirement: Publication-before-checkpoint crash safety

Each epoch incremental transaction SHALL follow: validate snapshot; derive bounded next state; publish/verify every immutable output; atomically write that epoch checkpoint; then mark eligible segment/epoch state. A finalized epoch SHALL not become retention-eligible from `processed` alone: its final Canonical Session v1 and manifest/payload integrity MUST also be verified. Parent aggregate publication SHALL reference only durable matching epoch outputs/checkpoints.

#### Scenario: Crash before output

- **WHEN** ETL stops after validation/state derivation but before immutable publication
- **THEN** old checkpoint remains and retry reprocesses the epoch deterministically

#### Scenario: Crash after output before checkpoint

- **WHEN** all epoch outputs exist but checkpoint remains old
- **THEN** retry verifies matching outputs and advances the epoch checkpoint once

#### Scenario: Crash after checkpoint

- **WHEN** epoch checkpoint is durable but epoch manifest processed transition is not
- **THEN** reconciliation repairs processed state from verified checkpoint/output proof without changing parent lineage

### Requirement: Duplicate prevention uses lineage, not event heuristics

ETL SHALL identify consumed input by exact parent/epoch/segment/sequence/marker and digest lineage. It SHALL identify published output by deterministic parent/epoch key plus content/provenance digest. It SHALL not deduplicate operations only by timestamp, HTTP fields, payload equality, socket tuple, or process ID. Same captured operation at distinct epoch/WAL provenance remains two observations unless existing retransmission rules prove duplicate byte range. Forked checkpoints, predecessor mismatch, parent/epoch mismatch, checkpoint ahead of WAL, same boundary with different digest, or output-set mismatch SHALL fail closed and block retention.

#### Scenario: Same request in two epochs

- **WHEN** workload legitimately sends identical HTTP requests before and after a rollover
- **THEN** ETL emits distinct operations with distinct epoch/WAL provenance and does not collapse them by payload equality

#### Scenario: Checkpoint fork

- **WHEN** two checkpoint revisions claim conflicting parent/epoch input/output digests
- **THEN** reconciliation fails without choosing by timestamp

#### Scenario: Same HTTP request twice

- **WHEN** workload legitimately sends identical request at different WAL ranges
- **THEN** ETL emits distinct operations with distinct provenance-derived IDs

### Requirement: Resume preserves provenance and clock domains

Incremental outputs, continuation artifacts, and final epoch sessions SHALL preserve stable parent recording ID, epoch ID/ordinal, epoch WAL identity, boot/clock identity, connection generation, capture time range, all contributing WAL segment/offset/sequence/marker/digest ranges, loss windows, source lifecycle status/reason, checkpoint/output identities, and recovery warnings. Process-attempt ID, restart time, polling cadence, publication time, and rollover scheduling SHALL be observational only and SHALL not affect canonical identity/order.

A connection generation may cross segment and epoch boundaries through a verified bounded continuation seed. A logical operation belongs to the epoch where its protocol decoder reaches terminal completion. If request/partial frame evidence begins in N and response/completion arrives in N+1, N+1 emits the sole operation with both epoch ranges; the already-published N session is not mutated. If the run ends before completion, the final epoch that owns the last trusted evidence emits one incomplete operation. Unsupported or invalid continuation produces explicit incomplete/loss provenance rather than a clean new connection or silently omitted evidence.

#### Scenario: Recorder restart in same epoch

- **WHEN** recorder/ETL restarts after a committed checkpoint and safely reopens the same epoch
- **THEN** resumed output retains original parent/epoch/WAL identity and deterministic IDs

#### Scenario: Connection spans epoch rollover

- **WHEN** one TCP connection remains open across a bounded epoch boundary
- **THEN** each epoch preserves observed evidence and neither session claims complete future bytes from the other epoch

#### Scenario: Parent aggregate after restart

- **WHEN** an old epoch is already published and the successor process restarts
- **THEN** parent aggregation keeps the old session once and resumes only the successor lineage

### Requirement: Incremental lag and backpressure are bounded and visible

ETL queue, worker count, per-epoch checkpoint state, output batch size, retry/backoff, continuation state, and maximum lag thresholds SHALL be bounded. Capture/WAL durability and rollover SHALL not wait for ETL publication, decoder progress, or continuation export during normal operation. ETL lag SHALL gate epoch retention and successor processing readiness and may degrade processing/overall health, but SHALL not authorize unprocessed WAL deletion or invalidate successor capture. Quota exhaustion caused by lag SHALL stop capture through explicit quota policy with exact counters rather than drop accepted data silently.

Health/status SHALL expose active and finalized epoch committed markers, per-epoch checkpoint markers, continuation dependency state (`pending`, `ready`, `consumed`, `unavailable`, or `failed`), aggregate and per-epoch lag records/bytes/age, pending/sealed/processed/retention states, last publication, retry/failure code, and output bytes without captured values.

#### Scenario: ETL temporarily unavailable

- **WHEN** an epoch ETL worker fails while active successor WAL retains headroom
- **THEN** recorder continues durable capture, reports growing per-epoch lag, and keeps that epoch ineligible for cleanup

#### Scenario: Lag consumes quota

- **WHEN** unprocessed finalized epochs reach reserved quota and no cleanup is eligible
- **THEN** recorder stops/fails visibly rather than delete, skip, or fork input

### Requirement: Incremental and one-shot ETL remain equivalent

Standalone ETL SHALL remain supported for stopped recordings and finalized epochs. For a finalized parent run, incremental execution across committed epoch snapshots with verified continuation seeds and clean one-shot execution over the same ordered parent WAL evidence SHALL produce equivalent terminal Canonical Session v1 operations: deterministic IDs, completion-owner epoch, timeline order, payload digests, completeness, replayability, cross-epoch provenance, and issue ordering. Per-epoch session bytes remain independently immutable; the parent aggregate is a deterministic ordered view and SHALL not alter them.

#### Scenario: Equivalence fixture

- **WHEN** a deterministic WAL fixture is processed incrementally across every commit boundary and separately as one finalized epoch
- **THEN** canonical epoch session bytes and manifest/checksums match exactly and parent association is identical

#### Scenario: Crash-resume equivalence

- **WHEN** ETL is killed at every publication/checkpoint boundary across repeated epochs and resumed
- **THEN** final parent aggregate and every epoch output match uninterrupted execution with no duplicate/missing operation

### Requirement: Incremental ETL acceptance

Rootless automated tests SHALL include snapshot concurrency, serialized-state round trip, fragmented long-lived HTTP across segments, loss windows, bounds, publication/checkpoint fault matrix, fork/corruption detection, quota lag, and one-shot equivalence. Privileged acceptance SHALL prove incremental processing of real eBPF WAL during continuous recording, ETL process crash/restart, deterministic final output, and retained checkpoint/publication evidence tied to acceptance fingerprint, compatible environment, and source provenance. Exact commit/tree identity is release-only.

#### Scenario: Privileged incremental proof

- **WHEN** real workload produces committed records during multi-segment recording and ETL is terminated/restarted
- **THEN** checkpoint resumes from validated lineage, final output is deterministic, and no committed operation is duplicated or omitted

### Requirement: Decoder snapshot and cursor restoration fail closed

The decoder reconstruction snapshot SHALL bind decoder kind/version, snapshot version, parent recording ID, current epoch ID/ordinal, optional predecessor/successor IDs, session identity, marker cursor/digest, segment lineage, continuation-in/out identity/digest, bounded serialized length, active/finalized connection evidence, continuation fragments, loss evidence, and decoder limits. Restore SHALL reject malformed bytes, unsupported versions/kinds, parent/epoch/session mismatch, predecessor/successor mismatch, cursor mismatch, duplicate/inconsistent state, bound mismatch, and WAL fork/gap. A fresh decoder SHALL never replace failed restore. Repeated recovery/retry over unchanged input SHALL restore identical serialized state and operation fingerprints.

#### Scenario: WAL fork or gap

- **WHEN** a resumed epoch snapshot omits a sequence or changes a checkpointed segment digest/range
- **THEN** ETL fails closed before publication and preserves prior checkpoint/output

#### Scenario: Pending continuation restart

- **WHEN** checkpoint is taken with pending fragmented connection bytes and ETL restarts
- **THEN** restore reconstructs same state and uninterrupted/restarted publication is byte-equivalent without duplicate operations

### Requirement: Finalized epoch publication is independent of successor capture

A finalized epoch SHALL progress through ETL and immutable publication while its successor captures. This concurrency SHALL use verified continuation references and SHALL not make the successor active before authoritative catalog activation.

#### Scenario: Concurrent incremental ETL

- **WHEN** epoch N is finalized and epoch N+1 is active
- **THEN** ETL may publish N while capture commits N+1, and both parent/epoch lineages remain independently recoverable

#### Scenario: No duplicate publication after rollover

- **WHEN** rollover and ETL retry observe the same finalized epoch boundary more than once
- **THEN** one matching session/output/checkpoint is retained and every retry reports/adopts the same identity

### Requirement: Cross-epoch continuation and completion ownership

A successor decoder SHALL restore continuation only from the unique verified predecessor. A logical operation SHALL appear in exactly one canonical epoch session: the session of the epoch where decoding reaches terminal completion. Predecessor sessions may retain immutable continuation references and incomplete connection evidence, but SHALL not publish a provisional replayable duplicate that later completion would supersede. At final run shutdown, the last trusted epoch emits one incomplete operation for unresolved continuation.

#### Scenario: HTTP request and response cross rollover

- **WHEN** request bytes are committed in epoch N and response bytes complete the protocol operation in epoch N+1 through a valid continuation
- **THEN** epoch N+1 publishes exactly one operation with both epoch/WAL provenance ranges and epoch N publishes no duplicate operation

#### Scenario: Long-lived connection crosses multiple epochs

- **WHEN** one TCP connection remains established across epochs 1 through 4 without close/reopen evidence
- **THEN** continuation preserves one connection generation with bounded state and no synthetic reconnect or duplicate operation

#### Scenario: Pipelined correlation crosses rollover

- **WHEN** multiple outstanding operations cross a boundary
- **THEN** correlation state restores deterministically and each terminal operation is emitted once by its completion-owner epoch

#### Scenario: Continuation unsupported or exhausted

- **WHEN** continuation is unsupported, corrupt, missing, or exceeds connection/request/reassembly limits
- **THEN** affected evidence receives explicit incomplete/loss provenance and no clean successor protocol state is claimed

#### Scenario: Final unresolved continuation

- **WHEN** the parent run ends before a carried operation receives trusted completion evidence
- **THEN** the final trusted epoch emits one incomplete operation and no earlier immutable session is mutated

### Requirement: Continuation readiness is separate from capture readiness

ETL SHALL create one bounded continuation dependency per verified predecessor/successor pair after capture records the predecessor final marker. `pending` means predecessor ETL has not yet caught up; `ready` means one checksummed continuation artifact exactly matches the predecessor final marker and lineage; `consumed` records successor restoration and checkpoint proof; `unavailable`/`failed` records explicit unsupported, invalid, over-limit, or retry-exhausted processing semantics. This state belongs to ETL lineage, not WAL/topology authority.

Successor WAL MAY accumulate committed bytes while its ETL is `blocked_on_predecessor_continuation`. Successor ETL SHALL verify/consume continuation before decoding cross-epoch state, then process accumulated committed WAL deterministically from its own checkpoint. Invalid continuation SHALL preserve valid successor WAL and emit incomplete/loss provenance; it SHALL not roll back `epochs.json`, invalidate capture, or invent a clean decoder.

#### Scenario: Predecessor ETL lags final marker

- **WHEN** predecessor WAL is sealed at marker 1,000 while its ETL cursor is 900 and successor capture is active
- **THEN** continuation is `pending`, capture readiness may remain true, successor WAL continues within quota, and successor processing readiness is degraded/blocked

#### Scenario: Continuation ready after successor WAL accumulation

- **WHEN** successor has committed WAL before predecessor ETL reaches marker 1,000 and a valid continuation becomes `ready`
- **THEN** successor ETL consumes the continuation once and processes its already-recorded WAL from its own checkpoint without replaying or skipping committed input

#### Scenario: Invalid continuation does not invalidate capture

- **WHEN** continuation checksum, identity, version, marker lineage, or bounds fail after successor WAL is valid
- **THEN** successor WAL/topology remain intact, ETL fails closed or emits explicit incomplete/loss provenance, and capture status is not changed to capture-failed solely by decoder handoff

#### Scenario: Multiple epochs have independent processing lag

- **WHEN** epoch 1 is complete, epoch 2 continuation is ready, epoch 3 is processing, and active epoch 4 waits for its predecessor
- **THEN** each dependency has bounded state and deterministic watermarks, capture remains independent while safety headroom exists, and parent status reports per-epoch processing readiness without merging cursors

#### Scenario: Sustained lag reaches quota safety limit

- **WHEN** protected unprocessed epochs consume reserved headroom and no eligible cleanup can restore a safe successor reservation
- **THEN** capture continues only until the explicit safety boundary, then stops/fails visibly before accepting unaccountable observations; no WAL or decoder lineage is silently dropped
