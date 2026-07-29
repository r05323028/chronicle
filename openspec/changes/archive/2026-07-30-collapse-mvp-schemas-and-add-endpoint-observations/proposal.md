## Why

Chronicle has no released artifacts or compatibility commitments, yet internal capture, canonical, and WAL models already carry parallel V1/V2/V3 representations and compatibility dispatch. This premature versioning increases implementation and test cost while production capture still cannot satisfy C3.1 because socket evidence omits endpoint provenance and ETL emits `unknown:0` endpoints.

## What Changes

- **BREAKING** Collapse every internal artifact domain to one mutable schema v1 for MVP: capture events, eBPF ABI, WAL, canonical sessions, manifests, fixtures, and related JSON reports.
- **BREAKING** Delete obsolete V1/V2/V3 DTOs, dispatch enums, compatibility readers, migration layers, and repository artifacts for superseded internal formats.
- Establish an architecture policy that schema versions remain at v1 until an explicit OpenSpec compatibility-freeze change authorizes later versions.
- Replace version-specific capture models with one `CaptureEvent { schema_version: 1, kind: CaptureEventKind }`.
- Extend Capture Event v1 `SocketEvidence` and eBPF ABI v1 with local endpoint, remote endpoint, and active/passive socket role.
- Keep payload fragments keyed only by socket identity; record endpoint evidence once and reuse it through the socket cache.
- Reconstruct canonical client/server endpoints from cached socket evidence, assigning sides from socket role rather than ingress/egress direction.
- Migrate repository fixtures, golden files, documentation, and tests atomically. No backward-compatible readers remain for repository history.

## Capabilities

### New Capabilities
- `mvp-schema-versioning`: Defines one mutable v1 per internal artifact domain, repository-only migration, and explicit compatibility-freeze gate.
- `endpoint-provenance-reconstruction`: Defines cached endpoint evidence and role-based canonical client/server reconstruction across capture, WAL, and ETL.

### Modified Capabilities
- `ebpf-capture-adapter`: Extends Capture Event v1 and ABI v1 with endpoint/socket-role evidence while removing version-specific capture dispatch.
- `fixture-recording-pipeline`: Migrates fixtures and fixture capture onto the same active Capture Event v1 used by production.
- `http11-operations`: Replaces Canonical v1/v2 compatibility behavior with one mutable Canonical v1 contract.
- `local-session-artifacts`: Migrates canonical manifests and stored sessions to current v1 and removes obsolete compatibility readers.

## Impact

- **Code:** `chronicle-common`, `chronicle-capture`, `chronicle-capture-ebpf` including eBPF program ABI, `chronicle-wal`, `chronicle-session`, `chronicle-etl`, `chronicle-canonical`, `chronicle-protocol-builtins`, `chronicle-storage`, `chronicle-application`, and CLI format handling.
- **Artifacts:** All checked-in capture fixtures, WAL fixtures, canonical sessions, manifests, golden output, and schema-version tests are regenerated or rewritten as v1.
- **Architecture:** `docs/architecture.md`, canonical-model documentation, WAL format documentation, and active production-pipeline design language must reflect mutable MVP v1 policy and endpoint-evidence flow.
- **Compatibility:** Intentionally none for obsolete repository-only schemas. Non-v1 input is rejected; no old-version conversion path is retained.
- **Dependencies:** No new dependency or service. Existing socket cache, capture boundary, WAL envelope, and reconstruction pipeline are extended in place.
