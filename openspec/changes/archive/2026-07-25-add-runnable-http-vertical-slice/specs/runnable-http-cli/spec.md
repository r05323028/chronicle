## ADDED Requirements

### Requirement: Minimal record command
CLI SHALL extend existing `record` command as `record --source fixture --input FILE --root ROOT`. All three options SHALL be required for P0. Command SHALL orchestrate fixture validation, capture-source iteration, WAL append/flush/read, session assembly, HTTP processing, and filesystem publication through application services.

#### Scenario: Record happy path
- **WHEN** developer records valid fragmented HTTP fixture into new root
- **THEN** command exits 0 and prints session ID/root in selected format

#### Scenario: Malformed fixture
- **WHEN** fixture JSON/schema/event validation fails
- **THEN** command exits 3, prints stable safe error, and publishes no session

#### Scenario: Existing output collision
- **WHEN** generated/selected session destination already exists
- **THEN** command exits 3 without overwrite

### Requirement: Minimal inspect command
CLI SHALL preserve positional session ID and add required root: `inspect SESSION_ID --root ROOT`. It SHALL support global `--format human|json`, default human, and delegate to application/storage service.

#### Scenario: Inspect happy path
- **WHEN** known persisted session is inspected
- **THEN** command exits 0 with safe summary required by local-session-artifacts spec

#### Scenario: Unknown session
- **WHEN** session ID is valid UUID but absent
- **THEN** command exits 3 with stable not-found error

#### Scenario: Invalid session ID
- **WHEN** positional session ID is not UUID
- **THEN** command exits 3 without path access outside root

### Requirement: Minimal replay command
CLI SHALL preserve positional session ID and expose `replay SESSION_ID --root ROOT --target ORIGIN --allow-host IP [--allow-read] [--allow-write] [--timing asap] [--execute]`. Defaults SHALL be human output, immediate timing, dry-run, no effect authorization, no target default, and no network.

#### Scenario: Replay dry-run
- **WHEN** caller supplies session/root/target/allow-host but omits execute
- **THEN** command prints plan, makes no request, and exits 0

#### Scenario: Replay denied
- **WHEN** execute is requested without target allow or operation effect policy
- **THEN** command exits 4 and sends zero requests

#### Scenario: Replay local success
- **WHEN** execute, matching loopback allow-host, effect authorization, and deterministic server all pass
- **THEN** command exits 0 after verification Passed/Skipped summary

#### Scenario: Verification failure
- **WHEN** execution succeeds but deterministic verification has Failed, Inconclusive, or Unsupported result
- **THEN** command exits 6 with clear human/JSON summary

#### Scenario: Network failure
- **WHEN** authorized target refuses, disconnects, has I/O failure, or times out before complete supported response
- **THEN** command exits 5 with typed stable category and no verification result

### Requirement: Stable CLI exit and output contract
CLI SHALL use exit 0 for success, dry-run, and only Passed/Skipped results; 2 for Clap usage; 3 for fixture/session/storage/WAL data errors; 4 for safety/unsupported plan denial; 5 for refusal/timeout/disconnect/I/O or protocol execution errors before complete observed response; and 6 for completed verification Failed/Inconclusive/Unsupported. Human success goes stdout, errors stderr. JSON errors SHALL be valid JSON with stable code/message and no secrets.

#### Scenario: Clap usage error
- **WHEN** required argument is absent or invalid enum supplied
- **THEN** Clap exits 2 and no application side effect occurs

#### Scenario: JSON error
- **WHEN** application error occurs under JSON format
- **THEN** stderr contains one valid JSON error object and stdout has no partial result

### Requirement: CLI remains outer adapter
HTTP parsing, canonicalization, storage, safety decisions, network execution, and verification SHALL live outside CLI crate. CLI SHALL parse arguments, map commands, render application result, and map typed errors to exit codes only. Existing `doctor` remains configuration-only and `etl` remains explicit scaffold/non-runnable command.

#### Scenario: Protocol logic audit
- **WHEN** CLI source is reviewed
- **THEN** no HTTP message parser, header sanitizer, filesystem serializer, or socket client exists there

#### Scenario: Doctor behavior
- **WHEN** doctor runs
- **THEN** it reports only local configuration checks and does not claim external probes

### Requirement: Deterministic local HTTP demonstration server
Test support SHALL provide in-process Tokio loopback server on ephemeral port with GET no body, POST body handling, deterministic response headers, non-2xx, binary response, pass mode, and mismatch mode. Core tests SHALL require no Docker, external service, fixed port, root, or Internet.

#### Scenario: GET endpoint
- **WHEN** authorized replay sends fixture GET
- **THEN** server returns deterministic fixed-length response suitable for Passed verification

#### Scenario: POST endpoint
- **WHEN** authorized replay sends binary/fixed body POST
- **THEN** server observes exact method/path/body and returns deterministic response

#### Scenario: Verification mismatch mode
- **WHEN** server is configured to change status/header/body
- **THEN** replay completes and verifier reports Failed

### Requirement: Vertical integration proof
One integration test SHALL exercise fixture → CaptureEvent source → WAL append → WAL read → bounded assembly → HTTP detect/decode/canonicalize → filesystem persistence → inspect → replay plan → local HTTP execution → verification. Test SHALL prove WAL and canonical persistence are not bypassed.

#### Scenario: Full end-to-end pass
- **WHEN** basic fragmented fixture is recorded then replayed against deterministic server
- **THEN** test observes persisted session, outbound expected request, and Passed verification

#### Scenario: WAL boundary proof
- **WHEN** test compares fixture event count with decoded WAL records and corrupts/omits required WAL record in negative case
- **THEN** processing count matches valid WAL and corruption prevents canonical publication

#### Scenario: Canonical persistence boundary proof
- **WHEN** successful record completes and test removes/renames fixture and WAL before inspect/replay
- **THEN** inspect/replay still succeed from filesystem session and payloads

### Requirement: Required test coverage
Implementation SHALL include unit tests for detection; fragmented request/response; Content-Length; multiple exchanges; malformed/truncated data; duplicate headers; binary bodies; capability integrity; header sanitization; replay policy; and verifier pass/fail. CLI tests SHALL cover record, inspect, dry-run, denied execution, allowed loopback execution, verification failure, malformed fixture, and unknown session.

#### Scenario: Unit and CLI suites
- **WHEN** workspace tests run
- **THEN** each listed behavior has deterministic test without external dependency

### Requirement: Documentation and validation gates
README, architecture, canonical, replay safety, WAL limitation, and capability matrix SHALL be updated after implementation without overstating eBPF, unsupported HTTP, recovery, TLS, persistence, or replay support. OpenSpec and Rust gates SHALL pass.

#### Scenario: Capability documentation
- **WHEN** implementation is complete
- **THEN** docs mark only actual HTTP subset capabilities Available, retain fake, and leave every other protocol non-Available

#### Scenario: Repository gates
- **WHEN** change is ready for review
- **THEN** `cargo fmt --all --check`, `cargo check --workspace --all-targets --locked`, `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings`, and `cargo test --workspace --all-features --locked` all pass

#### Scenario: OpenSpec validation
- **WHEN** artifacts and implementation are complete
- **THEN** strict OpenSpec validation for change passes with all task checkboxes reflecting reality
