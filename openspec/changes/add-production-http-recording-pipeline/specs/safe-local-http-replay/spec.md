## MODIFIED Requirements

### Requirement: Dry-run default and complete plan visibility
Replay SHALL load canonical filesystem session, build deterministic plan, and perform no network unless explicit execution passes session integrity/capability plus target/effect authorization preflight. Planner SHALL represent every operation once as executable candidate or non-executable with stable completeness/protocol/loss reason. Non-executable operations SHALL NOT block executable siblings. Any target/effect authorization denial for executable candidates SHALL block all requests. Executor SHALL return one ordered result per planned operation plus explicit outcome, retaining completed, failed, non-executable, and later unattempted operations. JSON SHALL use shared atomic renderer.

#### Scenario: Default dry-run
- **WHEN** replay runs without `--execute`
- **THEN** zero network I/O occurs and execution outcome/result list represents every operation as not attempted

#### Scenario: Execute mixed session
- **WHEN** session has complete supported operations plus incomplete/unsupported/ambiguous-loss operations and executable candidates pass authorization
- **THEN** Chronicle sends only executable operations in deterministic order and reports every non-executable operation as not attempted with stable reason

#### Scenario: Execute with authorization denial
- **WHEN** any executable candidate lacks explicit target/effect authorization
- **THEN** no operation is sent, outcome is `stopped_policy`, executable candidates are policy-not-attempted, and non-executable reasons remain intact

#### Scenario: No executable operations
- **WHEN** session contains no complete supported operation
- **THEN** zero network occurs, outcome is `stopped_invalid_session`, and every operation has explicit non-executable result

#### Scenario: Deterministic operation order
- **WHEN** plan contains operations across connections
- **THEN** order is deterministic by scheduled offset and operation sequence and result order matches plan

#### Scenario: Runtime failure after earlier success
- **WHEN** transport fails on operation after one or more completed operations
- **THEN** outcome is `stopped_transport` and report retains prior completed, failed current, and later unattempted results without rollback claim

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

## ADDED Requirements

### Requirement: Explicit replay execution outcome
Existing `ReplayExecution` SHALL include `ReplayOutcome`; implementation SHALL NOT introduce duplicate execution model. Exact serialized outcomes SHALL be `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`. Precedence SHALL be dry-run, invalid session/capability/no executable operations, target/effect policy, runtime transport/verification, then completed/completed-with-skips. Non-executable mixed-session entries SHALL lead to `completed_with_skips` when every executable candidate passes. Expected operational stops SHALL return full execution/report; registry construction/invariant failure SHALL remain typed error.

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
Every planned operation SHALL have exactly one result with operation ID, executable/non-executable decision, explicit execution state (`completed`, `failed`, `not_attempted`), transport category when failed, verification status/details when applicable, and stable not-attempted reason (`non_executable_*`, `dry_run`, `policy_denied`, or `stopped_after_*`). Report aggregation SHALL equal per-operation results.

#### Scenario: Partial result accounting
- **WHEN** execution stops after operation N of M
- **THEN** report counts completed + failed + unattempted exactly M with no omitted or duplicated operation

#### Scenario: Continued execution mode absent
- **WHEN** one operation fails in P1
- **THEN** Chronicle stops sequential execution and marks all later operations unattempted; it does not claim failure continuation support

### Requirement: Session replayability classification
Inspect/planner SHALL classify session `fully_replayable` when every operation is executable, `partially_replayable` when at least one operation is executable and at least one is non-executable, and `not_replayable` when no operation is executable or integrity/capability invalidates session. Classification SHALL NOT authorize traffic; explicit loopback/allow-host/effect/execute gates still apply.

#### Scenario: Partially replayable session
- **WHEN** session has one complete supported operation and one degraded operation
- **THEN** inspect/planner reports partially replayable with executable/non-executable counts and per-operation blockers

#### Scenario: Not replayable session
- **WHEN** every operation is incomplete/unsupported or session integrity fails
- **THEN** inspect/planner reports not replayable and execution performs zero network

### Requirement: Replay authorization remains loopback-only
P1 replay SHALL preserve explicit `http://<loopback-ip>:<port>` target, exact repeated allow-host match, explicit read/write effect authorization, dry-run default, and explicit execute gate. Runtime Authorization replacement SHALL come only from configured environment-variable name and SHALL never expose its value. Captured Host, Authorization, Cookie, forwarding, hop-by-hop, Connection-token, and Transfer-Encoding headers SHALL NOT affect target/policy; replay SHALL rebuild target Host/Content-Length. It SHALL use one request per connection and SHALL never default to recorded destination, follow redirects, perform DNS target expansion, retry, proxy, TLS, or authorize incomplete/ambiguous operations.

#### Scenario: Explicit target override
- **WHEN** authorized replay executes
- **THEN** every connection uses supplied loopback target and original destination is never contacted

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
HTTP verification SHALL retain existing exact status, exact body size/SHA-256, and ordered header comparison excluding fixed documented ignore set. P1 SHALL not add heuristic, secret-bearing, or user-defined comparison. Missing expected response, transport failure, and incomplete expected operation SHALL map to specified result/outcome rather than silent pass.

#### Scenario: Exact verification pass
- **WHEN** observed status, selected ordered headers, and body digest/size match
- **THEN** verification is Passed

#### Scenario: Exact verification mismatch
- **WHEN** any compared status/header/body field differs
- **THEN** verification is Failed with metadata-only detail and body/header values omitted

### Requirement: Bounded replay and report
Replay SHALL retain five-second connect/operation timeout, bound observed body to 8 MiB, and accept at most canonical 10,000 operations. Canonical ETL SHALL fail before publishing operation 10,001; replay SHALL reject malformed over-limit imported session before network rather than omit/report-truncate operations. Response overflow SHALL produce explicit failed current result without unbounded allocation.

#### Scenario: Response body limit
- **WHEN** replay target response exceeds body bound
- **THEN** current operation fails with bounded category, prior results remain, and later operations are unattempted

#### Scenario: Over-limit imported session
- **WHEN** replay loads otherwise valid imported session containing more than 10,000 operations
- **THEN** replay rejects before network with typed invalid-session outcome and omits no operation silently
