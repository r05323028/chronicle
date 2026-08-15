## MODIFIED Requirements

### Requirement: Dry-run default and complete plan visibility

Replay SHALL load one verified epoch session or a verified deterministic parent aggregation, build one deterministic plan, and perform no network unless authorization preflight passes. Parent aggregation SHALL order epoch sessions by epoch ordinal/lineage, preserve each operation's epoch provenance, and never merge bytes or infer an operation spanning epochs. Every operation SHALL appear once as executable candidate or non-executable with stable reason. Any target/effect denial for executable candidates SHALL block all requests; non-executable epoch siblings SHALL not block authorized executable siblings. Executor SHALL return one ordered result per planned operation through existing shared atomic JSON.

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

## ADDED Requirements

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

Replay SHALL not synthesize a request/response, carry decoder state, or infer protocol completion across epoch sessions. A connection or operation marked incomplete at rollover remains non-executable unless its own epoch canonical evidence independently satisfies existing replayability rules. Parent aggregation may execute independent operations sequentially through existing executor semantics; it SHALL not recreate original cross-epoch timing, pipelining, or live recorder behavior.

#### Scenario: Long-lived connection at rollover

- **WHEN** predecessor epoch ends with pending HTTP bytes and successor contains continuation bytes
- **THEN** each epoch operation remains incomplete/non-executable unless independently complete, and replay sends no synthesized combined request

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
