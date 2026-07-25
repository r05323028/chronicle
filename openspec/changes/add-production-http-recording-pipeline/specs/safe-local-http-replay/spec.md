## MODIFIED Requirements

### Requirement: Dry-run default and complete plan visibility
Replay SHALL load canonical filesystem session, build deterministic plan, and perform no network I/O unless explicit execution passes target/effect/session/protocol-capability preflight. Planner SHALL represent every operation as allowed, denied, or unsupported with stable reasons rather than failing at first denial. Any preflight denial SHALL block all requests. Executor SHALL return one result per planned operation plus explicit overall outcome, retaining already completed, failed, and unattempted operations when execution stops. Human mode SHALL show plan before execution; JSON mode SHALL construct/validate plan before network and include it in one atomic final report.

#### Scenario: Default dry-run
- **WHEN** replay runs without `--execute`
- **THEN** zero network I/O occurs and execution outcome/result list represents every operation as not attempted

#### Scenario: Execute with denied operation
- **WHEN** any operation is denied or unsupported during preflight
- **THEN** no operation is sent, overall outcome is stopped-by-policy/invalid-session, and every operation has stable not-attempted reason

#### Scenario: Deterministic operation order
- **WHEN** plan contains operations across connections
- **THEN** order is deterministic by scheduled offset and operation sequence and result order matches plan

#### Scenario: Runtime failure after earlier success
- **WHEN** transport fails on operation after one or more completed operations
- **THEN** outcome is stopped-by-transport and report retains prior completed, failed current, and later unattempted results without rollback claim

### Requirement: Complete verification status model and output
Verification SHALL use Passed, Failed, Skipped, Inconclusive, and Unsupported. Missing recorded response or incomplete/truncated/unmatched expected operation SHALL be Inconclusive/Unsupported and non-executable; Skipped SHALL mean no verification ran because dry-run, policy denial, or execution stopped earlier. Refusal, timeout, disconnect, I/O, incomplete observed response, and bounded-response overflow SHALL be typed failed operation with overall transport outcome rather than discarding earlier results. Human and JSON report SHALL include per-operation and aggregate statuses/counts/outcome with stable categories and no body/header values.

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
Existing `ReplayExecution` SHALL include `ReplayOutcome`; implementation SHALL not introduce a duplicate parallel execution model. Outcomes SHALL represent completed, dry-run, stopped by policy rejection, stopped by invalid session/protocol capability, stopped by transport failure, and stopped by non-passing verification. Precedence SHALL be dry-run, invalid-session/capability, policy, runtime transport/verification, completed. Protocol registry/capability SHALL be checked before network; missing persisted-session capability is invalid-session outcome while registry construction/invariant failure remains typed error. Expected operational stops SHALL return execution/report rather than discard results.

#### Scenario: All operations completed
- **WHEN** all authorized operations execute and verify Passed
- **THEN** outcome is completed and all results are completed

#### Scenario: First operation transport failure
- **WHEN** first authorized operation fails transport
- **THEN** outcome is stopped-by-transport, first result is failed, and all remaining results are explicitly unattempted

#### Scenario: Policy rejection
- **WHEN** session contains incomplete or unauthorized operation
- **THEN** outcome represents policy/invalid-session stop, every result is unattempted, and no adapter connects

#### Scenario: Verification mismatch
- **WHEN** completed response mismatches status/header/body expectation
- **THEN** transport state is completed, verification is Failed, outcome is stopped-by-verification, later results are unattempted, and exit is 6

#### Scenario: Missing protocol capability
- **WHEN** persisted operation protocol lacks required replay/verifier capability
- **THEN** invalid-session outcome represents every operation and no adapter connects

### Requirement: Operation replay result completeness
Every planned operation SHALL have exactly one result with operation ID, decision, explicit execution state (`completed`, `failed`, `not_attempted`), transport category when failed, verification status/details when applicable, and stable not-attempted reason. Report aggregation SHALL equal per-operation results.

#### Scenario: Partial result accounting
- **WHEN** execution stops after operation N of M
- **THEN** report counts completed + failed + unattempted exactly M with no omitted or duplicated operation

#### Scenario: Continued execution mode absent
- **WHEN** one operation fails in P1
- **THEN** Chronicle stops sequential execution and marks all later operations unattempted; it does not claim failure continuation support

### Requirement: Replay authorization remains loopback-only
P1 replay SHALL preserve explicit `http://<loopback-ip>:<port>` target, exact repeated allow-host match, explicit read/write effect authorization, dry-run default, and explicit execute gate. It SHALL never default to recorded destination, use recorded/header values as policy, follow redirects, perform DNS target expansion, retry, proxy, TLS, or authorize incomplete/ambiguous operations.

#### Scenario: Explicit target override
- **WHEN** authorized replay executes
- **THEN** every connection uses supplied loopback target and original destination is never contacted

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
Replay SHALL retain five-second connect/operation timeout, bound observed body to 8 MiB, and bound reports to canonical session maximum 10,000 operations. Exceeded response/report limits SHALL produce explicit failure/unsupported result without unbounded allocation.

#### Scenario: Response body limit
- **WHEN** replay target response exceeds body bound
- **THEN** current operation fails with bounded category, prior results remain, and later operations are unattempted

#### Scenario: JSON serialization failure
- **WHEN** completed in-memory replay report cannot be serialized
- **THEN** CLI emits safe typed data error with no partial invalid JSON and does not repeat network execution
