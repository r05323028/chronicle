## Context

Chronicle 0.1.x has a validated capture → WAL → ETL → canonical → replay architecture. The current CLI (`crates/chronicle-cli/src/main.rs`, 967 lines) exposes that architecture directly: `record --source fixture|ebpf --wal-dir ...`, `etl --wal-dir --output`, `replay SESSION --root --target --allow-host ... [--execute]`, `inspect SESSION --root`, plus daemon commands `recorder` and `recorder-status`. Application services exist and are proven: `record_live_ebpf` (lib.rs:753), `record_continuous_ebpf` (lib.rs:1377), `process_and_publish_recording_wal` (lib.rs:4315), `inspect_session` (lib.rs:4752), `replay_session_with_plan` (lib.rs:5038), `doctor_report` (lib.rs:217), `RecorderLease`, and `FilesystemSessionStore` (which publishes to `<root>/sessions/<uuid>/manifest.json` and currently has no listing capability).

The redesign keeps this architecture and the recording/safety invariants intact; it adds an orchestration layer on top. Constraints: no GUI, no TLS decryption, no new protocols, no remote orchestration, no Kubernetes/Docker implementation, no storage backend redesign, no daemon lifecycle redesign beyond CLI compatibility. WAL durability/recovery, bounded recording, loss accounting, P1/P2 acceptance semantics, deny-by-default replay, and privileged Linux capture requirements are all preserved.

## Goals / Non-Goals

**Goals:**

- Five public commands: `record`, `replay`, `list`, `inspect`, `doctor` — intent-first, mechanics hidden.
- `chronicle record -- COMMAND...` as the primary flow; `--pid`/`--cgroup` for already-running workloads.
- `chronicle replay <RECORDING> -- COMMAND...` symmetric with record; explicit `--target URL` mode preserved.
- Stable recording identity (`rec_<uuid>`), `latest`, optional names, predictable default data directory.
- Old commands and flag forms survive 0.1.x as hidden deprecated compatibility entrypoints with migration paths, removal targeted at 0.2.
- Human output reads like a conversation; JSON and exit codes stay automation-stable.

**Non-Goals:**

- Redesigning WAL, ETL, canonical model, recovery, `FilesystemSessionStore` publication, replay planner/executor/verifier, recorder daemon lifecycle, or lease/quota ownership.
- Making the catalog a durability or recovery authority.
- Introducing new protocols, remote orchestration, TLS decryption, GUI, or container-specific implementation.
- Auto-authorizing writes, authentication, publication, or unknown effects in any mode.
- Changing the recording identity storage format.

## Decisions

### D1. Orchestration layer in chronicle-application; CLI stays thin

New high-level workflows (`record_command`, `replay_command_with_scope`, `resolve_recording`, `list_recordings`, `record_live_selector`) live in `chronicle-application` and wrap the existing proven services. `chronicle-cli` keeps its role: parse args, map commands, render results, map outcomes to exit codes. Rationale: preserves the "CLI remains outer adapter" contract (runnable-http-cli spec) and keeps all safety-critical sequencing testable without spawning processes. Alternative considered: orchestrating from the CLI crate — rejected because it duplicates application logic and weakens the outer-adapter boundary.

### D2. Command-mode recording: preflight, exact domain lock, attach-before-exec

`record -- COMMAND...` runs this sequence:

1. Resolve the data directory and exact domain-lock path, then run target-independent, non-mutating preflight: arguments, supported Linux build with `linux-ebpf`, cgroup v2/BTF/capabilities, prospective private data-directory access, and ability to create a supervisor-controlled scope. Unsupported preflight exits before recording ID, sidecar, WAL, catalog, scope, or target creation.
2. Acquire the exact domain lock and revalidate mutable prerequisites under it. Public mode defaults the lock root to the data directory; configured mode uses an explicit normalized `domain_lock_root`. The shared helper opens the same exact `<domain_lock_root>/.chronicle-domain.lock` path as `RecorderLease`; device identity alone SHALL NOT be treated as lock equivalence. Public record and daemon recorder conflict only when configured for the same normalized domain-lock path, and configuration/docs SHALL make that alignment explicit.
3. Allocate a `RecordingId` in memory. For command mode, create and validate the cgroup-v2 scope before durable recording allocation. Persist the private recording-intent sidecar and initial catalog state under the held lock only after preflight and scope creation succeed. The lock remains held through WAL creation, capture, ETL, canonical publication, and catalog update.
4. Spawn a bootstrap child blocked on a parent-controlled readiness pipe before target exec; move it into the scope; attach capture to the exact scope through a lower-level API that accepts the caller-allocated recording ID/metadata; signal readiness only after attachment succeeds; then release the bootstrap to harden credentials and directly `execve` the target. The guarantee is that no target-executable instruction runs before attachment, not that the bootstrap executes no instructions.
5. Derive target UID/GID from trusted OS process credentials captured before elevation, not user-controlled environment variables. Before exec: clear supplementary groups; set real/effective/saved GID and UID; clear inheritable/permitted/effective/ambient capability sets; set `no_new_privs`; close or `CLOEXEC` Chronicle-owned descriptors except the bootstrap handshake; reset inherited signal mask/dispositions; verify the resulting credentials. Failure or `execve` failure is a Chronicle launch failure, not child exit 127, and triggers cleanup without release to target code.
6. The cgroup is a lifecycle/capture boundary, not a security sandbox. Chronicle claims coverage only for processes remaining in the supervised scope; it SHALL NOT claim containment of a hostile same-UID target. Scope membership-control safety is preflighted, and unexpected membership/identity change aborts rather than silently claiming complete capture.
7. Human mode may pass through target stdout/stderr. JSON mode reserves Chronicle stdout/stderr and connects child streams directly to platform null sink before exec; no child-output file/buffer exists. Raw argv/output never enters diagnostics or `Try:`.
8. On target exit or SIGINT/SIGTERM: bounded graceful scope shutdown, detach, final sample, final marker sync, metadata finalization, ETL via `process_and_publish_recording_wal`, publication via `FilesystemSessionStore`, then catalog update; release the lock last. Faults after scope creation run the same idempotent cleanup state machine. Abrupt supervisor `SIGKILL` cannot promise synchronous cleanup; doctor reports orphaned owned scopes and documented manual recovery rather than killing uncertain processes automatically.
9. If recovery-authoritative WAL exists but ETL/publication/catalog update fails, retain the ID/WAL and mark `recoverable`. `chronicle record --retry RECORDING` reacquires the same lock and retries recovery/finalization/publication for that ID only; it never reruns capture or the target and publication remains idempotent.

Rationale: reuse of the existing cgroup selector, production recorder, WAL, ETL, and publication services preserves capture and durability behavior. The current `record_live_ebpf` allocates its own ID, so orchestration must call/refactor the lower-level `record_production` path rather than merely add a readiness callback around the current wrapper.

Public `--pid`/`--cgroup` selector modes run selector and privilege preflight before durable allocation, then use the same caller-allocated-ID WAL → ETL → publish → catalog workflow while holding the exact domain lock. They never terminate the selected workload and preserve shared-scope acknowledgement and PID direct-TGID safety. Only hidden legacy `record --source ebpf --wal-dir ...` retains no implicit ETL.

### D3. Recording identity and additive private storage

- Public display uses `rec_<full-uuid>`; public input accepts that form and bare UUID compatibility input, never prefixes. Internal `RecordingId(UUID)` and `RecordingId == SessionId` remain unchanged. Hidden legacy output remains byte-compatible and may retain its prior bare-ID contract.
- Data-directory precedence is `--data-dir` → configured `AppConfig.data_dir` → `CHRONICLE_DATA_DIR` → platform default. Tests inject environment/home lookup rather than mutating process-global environment in parallel. Legacy `--root` keeps its exact meaning.
- Layout is additive: `<data>/sessions/` remains the existing `FilesystemSessionStore` layout; `<data>/recordings/<bare-uuid>/` holds existing WAL/metadata plus private `recording-intent.json`; `<data>/catalog.json` is advisory. Directory names use bare canonical UUIDs because existing storage paths do; `rec_` is display syntax only.
- New path handling reuses private-directory/no-symlink rules: operate only on immediate regular entries under trusted directory handles, reject symlinked roots/children and ID-directory mismatches, cap catalog input/output at 16 MiB and 10,000 entries, cap sidecars at 4 KiB, and fail safely rather than recurse outside the data directory.
- Catalog v1 entry is `{recording_id, name?, created_at, ended_at?, status, session_id?, child_exit?}` with stable status values `in_progress`, `recoverable`, `published`, `failed`, and `inconsistent`. Canonical session evidence wins for identity/start/end/operation facts; recovery-authoritative WAL determines recoverability; catalog state supplies only advisory lifecycle/name/child facts. Contradictions produce `inconsistent`, never silent overwrite.
- Reconciliation first builds a bounded in-memory view. Persisted rebuild requires the exact domain lock; while daemon owns it, reads use the in-memory reconciled view and no catalog file is mutated. Reconciliation adopts matching artifacts and never runs ETL or publication.
- Names are exact-match UTF-8, 1–128 bytes, contain no control characters, and reject `latest` plus case-sensitive `rec_` prefix; they are unique under the domain lock. Human rendering additionally escapes all untrusted references.
- `latest` considers only published, inspectable entries and uses canonical `started_at`, tie-broken lexicographically by recording ID. `list` shows every bounded entry, derives canonical facts when present, and renders effective status using the precedence above.

Rationale: catalog keeps names/status/list cheap while authoritative WAL and session data remain unchanged. New catalog and sidecar are persisted formats, but private/advisory v1 formats—not changes to existing authoritative formats.

### D4. Replay: pre-plan, supervised-scope listener discovery, no blind scanning

Command mode `replay <RECORDING> -- COMMAND...`:

1. Resolve the recording and hydrate the canonical session. Run target-independent planning before scope creation or spawn: integrity/capability, replayability, and effect authorization. Any predictable denial (including missing `--allow-write`) returns the complete zero-traffic report without running target code.
2. Create a supervisor-controlled cgroup-v2 scope. Command mode is Linux-only in 0.1.x and requires a scope; process groups are not ownership boundaries. Preflight must establish that target credentials cannot add unrelated processes to or escape through writable membership-control ancestors; otherwise fail with remediation to portable `--target` mode. The scope is lifecycle accounting, not a general sandbox.
3. Spawn the target with the same credential/descriptor/signal hardening as record mode. Readiness discovery enumerates stable `(pid,start_time,fd,socket_inode)` evidence for every current `cgroup.procs` member, joins socket inodes against `/proc/net/tcp{,6}`, keeps exact loopback listeners, intersects recorded protocol/port evidence, and requires one unique endpoint. `/proc/<pid>/net/tcp*` alone is namespace-wide and never ownership evidence. Membership/FD/inode/endpoint evidence is revalidated immediately before connection; changes restart the bounded snapshot or fail with zero traffic. No host-wide port scan.
4. Preserve the exact discovered address family: IPv4 produces the exact discovered loopback origin; IPv6 produces a bracketed origin such as `http://[::1]:<port>`. Never authorize one family from evidence for the other. Build existing replay options with inferred execution/target/host/read gates and call the sole existing planner/executor path.
5. Readiness has 30-second hard deadline and at most five unstable snapshots. JSON child output uses null sink as in record mode. Scope shutdown uses TERM 5 seconds, then KILL/wait 5 seconds, with 15-second absolute cleanup deadline. Timeout reports typed failure/orphan risk rather than success. Abrupt supervisor kill has same documented orphan limitation; pre-existing `--target` workloads are never terminated.

Inference table (explicit in docs/replay-safety.md):

| Gate | Command mode (`-- COMMAND`) | Explicit mode (`--target URL`) |
| --- | --- | --- |
| Execution intent | inferred (running replay implies intent) | explicit (execution acknowledgement) |
| Target authorization | inferred from unique spawned-scope loopback listener | explicit `--target` + exact host authorization |
| Host matching | inferred (same discovery) | explicit host authorization |
| Read effects | inferred | explicit read authorization |
| Write effects | explicit authorization | explicit authorization |
| Authentication / Publish / Unknown | always denied | always denied |
| Recorded destination | never contacted | never contacted |

Explicit mode keeps the full explicit syntax: `chronicle replay REC --target http://127.0.0.1:8080 --execute --allow-host 127.0.0.1 --allow-read` for read-only replay, and the same form with `--allow-write` for write replay. Authentication/publication/unknown remain denied in both modes.

Inference is all-or-nothing across executable Read/Write candidates: if any executable candidate lacks effect authorization (for example a Write without `--allow-write`), the whole replay stops with `stopped_policy` and zero traffic. Unsupported/incomplete operations retain their existing non-executable reasons and do not block authorized executable siblings.

The planner/executor/verifier in `chronicle-replay` are unchanged; only a narrow policy-construction API may be added (e.g. a constructor that expresses the inferred gates) — if the existing `LoopbackReplayOptions`/`ReplayPolicy` structs already express all gates, no replay-crate change is needed at all.

### D5. Compatibility: internal namespace, hidden aliases, one business logic

- New explicit internal namespace: `chronicle internal recorder`, `internal recorder-status`, `internal etl`, and `internal record-fixture`. The old top-level `recorder`, `recorder-status`, `etl`, and legacy `record`/`replay`/`inspect` flag forms remain as hidden deprecated 0.1.x aliases routed into the **same** application services as the new commands. No duplicate logic.
- Deprecation contract: human mode → stderr line `warning: 'chronicle etl' is deprecated; use 'chronicle internal etl'` (or equivalent); JSON success keeps stdout unchanged/atomic and emits exactly one v1 warning object on stderr. `invocation` and `replacement` are fixed canonical form identifiers/syntax templates, never raw argv or secret-bearing values. Failed legacy JSON emits only the normal error JSON with a safe replacement hint.
- Error paths on legacy forms point to new syntax.
- Removal boundary: 0.2, after docs, systemd unit, acceptance scripts, and CLI tests migrate to the new surface (systemd targets `chronicle internal recorder --config ...`).

### D6. Output and exit contracts

- Human record/replay output follows the normative examples in `user-intent-cli/spec.md`; durations format as `Xm Ys`. `Try:` prints `chronicle inspect <id>` and `chronicle replay <id> -- COMMAND...` without echoing raw argv.
- Mutable-v1 policy remains authoritative: every new public report is version 1, and existing replay/doctor versions remain 1. Public recording-oriented reports and hidden legacy session/mechanics reports have separately documented v1 command contracts; no v2, version dispatch, or compatibility freeze is introduced. Field meaning changes require an explicit compatibility-freeze OpenSpec change, not an ad hoc version bump.
- Public record v1 includes canonical recording ID, optional name, stable lifecycle/shutdown reason, duration milliseconds, operation/drop totals, categorized existing counters, and nullable structured child result (`{"kind":"exit_code","code":N}` or `{"kind":"signal","signal":N}`). Public inspect v1 adds recording identity around the existing bounded inspect facts; list v1 uses fields defined in the spec. Status/reason enums and nullability are golden-tested.
- Exit family 0/2/3/4/5/6 remains. Child application exit is factual and not forwarded; bootstrap/hardening/exec/readiness failures are Chronicle failures. Recording resolution failures exit 3.
- Shared atomic rendering is reused. In JSON mode child output never shares Chronicle stdout/stderr; deprecation `invocation` values are fixed non-secret form identifiers, not raw argv.

## Risks / Trade-offs

- **[Migration]** Acceptance scripts and systemd unit invoke legacy forms (`etl --wal-dir`, `record --source ebpf`). **Mitigation:** compatibility entrypoints keep exact behavior through 0.1.x; migration tasks update scripts in lockstep; removal deferred to 0.2.
- **[JSON contract]** Legacy JSON success output must stay byte-stable for consumers. **Mitigation:** deprecation diagnostics in JSON mode go to stderr only; stdout JSON is unchanged and atomic.
- **[cgroup scope control]** Many environments cannot create a scope whose membership controls are unavailable to dropped target credentials. **Mitigation:** preflight proves required access separation or command mode fails before target/recording creation with remediation to explicit selector/`--target` mode; no process-group fallback and no sandbox claim.
- **[Listener discovery]** Spawned apps may bind on non-recorded ports, expose IPv6 only, change membership/FDs during discovery, or fail to listen. **Mitigation:** bounded stable-snapshot/revalidation, exact family preservation, actionable zero-traffic failure, and `--target` escape hatch.
- **[Catalog divergence]** Catalog could disagree with WAL/session evidence after crashes or external edits. **Mitigation:** catalog is advisory and rebuildable; commands trust canonical/WAL evidence and surface disagreements.
- **[Record ETL on CLI]** Auto-ETL could race a daemon recorder. **Mitigation:** both acquire the same exact normalized `.chronicle-domain.lock` path and hold it through mutation; `RecorderLease::state_is_owned` is only a subordinate state-lock check and SHALL NOT be used as proof of domain-lock equivalence.

## Migration Plan

1. Land new surface behind new command names while legacy forms still parse (same binary); add the `chronicle internal` namespace.
2. Update `docs/operations.md`, `docs/replay-safety.md`, `docs/continuous-recorder*.md`, and README to the new syntax; document the inference table and deprecation schedule.
3. Migrate acceptance scripts (`scripts/acceptance/lib/scenarios/{p1,p2,extensions}/**`) and the systemd unit to the new forms; keep one dedicated legacy-compat scenario asserting the deprecated forms still work with warnings.
4. Migrate `crates/chronicle-cli/tests/cli_contract.rs` and `privileged_signal.rs`; add new contract tests for the five commands plus legacy migration tests.
5. Rollback requires reverting the coordinated CLI/application/common/storage changes, not only the CLI crate. Existing WAL/canonical/session formats remain readable; an old binary can inspect `<data>/sessions` or process an explicitly selected WAL through legacy paths, but it ignores catalog names, `latest`, sidecars, and public retry. Additive private files may remain. No authoritative-format migration is required.

## Open Questions

- Whether `--allow-write` in command mode should also permit a one-shot explicit prompt for write operations, or remain flag-only for 0.1.x (default: flag-only, prompt is future work).
