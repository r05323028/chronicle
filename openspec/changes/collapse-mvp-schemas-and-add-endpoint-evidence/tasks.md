## 1. Policy and Schema Inventory

- [x] 1.1 Add authoritative mutable-v1 MVP Versioning Policy and explicit compatibility-freeze gate to `docs/architecture.md`.
- [x] 1.2 Inventory capture, ABI, WAL, canonical, manifest, checkpoint, fixture, report version constants/types/readers and checked-in artifacts; identify sole current v1 representation and obsolete paths to delete.
- [x] 1.3 Add repository-level checks or focused tests that reject non-v1 inputs and prevent active internal V1/V2/V3 dispatch enums.

## 2. Capture Event v1 Collapse

- [x] 2.1 Add `SocketRole::{Active, Passive}` plus validated local and remote `Endpoint` fields to `SocketEvidence`.
- [x] 2.2 Replace `CaptureEventV1`, `CaptureEventV2`, and `CaptureEvent::V1/V2` with single `CaptureEvent { schema_version: 1, kind }`; remove v2 constants and compatibility serialization/decoding.
- [x] 2.3 Update capture constructors, codecs, sources, consumers, and errors to use sole Capture Event v1 and reject any other schema value.
- [x] 2.4 Remove `V2FixtureSource`, `V2FixtureObservation`, and source-version normalization paths.
- [x] 2.5 Add capture unit tests for v1 round-trip, non-v1 rejection, endpoint/family/role validation, and absence of version dispatch.

## 3. eBPF ABI v1 Endpoint Evidence

- [x] 3.1 Document and implement bounded ABI v1 layouts for pre-autobind connect intent plus established IPv4/IPv6 local address/port, remote address/port, and active/passive role without adding ABI v2.
- [x] 3.2 Update eBPF producer to populate active remote connect intent and complete final establishment endpoint evidence from connect/sock-ops contexts.
- [x] 3.3 Add passive-established sock-ops handling that emits passive socket endpoint evidence and stable socket identity.
- [x] 3.4 Update userspace `RawKernelObservation` decoder for new ABI v1 sizes/fields and typed rejection of malformed family, address, port, role, size, byte order, or version.
- [x] 3.5 Update `CaptureAdapter` socket cache to insert/refresh complete endpoint evidence, reject conflicting tuple/role for same identity, and keep payload fragments endpoint-free.
- [x] 3.6 Rebuild embedded eBPF object and update ABI hashes/metadata consumed by preflight and acceptance paths.
- [x] 3.7 Add portable producer/decoder byte-layout and adapter-cache tests for IPv4, IPv6, active, passive, missing evidence, and conflicts.
- [x] 3.8 Extend privileged Linux adapter acceptance to prove active and passive endpoint/role evidence while preserving bounded payload and loss behavior.

## 4. Fixture v1 and Source Parity

- [x] 4.1 Modify fixture schema v1 connection definitions in place to carry deterministic socket identity, local/remote endpoints, and socket role; keep non-v1 rejection.
- [x] 4.2 Make fixture source emit socket evidence before payload fragments using same `CaptureEvent` type and reconstruction path as production.
- [x] 4.3 Migrate every checked-in fixture JSON and capture golden to current fixture v1/Capture Event v1 shape; delete obsolete fixture bytes.
- [x] 4.4 Replace fixture-only `ConnectionKey`/`CaptureEventV1` assembly path with bounded socket-identity assembly shared with production.
- [x] 4.5 Add parity tests proving equivalent fixture and production evidence enters same capture/reconstruction model without version conversion.

## 5. WAL v1 Collapse

- [x] 5.1 Make current segmented, group-committed, recoverable WAL implementation sole WAL format v1 while preserving commit markers, checksums, loss windows, bounds, and recovery semantics.
- [x] 5.2 Delete legacy P0 WAL codec/reader and `VersionedWalReader`, `VersionedReaderInner`, `V2WalReader`, WAL v2 constants/helpers, and format-dispatch branches.
- [x] 5.3 Rename surviving version-specific WAL types/helpers/constants to unversioned or sole-v1 names and update all callers/metadata.
- [x] 5.4 Set every active WAL envelope and record-kind schema field to 1 and return typed error for any other declared format/schema.
- [x] 5.5 Regenerate checked-in WAL fixtures/goldens for current v1 and remove tests asserting obsolete format readability.
- [x] 5.6 Add WAL v1 tests for segmented append/read, group commit, rotation, recovery, corruption, terminal loss, non-v1 rejection, and no compatibility dispatch.

## 6. Reconstruction and ETL Endpoint Provenance

- [x] 6.1 Extend reconstruction-domain input to preserve `SocketEvidence` and maintain recording-bounded socket provenance cache keyed by `SocketIdentity`.
- [x] 6.2 Collapse `ReconstructionInput::from_capture_event` to sole Capture Event v1 path and remove fixture-v1/production-v2 branches.
- [x] 6.3 Resolve payload fragments through cached evidence; return typed missing/conflicting endpoint provenance errors instead of synthesizing values.
- [x] 6.4 Map active sockets as local=client/remote=server and passive sockets as remote=client/local=server; use ingress/egress only for byte direction after role assignment.
- [x] 6.5 Replace ETL production `unknown:0` endpoint construction with cached role-based endpoint reconstruction and preserve socket provenance in existing canonical metadata.
- [x] 6.6 Add reconstruction/ETL tests for active/passive IPv4 and IPv6, bidirectional payload, same-direction opposite-role assignment, socket reuse, missing evidence, conflicting evidence, and no `unknown:0` output.

## 7. Canonical, Storage, Metadata, and Reports v1

- [x] 7.1 Keep current complete `CanonicalSession` as sole Canonical v1; remove canonical v2/v3 DTOs, dispatch, migration defaults, and stale constants/namespaces.
- [x] 7.2 Update canonical validation and protocol-data handling so writer/reader accept only current v1 and preserve reconstructed endpoint provenance.
- [x] 7.3 Collapse session manifest, recording metadata, ETL checkpoint, and machine-readable command result models to one current v1 each; delete obsolete compatibility readers.
- [x] 7.4 Migrate canonical sessions, manifests, metadata, checkpoint, inspect/replay JSON goldens, and related checksums together.
- [x] 7.5 Add canonical/storage tests for v1 round-trip, endpoint persistence, checksum/hydration, non-v1 rejection, and absence of old-version fallback.

## 8. Application and CLI Integration

- [x] 8.1 Update application recording, WAL ingestion, ETL, inspect, replay, doctor, and metadata code to sole capture/WAL/canonical v1 APIs and constants.
- [x] 8.2 Update CLI contracts and test helpers to construct current v1 artifacts only; remove V1/V2/V3 dispatch expectations.
- [x] 8.3 Verify fixture and production record flows publish same current v1 artifact family and typed failures remain safe and bounded.

## 9. Documentation and Active OpenSpec Reconciliation

- [x] 9.1 Rewrite `docs/canonical-model.md` to describe sole mutable Canonical v1 and remove v2/v3 compatibility language.
- [x] 9.2 Rewrite `docs/wal-format.md` to describe current segmented format as sole WAL v1 and remove legacy/v2 reader guidance.
- [x] 9.3 Update capture/eBPF/ETL documentation with endpoint-evidence cache and role-based client/server assignment; remove `unknown:0` as valid production behavior.
- [x] 9.4 Reconcile unarchived `add-production-http-recording-pipeline` proposal, design, specs, and remaining tasks so they no longer require Capture Event v2, WAL v2, Canonical v2/v3, source-version adapters, or compatibility readers.
- [x] 9.5 Update README and contributor guidance where they describe schema versions or artifact compatibility.

## 10. Validation and Acceptance

- [x] 10.1 Run focused capture, ABI, fixture, WAL, reconstruction, ETL, canonical, storage, application, and CLI tests after each domain migration.
- [x] 10.2 Run `cargo fmt --check`, full workspace tests, configured clippy policy, eBPF build/object checks, and supported-platform compile checks.
- [x] 10.3 Run privileged Linux IPv4/IPv6 active/passive acceptance and record exact verified environment without widening compatibility claims.
  - Verified 3/3 ignored adapter tests on 2026-07-29 in Multipass `chronicle-ubuntu`: Ubuntu 24.04.4 LTS, Linux `6.8.0-136-generic`, `aarch64`, cgroup v2, readable BTF, privileged root. Other environments remain unverified.
- [x] 10.4 Run repository searches proving no active Capture Event V1/V2 dispatch, WAL v2/versioned reader, Canonical v2/v3 dispatch, or production `unknown:0` endpoint fallback remains.
- [x] 10.5 Run `openspec validate collapse-mvp-schemas-and-add-endpoint-evidence` and validate reconciled active change artifacts.
- [x] 10.6 Run `graphify update .` after code/document changes and confirm graph report reflects sole-v1 architecture and endpoint provenance flow.
