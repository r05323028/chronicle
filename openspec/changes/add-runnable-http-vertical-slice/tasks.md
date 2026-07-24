## 1. Confirm contracts and supported subset

- [ ] 1.1 `[workspace, chronicle-protocol, chronicle-canonical]` Encode design constants and typed warning/error codes for HTTP head limit (64 KiB), header count (128), one-segment fixture rule, immediate timing, and supported/unsupported matrix; depends on no implementation task; validate with compile-time/unit assertions; changes public constants/error enums, not persisted format.
- [ ] 1.2 `[Cargo.toml, affected crate manifests]` Add only `httparse`, `sha2`, and existing Tokio `net/io-util/time` features in crates that use them, and promote existing capture/ETL/protocol/session/storage/replay path dependencies needed by application tasks from dev-only before task 3; compare `cargo tree` before/after; depends on 1.1; validate locked workspace check; changes dependency surface only.

## 2. Define fixture schema and fixtures

- [ ] 2.1 `[chronicle-capture]` Add fixture v1 serde types, strict hexadecimal decoder, connection/event validation, and `FixtureCaptureSource` adapter producing existing `CaptureEvent`; depends on 1; validate valid binary and every malformed-field case; introduces public fixture input format v1, no canonical change.
- [ ] 2.2 `[fixtures/http/]` Add credential-free fragmented basic, multiple-exchange, binary-body, malformed, truncated, duplicate-header, non-2xx, and verification-mismatch fixtures using reserved/non-production endpoints; depends on 2.1; validate parse snapshots and secret-pattern scan; adds stable test input files only.

## 3. Integrate fixture source with WAL

- [ ] 3.1 `[chronicle-application, chronicle-wal]` Implement record-stage preflight and fresh WAL directory creation, then pull `CaptureSource`, encode each event, append matching WAL v1 records, and flush before readback; depends on 2; validate record count/order/CRC and injected append/flush failures; changes application service API, not WAL wire format.
- [ ] 3.2 `[chronicle-application, chronicle-etl]` Reopen single produced segment with `WalReader`, expose final valid `WalCheckpoint` and bounded ETL issues for manifest, and reject total encoded captures exceeding one segment before writing; depends on 3.1; validate clean end, partial tail without checkpoint advance, corruption fatal, and same-session WAL/session destination refusal while shared root may contain other sessions; checkpoint becomes manifest v1 field, restart repair remains deferred.

## 4. Validate and minimally extend session assembly

- [ ] 4.1 `[chronicle-session, chronicle-etl]` Preserve grouping/direction/global order and truncation propagation only; reject fixture tuple reuse before assembler; defer residual/unsupported HTTP-derived completeness to task 8.2; depends on 3; validate fragmented/interleaved chunks, repeated payload under distinct sequences, and no invented bytes; no capture wire schema change.
- [ ] 4.2 `[chronicle-session tests]` Add duplicate/missing sequence boundary tests at fixture/WAL layer and hard-limit tests proving no silent drop or partial publish; depends on 4.1; validate typed errors and unchanged default limits; no public/persisted format change.

## 5. Implement HTTP detector

- [ ] 5.1 `[chronicle-protocol-builtins::http]` Replace HTTP scaffold detector with bounded fragmented-prefix detection for valid HTTP/1.1 request lines and probable server-first status lines, rejecting TLS and HTTP/2 preface; depends on 1 and 4; validate confirmed/probable/need-more/rejected/unknown cases; fills existing public detector capability only.

## 6. Implement stateful HTTP decoder

- [ ] 6.1 `[chronicle-protocol-builtins::http]` Implement separate directional buffers and `httparse` head parsing for HTTP/1.1 request/response lines, 128 ordered duplicate headers, no-body requests, and exact bodies only with one unsigned-decimal Content-Length field across arbitrary fragmentation/coalescing; reject duplicate/comma-list/signed/overflow values; depends on 5; validate all framing cases and binary bytes; introduces typed internal `HttpMessageV1`, no canonical format yet.
- [ ] 6.2 `[chronicle-protocol-builtins::http]` Implement FIFO pending-request state including HEAD body rule, sequential exchange tracking, orphan/missing peer handling, and pipeline-depth detection; depends on 6.1; validate two exchanges, HEAD, missing response, orphan response, and pipelining; changes decoder output contract behind existing trait.
- [ ] 6.3 `[chronicle-protocol-builtins::http]` Implement deterministic opaque/warning output for malformed syntax, residual/truncated data, conflicting Content-Length, Transfer-Encoding, close-delimited responses, 1xx, CONNECT, upgrades, unsupported targets/versions, and over-limit heads while retaining prior complete exchanges; depends on 6.1-6.2; validate each stable warning code and byte preservation; unsupported cases remain non-replayable.

## 7. Extend canonical model minimally

- [ ] 7.1 `[chronicle-canonical]` Add Canonical Session v2 writer support, backend-neutral `PayloadRef::Artifact`, and defaultable `CanonicalWarning { code, message }` on operations while retaining existing Object and v1 read compatibility; depends on 1; validate v1/v2 round trips and unknown-newer rejection; changes persisted canonical schema to v2.
- [ ] 7.2 `[chronicle-protocol]` Carry enough timestamp/sequence context through protocol stream/canonicalization seam for operation offsets relative to session start without HTTP branch in ETL; update fake implementation compatibly; depends on 7.1; validate fake vertical and missing-timestamp warning behavior; changes public protocol trait/view interfaces.

## 8. Implement HTTP canonicalizer

- [ ] 8.1 `[chronicle-protocol-builtins::http]` Define serde `HttpOperationDataV1` with typed method/target, ordered byte-valued headers, status/reason, message sequences, pipeline depth, warnings, replay attributes, and verification metadata; serialize under versioned media type; depends on 6 and 7; validate typed round trip with duplicate/binary header values; adds protocol extension schema v1.
- [ ] 8.2 `[chronicle-protocol-builtins::http, chronicle-etl]` Map correlated messages into canonical Request operations, body refs, Read/Write/Unknown effects, timing, summaries, warnings, truncation/incompleteness, and connection replayability; depends on 8.1; validate GET, POST, unknown method, missing response, malformed and truncated cases; writes canonical v2.

## 9. Implement filesystem artifact store

- [ ] 9.1 `[chronicle-storage]` Implement filesystem adapter for existing load/get traits plus concrete `publish(PublishSession { session, checkpoint, issues, replayability })`, exact UUID lookup, manifest v1/session v2 serialization, session-qualified artifact keys, SHA-256 payloads, checksum/size validation, and protocol-neutral inline externalization; depends on 7; validate save/load, direct key lookup, non-HTTP payload, corruption, not-found, and path traversal; introduces publish API, manifest v1/layout, and Artifact refs.
- [ ] 9.2 `[chronicle-storage]` Implement staging publication, create-new writes, file/directory sync, manifest-last atomic absent-destination rename, collision refusal, cleanup, Unix `0700` directories/`0600` files, and equivalent non-Unix fail-closed ACL policy; depends on 9.1; validate fault injection, permissions, and concurrent same-ID publication; persistence semantics/public adapter change only.
- [ ] 9.3 `[chronicle-storage, chronicle-application]` Implement protocol-neutral artifact hydration with SHA-256 verification for replay while inspect verifies manifest/session checksum plus payload existence/metadata size without reading body contents; depends on 9.1; validate same-size payload corruption is caught on replay, not overstated by inspect, and fixture/WAL deletion is harmless; no new persisted format.

## 10. Implement record application flow

- [ ] 10.1 `[chronicle-application]` Compose fixture source → WAL write/flush/read → ETL/HTTP → `FilesystemSessionStore::publish` as one typed `record_fixture` service with no CLI/protocol logic; dependency promotion completed in 1.2; depends on 3-9; validate returned session ID/checkpoint/issues and no publication on each stage failure; changes application public service/result types.
- [ ] 10.2 `[chronicle-application]` Enforce bounded issue summaries, non-replayable reasons, fresh root/session destination, and safe error rendering without payload/header values; depends on 10.1; validate malformed/truncated/partial-tail paths; writes manifest v1 fields.

## 11. Implement inspect application flow

- [ ] 11.1 `[chronicle-application, chronicle-storage]` Add inspect service loading manifest/canonical session only and producing protocol-neutral summary fields/replay blockers from canonical attributes/warnings; depends on 9-10; validate complete and non-replayable sessions plus missing/corrupt ID; changes public inspect result schema.
- [ ] 11.2 `[chronicle-application]` Add deterministic human and JSON render models that never include bodies, arbitrary header values, or credentials; depends on 11.1; validate golden outputs and sensitive-value absence; introduces machine-readable inspect output v1.

## 12. Implement replay policy and target mapping

- [ ] 12.1 `[chronicle-replay]` Refactor planner to retain per-operation Allowed/Denied/Unsupported decisions and full dry-run plan, send zero requests when any preflight decision is not allowed, and keep deterministic sequence order; depends on 7 and 8.1-8.2 (not final 8.3 status flip); validate default deny, dry-run visibility, read/write/unknown decisions, and zero-I/O preflight denial; changes public ReplayPlan/decision types.
- [ ] 12.2 `[chronicle-replay, chronicle-application]` Implement strict plain-HTTP loopback IP origin parser, exact allow-host matching, recorded-target separation, execute/effect gates, and immediate-only execution; depends on 12.1; validate IPv4/IPv6 loopback and every unsafe URL class before socket call; changes target/policy public API and CLI-facing errors.

## 13. Implement real local HTTP replay adapter

- [ ] 13.1 `[chronicle-protocol-builtins::http]` Implement ordered header sanitizer: remove all captured Host and emit exactly one target Host; Connection-token/hop-by-hop removal; captured auth/cookie/forwarding/Expect stripping; one recomputed Content-Length; safe duplicate preservation; depends on 8.1-8.2 and 12; validate zero/one/duplicate Host and secret absence; fills replay capability.
- [ ] 13.2 `[chronicle-protocol-builtins::http, chronicle-application config]` Read optional Authorization only from configured environment name into SecretBytes/ReplayContext, never literals/logs; reject CR/LF/NUL/DEL/disallowed controls before socket write; fail safely when replacement absent; ensure config can only narrow and cannot satisfy CLI execution gates; depends on 13.1; validate redaction, injection bytes, missing env, and permissive-config denial; extends replay config/public context usage.
- [ ] 13.3 `[chronicle-protocol, chronicle-protocol-builtins::http]` Extend `ObservedResponse` with optional versioned protocol data and typed transport error categories (update fake compatibly), then implement raw Tokio TCP adapter with one request/connection, bounded framing that stops at complete Content-Length/no-body response without waiting EOF, 5-second deadline, no retry/proxy/TLS/redirect follow, and `HttpObservedResponseV1` status/ordered byte headers; depends on 13.1; validate held-open fixed response, timeout/refusal exit-5 errors, and unsupported observed framing; changes public response/error seam and fills replay trait.

## 14. Implement HTTP verifier

- [ ] 14.1 `[chronicle-protocol-builtins::http]` Compare exact status, exact body size/SHA-256, and ordered non-ignored response headers using fixed ignore set; never include body/secret values in details; depends on 8.1-8.2, 9, and 13; validate pass plus status/header/body failures and ignored dynamic headers; fills verification capability.
- [ ] 14.2 `[chronicle-protocol, chronicle-replay, chronicle-application]` Emit Passed/Failed/Skipped/Inconclusive/Unsupported with stable categories and aggregate human/JSON result: missing expectation/truncation Inconclusive/non-executable, dry-run/policy no-run Skipped, unsupported framing Unsupported, completed mismatches Failed; transport errors remain exit-5 execution errors. Stop sequential execution on first runtime or non-passing completed verification and report executed/unattempted operations without rollback claim; depends on 14.1; validate every status, stop behavior, and machine schema; changes verification/result interfaces.
- [ ] 14.3 `[chronicle-protocol-builtins registry/tests]` Final status flip: register all five HTTP implementation objects and mark only corresponding capabilities Available; retain fake and all other statuses; depends on 5, 6, 8.1-8.2, 13.3, and 14.2; validate bidirectional Available⇔Some invariant and later README matrix; changes public capability declaration.

## 15. Wire CLI commands

- [ ] 15.1 `[chronicle-cli, chronicle-application]` Extend existing Record/Inspect/Replay variants with exact positional/options and global human|json format while keeping CLI as argument/render/exit adapter; retain scaffold ETL and configuration-only Doctor honestly; depends on 10-14; validate Clap help and argument parsing; changes public CLI surface.
- [ ] 15.2 `[chronicle-cli]` Map typed outcomes to exit 0/2/3/4/5/6, stdout/stderr rules, JSON errors, and print plan before execution; depends on 15.1; validate each exit path and zero network on denied/usage errors; establishes public exit/output contract.

## 16. Add local demonstration server

- [ ] 16.1 `[test support under chronicle-application or protocol-builtins]` Build minimal in-process Tokio loopback server on ephemeral port with GET, POST, custom header, non-2xx, binary body, pass, and mismatch modes; depends on decoder/client tasks 6 and 13; validate observed request capture and deterministic responses; test-only API, no runtime persisted/public format.
- [ ] 16.2 `[examples or documented test helper]` Add optional manual server entry point binding explicit loopback port without framework/Docker; depends on 16.1; validate local demo commands; no production server claim.

## 17. Add unit, integration, and CLI tests

- [ ] 17.1 `[crate unit tests]` Complete required unit matrix: detector; fragmented request/response; single/duplicate/list/invalid Content-Length; multiple exchanges; malformed/truncated; duplicate headers; binary bodies; capability integrity; exact-one Host sanitizer; credential injection rejection; config precedence; held-open response framing; policy; verifier pass/fail/status mapping; depends on 2-16; validate focused crate tests; no interface change.
- [ ] 17.2 `[chronicle-application integration tests]` Add full fixture→WAL→assembly→HTTP→canonical→filesystem→inspect→plan→local execute→verify test, assert WAL record count, and remove fixture/WAL before inspect/replay to prove boundaries; depends on 16; validate Passed and mismatch Failed runs; exercises fixture/manifest/canonical formats.
- [ ] 17.3 `[chronicle-cli integration tests]` Cover successful record/inspect, dry-run no network, denial without authorization, allowed loopback replay, verification exit 6, malformed fixture, unknown session, safe JSON, and exit codes; depends on 15-16; validate spawned binary deterministically; public CLI contract coverage.

## 18. Update documentation and capability declarations

- [ ] 18.1 `[README.md, docs/architecture.md, docs/protocol-plugin-model.md]` Document runnable flow, exact supported HTTP subset, socket-chunk terminology, Available capability matrix, fake retention, and unchanged status of all other protocols/eBPF; depends on passing implementation tests; validate docs against registry test; documentation only.
- [ ] 18.2 `[docs/canonical-model.md, docs/replay-safety.md, docs/wal-format.md, CONTRIBUTING.md]` Document canonical v2/Artifact refs, filesystem manifest/layout/atomicity, loopback replay/header/credential policy, verifier semantics, one-segment and no-repair limits, dependencies, and fixtures without secrets; depends on 7, 9, 12-14; validate examples/help and no overstated support; documents persisted/public interfaces.

## 19. Run repository and OpenSpec validations

- [ ] 19.1 `[workspace]` Run `cargo fmt --all --check` and `cargo check --workspace --all-targets --locked`; fix only change-caused issues; depends on all implementation/docs tasks; validation only.
- [ ] 19.2 `[workspace]` Run `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` and `cargo test --workspace --all-features --locked`; record results and residual platform limits; depends on 19.1; validation only.
- [ ] 19.3 `[openspec/changes/add-runnable-http-vertical-slice]` Run `openspec validate add-runnable-http-vertical-slice --strict`, verify artifacts/tasks match implementation and no production source was invented beyond scope, then perform independent safety/capability review before archive; depends on 19.2; validation/change metadata only.
