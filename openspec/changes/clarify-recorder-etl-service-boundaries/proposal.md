## Why

Chronicle is approaching its first public release, and the production pipeline is currently one co-located process: `ContinuousRecorderService` owns both Recorder state (capture, local WAL, epochs) and incremental ETL state (`IncrementalWorker`, `IncrementalProcessor`, protocol registry, incremental checkpoints, continuation state) over filesystem-backed storage. That is a valid local deployment choice, but the architecture documentation must not let it become a permanent correctness contract. The intended long-term production architecture separates Recorder, Local WAL, a Durable Evidence Store, an independent ETL worker, and a Canonical Store.

Current documentation also drifts from the implementation in three ways:

1. `docs/architecture.md` describes ETL as consuming "the final recovery-authoritative prefix" and publishing "one deterministic Canonical Session v1", which no longer describes the implementation: ETL also runs incrementally during recording over the committed WAL prefix, publishes canonical delta batches, persists durable incremental checkpoints, processes epoch rollover and cross-epoch continuation, supports multi-epoch parent recordings, and performs a final authoritative one-shot publication.
2. `docs/architecture/crate-boundaries.md` still presents a "Current graph (implementation start, 2026-08-11)" containing old problem edges (`chronicle-session -> chronicle-wal`, `chronicle-cli` -> lower-layer crates) that the Cargo graph no longer has and that `validation/architecture.toml` forbids.
3. `.chronicle-domain.lock` is described as protecting "name claim, capture, ETL, publication, and catalog update as one transaction", which is correct for the local filesystem deployment but must not become a universal distributed architecture invariant.

This change establishes durable logical boundaries before the first public release. It is architecture/specification and documentation only; it implements no runtime code.

## What Changes

- Defines four logical components - Recorder, Durable Evidence Store, ETL, Canonical Store - plus the Local WAL as the capture durability authority, and states that current co-location is an implementation/deployment choice rather than a permanent correctness dependency.
- Adds normative architecture invariants: Recorder/ETL separation, local WAL authority, ETL independence, storage-boundary independence from protocol boundaries, ETL ownership of publication/verification/checkpoint ordering, and replay independence.
- Documents `.chronicle-domain.lock` as a local filesystem/domain coordination mechanism and names the future independent authority domains (Recorder local authority, Evidence Store authority, ETL authority).
- Updates `docs/architecture.md` so the recording pipeline and ETL overview accurately describe current incremental ETL, committed-prefix processing, delta batches, durable checkpoints, epoch rollover, cross-epoch continuation, multi-epoch parent recordings, and final one-shot publication, without turning the overview into implementation documentation.
- Updates `docs/architecture/crate-boundaries.md` so the current graph is the actual current Cargo graph (target graph already equals it); historical dependency problems move out of the current architecture contract.
- Adds concise durable rules to `AGENTS.md`; keeps `validation/architecture.toml` unchanged (the existing allowlist already represents the desired direction).
- No runtime code, no new crates, no crate splits, no public CLI change, no recording behavior change, no WAL/checkpoint/canonical format change, no S3 or distributed implementation.

## Capabilities

### New Capabilities

- `recorder-etl-service-boundaries`: durable Recorder / Local WAL / Durable Evidence Store / ETL / Canonical Store logical boundaries, the co-location and independence invariants, domain-lock scope, and the requirement that architecture documentation match the implementation.

### Modified Capabilities

None. `workspace-dependency-boundaries` already captures the crate-level direction (including ETL as complete Extract-Transform-Load and the current acyclic graph) and remains accurate.

## Scope

- Architecture/specification documentation only: `docs/architecture.md`, `docs/architecture/crate-boundaries.md`, `AGENTS.md`, and the new capability spec.
- Verify that `validation/architecture.toml` already represents the desired crate direction and leave it unchanged.

## Non-goals (explicitly deferred)

- S3 implementation, MinIO implementation, object-store authentication.
- WAL shipper implementation, object discovery/polling protocol, event notifications.
- Distributed ETL worker scheduling, distributed leases, Kubernetes manifests, horizontal scaling.
- PostgreSQL metadata repository, new storage adapters.
- Causal request correlation, new protocol support.
- Public ETL CLI (the hidden `chronicle internal etl` operational surface stays as-is).
- Migration layers for unreleased internal formats.
- Splitting the current co-located runtime, splitting configuration structs, or introducing speculative runtime APIs.

No placeholder implementation code is added for any deferred item.

## Implementation implications

- This change touches documentation and one new capability spec only; it does not alter Cargo dependencies, persisted formats, CLI surface, or runtime behavior.
- The existing seams named in the design (`RecordingStore`/artifact abstractions in `chronicle-storage`, ETL-owned publication/checkpoint helpers in `chronicle-etl`, `MetadataRepository`/`ArtifactStore` traits) are documented as future seams; none are implemented or reshaped here.
- Future independently deployed ETL must be able to operate without Recorder process lifecycle, process memory, local filesystem namespace, or configuration ownership; no current artifact is changed to enable that yet.

## Acceptance criteria

1. Canonical architecture documentation clearly distinguishes Recorder, Local WAL, Durable Evidence Store, ETL, and Canonical Store.
2. The current co-located filesystem implementation is explicitly described as a current deployment choice rather than the permanent architecture.
3. No architecture invariant requires Recorder and ETL to share a process, memory, capture ownership, or a local filesystem namespace.
4. `.chronicle-domain.lock` is documented as a local filesystem/domain coordination mechanism rather than a universal distributed transaction lock.
5. Local WAL remains the capture durability and recovery authority.
6. Remote Evidence Store is explicitly defined as a durable handoff/distribution boundary.
7. ETL remains responsible for reconstruction, protocol decoding, canonicalization, publication, verification, and checkpoint ordering.
8. Replay remains independent from Recorder, WAL, ETL, and evidence-store internals.
9. WAL/segment/epoch/object boundaries do not define protocol reconstruction boundaries.
10. Architecture documentation accurately reflects the current incremental ETL, checkpoint, continuation, multi-epoch, and final one-shot behavior.
11. Historical dependency problems are not presented as the current dependency graph.
12. The actual current crate dependency graph remains acyclic and compliant with `validation/architecture.toml`.
13. No public CLI behavior changes.
14. No runtime behavior changes.
15. No persisted schema changes.
16. No S3 or distributed ETL implementation is introduced.

## Validation plan

- `openspec validate --all --strict --no-interactive`.
- `./scripts/validate.sh fast` (includes architecture and tooling meta-validation).
- Compare the documented crate graph (`docs/architecture.md` Boundaries, `docs/architecture/crate-boundaries.md`) against actual `cargo metadata` output.
- Search canonical documentation for stale statements about: one final ETL only; one recording always producing one session; Recorder and ETL necessarily sharing a process; `.chronicle-domain.lock` acting as a universal distributed lock.
- Verify `AGENTS.md`, `docs/architecture.md`, and `docs/architecture/crate-boundaries.md` agree on the durable architecture rules.

No unrelated implementation work is performed in this change.
