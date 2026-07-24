## ADDED Requirements

### Requirement: Dry-run default and complete plan visibility
Replay SHALL load canonical filesystem session, build deterministic plan, and perform no network I/O unless explicit execution passes every safety gate. Planner SHALL represent per-operation allowed, denied, or unsupported decisions with stable reasons rather than failing at first denial. Any preflight denial SHALL block all requests. After allowed preflight, sequential executor SHALL stop on first transport error or completed Failed/Inconclusive/Unsupported verification and SHALL report already executed versus unattempted operations without claiming rollback. Plan SHALL be shown before any execution.

#### Scenario: Default dry-run
- **WHEN** replay is invoked without `--execute`
- **THEN** it prints full plan, performs zero socket connects, and exits success even when decisions are denied

#### Scenario: Execute with denied operation
- **WHEN** `--execute` is present but any operation lacks effect authorization or is unsupported
- **THEN** whole run exits safety denial and performs zero requests

#### Scenario: Deterministic operation order
- **WHEN** plan contains multiple operations
- **THEN** operations order by immediate scheduled offset then canonical sequence

#### Scenario: Runtime failure after earlier success
- **WHEN** earlier allowed operation executes but later operation has transport or verification failure
- **THEN** executor stops, reports earlier operation executed and remaining operations unattempted, and makes no rollback claim

### Requirement: Explicit loopback target authorization
Execution SHALL require explicit target origin, repeated allow-host matching target, and loopback IP literal. Target grammar SHALL permit only `http://<loopback-ip>:<port>` with optional trailing slash. It SHALL reject userinfo, DNS names, wildcard/unspecified/non-loopback/multicast hosts, path/query/fragment, HTTPS, and missing port. Recorded destination SHALL never be target fallback.

#### Scenario: Allowed IPv4 target
- **WHEN** target is `http://127.0.0.1:<port>` and `--allow-host 127.0.0.1` is present
- **THEN** target gate passes while remaining execution gates still apply

#### Scenario: Recorded target omitted
- **WHEN** caller supplies no target
- **THEN** replay does not use recorded server and execution is denied

#### Scenario: Unsafe target
- **WHEN** target contains userinfo, wildcard, external IP, DNS host, base path, query, fragment, HTTPS, or mismatched allow-host
- **THEN** execution is denied before DNS/network activity and error redacts userinfo

### Requirement: Separate effect authorization
Execution SHALL require `--allow-read` for Read and `--allow-write` for Write in addition to `--execute` and target gates. Existing config target mappings, `dry_run=false`, and allow booleans SHALL NOT satisfy or broaden missing command-line gates; config may only narrow policy or provide timeout/credential environment-variable names. Unknown, Authentication, Publish, unsupported, incomplete, and truncated operations SHALL remain non-executable in P0.

#### Scenario: GET authorization
- **WHEN** complete GET operation has explicit read authorization and all other gates pass
- **THEN** operation may execute

#### Scenario: POST denied without write authorization
- **WHEN** POST operation is planned with read only
- **THEN** plan marks policy denied and executor sends no request

#### Scenario: Execute flag alone
- **WHEN** caller passes `--execute` but no effect flag
- **THEN** execution is denied and execute flag does not broaden policy

#### Scenario: Permissive config cannot authorize CLI
- **WHEN** config sets dry-run false or allows effects but required CLI target, allow-host, effect, or execute flag is absent
- **THEN** execution remains denied and config does not supply missing authorization

### Requirement: Origin mapping and request preservation
Replay SHALL replace recorded origin with explicit target while preserving supported origin-form path/query bytes, method, safe ordered headers, and exact request body. It SHALL never access fixture, WAL, eBPF, or recorded production destination.

#### Scenario: Path/query replay
- **WHEN** recorded request target is `/items?q=a%20b`
- **THEN** target receives same origin-form path/query bytes

#### Scenario: Binary POST body
- **WHEN** persisted request body contains binary bytes
- **THEN** adapter hydrates verified artifact and sends exact bytes with recomputed Content-Length

#### Scenario: WAL absent
- **WHEN** fixture and WAL are deleted after session persistence
- **THEN** replay plan and execution still work from canonical/artifact store

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
Verifier SHALL compare exact status, exact body size/SHA-256, and recorded end-to-end response headers except fixed case-insensitive ignore set `date`, `server`, `content-length`, `connection`, `transfer-encoding`, `keep-alive`, and `set-cookie`. `ObservedResponse` SHALL carry versioned protocol data; HTTP adapter SHALL encode `HttpObservedResponseV1` with status and ordered raw-byte headers while body remains payload ref. Duplicate compared values SHALL preserve order. It SHALL not perform heuristic semantic diff.

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
Verification SHALL use Passed, Failed, Skipped, Inconclusive, and Unsupported. Missing recorded response SHALL be Inconclusive and non-executable; Skipped SHALL mean no verification ran because plan was dry-run or policy-denied. Recorded truncation/incompleteness SHALL be Inconclusive; unsupported framing SHALL be Unsupported; deterministic mismatch after a complete observed response SHALL be Failed. Refusal, timeout, disconnect, I/O, or incomplete observed response SHALL be typed execution error with exit 5 and no verification result. Human and JSON output SHALL include per-operation and aggregate statuses with stable categories.

#### Scenario: Missing recorded response
- **WHEN** request has no recorded response expectation
- **THEN** plan marks operation non-executable and verification state Inconclusive

#### Scenario: Verification not run
- **WHEN** plan is dry-run or policy-denied and no execution occurs
- **THEN** operation verification state is Skipped

#### Scenario: Truncated expectation
- **WHEN** recorded response is truncated or incomplete
- **THEN** plan is non-executable and verification state is Inconclusive

#### Scenario: Unsupported expectation
- **WHEN** recorded or observed response uses unsupported framing
- **THEN** status is Unsupported with explicit feature code

#### Scenario: Machine-readable result
- **WHEN** JSON output is selected
- **THEN** result includes session/run IDs, plan decisions, operation statuses/details, and aggregate counts without payload or secrets
