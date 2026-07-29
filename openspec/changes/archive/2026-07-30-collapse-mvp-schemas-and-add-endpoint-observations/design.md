## Context

Chronicle remains pre-release. No capture event, WAL, canonical session, manifest, fixture, or report format has been published as a compatibility contract. Repository history nevertheless introduced parallel capture V1/V2 models, a legacy WAL v1 plus segmented WAL v2, canonical v2/v3 language, compatibility dispatch, and source-specific fixture/production paths.

Current production flow is:

`eBPF ABI -> RawKernelObservation -> CaptureAdapter -> CaptureEvent -> WAL -> ReconstructionInput -> ETL -> CanonicalSession`

`CaptureAdapter` already caches `SocketEvidence` by `SocketIdentity`, and `PayloadFragment` already references socket identity rather than carrying a tuple. Missing fields are local endpoint, remote endpoint, and active/passive role. Reconstruction currently discards socket evidence after lifecycle conversion, then creates production canonical connections with `client = unknown:0` and `server = unknown:0`.

Constraints:

- All internal artifact domains have one active schema, v1, until explicit compatibility freeze.
- Existing segmented/group-committed WAL behavior, loss evidence, socket identity, capture boundaries, and protocol-neutral layering remain valuable.
- Kernel ABI stays private to `chronicle-capture-ebpf`.
- Repository fixtures and goldens may be rewritten atomically; no compatibility reader is required.
- Endpoint evidence is captured once as socket evidence and is not copied into each payload fragment.

## Goals / Non-Goals

**Goals:**

- Establish and document mutable-v1 MVP versioning policy.
- Replace capture V1/V2 models and dispatch with one unversioned Rust model whose serialized `schema_version` is always 1.
- Keep current segmented, recoverable WAL behavior while making it sole WAL v1 format.
- Keep one Canonical v1 and one current manifest/report v1 shape.
- Add local/remote endpoints and active/passive role to Capture Event v1 and private ABI v1.
- Preserve socket evidence through reconstruction and produce real canonical client/server endpoints.
- Migrate all repository fixtures, goldens, tests, docs, and active planning artifacts together.

**Non-Goals:**

- Compatibility with any obsolete repository-only bytes or JSON.
- Runtime migrations, dual writes, down-conversion, or format negotiation.
- Inferring socket role from ingress/egress direction.
- Reconstructing application meaning in capture adapter or eBPF code.
- Adding protocol support, external storage, or new dependencies.
- Declaring compatibility freeze. That requires separate explicit OpenSpec change.

## Decisions

### 1. One mutable v1 per artifact domain

Every serialized internal artifact keeps explicit `schema_version: 1` or equivalent format-version field, but each domain has exactly one current model and one reader/writer path. A non-v1 value fails with typed unsupported-schema error. Evolution before compatibility freeze edits current v1 model and migrates repository artifacts in same change.

No domain retains V1/V2/V3 dispatch enum, compatibility adapter, migration default, legacy reader, or source-specific schema version. Root Rust domain types use unversioned names (`CaptureEvent`, `WalRecordEnvelope`, `CanonicalSession`, `SessionManifest`) even though serialized value remains v1.

Version may increase only after explicit OpenSpec change declares compatibility freeze, identifies contract scope, and defines reader/writer support policy.

**Alternative:** Preserve old readers because files already exist in git. Rejected: git history is not deployed compatibility surface, and dual paths increase defects without user value.

### 2. Collapse capture model into current evidence-oriented shape

Delete `CaptureEventV1`, `CaptureEventV2`, `CaptureEvent::V1/V2`, `CAPTURE_EVENT_V2_SCHEMA_VERSION`, `V2FixtureSource`, `V2FixtureObservation`, and conversion branches between source versions. Keep one model:

```rust
pub struct CaptureEvent {
    pub schema_version: u16, // validated as 1
    #[serde(flatten)]
    pub kind: CaptureEventKind,
}
```

Current evidence-oriented `CaptureEventKind` becomes v1 directly. Old flat fixture-only capture event shape is removed rather than retained as another capture version.

`SocketEvidence` gains:

```rust
pub local_endpoint: Endpoint,
pub remote_endpoint: Endpoint,
pub role: SocketRole,
```

`SocketRole` has only `Active` and `Passive`. Endpoint addresses are normalized textual IP values at capture-domain boundary and ports are non-zero `u16` values. Network family must match both addresses.

Linux connect hooks run before TCP autobind, so local ephemeral port is not reliably available. `SocketConnectObserved` therefore carries separate `SocketConnectIntent` containing identity, timestamp, scope, family, remote endpoint, active role, and process evidence, but no claimed local endpoint. `SocketConnected(SocketEvidence)` is first complete endpoint-provenance event for active and passive sockets.

`PayloadFragment` gains no endpoint or role fields. It continues carrying socket identity and transport evidence. Capture adapter may retain pending connect intent privately, but stores only complete endpoint evidence in existing `BTreeMap<SocketIdentity, SocketEvidence>` and correlates payload/lifecycle observations through complete cache.

**Alternative:** Add Capture Event v3 or optional endpoint fields to old V2. Rejected: no frozen v2 exists, and optional provenance would preserve C3.1 failure.

### 3. Extend private ABI v1 in place

Keep `CHRN`, little-endian marker, and ABI version value 1. Modify producer and userspace decoder together. Socket-evidence item layout includes:

- address family;
- 16-byte local address;
- little-endian local port;
- 16-byte remote address;
- little-endian remote port;
- one-byte role discriminant (`active` or `passive`);
- existing timestamp, socket cookie, first-seen timestamp, namespace, cgroup, PID, and TID evidence.

IPv4 uses first four address bytes with remaining bytes zero; IPv6 uses all sixteen. Userspace validates family, address encoding, non-zero ports, known role, item size, magic, byte order, and ABI value before constructing `SocketEvidence`.

Connect records carry remote intent only because local ephemeral port is not guaranteed before autobind. Establishment records carry complete local/remote endpoint tuple and role. Payload records retain bounded identity/transport/payload layout and do not repeat endpoint tuple. Active establishment completes pending connect intent; passive establishment creates complete cache entry with passive role. eBPF sock-ops handling therefore covers both active and passive established callbacks. Contradictory endpoint/role evidence for same `SocketIdentity` is typed invalid evidence, not silently overwritten.

**Alternative:** Introduce ABI v2. Rejected: producer and consumer ship together and no ABI v1 compatibility contract exists.

**Alternative:** Decode IP/TCP headers from every skb to recover endpoints. Rejected: duplicates tuple data on every fragment, expands verifier-sensitive code, and makes direction appear authoritative for role.

### 4. Fixture and production sources emit same Capture Event v1

Fixture artifact remains fixture schema v1, but fixture adapter emits same `CaptureEvent` model as production. Each fixture connection definition produces deterministic synthetic `SocketIdentity` plus one `SocketEvidence` carrying endpoints and role; payload events reference that identity. Existing fixture client/server definitions map to active role with local=client and remote=server unless fixture explicitly models passive role.

Remove fixture-v1-to-production-v2 normalization and fixture-only `CaptureEventV1` assembly. Fixture sequence remains source ordering evidence, not separate capture schema.

**Alternative:** Keep fixture events on old CaptureEventV1 because fixtures are deterministic. Rejected: source type must not define schema version and parallel assembly paths recreate same complexity.

### 5. Reconstruction caches socket provenance and role establishes sides

Reconstruction input preserves socket evidence rather than reducing lifecycle events to identity alone. Ordered processing maintains bounded map:

`SocketIdentity -> { local_endpoint, remote_endpoint, role, recording_scope, family }`

Socket-evidence events insert or validate entry. Payload events resolve entry by identity. Missing evidence, conflicting tuple, conflicting family, or conflicting role yields typed reconstruction error; ETL never substitutes `unknown:0`.

Canonical endpoint assignment is:

- active socket: `client = local`, `server = remote`;
- passive socket: `client = remote`, `server = local`.

Ingress/egress may map payload bytes to client-to-server or server-to-client only after role establishes sides. It never chooses which endpoint is client or server. Tests use same ingress/egress direction with opposite roles to prove endpoint assignment is role-driven.

Socket cache is recording-bounded and cleared between recordings. Socket identity remains boot ID + socket cookie + first-seen generation evidence, so tuple or PID reuse cannot merge connections.

**Alternative:** Infer client from egress and server from ingress. Rejected: direction changes per byte flow and cannot establish stable endpoint roles.

### 6. Current segmented WAL becomes sole WAL v1

Retain segmented append-only framing, group commit, commit markers, checksums, recovery, loss windows, and current envelope semantics. Change its declared format value to 1 and make it sole implementation. Delete legacy P0 v1 codec/reader, `VersionedWalReader`, `VersionedReaderInner`, `V2WalReader`, v2 constants/file helpers, and dispatch tests. Rename surviving types/helpers to unversioned or v1 names as appropriate.

All WAL record-kind payload schema values are 1. Reader accepts only current segmented WAL v1 and rejects any other format value. Checked-in WAL bytes and tests are regenerated; obsolete legacy bytes are deleted.

**Alternative:** Keep old v1 and rename segmented format v3. Rejected: preserves unused compatibility burden and contradicts MVP policy.

### 7. Canonical, manifests, and reports remain current v1

`CANONICAL_SCHEMA_VERSION` remains 1. Current complete canonical model is serialized as v1 directly. Remove Canonical v2/v3 DTOs, namespaces, fallback defaults, and version dispatch where present. `CanonicalConnection` continues storing client/server endpoints; ETL now supplies observed values and may preserve socket provenance in existing attributes/source metadata without another canonical version.

Current session manifest, recording metadata, checkpoint, and machine-readable command report shapes remain or become v1. Delete obsolete readers and migrate checked-in JSON. Protocol payload models have one active v1 representation; multiple root artifact schema dispatch is not introduced.

**Alternative:** Keep canonical compatibility because storage tests mention old v1/v2. Rejected: tests describe repository history, not shipped contract.

### 8. Architecture and active planning docs become authoritative

Add MVP Versioning Policy to `docs/architecture.md` and align `docs/canonical-model.md` and `docs/wal-format.md` with sole current v1 formats. Remove claims that fixture and production capture use different Capture Event versions or that old internal artifacts remain readable.

Update conflicting unarchived OpenSpec artifacts, especially `add-production-http-recording-pipeline`, so future task execution does not reintroduce Capture Event v2, WAL v2, canonical v2/v3, compatibility DTOs, or migration readers.

## Risks / Trade-offs

- **Repository fixtures become unreadable by older commits** -> Expected; git checkout supplies matching code and fixtures.
- **ABI producer/decoder mismatch during development** -> Land producer, decoder, embedded object, size tests, and fixture bytes atomically; ABI header still rejects mismatch.
- **Passive socket evidence is incomplete on some hooks/kernels** -> Gate acceptance on proven active and passive establishment evidence; fail typed rather than invent role/endpoints.
- **Socket evidence can be lost before payload** -> Existing adapter cache requirement prevents emitting uncorrelated payload; loss remains explicit and no `unknown:0` fallback is allowed.
- **Broad rename causes temporary compile breakage** -> Implement domain-by-domain in dependency order and keep each task with focused tests.
- **Another active change reintroduces version proliferation** -> Update its artifacts in same repository migration and add policy-focused repository checks.

## Migration Plan

Repository-only migration; no deployed data migration or dual-read phase.

1. Add architecture policy and inventory all schema constants, versioned DTOs, dispatch enums, compatibility readers, fixtures, goldens, and active OpenSpec references.
2. Define unversioned Capture Event v1 plus endpoint/role types; migrate fixture and production constructors; delete old capture variants and adapters.
3. Change eBPF producer ABI v1 and userspace decoder together; rebuild embedded object; add active/passive endpoint evidence tests.
4. Preserve socket evidence through reconstruction; replace `unknown:0` path with role-based endpoint assignment and typed missing/conflict errors.
5. Make current segmented WAL sole v1; remove legacy reader/dispatch; regenerate WAL fixtures and recovery goldens.
6. Collapse canonical, manifest, checkpoint, and report code/docs to current v1; regenerate stored-session and CLI goldens.
7. Update all repository tests, fixtures, architecture docs, and conflicting active OpenSpec artifacts in same change.
8. Run workspace, format, lint, OpenSpec validation, portable ABI tests, and privileged Linux acceptance.

Rollback before merge is source-control revert only. No runtime downgrade path or obsolete reader is retained.

## Testing Plan

- Capture serde round-trip for sole `CaptureEvent` v1; non-v1 rejection; repository search proves no capture V1/V2 dispatch remains.
- ABI byte-layout tests for IPv4/IPv6, active/passive role, local/remote ports, exact sizes, malformed family/address/port/role, and producer/decoder parity.
- Adapter tests prove endpoint evidence enters socket cache once and payload fragments contain identity but no endpoint tuple.
- Fixture tests prove fixture and production sources emit same Capture Event type and both feed same reconstruction path.
- Reconstruction/ETL tests cover active and passive assignment, bidirectional payload, socket reuse, missing evidence, conflicting evidence, and absence of `unknown:0`.
- WAL tests cover current v1 segmented append/read, group commit, recovery, loss windows, corruption rejection, non-v1 rejection, and absence of legacy dispatch.
- Canonical/storage tests cover current v1 round-trip, checksum/hydration, endpoint persistence, non-v1 rejection, and no old-version fallback.
- Golden/CLI tests use only migrated v1 fixtures and deterministic output.
- Privileged acceptance proves IPv4 and IPv6 endpoint/role evidence on supported environment, including active and passive sockets.
- `cargo fmt --check`, workspace tests, clippy policy, `openspec validate`, and graph update pass.

## Acceptance Criteria

- Every internal artifact domain declares exactly one active schema/format value: 1.
- No Capture Event V1/V2 structs or dispatch enum remains; serialized event is `CaptureEvent { schema_version: 1, kind }`.
- No WAL v2 constant, versioned reader, or legacy WAL v1 compatibility path remains; current segmented format is WAL v1.
- Canonical writer/reader and stored artifacts use only current Canonical v1; no Canonical v2/v3 dispatch or fallback remains.
- ABI header remains version 1 and socket evidence contains validated local endpoint, remote endpoint, and active/passive role.
- Payload fragments contain no endpoint tuple and correlate through socket identity/cache.
- ETL assigns canonical endpoints from cached evidence and socket role, never from ingress/egress alone, and never emits `unknown:0`.
- Fixture and production inputs converge on same capture/reconstruction model.
- All repository fixtures, goldens, tests, docs, and active OpenSpec artifacts follow policy.
- Version increase is blocked by documented requirement for explicit compatibility-freeze OpenSpec change.

## Open Questions

None. Compatibility policy, endpoint fields, role semantics, and repository-only migration are explicit change constraints.
