## ADDED Requirements

### Requirement: Active WAL remains on local durable spool

P2 SHALL require active recorder state, active WAL segment, writer lease, recovery repair, and mutable ETL checkpoint workspace on one local filesystem that supports private files, advisory locks, file/data sync, directory sync, and atomic same-filesystem rename. `RecordingStore` SHALL NOT pretend object storage can satisfy append, `fdatasync`, lock, or verified-tail-repair semantics. No remote backend SHALL be used as commit authority or required for capture acknowledgement.

#### Scenario: Local spool supports required semantics

- **WHEN** recorder preflight validates configured state root
- **THEN** active WAL may open only after lock/sync/rename/private-mode probes pass

#### Scenario: Object URL configured as active WAL

- **WHEN** operator attempts to use non-local object location for active WAL
- **THEN** configuration is rejected before capture

### Requirement: RecordingStore publishes immutable artifacts

`RecordingStore` boundary SHALL begin after artifact bytes and identity are immutable. Supported P2 artifact kinds SHALL include sealed WAL segment copies, canonical incremental batches, recovery reports, and bounded provenance indexes. Final Canonical Session v1 and its multi-file manifest/payload publication SHALL remain solely owned by existing `FilesystemSessionStore`; `RecordingStore` SHALL NOT replace or wrap that authority in P2. Mutable recorder state, active segment, lock, checkpoint, final session staging, and in-progress bytes SHALL remain outside upload boundary.

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

### Requirement: Explicit manifests own artifact reachability

Recorder/ETL SHALL decide required artifact set from versioned manifests/checkpoints, not from store listing completeness or modification time. Every published artifact reference SHALL contain kind, deterministic key, size, SHA-256, schema version, source epoch/WAL range, creation transaction identity, and retention class. Manifest/checkpoint referencing artifact SHALL be persisted only after store confirms durable matching publication.

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

Immutable artifact lifecycle SHALL be `staged -> published -> referenced -> retention_eligible -> deleted`, with `quarantined` alternative on integrity/lineage conflict. Published transition SHALL require durable store confirmation; referenced SHALL require durable manifest/checkpoint; retention eligibility SHALL require policy and all dependent lineage; delete SHALL use exact key/digest precondition and durable tombstone/report. Automatic deletion SHALL not act on staged, unreferenced-young, conflicting, quarantined, or required-by-retained-lineage artifact.

Source WAL and derived canonical artifacts SHALL have independent retention classes. Deletion of one SHALL not imply deletion eligibility of other. Failed delete SHALL remain retryable and SHALL not make manifest claim object absent unless direct verification confirms.

#### Scenario: Derived output retained

- **WHEN** source WAL retention expires but canonical artifact policy retains output
- **THEN** source cleanup does not remove canonical object or its provenance manifest

#### Scenario: Delete precondition mismatch

- **WHEN** key content differs from expected digest at deletion
- **THEN** store quarantines/reports conflict and does not delete

### Requirement: P2 upload boundary stops at sealed immutable artifacts

P2 SHALL expose only locally sealed/digested immutable artifacts to `RecordingStore`. Local commit/acknowledgement and ETL input authority SHALL not wait on or derive from any upload concept. Active segment, lease, recovery repair, mutable checkpoint, and canonical session transaction SHALL remain local-owned boundaries.

P2 SHALL implement filesystem backend only. It SHALL NOT add S3 client, bucket credentials, multipart upload, network retry engine, cloud configuration, remote durability claim, or normative remote-backend behavior.

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

P2 filesystem RecordingStore backend SHALL pass conformance suite for put-if-absent, matching retry, conflicting retry, integrity read, bounded list, preconditioned delete, path/key safety, crash staging, durability uncertainty, and lifecycle behavior. Conformance SHALL use rootless fault injection and SHALL not claim any remote backend support.

#### Scenario: Filesystem conformance

- **WHEN** rootless conformance suite runs with injected write/sync/rename/delete failures
- **THEN** final objects/checkpoints are absent or exact, retries converge, and no conflicting object is overwritten
