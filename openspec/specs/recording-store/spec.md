# recording-store

## Purpose

Define local durable spool and immutable RecordingStore boundaries, lifecycle, integrity, and conformance obligations.

## Requirements

### Requirement: Active WAL remains on local durable spool

The recorder SHALL require active recorder state, active WAL segment, writer lease, recovery repair, and mutable ETL checkpoint workspace on one local filesystem that supports private files, advisory locks, file/data sync, directory sync, and atomic same-filesystem rename. `RecordingStore` SHALL NOT pretend object storage can satisfy append, `fdatasync`, lock, or verified-tail-repair semantics. No remote backend SHALL be used as commit authority or required for capture acknowledgement.

#### Scenario: Local spool supports required semantics

- **WHEN** recorder preflight validates configured state root
- **THEN** active WAL may open only after lock/sync/rename/private-mode probes pass

#### Scenario: Object URL configured as active WAL

- **WHEN** operator attempts to use non-local object location for active WAL
- **THEN** configuration is rejected before capture

### Requirement: RecordingStore publishes immutable artifacts

`RecordingStore` SHALL publish only immutable artifacts after bytes and identity are fixed. Supported artifacts SHALL include per-epoch immutable ETL batches, recovery reports, bounded parent/epoch provenance indexes, checkpoints where store policy permits, and future sealed segment copies; final Canonical Session v1 manifest/payload publication remains solely owned by existing `FilesystemSessionStore`. Artifact identity SHALL include stable parent recording ID, epoch ID/ordinal, artifact kind, deterministic key, size, SHA-256, schema/pipeline version, source epoch/WAL range, and retention class. Generic overwrite SHALL not exist. The store SHALL provide put-if-absent, verified read/head, bounded diagnostics listing, and exact digest-preconditioned delete with stable created/matching/conflict/not-found/unsupported/transient/permanent outcomes.

#### Scenario: First epoch artifact publish

- **WHEN** absent epoch artifact key receives matching immutable bytes
- **THEN** store durably creates it and returns `created`

#### Scenario: Idempotent epoch retry

- **WHEN** same parent/epoch key already contains exact expected bytes/provenance
- **THEN** store verifies and returns `already-exists-matching`

#### Scenario: Conflicting epoch retry

- **WHEN** same key exists with different epoch lineage, length, digest, or schema
- **THEN** store returns conflict and preserves existing object

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

RecordingStore SHALL own immutable artifact persistence, identity, integrity verification, artifact lifecycle metadata, and filesystem backend behavior. It SHALL NOT own active WAL append/commit authority, epoch rollover authority, WAL repair, incremental checkpoint advancement, parent aggregate truth, or final Canonical Session publication. WAL commit markers/recovery remain authoritative for committed bytes; recorder owns lifecycle/rollover; incremental ETL owns checkpoint/output ordering; parent aggregation is valid only from verified epoch manifests/checkpoints; `FilesystemSessionStore` owns final Canonical Session publication.

#### Scenario: Store cannot bypass epoch WAL authority

- **WHEN** caller submits active/unsealed WAL mutation, rollover, marker, or repair request through RecordingStore
- **THEN** operation is rejected as unsupported and WAL/epoch authority remains unchanged

#### Scenario: Checkpoint ordering remains ETL-owned

- **WHEN** incremental ETL publishes an epoch artifact through RecordingStore
- **THEN** store returns persistence/integrity outcome only; ETL advances the epoch checkpoint after publication verification

#### Scenario: Parent aggregate remains derived

- **WHEN** caller submits a parent index that omits, reorders, or contradicts verified epoch outputs
- **THEN** store does not make it authoritative; application/ETL must fail closed

#### Scenario: RecordingStore cannot bypass WAL authority

- **WHEN** caller submits an active or unsealed WAL mutation, commit-marker request, or repair request through RecordingStore
- **THEN** operation is rejected as unsupported and WAL authority remains unchanged

#### Scenario: Final session publication remains FilesystemSessionStore-owned

- **WHEN** caller attempts to publish final Canonical Session manifest/payload through RecordingStore
- **THEN** operation is rejected as unsupported and final publication remains an unchanged `FilesystemSessionStore` transaction

### Requirement: Explicit manifests own artifact reachability

Recorder/ETL SHALL decide required epoch artifact sets from versioned manifests, v2 checkpoints, continuation references, and parent aggregation descriptors, not store listing completeness or modification time. Every artifact reference SHALL include parent/epoch identity, deterministic key, size, SHA-256, schema/pipeline version, source WAL range/digest, continuation predecessor/successor role where applicable, creation transaction identity, and retention class. A parent aggregate SHALL be persisted only after every required epoch session, continuation artifact, and checkpoint reference is durably verified. Retention decisions SHALL use local validated WAL, authoritative epoch catalog, checkpoint/continuation lineage, final-session proof, and parent/reference proof; duplicated sealed copies and listing SHALL never be required for correctness.

#### Scenario: Listing omits recent epoch artifact

- **WHEN** direct verified head/get finds an epoch object absent from bounded listing
- **THEN** checkpoint/parent correctness uses the direct reference and does not republish under another identity

#### Scenario: Orphan epoch artifact after crash

- **WHEN** an artifact was published but no checkpoint or parent manifest references it
- **THEN** retry verifies/adopts deterministic matching bytes or bounded garbage collection removes it only after safety-age and reference checks

#### Scenario: Partial parent aggregate

- **WHEN** one required epoch artifact is absent or digest-conflicting
- **THEN** parent aggregate remains unpublished/unverified and no retention transition depends on it

#### Scenario: Listing omits recent object

- **WHEN** store list does not return newly published key but direct verified head/get succeeds
- **THEN** checkpoint correctness uses direct reference and does not republish under another identity

#### Scenario: Orphan from crash

- **WHEN** object was published but no checkpoint references it
- **THEN** retry verifies/adopts deterministic key or later bounded orphan cleanup removes it after safety checks

### Requirement: Filesystem backend preserves local compatibility

The recorder SHALL ship one filesystem RecordingStore backend. It SHALL preserve private mode expectations, checksum validation, staging under destination filesystem, file/directory sync, atomic no-replace behavior, path confinement, deterministic layout, and safe recovery of stale private staging. Existing `FilesystemSessionStore` remains separate owner of manifest-last canonical session publication; its readers and stopped-recording outputs SHALL remain usable without cloud configuration.

Filesystem backend SHALL reject traversal, symlink escape, cross-filesystem publication requiring non-atomic copy, non-v1 metadata, checksum mismatch, and existing conflicting final path. It SHALL not weaken existing Canonical Session publication semantics to fit future backends.

#### Scenario: Existing filesystem session

- **WHEN** an existing canonical session is read/inspected through the existing local root
- **THEN** recorder storage changes do not require migration or republish

#### Scenario: Cross-filesystem staging

- **WHEN** configured staging and destination cannot use same-filesystem atomic rename
- **THEN** filesystem backend rejects configuration or stages inside destination filesystem

#### Scenario: Path traversal

- **WHEN** artifact key attempts absolute/parent/symlink escape
- **THEN** backend rejects without access outside root

### Requirement: Artifact lifecycle is monotonic and retention-safe

Immutable artifact lifecycle SHALL be `staged -> published -> referenced -> retention_eligible -> deleted`, with quarantine/evidence-preservation on integrity or lineage conflict. Published requires durable store confirmation; referenced requires durable epoch checkpoint, final session, or verified parent manifest as applicable; epoch source WAL and derived canonical artifacts have independent retention classes. A finalized epoch source SHALL be eligible for deletion only after its own checkpoint/output/final-session proof, predecessor ETL cursor reaches its authoritative final marker, any continuation dependency is consumed or terminally resolved with incomplete/loss proof, and all required parent/epoch lineage references are durable. A pending, ready-but-unconsumed, or retryable failed continuation protects predecessor source WAL and checkpoints. A successor epoch becoming active SHALL not make predecessor evidence eligible. Delete SHALL use exact key/digest precondition and durable tombstone/report.

#### Scenario: Epoch output retained after WAL deletion

- **WHEN** finalized epoch source WAL reaches policy expiry after verified canonical publication
- **THEN** source cleanup does not remove retained epoch artifacts or parent provenance

#### Scenario: Successor active during predecessor cleanup

- **WHEN** epoch N+1 captures while epoch N awaits or reaches retention eligibility
- **THEN** cleanup evaluates N's proof independently and cannot delete N+1 or use N+1 progress as proof for N

#### Scenario: Delete precondition mismatch

- **WHEN** source/artifact bytes differ from recorded digest at deletion
- **THEN** deletion is refused, conflict is retained, and lineage is not marked absent

#### Scenario: Derived output retained

- **WHEN** source WAL retention expires but canonical artifact policy retains output
- **THEN** source cleanup does not remove canonical object or its provenance manifest

### Requirement: Recorder upload boundary stops at sealed immutable artifacts

The recorder SHALL expose only locally sealed/digested immutable epoch artifacts to `RecordingStore`. Local capture acknowledgement, epoch rollover, commit markers, recovery, ETL input authority, and final session publication SHALL not wait on or derive from upload. The recorder SHALL implement filesystem backend only and SHALL not add remote client/credentials/retry/cloud configuration or normative remote-backend behavior.

#### Scenario: Sealed epoch copy disabled

- **WHEN** finalized epoch WAL remains only on local durable spool
- **THEN** retention correctness still uses local WAL/epoch manifest/checkpoint/final-session/parent proof and does not require a store copy

#### Scenario: Remote backend requested

- **WHEN** configuration requests an S3-compatible backend
- **THEN** recorder returns stable unsupported-backend result and local filesystem operation remains available

#### Scenario: Optional sealed WAL copy disabled

- **WHEN** the recorder retains sealed WAL only on its local durable spool
- **THEN** retention eligibility still uses validated local WAL/manifest/checkpoint/final-session proof and does not require a `RecordingStore` copy

### Requirement: Store failures are typed and bounded

Store operations SHALL classify permission, space/quota, integrity/lineage conflict, unsupported version/semantics, transient I/O, permanent I/O, and uncertain durability. Recorder/ETL SHALL use bounded retries only for explicitly idempotent operations and SHALL expose per-epoch lag/failure without captured values. It SHALL never invent success after timeout, retry overwrite, or delete local required epoch evidence because publication is uncertain.

#### Scenario: Epoch publication durability uncertain

- **WHEN** store cannot determine whether an immutable epoch put completed
- **THEN** retry directly verifies key/digest/provenance before reuse and epoch checkpoint does not advance on unresolved state

#### Scenario: Filesystem full during rollover ETL

- **WHEN** epoch artifact or final session publication fails for space
- **THEN** output/checkpoint/retention does not advance, successor WAL remains protected, and quota/lag diagnostics expose stable error

#### Scenario: Publication durability uncertain

- **WHEN** backend cannot determine whether immutable put completed
- **THEN** retry performs direct key/digest verification before reuse and checkpoint does not advance on unresolved state

#### Scenario: Filesystem full

- **WHEN** filesystem publication fails for space
- **THEN** output/checkpoint does not advance, source WAL remains protected, and quota/lag diagnostics expose stable error

### Requirement: Backend conformance is executable

The filesystem RecordingStore conformance suite SHALL cover parent/epoch key identity, put-if-absent, matching/conflicting retry, integrity read, bounded list, digest-preconditioned delete, path/key safety, crash staging, durability uncertainty, lifecycle behavior, and ownership-boundary rejection. It SHALL prove store cannot bypass WAL/epoch authority, advance incremental checkpoints, publish final sessions, or make an incomplete parent aggregate authoritative. Rootless fault injection SHALL not claim remote backend support.

#### Scenario: Epoch filesystem conformance

- **WHEN** conformance runs with injected write/sync/rename/delete failures across predecessor/successor epochs
- **THEN** final artifacts/checkpoints/parent references are absent or exact, retries converge, and no conflicting object is overwritten

#### Scenario: Filesystem conformance

- **WHEN** rootless conformance suite runs with injected write/sync/rename/delete failures
- **THEN** final objects/checkpoints are absent or exact, retries converge, and no conflicting object is overwritten

### Requirement: Parent aggregation references verified epoch artifacts

A parent aggregate/index SHALL be a versioned metadata artifact whose entries are ordered by authoritative epoch ordinal and contain verified session, checkpoint, continuation, manifest, and retention digests. It SHALL never embed unverified payload or promote source WAL.

#### Scenario: Aggregate rebuild

- **WHEN** process restart finds verified epoch outputs but a missing parent aggregate
- **THEN** application rebuilds the same ordered aggregate from manifests/checkpoints without duplicating epoch artifacts

#### Scenario: Aggregate conflict

- **WHEN** existing parent aggregate differs from verified epoch entry set
- **THEN** store/application reports lineage conflict and does not overwrite or mark parent complete

### Requirement: Continuation artifacts are immutable retention dependencies

Continuation-in/out artifacts SHALL be immutable, content-addressed, and referenced by the epoch manifest/checkpoint before retention transitions. A successor session or checkpoint may depend on its predecessor continuation artifact, but publication/retry SHALL verify exact digest and lineage and SHALL never overwrite or synthesize a replacement from filesystem order. Retention SHALL retain every continuation artifact required by a published or recoverable successor operation and SHALL retain predecessor source WAL/checkpoints while that artifact has not yet been produced or safely consumed.

#### Scenario: Continuation artifact retry

- **WHEN** retry encounters a matching continuation key
- **THEN** it verifies and reuses the artifact without duplicate state or overwrite

#### Scenario: Continuation dependency retention

- **WHEN** predecessor raw WAL is eligible for deletion but a retained successor references its continuation artifact
- **THEN** cleanup retains the required artifact/provenance and deletes only independently eligible source evidence
