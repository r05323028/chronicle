## Why

Chronicle currently proves its architecture only with an in-memory fake protocol, so it cannot demonstrate that recorded socket bytes can become durable, inspectable operations and execute safely against a real target. This change delivers first runnable, dependency-light HTTP/1.1 slice while preserving fixture capture as same transport-neutral input contract future eBPF capture will use.

## What Changes

- Add versioned, deterministic fixture capture input containing realistic ordered socket-byte events, binary payloads, endpoint and workload context, lifecycle hints, and truncation state.
- Orchestrate fixture events through existing segmented WAL, bounded session assembly, protocol registry, canonicalization, and local persistence; fixture data MUST NOT bypass WAL.
- Implement honest built-in plaintext HTTP/1.1 detection, stateful decoding, request-response correlation, canonicalization, local replay, and deterministic verification for explicitly bounded subset.
- Add filesystem-backed canonical session and payload storage with versioned manifests, checksums, atomic publication, and lookup by session ID.
- Make recorded sessions safely inspectable without printing payload bodies by default.
- Extend existing `record`, `inspect`, and `replay` CLI boundaries into minimal runnable flow; retain safe defaults, clear exit codes, and application-layer orchestration.
- Require dry-run by default, explicit target override, loopback allow policy, and explicit execution opt-in before network I/O. Recorded destinations and captured credentials are never replay defaults.
- Add deterministic local HTTP test server plus unit, integration, and CLI coverage proving fixture → WAL → canonical persistence → inspect → local replay → verification.
- Update capability declarations and documentation so HTTP/1.1 is `Available` only where implementation exists; keep fake protocol available and all other protocols unchanged.
- Explicitly defer eBPF, TLS/HTTPS, broad HTTP semantics, production persistence, generalized recovery, external services, and distributed/concurrent replay.

## Capabilities

### New Capabilities
- `fixture-recording-pipeline`: Versioned fixture ingestion through WAL, ordered session assembly, checkpoint reporting, and deterministic incomplete-input behavior.
- `http11-operations`: Supported HTTP/1.1 detection, decoding, correlation, canonical mapping, opaque fallback, and capability registration.
- `local-session-artifacts`: Filesystem persistence and safe inspection of versioned canonical sessions and content-addressed binary payloads.
- `safe-local-http-replay`: Default-deny loopback target planning, HTTP execution, header and credential safety, and response verification.
- `runnable-http-cli`: Minimal `record`, `inspect`, and `replay` command contracts plus end-to-end demonstration and test behavior.

### Modified Capabilities

None. No main OpenSpec capabilities exist yet.

## Impact

- Affected crates: `chronicle-capture`, `chronicle-wal`, `chronicle-session`, `chronicle-etl`, `chronicle-canonical`, `chronicle-protocol`, `chronicle-protocol-builtins`, `chronicle-storage`, `chronicle-replay`, `chronicle-application`, and `chronicle-cli`.
- Public/persisted interfaces: fixture schema v1; canonical schema v2 or smallest backward-compatible protocol extension; filesystem manifest and operation format v1; replay/verification result types; CLI arguments and exit-code contract.
- Expected focused dependencies: `httparse` for bounded HTTP/1 head parsing, `sha2` for portable content digests, and Tokio `net`, `io-util`, and `time` features for minimal plain-HTTP client/test server. No web framework, TLS stack, database client, object-store client, or new async runtime.
- Tests require no root privileges, Docker, external service, or network beyond process-local loopback sockets.
