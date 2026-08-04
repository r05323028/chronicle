# recording-store

## Purpose

Define local durable spool and immutable RecordingStore boundaries, lifecycle, integrity, and conformance obligations.

## Requirements

### Requirement: Active WAL remains on local durable spool

P2 SHALL require active recorder state, active WAL segment, writer lease, recovery repair, and mutable ETL checkpoint workspace on one local filesystem that supports private files, advisory locks, file/data sync, directory sync, and atomic same-filesystem rename. `RecordingStore` SHALL NOT pretend object storage can satisfy append, `fdatasync`, lock, or verified-tail-repair semantics. No remote backend SHALL be used as commit authority or required for capture acknowledgement.

#### Scenario: Local spool supports required semantics

- **WHEN** recorder preflight validates configured state root
- **THEN** active WAL may open only after lock/sync/rename/private-mode probes pass

#### Scenario: Object URL configured as active WAL

- **WHEN** operator attempts to use non-local object location for active WAL
- **THEN** configuration is rejected before capture

### Requirement: RecordingStore publishes immutable artifacts

`RecordingStore` boundary SHALL begin after artifact bytes and identity are immutable. P2 MAY expose sealed WAL segment copies as future-compatible immutable artifacts, but they are optional and not required for retention correctness. Canonical incremental batches, recovery reports, and bounded provenance indexes remain supported immutable kinds. Final Canonical Session v1 and its multi-file manifest/payload publication SHALL remain solely owned by existing `FilesystemSessionStore`; `RecordingStore` SHALL NOT replace or wrap that authority in P2. Mutable recorder state, active segment, lock, checkpoint, final session staging, and in-progress bytes SHALL remain outside upload boundary.

Store contract SHALL provide typed operations equivalent to put-if-absent with expected length/SHA-256/schema/provenance, get/read with integrity verification, metadata/head, explicitly bounded prefix/list for diagnostics only, and delete with exact identity/digest precondition. Store SHALL return stable created/already-exists-matching/conflict/not-found/unsupported/transient/permanent outcomes. Generic overwrite of immutable key SHALL not exist.

#### Scenario: First immutable publish

- **WHEN** absent key receives bytes matching declared length/digest/provenance
- **THEN** store durably creates object and returns created

#### Scenario: Idempotent retry

- **WHEN** same key already contains exact expected object
- **THEN** store verifies and returns already-exists-matching

#### Scenario: Conflicting retry

- **WHEN** same key exists with different length/digest/provenance
- **THEN** store returns conflict and preserves existing object

### Requirement: RecordingStore ownership boundaries

RecordingStore SHALL own only immutable artifact storage semantics, artifact identity, integrity verification, artifact lifecycle metadata, and the filesystem backend implementation. It SHALL NOT own active WAL append authority, WAL commit-marker authority, WAL repair, incremental checkpoint authority, or final Canonical Session publication authority. WAL commit markers and recovery remain authoritative for committed bytes; incremental ETL remains authoritative for checkpoint advancement; existing `FilesystemSessionStore` remains authoritative for final Canonical Session manifest/payload publication.

#### Scenario: RecordingStore cannot bypass WAL authority

- **WHEN** caller submits an active or unsealed WAL mutation, commit-marker request, or repair request through RecordingStore
- **THEN** operation is rejected as unsupported and WAL authority remains unchanged

#### Scenario: Checkpoint ordering remains ETL-owned

- **WHEN** incremental ETL publishes an immutable artifact through RecordingStore
- **THEN** RecordingStore returns only artifact persistence/integrity outcome; ETL alone may advance `IncrementalEtlCheckpoint v1` after publication verification

#### Scenario: Final session publication remains FilesystemSessionStore-owned

- **WHEN** caller attempts to publish final Canonical Session manifest/payload through RecordingStore
- **THEN** operation is rejected as unsupported and final publication remains an unchanged `FilesystemSessionStore` transaction

### Requirement: Explicit manifests own artifact reachability

Recorder/ETL SHALL decide required artifact set from versioned manifests/checkpoints, not from store listing completeness or modification time. Every published artifact reference SHALL contain kind, deterministic key, size, SHA-256, schema version, source epoch/WAL range, creation transaction identity, and retention class. Manifest/checkpoint referencing artifact SHALL be persisted only after store confirms durable matching publication. Retention decisions SHALL use local validated WAL, lifecycle manifest, checkpoint lineage, and final-session proof; duplicated sealed WAL copies and store listing SHALL never be required for retention correctness.

Future eventually consistent listing SHALL not affect correctness. Orphan immutable artifacts from publish-before-checkpoint crash MAY exist; deterministic retry SHALL adopt exact matching artifact, while bounded garbage collection MAY remove unreferenced objects only after configured safety age and proof they are absent from every retained manifest/checkpoint.

#### Scenario: Listing omits recent object

- **WHEN** store list does not return newly published key but direct verified head/get succeeds
- **THEN** checkpoint correctness uses direct reference and does not republish under another identity

#### Scenario: Orphan from crash

- **WHEN** object was published but no checkpoint references it
- **THEN** retry verifies/adopts deterministic key or later bounded orphan cleanup removes it after safety checks

### Requirement: Filesystem backend preserves local compatibility

P2 SHALL ship one filesystem RecordingStore backend. It SHALL preserve private mode expectations, checksum validation, staging under destination filesystem, file/directory sync, atomic no-replace behavior, path confinement, deterministic layout, and safe recovery of stale private staging. Existing `FilesystemSessionStore` remains separate owner of manifest-last canonical session publication; its readers and P1 stopped-recording outputs SHALL remain usable without cloud configuration.

Filesystem backend SHALL reject traversal, symlink escape, cross-filesystem publication requiring non-atomic copy, non-v1 metadata, checksum mismatch, and existing conflicting final path. It SHALL not weaken existing Canonical Session publication semantics to fit future backends.

#### Scenario: Existing filesystem session

- **WHEN** P1 canonical session is read/inspected through existing local root
- **THEN** P2 storage changes do not require migration or republish

#### Scenario: Cross-filesystem staging

- **WHEN** configured staging and destination cannot use same-filesystem atomic rename
- **THEN** filesystem backend rejects configuration or stages inside destination filesystem

#### Scenario: Path traversal

- **WHEN** artifact key attempts absolute/parent/symlink escape
- **THEN** backend rejects without access outside root

### Requirement: Artifact lifecycle is monotonic and retention-safe

Immutable artifact lifecycle SHALL be `staged -> published -> referenced -> retention_eligible -> deleted`, with evidence-preservation alternative on integrity/lineage conflict. Published transition SHALL require durable store confirmation; referenced SHALL require durable manifest/checkpoint; retention eligibility SHALL require policy and all dependent lineage; delete SHALL use exact key/digest precondition and durable tombstone/report. Automatic deletion SHALL not act on staged, unreferenced-young, conflicting, quarantined, or required-by-retained-lineage artifact.

Source WAL and derived canonical artifacts SHALL have independent retention classes. Deletion of one SHALL not imply deletion eligibility of other. Failed delete SHALL remain retryable and SHALL not make manifest claim object absent unless direct verification confirms.

#### Scenario: Derived output retained

- **WHEN** source WAL retention expires but canonical artifact policy retains output
- **THEN** source cleanup does not remove canonical object or its provenance manifest

#### Scenario: Delete precondition mismatch

- **WHEN** key content differs from expected digest at deletion
- **THEN** store preserves/reports conflict and does not delete

### Requirement: P2 upload boundary stops at sealed immutable artifacts

P2 SHALL expose only locally sealed/digested immutable artifacts to `RecordingStore`. Local commit/acknowledgement and ETL input authority SHALL not wait on or derive from any upload concept. Active segment, lease, recovery repair, mutable checkpoint, and canonical session transaction SHALL remain local-owned boundaries. Sealed WAL copies, if exposed, are optional future-compatible artifacts; P2 retention correctness SHALL not depend on copying them into `RecordingStore`.

P2 SHALL implement filesystem backend only. It SHALL NOT add S3 client, bucket credentials, multipart upload, network retry engine, cloud configuration, remote durability claim, or normative remote-backend behavior.

#### Scenario: Optional sealed WAL copy disabled

- **WHEN** P2 retains sealed WAL only on its local durable spool
- **THEN** retention eligibility still uses validated local WAL/manifest/checkpoint/final-session proof and does not require a `RecordingStore` copy

#### Scenario: Remote backend requested

- **WHEN** configuration requests S3-compatible backend in P2
- **THEN** recorder returns stable unsupported-backend result and local filesystem behavior remains available

### Requirement: Store failures are typed and bounded

Store operation SHALL classify permission, space/quota, integrity conflict, unsupported semantics/version, transient I/O, permanent I/O, and uncertain durability. Recorder/ETL SHALL use bounded retries with backoff only for explicitly retryable operations and SHALL expose lag/failure without captured values. It SHALL never retry non-idempotent overwrite, invent success after timeout, or delete local required evidence because remote publication is uncertain.

#### Scenario: Publication durability uncertain

- **WHEN** backend cannot determine whether immutable put completed
- **THEN** retry performs direct key/digest verification before reuse and checkpoint does not advance on unresolved state

#### Scenario: Filesystem full

- **WHEN** filesystem publication fails for space
- **THEN** output/checkpoint does not advance, source WAL remains protected, and quota/lag diagnostics expose stable error

### Requirement: Backend conformance is executable

P2 filesystem RecordingStore backend SHALL pass conformance suite for put-if-absent, matching retry, conflicting retry, integrity read, bounded list, preconditioned delete, path/key safety, crash staging, durability uncertainty, lifecycle behavior, and ownership-boundary rejection. Conformance SHALL prove RecordingStore cannot bypass WAL authority, advance incremental checkpoints, or publish final sessions. Conformance SHALL use rootless fault injection and SHALL not claim any remote backend support.

#### Scenario: Filesystem conformance

- **WHEN** rootless conformance suite runs with injected write/sync/rename/delete failures
- **THEN** final objects/checkpoints are absent or exact, retries converge, and no conflicting object is overwritten
