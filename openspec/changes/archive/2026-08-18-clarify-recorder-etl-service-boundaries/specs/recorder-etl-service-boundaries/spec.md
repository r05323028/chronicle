## ADDED Requirements

### Requirement: Recorder and ETL are distinct logical service boundaries

Chronicle SHALL define Recorder, Local WAL, Durable Evidence Store, ETL, and Canonical Store as distinct logical components. Recorder SHALL own capture, protocol-neutral evidence, local WAL append/commit/recovery, segment and epoch rollover, capture-side loss accounting, and future durable evidence shipping, and MUST NOT own protocol decoding, canonical operation semantics, causal request correlation, replay semantics, or final canonical storage layout. ETL SHALL own reconstruction, protocol decoding, canonicalization, incremental and final publication, publication verification, and checkpoint advancement ordering. Recorder and ETL MAY be co-located in the current local deployment, but their correctness MUST NOT depend on sharing a process, memory, capture ownership, or a local filesystem namespace. The current co-located filesystem implementation SHALL be documented as a deployment choice rather than the permanent architecture.

#### Scenario: A future ETL worker runs without the Recorder process

- **WHEN** an independently deployed ETL worker processes evidence published by a Recorder that has already exited
- **THEN** no architecture invariant is violated and ETL correctness does not require the Recorder process, its memory, or its capture ownership

#### Scenario: Maintainer reads the architecture overview

- **WHEN** a maintainer reads the canonical architecture documentation
- **THEN** it distinguishes Recorder, Local WAL, Durable Evidence Store, ETL, and Canonical Store and states that co-location is an implementation choice

### Requirement: Local WAL is the capture durability and recovery authority

Local WAL append, commit, and recovery SHALL remain the capture durability authority. Remote or object evidence storage is a durable handoff/distribution boundary and MUST NOT replace local WAL durability in the capture hot path. `chronicle-wal` SHALL remain independent of S3 or any other remote storage implementation.

#### Scenario: Evidence is shipped to a remote store

- **WHEN** recording evidence is published to a future S3-compatible store
- **THEN** the local WAL remains the recovery authority for capture and remote storage does not enter the capture hot path

#### Scenario: Remote storage dependency proposed in WAL

- **WHEN** a change proposes adding an object-store client dependency to `chronicle-wal`
- **THEN** the documented dependency rule and dependency-review process reject the proposal, and the remote store remains a downstream handoff boundary

### Requirement: ETL consumes recovery-authoritative evidence through a durable evidence contract

ETL SHALL consume recovery-authoritative recording evidence. A local filesystem WAL path is the current implementation transport and is NOT the permanent logical ETL contract. ETL MUST NOT require ownership of the capture runtime, eBPF attachment, Recorder process lifecycle, Recorder process memory, or a Recorder-local filesystem namespace. ETL SHALL remain responsible for connection reconstruction, cross-segment reconstruction, cross-epoch continuation, protocol detection/decoding, canonical operations, incremental deltas, canonical publication, publication verification, and safe checkpoint advancement. ETL SHALL remain a complete Extract-Transform-Load boundary; publication semantics MUST NOT move into `chronicle-application`.

#### Scenario: ETL reads from a durable copy rather than the live WAL directory

- **WHEN** ETL processes evidence obtained from a durable evidence store instead of the Recorder-owned WAL path
- **THEN** the ETL contract is satisfied because the input is recovery-authoritative evidence, without owning the Recorder runtime

#### Scenario: Publication semantics moved to application

- **WHEN** a change moves canonical publication or checkpoint ordering out of ETL into `chronicle-application`
- **THEN** it is rejected: ETL owns canonical publication, publication verification, and checkpoint advancement ordering

### Requirement: Durable Evidence Store is a handoff boundary

A Durable Evidence Store boundary SHALL exist between Recorder and ETL with responsibilities: receive immutable recording evidence, preserve checksums, preserve explicit parent/epoch lineage, expose durable evidence for later processing, support idempotent publication semantics, and allow Recorder and ETL lifecycles to be independent. The current implementation MAY use filesystem-backed artifacts; future implementations MAY use an S3-compatible object store. The Durable Evidence Store is NOT the local WAL durability authority, a protocol decoder, a canonical model owner, or a replay component.

#### Scenario: Recorder and ETL lifecycles diverge

- **WHEN** the Recorder publishes evidence and exits before ETL processes it
- **THEN** the evidence store preserves the evidence with checksums and lineage for later ETL processing

#### Scenario: Evidence store decodes protocols

- **WHEN** protocol decoding is proposed inside the evidence store
- **THEN** it is rejected: the evidence store is not a protocol decoder

### Requirement: Canonical Store persists canonical evidence independently

A Canonical Store boundary SHALL persist canonical sessions and canonical payload artifacts, expose canonical evidence for inspect/replay, and preserve immutable publication semantics. `FilesystemSessionStore` is the current implementation and is NOT a permanent requirement; future implementations MAY use other storage backends. Replay SHALL consume canonical evidence and MUST NOT require Recorder, WAL, ETL, or Evidence Store internals.

#### Scenario: Replay after capture/WAL evidence is unavailable

- **WHEN** replay operates on persisted canonical sessions without the original WAL or evidence store
- **THEN** replay remains functional because it depends only on canonical evidence

#### Scenario: Canonical store backend replaced

- **WHEN** a future canonical store implementation replaces the filesystem backend
- **THEN** the architecture does not treat `FilesystemSessionStore` as a permanent requirement

### Requirement: Storage boundaries do not define protocol boundaries

WAL segment, WAL epoch, object-store object, and ETL batch boundaries MUST NOT define protocol or logical interaction boundaries. Cross-epoch state requires bounded, checksummed, lineage-verified continuation evidence; a WAL epoch boundary is not a protocol reconstruction boundary.

#### Scenario: Cross-epoch session reconstruction

- **WHEN** a session continues across an epoch boundary
- **THEN** reconstruction uses continuation evidence and the epoch boundary itself is not treated as a protocol boundary

### Requirement: Domain lock is a local coordination mechanism

`.chronicle-domain.lock` SHALL remain a local filesystem/domain coordination mechanism that protects name claim, capture, ETL, publication, and catalog update as one transaction within one local deployment. Its current behavior MUST NOT be weakened. It MUST NOT be defined as a universal distributed transaction lock, and a single filesystem lock is NOT required to span future processes, nodes, object stores, or ETL workers. Future systems MAY define independent authority domains: Recorder local authority (local lease/lock, capture ownership, local WAL, WAL recovery, local shipping state), Evidence Store authority (immutable evidence publication, checksums, lineage, object visibility), and ETL authority (worker ownership/lease, incremental processing checkpoint, continuation processing, output-before-checkpoint ordering, canonical publication).

#### Scenario: Two nodes publish to one object store

- **WHEN** a future deployment uses independent Recorder and ETL workers without a shared filesystem
- **THEN** `.chronicle-domain.lock` is not required to coordinate them and no invariant claims it must

#### Scenario: Local recorder still uses the domain lock

- **WHEN** a second process targets the same local filesystem domain
- **THEN** the existing domain-lock behavior continues to reject the second owner

### Requirement: Architecture documentation matches the implementation

Canonical architecture documentation SHALL describe the actual current state: incremental ETL during recording, committed-prefix-only processing, canonical delta batches, durable incremental checkpoints, epoch rollover, cross-epoch continuation, multi-epoch parent recordings, and final authoritative one-shot ETL/publication. The documented crate dependency graph SHALL match the actual current Cargo graph and `validation/architecture.toml`. Historical dependency problems MUST NOT be presented as the current dependency graph; they belong in Git history and archived OpenSpec changes. Documentation SHALL NOT state that ETL is one final pass only, that one recording always produces one session, or that Recorder and ETL necessarily share a process.

#### Scenario: Stale graph presented as current

- **WHEN** crate-boundary documentation shows old problem edges (`chronicle-session -> chronicle-wal`, `chronicle-cli` to lower-layer crates) as the current graph
- **THEN** the documentation is corrected to the actual current graph and the historical state is not presented as current

#### Scenario: One-final-ETL wording found

- **WHEN** canonical documentation describes ETL only as a final one-shot pass
- **THEN** it is updated to also describe incremental ETL, checkpoints, continuation, multi-epoch parent recordings, and the final one-shot publication
