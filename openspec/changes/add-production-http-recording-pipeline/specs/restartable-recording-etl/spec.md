## ADDED Requirements

### Requirement: Recording-scoped ETL input
ETL SHALL accept one recording WAL directory, recover and validate metadata/segments, and process exactly recovered committed snapshot through last valid v2 `CommitMarker` (or unchanged declared P0 v1 boundary). It SHALL process `completed`, `failed`, and recovered `aborted` recordings when committed prefix is valid, preserving source status/reason; it SHALL reject active `starting`/`recording`, unsupported versions, corrupt committed prefix/marker reference, contradictory metadata, or WAL digest conflict. Complete frames after final valid marker SHALL remain written-not-durable uncertainty and SHALL NOT be promoted.

#### Scenario: Completed recording input
- **WHEN** valid stopped recording is supplied
- **THEN** ETL discovers all ordered segments and processes exactly through recovered commit-marker boundary

#### Scenario: Active recording rejected
- **WHEN** record writer still holds recording lock
- **THEN** ETL fails without reading partial live state or publishing session

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
