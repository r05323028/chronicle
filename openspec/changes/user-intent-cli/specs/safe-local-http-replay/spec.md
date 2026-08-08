## MODIFIED Requirements

### Requirement: Dry-run default and complete plan visibility

Replay SHALL load the canonical session, build one deterministic plan, and perform no network unless authorization preflight passes. Explicit-target and hidden legacy modes remain dry-run unless `--execute`; command mode infers execution intent from `-- COMMAND...`. Before command-mode scope/target creation, Chronicle SHALL evaluate target-independent integrity, replayability, and effect gates; predictable denial returns the complete report with zero traffic and does not run target code. Planner SHALL represent every operation once as executable candidate or non-executable with stable reason. Non-executable operations SHALL NOT block executable siblings. Any target/effect denial for executable candidates SHALL block all requests. Executor SHALL return one ordered result per planned operation and explicit outcome through shared atomic JSON.

#### Scenario: Explicit mode default dry-run

- **WHEN** explicit-target or legacy replay runs without `--execute`
- **THEN** zero network occurs and every operation is represented as not attempted

#### Scenario: Command execution intent

- **WHEN** command mode passes target-independent gates
- **THEN** `-- COMMAND...` supplies execution intent, while listener/host/read gates still require valid inference and Write remains explicit

#### Scenario: Predictable denial before spawn

- **WHEN** command-mode recording has an executable Write without `--allow-write`, invalid integrity, or no executable operations
- **THEN** complete denial report is emitted with zero traffic and target command is not spawned

#### Scenario: Execute mixed session

- **WHEN** session has authorized executable candidates plus incomplete/unsupported/ambiguous-loss operations
- **THEN** Chronicle sends only executable operations and retains stable non-executable results for siblings

#### Scenario: Authorization denial

- **WHEN** any executable candidate lacks required target/effect authorization
- **THEN** no operation is sent, outcome is `stopped_policy`, executable candidates are policy-not-attempted, and non-executable reasons remain intact

#### Scenario: No executable operations

- **WHEN** session contains no complete supported operation
- **THEN** zero network occurs, outcome is `stopped_invalid_session`, every blocker is retained, and command mode does not spawn target

#### Scenario: Deterministic operation order

- **WHEN** plan spans operations/connections
- **THEN** order remains deterministic by scheduled offset and operation sequence and result order matches plan

#### Scenario: Runtime failure after earlier success

- **WHEN** transport fails after completed operations
- **THEN** outcome is `stopped_transport` and report retains completed, failed-current, and later-unattempted results without rollback claim

### Requirement: Explicit loopback target authorization

Execution SHALL require explicit target origin, repeated exact allow-host, and loopback IP literal, OR in command mode one uniquely discovered supervised-scope listener. Explicit grammar remains `chronicle replay REC --target http://127.0.0.1:8080 --execute --allow-host 127.0.0.1 --allow-read`, adding independent `--allow-write` when needed. Target grammar permits only `http://<loopback-ip>:<port>` with optional trailing slash and rejects userinfo, DNS, wildcard/unspecified/non-loopback/multicast hosts, path/query/fragment, HTTPS, or missing port before DNS/network. Recorded destination is never fallback.

Command discovery SHALL join stable process-start/FD/socket-inode ownership evidence from scope members against `/proc/net/tcp{,6}`, never `/proc/<pid>/net/tcp*` alone, and require one loopback endpoint matching recorded protocol/port evidence. It SHALL revalidate membership/FD/inode/endpoint before connection and preserve exact family/address (`127.0.0.1` vs bracketed `::1`); one family never authorizes the other. Mutation, ambiguity, or absence fails with zero traffic. Explicit target and command modes are mutually exclusive.

#### Scenario: Allowed IPv4 target

- **WHEN** target is `http://127.0.0.1:<port>` and `--allow-host 127.0.0.1` is present
- **THEN** target gate passes while remaining execution gates still apply

#### Scenario: Command-mode inferred target

- **WHEN** `replay RECORDING -- COMMAND...` spawns an app whose only matching listener is the unique loopback listener consistent with recorded protocol/port evidence
- **THEN** target authorization is satisfied by the discovered listener and replay proceeds against loopback only

#### Scenario: Recorded target omitted

- **WHEN** caller supplies no target in explicit mode
- **THEN** replay does not use recorded server and execution is denied

#### Scenario: Unsafe target

- **WHEN** target contains userinfo, wildcard, external IP, DNS host, base path, query, fragment, HTTPS, or mismatched allow-host
- **THEN** execution is denied before DNS/network activity and error redacts userinfo

### Requirement: Separate effect authorization

Executable Read candidates SHALL require `--allow-read` and executable Write candidates SHALL require `--allow-write` in explicit mode; flags SHALL be independent and any missing required effect SHALL block all executable candidates with `stopped_policy`/zero traffic. Command mode SHALL infer Read only; Write remains explicit. All-or-nothing SHALL apply across executable Read/Write candidates, not unsupported/incomplete siblings: non-executable operations retain blockers and authorized executable siblings may still yield `completed_with_skips`. Existing config SHALL NOT supply missing CLI gates. Authentication, Publish, Unknown, unsupported, incomplete, and truncated operations SHALL remain non-executable. Spawned scope is not a sandbox; inference SHALL apply only to revalidated exact loopback listener of current command replay.

#### Scenario: GET authorization

- **WHEN** complete GET operation has explicit read authorization and all other gates pass
- **THEN** operation may execute

#### Scenario: Command-mode GET inference

- **WHEN** `replay RECORDING -- COMMAND...` discovers a unique loopback target and the recording contains only Read operations
- **THEN** complete Read operations may execute without an explicit read flag

#### Scenario: Command-mode write blocks whole replay

- **WHEN** command mode discovers a unique loopback target but the recording contains any Write operation without write authorization
- **THEN** the whole replay stops with `stopped_policy`, zero traffic, and every operation is unattempted; inferred read authorization does not yield a partial subset

#### Scenario: Mixed effects require both flags

- **WHEN** an explicit-mode recording contains both Read and Write operations and only `--allow-read` is present
- **THEN** the whole replay stops with `stopped_policy` and zero traffic; both flags are required

#### Scenario: POST denied without write authorization

- **WHEN** POST operation is planned with read only
- **THEN** plan marks policy denied and executor sends no request

#### Scenario: Command-mode write still explicit

- **WHEN** command mode discovers a unique loopback target but no write authorization is present
- **THEN** complete Write operations are policy-denied and no write request is sent

#### Scenario: Execute flag alone

- **WHEN** caller passes `--execute` but no effect flag
- **THEN** execution is denied and execute flag does not broaden policy

#### Scenario: Permissive config cannot authorize CLI

- **WHEN** config sets dry-run false or allows effects but required CLI target, allow-host, effect, or execute flag is absent
- **THEN** execution remains denied and config does not supply missing authorization

### Requirement: Origin mapping and request preservation

Replay SHALL replace recorded origin with the authorized explicit target or exact inferred command-mode listener while preserving supported origin-form path/query bytes, method, safe ordered headers, and exact body. It SHALL never access fixture/WAL/eBPF data or contact recorded production destination during execution.

#### Scenario: Path/query replay

- **WHEN** request target is `/items?q=a%20b`
- **THEN** authorized target receives identical origin-form path/query bytes

#### Scenario: Binary POST body

- **WHEN** persisted body contains binary bytes
- **THEN** adapter hydrates verified artifact and sends exact bytes with recomputed Content-Length

#### Scenario: WAL absent

- **WHEN** fixture and WAL are deleted after session persistence
- **THEN** replay still plans/executes from canonical/artifact store

### Requirement: Session replayability classification

Inspect/planner SHALL classify `fully_replayable` when every operation is executable, `partially_replayable` when executable and non-executable operations coexist, and `not_replayable` when none is executable or integrity/capability invalidates session. Classification does not authorize traffic: explicit mode still requires explicit target/allow-host/effects/execute; command mode requires the inferred execution/target/host/read gates plus explicit Write.

#### Scenario: Partially replayable session

- **WHEN** session has one complete supported operation and one degraded operation
- **THEN** inspect/planner reports partial replayability with counts and blockers

#### Scenario: Not replayable session

- **WHEN** every operation is incomplete/unsupported or integrity fails
- **THEN** inspect/planner reports not replayable and command mode does not spawn a target

### Requirement: Replay authorization remains loopback-only

Replay SHALL preserve explicit loopback target, exact repeated allow-host, independent effect authorization, dry-run default, and execute gate in explicit mode. Command mode infers only execution intent, exact revalidated scope-listener target/host, and Read; Write remains explicit and Authentication/Publish/Unknown denied. Predictable target-independent denial occurs before spawn. Spawned targets use dropped invoking-user credentials and bounded cleanup on every normal path; abrupt supervisor kill is reported as an orphan risk, not guaranteed cleanup. Pre-existing `--target` workloads are never terminated. Runtime Authorization replacement SHALL come only from configured environment-variable name and SHALL never expose its value. Captured Host, Authorization, Cookie, forwarding, hop-by-hop, Connection-token, and Transfer-Encoding headers SHALL NOT affect target/policy; replay SHALL rebuild target Host/Content-Length. It SHALL use one request per connection and SHALL never default to recorded destination, follow redirects, perform DNS target expansion, retry, proxy, TLS, or authorize incomplete/ambiguous operations. The existing planner, executor, and verification SHALL remain the sole execution model; no duplicate execution path SHALL be introduced.

#### Scenario: Explicit target override

- **WHEN** authorized replay executes
- **THEN** every connection uses supplied loopback target and original destination is never contacted

#### Scenario: Command-mode scope target

- **WHEN** command-mode replay executes against the discovered spawned-scope listener
- **THEN** every connection uses that loopback listener and the recorded destination is never contacted

#### Scenario: Captured credentials and forwarding headers

- **WHEN** recorded operation contains Host/auth/cookie/forwarding/hop-by-hop fields
- **THEN** fields cannot authorize or retarget replay, target headers are rebuilt, and runtime credential value comes only from approved environment source

#### Scenario: Original destination unreachable test

- **WHEN** original destination is instrumented and different authorized local target is supplied
- **THEN** only authorized target receives requests

#### Scenario: Redirect response

- **WHEN** authorized target responds 3xx with Location outside allowlist
- **THEN** response is observed/verified according to expectation and no redirect request is made

#### Scenario: Target resolution cannot expand authorization

- **WHEN** target is DNS name, unspecified, wildcard, non-loopback, or mismatched allow-host
- **THEN** replay is rejected before network I/O
