## Context

Chronicle has correct coarse boundaries but only fake behavior. `CaptureSource` emits versioned `CaptureEvent` socket-byte chunks; `SegmentedWalWriter` and `WalReader` provide CRC32C-framed order and partial-tail detection; `SessionAssembler` groups and bounds bidirectional chunks; ETL dispatches through compile-time protocol registrations; canonical, storage, replay, application, and CLI layers expose extension seams. Current gaps are concrete: HTTP registration has no implementations, application commands are stubs, persistence is memory-only, replay reconnects through fake adapters, and CLI errors collapse to exit 1.

This design keeps existing flow:

```text
FixtureCaptureSource
  -> CaptureEvent
  -> WAL v1 CaptureEvent records
  -> WalReader checkpoint
  -> SessionAssembler
  -> HTTP registration (detect/decode/canonicalize)
  -> Canonical Session v2
  -> FilesystemSessionStore
  -> inspect OR replay plan
  -> HTTP replay adapter
  -> HTTP verifier
```

Fixture parsing remains capture-only. Replay begins from filesystem canonical data and protocol interfaces; it never opens fixture, WAL, ETL, session, or eBPF components. Linux, root access, Docker, databases, object storage, and external connectivity are not required.

## Goals / Non-Goals

**Goals:**

- Provide first runnable plaintext HTTP/1.1 record/inspect/replay/verify demonstration.
- Exercise every durability and transformation boundary rather than connecting fixture bytes directly to HTTP code.
- Preserve binary bodies, duplicate headers, endpoint context, timestamps, truncation, warnings, and replay/verification intent.
- Keep replay dry-run and default-deny; require explicit loopback target, host allow entry, effect authorization, and `--execute` before I/O.
- Make malformed, truncated, ambiguous, and unsupported HTTP deterministic, inspectable, and non-replayable rather than overstated.
- Keep formats versioned and implementations replaceable by eBPF, PostgreSQL, and S3 adapters later.

**Non-Goals:**

- Real eBPF programs, probe attachment, PID/cgroup capture, Kubernetes, production capture, TCP packet reassembly, or connection lifecycle recovery.
- TLS interception, HTTPS replay, HTTP/2, HTTP/3, WebSocket, CONNECT tunnels, transparent proxying, or proxy-form requests.
- Chunked transfer coding, connection-close-delimited response bodies, informational `1xx` exchanges, replay of pipelined connections, upgrades, trailers, or compressed-body semantics.
- PostgreSQL, S3-compatible storage, other real protocols, dynamic/WASM plugins, distributed or concurrent replay, preserve-timing scheduling, generalized semantic diffs, credential vaults, WAL replication, full restart repair/retention/disk pressure, UI, Docker, or release automation.
- External `doctor` probes. Existing configuration-only `doctor` behavior remains honest.

## Decisions

### 1. Exclude eBPF and reuse capture contracts

`record --source fixture` constructs ordinary `CaptureEvent` values and pulls them through `CaptureSource`. eBPF is excluded because current adapter cannot load or emit events, Linux/root requirements would prevent portable tests, and hook semantics remain undecided. Fixture EOF means end of this finite capture batch, not observed TCP FIN. Future eBPF replaces only source adapter; downstream WAL record encoding and contracts remain unchanged.

Alternative rejected: send fixture bytes directly to HTTP decoder. It would prove parser only and bypass durability/session requirements.

### 2. Fixture schema and versioning

Fixture v1 is JSON:

```json
{
  "schema_version": 1,
  "connections": [{
    "id": "basic",
    "network_namespace": null,
    "client": {"host": "fixture-client", "port": 41000},
    "server": {"host": "recorded.invalid", "port": 8080},
    "transport": "tcp",
    "process": {"pid": 4242, "tid": null, "executable": "fixture-client"},
    "file_descriptor": 7
  }],
  "events": [{
    "sequence": 1,
    "timestamp": "2026-01-01T00:00:00Z",
    "connection_id": "basic",
    "direction": "client_to_server",
    "payload_hex": "474554202f...",
    "truncated": false,
    "flags": 0
  }]
}
```

Rules: schema must equal 1; IDs are unique; all references resolve; transport is `tcp`; global sequences start at 1 and are contiguous; timestamps are nondecreasing by sequence; hex is even-length and valid; endpoint ports are nonzero; event/body/session limits and total encoded WAL size are checked before WAL creation; checked-in fixtures use reserved/non-production names and contain no real credentials or personal data. `payload_hex` provides dependency-free binary representation. Connection metadata is expanded into current `ConnectionKey` and `CaptureEvent`; fixture IDs do not enter canonical schema. Distinct fixture connections may not share same current `ConnectionKey`, preventing tuple-reuse ambiguity until capture model gains connection generations.

Lifecycle events are absent in v1 because supported framing does not use FIN and current `CaptureEvent` has no lifecycle variant. EOF completes batch only; close-delimited response bodies remain unsupported. Future lifecycle fields require fixture schema v2 and corresponding transport-neutral capture contract, not ad hoc HTTP fields.

Alternative rejected: serialize `CaptureEvent` directly as fixture. That would couple human test input to capture-envelope JSON and make fixture a competing canonical format.

### 3. WAL write/read boundary

Application validates fixture, creates fresh `<root>/wal/<session-id>/`, appends each encoded `CaptureEvent` as existing WAL v1 `CaptureEvent` record with same sequence, then flushes before ETL opens WAL. Fixture and WAL sequence mismatch is impossible after validation but remains checked by ETL.

P0 accepts only captures whose complete encoded WAL fits one configured segment; larger input fails before append with clear size error. This one-segment preflight is a total-capture bound independent of per-connection assembler limits. It avoids inventing segment catalog/restart machinery while still exercising segmented writer and WAL framing. ETL reads named segment through `WalReader`, preserving record order and CRC/sequence validation.

`WalCheckpoint { segment_first_sequence, byte_offset, next_sequence }` after last valid record is copied into manifest `processing_checkpoint`. Partial tail produces ETL issue, does not advance checkpoint, marks session incomplete/non-replayable, and preserves operations decoded before tail. Bad CRC, magic, version, or sequence is fatal. Writer resume, tail truncation, multi-segment ETL, repair, retention, and checkpoint-driven restart are deferred.

### 4. Session reconstruction semantics

Existing `ConnectionKey` groups events. WAL global sequence orders chunks while each chunk retains direction. Duplicate sequence and missing sequence are rejected by fixture validation/WAL; repeated payload at different sequence is distinct data. Missing network bytes cannot be inferred and must be signaled by `truncated=true`. Existing connection, byte, and chunk limits remain hard typed failures with no silent drop.

`finish()` completes finite batch. Connection is incomplete when any event is truncated, HTTP decoder leaves residual bytes, request lacks final response, unsupported framing is encountered, or opaque evidence is emitted. Complete prior exchanges remain inspectable. These are ordered socket chunks, never packets and never claimed TCP reassembly.

### 5. HTTP parser strategy

Use focused `httparse` for request/status line and header syntax. Chronicle owns bounded streaming buffers, message framing, body extraction, state, warnings, and supported-subset policy. Limits: 64 KiB start-line plus headers, 128 header fields per message, and existing 8 MiB per-connection byte limit.

Comparison:

1. Internal parser: smallest dependency count but duplicates security-sensitive HTTP grammar and is harder to audit.
2. `httparse`: small, synchronous, focused, supports incremental head parsing and preserves duplicate fields; chosen.
3. `hyper`/`reqwest`/web framework: mature but adds broad runtime and behavior not needed for captured-byte parsing; rejected for P0.

### 6. HTTP detection

Detector joins only enough client-to-server bytes to the 64 KiB head limit. Valid token + space + request-target + space + `HTTP/1.1\r\n` is `Confirmed`; incomplete plausible prefix is `NeedMoreData`; `HTTP/1.1` server-first status line is `Probable`; TLS records, HTTP/2 preface, invalid syntax, and unrelated bytes are rejected/unknown. Explicit registered protocol override remains allowed, but override does not make malformed bytes replayable.

### 7. Stateful decoding and supported subset

One decoder owns separate bounded buffers per direction plus FIFO request metadata. It accepts arbitrary fragmentation and multiple messages in one chunk. Supported requests use HTTP/1.1, valid token methods, origin-form target, headers, no body when `Content-Length` absent, or exact fixed body when exactly one valid decimal `Content-Length` field exists. Supported responses use HTTP/1.1, final status lines, headers, and exact `Content-Length`; `HEAD`, `204`, and `304` responses have no body regardless of `Content-Length`. `Content-Length: 0` is supported.

Header names normalize to lowercase ASCII. Values remain raw bytes. `Vec<HttpHeaderV1 { name, value }>` preserves order and duplicates. Any duplicate `Content-Length`, comma-list form, non-decimal value, sign, or overflow is malformed even when duplicate values match. Bodies remain arbitrary bytes and are never UTF-8 decoded.

Unsupported behavior:

- `Transfer-Encoding`, including chunked: affected message and residual direction bytes become opaque with `unsupported_transfer_encoding`; connection is non-replayable.
- Ordinary responses without `Content-Length`: treated as unsupported close-delimited framing, even if zero bytes follow.
- `1xx`: affected exchange becomes unsupported; decoder does not silently correlate it as final response.
- HTTP pipelining: decoder may decode and FIFO-correlate complete messages for inspection, but records pipeline depth; any depth greater than one makes connection non-replayable because P0 replay is sequential.
- CONNECT, absolute-form/authority-form targets, `Upgrade`, WebSocket, HTTP/2 preface, and non-HTTP/1.1 versions: opaque/non-replayable.

Malformed complete syntax emits opaque evidence and stable warning code. At `finish`, residual incomplete head/body emits opaque evidence with `truncated_message`; already complete exchanges remain. Decoder does not panic or discard bytes.

### 8. Request-response correlation

Requests enter FIFO by completed request-head/body order. Final responses consume oldest request. One request plus one final response becomes one operation. A request without response becomes incomplete, non-executable operation with no expectation and an `Inconclusive` verification state. A response without request becomes opaque server evidence. Sequential exchanges are supported. HEAD body framing uses queued request method. Pipelining is decoded FIFO for inspection but blocked from replay as above.

Alternative rejected: alternate directions chunk-by-chunk like fake canonicalizer. Socket fragmentation and coalescing make chunk alternation unrelated to HTTP message boundaries.

### 9. Canonical HTTP representation and schema evolution

Canonical Session writer moves to schema v2 with two additive protocol-neutral changes:

- `PayloadRef::Artifact { key, checksum, size, content_type }` for backend-neutral persisted bodies; existing S3-oriented `Object` remains for compatibility.
- `CanonicalWarning { code, message }` list on operations; missing list defaults empty when reading schema v1.

Readers accept v1 and v2, reject unknown newer versions with typed error, and write v2. No HTTP fields are added to protocol-neutral structs. HTTP essentials live in typed `HttpOperationDataV1` serialized into `ProtocolData` with media type `application/vnd.chronicle.http-operation+json;version=1`:

- protocol/version, method, origin-form request target;
- ordered duplicate-preserving request/response headers;
- optional response status/reason;
- request and response start sequences;
- pipeline depth and stable parse/unsupported warning codes;
- replay attributes (target form, stripped-sensitive-header presence);
- verification expectation metadata.

Core operation fields carry body references, start/completion offsets, `Request` kind, effect, truncation/incompleteness, and warnings. Connection carries source/destination endpoints. Summary attributes (`http.method`, `http.request_target`, optional `http.response_status`) support protocol-neutral inspect/indexing but are not sole semantic representation.

Method effects: GET/HEAD/OPTIONS are `Read`; POST/PUT/PATCH/DELETE are `Write`; all others are `Unknown`. Unknown remains denied. Capture wall times map request first sequence and response last sequence to offsets relative to session start; missing times use deterministic sequence order with warning rather than fabricated wall-clock precision.

### 10. Binary payload storage and checksums

HTTP `request` and `recorded_response` refer to body bytes only. `FilesystemSessionStore::save_session` clones session, writes every inline body as content-addressed payload, and replaces it with `Artifact`. Empty bodies are represented as inline empty bytes or one digest consistently; implementation chooses inline empty to avoid files. SHA-256 names/checksums are `sha256:<lowercase-hex>`.

Use `sha2`; CRC32C remains WAL corruption check and is not collision-resistant content identity. Replay hydrates request bodies through `ArtifactStore` into a cloned canonical session before planning. Inspect verifies manifest/session checksum plus referenced payload existence and filesystem size without reading/checksumming body contents; replay hydration performs payload SHA-256 verification.

### 11. Filesystem layout

```text
<root>/
  wal/<session-id>/segment-00000000000000000001.wal
  sessions/<session-id>/
    manifest.json
    session.json
    payloads/<sha256-hex>
```

Manifest v1 contains session ID, canonical schema version, session file name and SHA-256, payload count, aggregate byte count, processing checkpoint, bounded ETL/warning summaries, completeness, and replayability reasons. `session.json` contains canonical session v2 with `Artifact` refs. WAL is separate and not required after publication. Lookup parses UUID and uses exact `<root>/sessions/<id>`; path traversal is impossible. Artifact keys are session-qualified (`sessions/<session-id>/payloads/<sha256-hex>`) so existing key-based lookup never scans or guesses session context.

Existing `MetadataRepository` and `ArtifactStore` traits remain for ordinary load/get and compatibility. Because current `save_session` cannot carry checkpoint/issues/replayability, application publication uses concrete `FilesystemSessionStore::publish(PublishSession { session, checkpoint, issues, replayability })`; this protocol-neutral method externalizes inline payloads and writes manifest atomically. Future PostgreSQL/S3 implementations can accept same publish input while reusing canonical refs and existing traits.

### 12. Atomic persistence

Store creates `sessions/.<session-id>.tmp-<random>`, writes payloads with create-new semantics, syncs files, writes and syncs `session.json`, writes manifest last, syncs staging directory where supported, then atomically renames staging directory to final absent destination. Existing destination fails; no overwrite. On failure, temporary directory is removed best-effort and final path stays absent. Parent-directory fsync is attempted on Unix and documented as best-effort elsewhere. On Unix, root/session/staging/payload directories are mode `0700` and manifest/session/payload files are `0600`; non-Unix deployments must provide equivalent private ACLs or fail closed when they cannot.

Alternative rejected: write manifest directly into final directory. Inspect/replay could observe partial publication.

### 13. Inspect behavior

Application loads manifest/session without WAL. Human output includes session ID, protocol, recorded client/server, operation count, method/target, response status, request/response body sizes, warnings/truncation/incompleteness, replayable yes/no, and each blocking reason. Bodies and credential header values never print. JSON output exposes same summaries and stable codes, not payload bytes or secret values. No payload-dump option is added in P0.

### 14. Replay target mapping

One explicit target origin applies to all HTTP connections in selected session. Accepted syntax is `http://<ip-literal>:<port>` with optional trailing `/`; target path, query, fragment, userinfo, wildcard, unspecified, multicast, or non-IP host is rejected. IP must be loopback and must exactly match one repeated `--allow-host <ip>` entry. Recorded scheme/host/port are never fallback and are never connected.

Replay replaces origin only. Original origin-form path and query are preserved byte-for-byte. No URL normalization, percent decoding, base-path join, DNS lookup, or HTTPS occurs.

### 15. Replay authorization and planning

Planner changes from fail-first to per-operation decisions so dry-run can show full safe plan. Default remains dry-run and all effects denied. `--allow-read` and `--allow-write` opt into corresponding effects; Unknown/Authentication/Publish remain unavailable in this HTTP P0 CLI. Existing config target mappings, `dry_run=false`, and allow booleans MUST NOT satisfy or broaden CLI execution gates; P0 CLI requires explicit command-line target, allow-host, effect flags, and execute on every invocation. Config may only narrow policy or provide timeout/credential environment-variable names. `--execute`, explicit target, matching loopback `--allow-host`, and required effect flags must all pass before executor receives plan. Any denied/unsupported operation blocks whole execution during preflight, so policy denial sends zero requests. After fully allowed preflight, executor runs sequentially; first transport error or completed Failed/Inconclusive/Unsupported verification stops remaining operations. HTTP effects already executed cannot be rolled back, so result reports executed operations and all unattempted remainder explicitly. Plan prints before execution in human and JSON modes.

`--execute` does not imply effect authorization. Recorded destination, target, allow policy, and runtime credentials are separate fields in output and code.

### 16. Captured-header filtering and runtime credentials

Replay preserves ordered end-to-end headers except:

- removes every captured `Host` field and emits exactly one `Host` containing replay target authority;
- removes hop-by-hop `Connection` plus headers named by its tokens, `Proxy-Connection`, `Keep-Alive`, `TE`, `Trailer`, `Transfer-Encoding`, and `Upgrade`;
- removes captured `Authorization`, `Proxy-Authorization`, `Cookie`, `Forwarded`, `X-Forwarded-*`, and `Expect`;
- recomputes one `Content-Length` from hydrated request body.

Captured secret values never appear in plan/log/errors. Optional replay Authorization may come only from environment variable named by config (`replay.authorization_env`) and enters `ReplayContext.credentials["authorization"]` as `SecretBytes`; config and CLI never accept literal credential bytes. Before wire serialization, replacement value must contain only valid HTTP field-value bytes (HTAB, visible bytes, or obs-text); CR, LF, NUL, DEL, and other controls are rejected to prevent request/header injection. Operation requiring captured auth remains blocked unless runtime replacement is configured and explicit authentication policy is added in future; demo does not require auth.

### 17. HTTP client strategy, connection reuse, redirects, TLS, and timing

Use Tokio TCP primitives already present in workspace, enabling focused `net`, `io-util`, and `time` features. Adapter writes one sanitized HTTP/1.1 request with `Connection: close`, reads bounded response only through complete supported framing (head plus exact Content-Length or no-body rule), and reuses same HTTP parser. It does not wait for EOF after a complete fixed-length response; EOF is relevant only to classify unsupported close-delimited/truncated responses. One new connection per operation matches existing executor. No cookies/session state or connection reuse; capture with pipelining is non-replayable. Sequential order follows plan `(scheduled_offset, sequence)`, but P0 accepts only `asap`; `preserve` fails before execution.

Redirects are never followed. A 3xx is observed and verified like any response, preventing second-hop escape. Default connect and whole-operation timeout is 5 seconds, configurable only through existing config with bounded positive duration if implementation needs manual demos; refusal, timeout, disconnect before complete supported response, and I/O failure return typed protocol transport errors and exit 5 without verification result or retry. `https` is rejected. No TLS crate/OpenSSL is added.

Comparison:

1. Raw Tokio client: supports exact local plaintext subset and explicit safety behavior; chosen.
2. Focused low-level HTTP client: possible later when connection reuse/TLS/proxy behavior is required.
3. `reqwest`/`hyper`: broader stack and implicit redirects/pooling/features exceed P0; rejected.

### 18. Verification semantics

Verifier compares recorded expectation with observed response:

- exact status code;
- exact body bytes via SHA-256 plus size (equivalent to byte comparison, without logging body);
- all recorded end-to-end response headers except case-insensitive fixed ignore set: `date`, `server`, `content-length`, `connection`, `transfer-encoding`, `keep-alive`, and `set-cookie`.

Compared header names are lowercase; duplicate value order is preserved and values compare as bytes. `ObservedResponse` gains optional versioned protocol data; HTTP adapter serializes `HttpObservedResponseV1` containing status and ordered raw-byte headers, while body remains payload ref. Fake adapter populates no protocol data. Dynamic values receive no heuristic normalization: ignored names are explicit; every other mismatch fails visibly. Missing recorded response is `Inconclusive` and non-executable. `Skipped` means no verification ran (dry-run or policy-denied plan). Recorded truncation/incompleteness is `Inconclusive` and non-executable. Unsupported recorded/observed framing is `Unsupported`. Status/header/body mismatch after a complete observed response is `Failed`. Transport errors return typed execution error and exit 5, not verification status. Details contain names, statuses, sizes, and digests only, never bodies or secret values.

Human summary lists each operation and aggregate counts. JSON emits session/run IDs, plan decisions, per-operation status/category/details, and aggregate counts. This is protocol-specific deterministic comparison, not generalized semantic diff.

### 19. CLI contract and exit codes

Existing command names and positional session IDs remain. `etl` stays scaffold-only; record performs ETL internally.

```text
chronicle [--config FILE] [--format human|json] record \
  --source fixture --input FILE --root ROOT

chronicle [--format human|json] inspect SESSION_ID --root ROOT

chronicle [--config FILE] [--format human|json] replay SESSION_ID --root ROOT \
  --target http://127.0.0.1:PORT --allow-host 127.0.0.1 \
  [--allow-read] [--allow-write] [--timing asap] [--execute]
```

Safe defaults: human output, dry-run, no effect authorization, `asap`, no target default, no network. Record success prints session ID and root. Missing/malformed fixture and unknown/malformed session fail clearly. Dry-run prints plan and exits 0 even with denied decisions; execution with denied decisions exits 4 and performs zero requests. Exit codes: 0 success/dry-run/Passed-or-Skipped verification; 2 Clap usage; 3 fixture/session/storage/WAL data error; 4 safety or unsupported-plan denial; 5 refusal/timeout/disconnect/I/O or protocol execution error before complete observed response; 6 one or more completed verification Failed/Inconclusive/Unsupported. JSON errors use stable code/message and omit secrets.

### 20. Dependencies

New runtime dependencies are limited to:

- `httparse`: audited head syntax; parser choice above.
- `sha2`: SHA-256 artifact identity and verification digests.
- existing `tokio` with `net`, `io-util`, and `time` features: async TCP and deadlines; no second runtime.

No full framework, URL parser, TLS/OpenSSL, database, object-store, Docker, Kubernetes, protobuf, gRPC, plugin runtime, or unsafe code. Target URL parsing is strict because accepted grammar is only loopback HTTP origin; std `IpAddr`/`SocketAddr` handles addresses.

### 21. Local demonstration server

Test support uses in-process Tokio `TcpListener` and same bounded request parsing, not framework. It binds `127.0.0.1:0` and exposes deterministic routes: GET no body, POST echo/fixed body, custom response header, non-2xx, binary body, and switchable mismatch. Tests receive chosen ephemeral port. Manual demo helper may bind requested `127.0.0.1:18080`; core suite never requires fixed port or external process.

### 22. Future migrations

PostgreSQL metadata can index manifest/session fields while S3 stores payload digest objects; `PayloadRef::Artifact`, checksums, canonical v2, and storage traits avoid HTTP coupling. Migration reads v1/v2 canonical sessions and writes future schema explicitly. Future eBPF emits same `CaptureEvent`; adding connection generation/lifecycle requires transport-neutral capture schema change and fixture v2. Neither migration changes replay dependency direction.

## Risks / Trade-offs

- [HTTP subset rejects common chunked/close-delimited responses] → Report stable unsupported codes, preserve opaque evidence, block replay, and add support in separate change.
- [One-segment WAL limit is not production recovery] → Preflight size, fail before write, retain checkpoint/partial-tail tests, and defer catalog/resume/repair explicitly.
- [Current `ConnectionKey` cannot distinguish reused tuple] → Fixture rejects duplicate tuple IDs; future capture schema adds generation identity before eBPF.
- [Raw client can mishandle broader HTTP] → Restrict target and message subset, reuse parser, cap memory/time, never follow redirects, and test exact wire behavior.
- [Canonical v2 affects fake tests/fixtures] → Add serde defaults, retain existing variants, accept v1/v2, and update schema docs/tests in same task.
- [Atomic directory rename durability differs by platform] → Sync files/directories where supported; never expose final directory before complete rename.
- [Captured credentials may persist before redaction] → Checked-in fixtures contain none; inspect/log never print bodies/header values; replay strips sensitive headers. General pre-persistence redaction remains separate security work and must be called out in docs.
- [Dry-run denied plan exits 0] → Stable decision codes make denial visible; only `--execute` denied path exits 4. This supports inspection without network.
- [Verification exactness yields expected dynamic failures] → Fixed ignored headers are explicit; no hidden normalization. Users must produce deterministic fixtures/server for P0.
- [No connection reuse changes stateful semantics] → Mark pipelined/stateful exchanges non-replayable; one request per connection is deliberate ceiling.
- [Runtime failure can occur after earlier HTTP effects] → Preflight blocks all known denials before I/O, executor stops on first runtime/verification failure, and output distinguishes executed from unattempted operations; no rollback claim.

## Migration Plan

1. Add dependencies and canonical v2 additive reader/writer support while retaining fake protocol tests.
2. Add fixture v1 and HTTP typed codec/canonicalizer behind existing registry; capability remains Planned until all corresponding implementation objects register.
3. Add filesystem store and application record/inspect services; publish only v2 sessions.
4. Add replay plan decisions, loopback policy, HTTP adapter/verifier, and CLI wiring.
5. Run fixture/WAL/canonical persistence integration first in dry-run, then loopback execution tests.
6. Update README capability matrix, architecture/canonical/replay/WAL docs, and fixtures only after tests prove claims.

Rollback removes CLI wiring/HTTP registration and leaves v2 session directories readable as data. No production migration exists. Implementations MUST NOT silently write v1 after rollback; operators remove demo roots or use version-aware reader.

## Open Questions

None blocking. Limits, formats, unsupported cases, policies, exit codes, and dependencies are fixed above. Any request to support multi-segment restart, DNS/non-loopback targets, chunked/close-delimited bodies, redirects, TLS, connection reuse, or configurable semantic verification requires separate reviewed change rather than implementation-time expansion.
