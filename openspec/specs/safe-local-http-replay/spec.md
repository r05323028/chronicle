## Purpose

Default-deny loopback target planning, safe HTTP execution, header and credential handling, and deterministic verification.

## Requirements

### Requirement: Dry-run default and complete plan visibility

Replay SHALL load one verified epoch session or a verified deterministic parent aggregation, build one deterministic plan, and perform no network unless authorization preflight passes. Parent aggregation SHALL order epoch sessions by epoch ordinal/lineage, include each terminal canonical operation exactly once with its completion-owner and contributing epoch provenance, and verify continuation references as integrity dependencies. Continuation-only evidence is represented as non-executable completeness/provenance metadata, not as a duplicate operation. Any target/effect denial for executable candidates SHALL block all requests; non-executable epoch siblings SHALL not block authorized executable siblings. Executor SHALL return one ordered result per planned terminal operation through existing shared atomic JSON.

#### Scenario: Replay one epoch

- **WHEN** user selects a finalized epoch session without `--execute`
- **THEN** plan contains only that epoch's operations, zero network occurs, and every operation is represented as not attempted

#### Scenario: Replay parent aggregation

- **WHEN** user selects a parent recording with multiple verified epoch sessions
- **THEN** plan orders operations deterministically by epoch lineage then canonical operation order and retains epoch IDs in every result

#### Scenario: Missing epoch in parent aggregation

- **WHEN** parent index references a missing/unverified epoch session
- **THEN** replay fails closed before network rather than silently replaying a partial parent

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

Replay SHALL replace recorded origin with the authorized explicit target or exact inferred command-mode listener while preserving supported origin-form path/query bytes, method, safe ordered headers, and exact body. Parent aggregation SHALL replay each operation against the one authorized target; epoch source destination, rollover order, and stored WAL paths SHALL never select a network target. Replay SHALL never access fixture/WAL/eBPF data or contact recorded production destination during execution.

#### Scenario: Parent target override

- **WHEN** authorized parent replay contains operations from several epochs
- **THEN** every request uses the one authorized loopback target and original destinations are never contacted

#### Scenario: Epoch WAL absent

- **WHEN** fixture and all epoch WALs are deleted after verified parent/session persistence
- **THEN** replay still plans/executes from canonical epoch/session artifacts

#### Scenario: Path/query replay

- **WHEN** request target is `/items?q=a%20b`
- **THEN** authorized target receives identical origin-form path/query bytes

#### Scenario: Binary POST body

- **WHEN** persisted body contains binary bytes
- **THEN** adapter hydrates verified artifact and sends exact bytes with recomputed Content-Length

#### Scenario: WAL absent

- **WHEN** fixture and WAL are deleted after session persistence
- **THEN** replay still plans/executes from canonical/artifact store

### Requirement: Safe request-header rewriting

Adapter SHALL remove every captured Host and emit exactly one target-authority Host, remove hop-by-hop fields and fields named by Connection, strip captured Authorization, Proxy-Authorization, Cookie, Forwarded, X-Forwarded-*, and Expect, remove Transfer-Encoding, and emit exactly one recomputed Content-Length. Other end-to-end headers SHALL preserve duplicate order and raw values.

#### Scenario: Host replacement

- **WHEN** request has zero, one, or duplicate captured Host fields
- **THEN** outbound request contains exactly one Host naming explicit loopback target authority only

#### Scenario: Sensitive captured headers

- **WHEN** request contains Authorization, Proxy-Authorization, Cookie, or forwarding identity fields
- **THEN** outbound request omits values and plan/log/error never prints them

#### Scenario: Connection-token header

- **WHEN** Connection header names custom hop-by-hop header
- **THEN** both Connection and named header are removed

#### Scenario: Duplicate safe headers

- **WHEN** request contains repeated safe end-to-end header
- **THEN** outbound request preserves both in original order

### Requirement: Runtime-only credential replacement

Literal credentials SHALL NOT be accepted in persisted config or CLI. Optional Authorization replacement SHALL be read from environment variable named by configuration into secret-redacting replay context. Before wire serialization, replacement SHALL reject CR, LF, NUL, DEL, and all control bytes except HTAB; captured credential SHALL never be replayed as fallback.

#### Scenario: Runtime credential configured

- **WHEN** configuration names an environment variable and future authorized operation requires replacement
- **THEN** adapter obtains bytes at runtime and debug/output renders redacted marker

#### Scenario: Runtime credential missing

- **WHEN** replacement is required but environment variable is absent
- **THEN** plan/execution fails safely without revealing variable contents or captured credential

#### Scenario: Runtime credential injection attempt

- **WHEN** environment credential contains CR, LF, NUL, DEL, or disallowed control byte
- **THEN** execution fails before socket write with safe invalid-field-value error

### Requirement: Minimal plain HTTP execution

HTTP adapter SHALL use sequential one-request-per-connection Tokio TCP with `Connection: close`, 5-second bounded connect/operation timeout, no retry, no proxy, no TLS, and no redirect following. Only immediate timing SHALL execute. Responses SHALL be bounded and parsed under supported HTTP rules; a complete fixed-length/no-body response SHALL finish without waiting for EOF.

#### Scenario: Successful local request

- **WHEN** all gates pass and loopback server returns supported fixed-length response
- **THEN** adapter sends one request, observes response, and closes connection

#### Scenario: Redirect response

- **WHEN** local server returns 3xx Location pointing elsewhere
- **THEN** adapter does not follow and verifier compares 3xx response itself

#### Scenario: Timeout

- **WHEN** local target does not complete a supported response within timeout
- **THEN** executor returns typed timeout error, exits 5, emits no verification result, and does not retry

#### Scenario: Fixed-length response keeps connection open

- **WHEN** server sends complete Content-Length response but keeps socket open
- **THEN** adapter completes at framed body boundary without false timeout

#### Scenario: Preserve timing requested

- **WHEN** execution requests preserve timing
- **THEN** plan is unsupported/denied before network because P0 executes immediate timing only

#### Scenario: Pipelined recorded connection

- **WHEN** canonical data marks pipeline depth greater than one
- **THEN** execution is unsupported rather than silently replaying with changed semantics

### Requirement: Deterministic HTTP verification

Verifier SHALL compare exact status, exact body size/SHA-256, and recorded end-to-end response headers except fixed case-insensitive ignore set `date`, `server`, `content-length`, `connection`, `transfer-encoding`, `keep-alive`, and `set-cookie`. `ObservedResponse` SHALL carry sole active protocol-data schema v1; HTTP adapter SHALL encode `HttpObservedResponse` with status and ordered raw-byte headers while body remains payload ref. Duplicate compared values SHALL preserve order. It SHALL not perform heuristic semantic diff.

#### Scenario: Verification pass

- **WHEN** status, compared headers, and body bytes match
- **THEN** verification status is Passed

#### Scenario: Status mismatch

- **WHEN** observed status differs
- **THEN** status is Failed with expected/observed codes

#### Scenario: Header mismatch

- **WHEN** non-ignored header differs
- **THEN** status is Failed and details name header without secret/body values

#### Scenario: Body mismatch

- **WHEN** body differs
- **THEN** status is Failed with expected/observed size and digest, not body bytes

#### Scenario: Dynamic ignored header

- **WHEN** only Date, Server, Content-Length, connection framing, or Set-Cookie differs
- **THEN** ignored difference does not fail verification

### Requirement: Complete verification status model and output

Verification SHALL use Passed, Failed, Skipped, Inconclusive, and Unsupported. Missing recorded response or incomplete/truncated/unmatched/ambiguous-loss expected operation SHALL be Inconclusive/Unsupported and non-executable; Skipped SHALL mean no verification ran because operation was non-executable, replay was dry-run/policy-denied, or execution stopped earlier. Refusal, timeout, disconnect, I/O, incomplete observed response, and bounded-response overflow SHALL be typed failed operation with overall transport outcome rather than discarding earlier results. Human/JSON report SHALL include every per-operation and aggregate status/count/outcome with stable categories and no body/header values.

#### Scenario: Missing recorded response

- **WHEN** operation has no expected response
- **THEN** preflight marks it non-executable and report identifies inconclusive expectation

#### Scenario: Verification not run

- **WHEN** replay is dry-run or policy-denied
- **THEN** each operation is not attempted with Skipped verification reason and zero network

#### Scenario: Truncated expectation

- **WHEN** canonical expected operation is truncated/incomplete
- **THEN** report is Inconclusive/non-executable and operation is not sent

#### Scenario: Unsupported expectation

- **WHEN** canonical operation is malformed, unmatched, pipelined, upgrade, or unsupported framing
- **THEN** report is Unsupported/non-executable with stable blocker

#### Scenario: Middle transport failure

- **WHEN** middle operation fails to connect, times out, disconnects, or returns incomplete response
- **THEN** prior results remain, failed operation includes transport category, later operations are unattempted, and exit maps to transport failure

#### Scenario: Machine-readable result

- **WHEN** user requests replay JSON
- **THEN** plan is validated before network and stdout later contains one valid versioned report with plan, overall outcome/counts, and every operation result in deterministic order

### Requirement: Explicit replay execution outcome

Existing `ReplayExecution` SHALL retain exact outcomes `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`. Parent aggregation with all executable epoch operations passing SHALL produce `completed`; authorized execution with non-executable operations or degraded/missing epoch siblings SHALL produce `completed_with_skips` only when the selected aggregate is explicitly marked partial and policy permits it. Integrity failure, missing required epoch, contradictory parent index, or no executable operation SHALL produce `stopped_invalid_session` before network.

#### Scenario: Parent aggregate completed

- **WHEN** every operation across verified selected epochs is executable, authorized, and verifies
- **THEN** outcome is `completed` and results retain epoch provenance

#### Scenario: Parent aggregate partial

- **WHEN** selected parent explicitly contains non-executable operations but all executable operations pass
- **THEN** outcome is `completed_with_skips` with stable epoch/operation blockers

#### Scenario: Aggregate integrity failure

- **WHEN** parent/epoch manifest or digest verification fails
- **THEN** outcome is `stopped_invalid_session`, no adapter connects, and no sibling epoch is silently attempted

#### Scenario: All operations completed

- **WHEN** all operations are executable, authorized, execute, and verify Passed
- **THEN** outcome is `completed` and all results are completed

#### Scenario: Executable subset completed

- **WHEN** all executable operations pass and one or more operations are non-executable
- **THEN** outcome is `completed_with_skips`; executable results are completed and invalid operations remain not attempted

#### Scenario: First operation transport failure

- **WHEN** first authorized operation fails transport
- **THEN** outcome is `stopped_transport`, first result is failed, and all remaining results are explicitly unattempted

#### Scenario: Policy rejection

- **WHEN** executable candidate is unauthorized by target/effect gates
- **THEN** outcome is `stopped_policy`, every executable result is unattempted, non-executable reasons remain, and no adapter connects

#### Scenario: Verification mismatch

- **WHEN** completed response mismatches status/header/body expectation
- **THEN** transport state is completed, verification is Failed, outcome is `stopped_verification`, later results are unattempted, and exit is 6

#### Scenario: Missing protocol capability

- **WHEN** persisted operation protocol lacks required replay/verifier capability
- **THEN** invalid-session outcome represents every operation and no adapter connects

### Requirement: Operation replay result completeness

Every planned operation, including operations from each selected epoch, SHALL have exactly one result with operation ID, parent/epoch identity, executable decision, execution state, transport category when failed, verification status/details when applicable, and stable not-attempted reason. Report aggregation SHALL equal per-operation results and shall not collapse same-shaped operations from distinct epoch/WAL provenance.

#### Scenario: Same request in different epochs

- **WHEN** two epochs contain byte-identical HTTP requests
- **THEN** plan/results contain two distinct provenance-derived operation IDs and two result entries

#### Scenario: Partial parent execution

- **WHEN** execution stops during epoch N of a parent plan
- **THEN** earlier epoch results remain, current result is failed, later operations across N and successors are explicitly unattempted

#### Scenario: Partial result accounting

- **WHEN** execution stops after operation N of M
- **THEN** report counts completed + failed + unattempted exactly M with no omitted or duplicated operation

#### Scenario: Continued execution mode absent

- **WHEN** one operation fails during a recording
- **THEN** Chronicle stops sequential execution and marks all later operations unattempted; it does not claim failure continuation support

### Requirement: Session replayability classification

Inspect/planner SHALL classify each epoch and the selected parent aggregate using existing `fully_replayable`, `partially_replayable`, and `not_replayable` states. Parent classification SHALL be `fully_replayable` only when every selected required epoch session is integrity-valid and every operation is executable; missing, unverified, or non-executable epoch evidence SHALL prevent a full classification. Classification SHALL not authorize traffic: explicit mode still requires loopback target, repeated allow-host, effect flags, and `--execute`; command mode retains exact inferred listener/read gates and explicit Write.

#### Scenario: Partial epoch parent

- **WHEN** one epoch is replayable and another has loss-degraded operations
- **THEN** parent inspect reports partial replayability with epoch-scoped counts/blockers

#### Scenario: Missing finalized epoch

- **WHEN** parent aggregate is incomplete because a required epoch publication is absent
- **THEN** planner reports not-replayable/integrity-incomplete and does not silently omit that epoch

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

### Requirement: Deterministic verification policy

HTTP verification SHALL retain existing exact status, exact body size/SHA-256, and ordered header comparison excluding fixed documented ignore set. Verification SHALL not add heuristic, secret-bearing, or user-defined comparison. Missing expected response, transport failure, and incomplete expected operation SHALL map to specified result/outcome rather than silent pass.

#### Scenario: Exact verification pass

- **WHEN** observed status, selected ordered headers, and body digest/size match
- **THEN** verification is Passed

#### Scenario: Exact verification mismatch

- **WHEN** any compared status/header/body field differs
- **THEN** verification is Failed with metadata-only detail and body/header values omitted

### Requirement: Bounded replay and report

Replay SHALL retain five-second connect/operation timeout, 8 MiB observed body bound, and canonical 10,000-operation limit per epoch/session. Parent aggregation SHALL enforce a bounded total planned-operation/report limit configured by application policy; over-limit or malformed aggregate input SHALL be rejected before network and SHALL not omit epoch operations. Report output SHALL include stable parent/epoch/session identities and counters without captured body/header values.

#### Scenario: Parent aggregate over limit

- **WHEN** selected parent would exceed configured total plan/report bound
- **THEN** replay returns typed invalid-session result before network and reports exact bound/counter

#### Scenario: Response body limit

- **WHEN** replay target response exceeds body bound
- **THEN** current operation fails with bounded category, prior results remain, and later operations are unattempted

#### Scenario: Over-limit imported session

- **WHEN** replay loads otherwise valid imported session containing more than 10,000 operations
- **THEN** replay rejects before network with typed invalid-session outcome and omits no operation silently

### Requirement: Parent and epoch replay selection is explicit and deterministic

CLI/application replay selection SHALL distinguish stable parent `RecordingId`, `EpochId`, and session ID. Selecting a parent resolves verified terminal operations and continuation dependencies from authoritative `epochs.json`; selecting an epoch resolves only that session. No selection uses newest directory, publication time, or filesystem mtime. Parent replay selection SHALL use only explicit `source_provenance.recording_id` / `epoch_id`; a session without explicit provenance is invalid/unresolved and fails closed with a typed error rather than synthesizing identity from identifier equality. A session matched by recording provenance without an explicit epoch identity fails closed.

#### Scenario: Explicit epoch selection

- **WHEN** user supplies an epoch/session identity
- **THEN** replay resolves exactly that verified session and does not include successor or predecessor epochs

#### Scenario: Parent selection

- **WHEN** user supplies stable parent recording identity
- **THEN** application resolves ordered verified epoch sessions from parent index and passes one immutable plan to existing replay executor

#### Scenario: Ambiguous legacy identity

- **WHEN** an old recording/session identifier maps to multiple epoch sessions without an unambiguous compatibility record
- **THEN** selection fails closed and asks for explicit epoch/session identity

### Requirement: Epoch boundary is replay safety boundary

A WAL epoch boundary SHALL not force a protocol reset during ETL, but replay SHALL never decode raw continuation fragments or synthesize a request/response at execution time. A terminal operation completed in a successor epoch is replayed once from that successor's canonical session, with predecessor ranges retained as provenance. A continuation-only predecessor session is independently inspectable but contributes no executable operation. If continuation is missing, invalid, unsupported, or incomplete, the affected operation remains non-executable/inconclusive under existing replayability rules; parent selection fails closed on a required gap rather than silently omitting it.

#### Scenario: Long-lived connection at rollover

- **WHEN** predecessor epoch ends with pending HTTP bytes and successor contains a valid verified continuation
- **THEN** replay uses only a successor-owned terminal operation if one exists, otherwise reports the predecessor fragment non-executable; it never synthesizes raw combined traffic

#### Scenario: Parent operation order

- **WHEN** parent aggregation contains operations from multiple epochs
- **THEN** order uses deterministic epoch ordinal and canonical order, not rollover wall time or ETL publication order

### Requirement: Replay authorization remains loopback-only across epochs

Replay SHALL preserve explicit loopback target, exact repeated allow-host, independent effect authorization, dry-run default, and execute gate. Parent/epoch source metadata SHALL never broaden target, host, effect, or execution authorization. Command mode infers only one exact revalidated loopback listener and Read; Write remains explicit. Captured Host, credentials, cookies, forwarding, hop-by-hop fields, recorded destination, WAL paths, and epoch count SHALL not affect authorization. Existing planner/executor/verifier remain the sole execution model.

#### Scenario: Mixed-epoch write policy

- **WHEN** selected parent contains a Write operation in any epoch and `--allow-write` is absent
- **THEN** whole executable parent plan stops with `stopped_policy`, zero traffic, and every executable result is unattempted

#### Scenario: Epoch source cannot retarget

- **WHEN** one epoch records a different production destination or port
- **THEN** authorized replay still uses supplied/inferred loopback target only

### Requirement: Replay completion-owned cross-epoch operations

Parent replay plans SHALL include terminal canonical operations exactly once and SHALL verify their continuation/provenance references. Continuation-only fragments SHALL affect completeness/classification but SHALL not be decoded or sent as requests by replay. A missing, invalid, or unsupported continuation makes the affected operation non-executable/inconclusive under existing deny-by-default rules.

#### Scenario: Terminal successor operation replay

- **WHEN** a successor session owns an operation completed from predecessor and successor evidence
- **THEN** parent replay plans one operation from the successor session and preserves predecessor provenance without duplicate traffic

#### Scenario: Continuation-only predecessor

- **WHEN** predecessor session contains only a pending continuation reference
- **THEN** inspect reports the pending/incomplete state and replay sends no standalone request from that fragment
