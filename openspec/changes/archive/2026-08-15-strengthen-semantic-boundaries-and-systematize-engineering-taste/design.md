# Design: strengthen semantic boundaries and systematize engineering taste

## Context

Chronicle is a 13-crate Rust workspace whose dependency direction is mechanically enforced by `validation/architecture.toml` + `scripts/validation.py architecture` (wired into `validate.sh fast` and release). The Cargo graph is clean: `chronicle-cli` depends only on `chronicle-application` in every dependency kind. However, `chronicle-application` publicly re-exports lower-layer vocabulary, and `chronicle-cli` consumes it. The semantic boundary exists only on paper.

This design (1) defines the dependency-vs-semantic boundary distinction and the invariants to enforce, (2) identifies the current leaks with evidence, (3) specifies the minimal application-facing corrections, (4) defines engineering taste principles with enforcement classification, (5) defines the review-finding-to-invariant feedback loop, (6) specifies the mechanical validator, (7) restructures `AGENTS.md` as a navigation map, and (8) answers the ten required analysis questions explicitly.

Design inspiration comes from OpenAI's "Harness engineering: leveraging Codex in an agent-first world": enforce invariants, not implementations; strict boundaries with a limited set of permissible edges; parse data shapes at the boundary without prescribing the library; progressive disclosure with mechanically validated knowledge artifacts; and turning recurring human judgment into mechanically enforced rules. The invariants below are Chronicle-specific translations, not a copy of that architecture.

## Existing behavior (verified on main @ cf2f948)

### Strong: Cargo dependency boundary

- `validation/architecture.toml` (v1) allowlists every normal/dev/build workspace edge for all 13 crates; unlisted edges are forbidden; optional/target-specific declarations are checked host-independently; package renames by identity; cycles rejected; five named critical forbids (`dependency_on_cli`, `session_to_wal`, `protocol_to_builtins`, `common_upward`, `cli_non_application`).
- `crates/chronicle-cli/Cargo.toml`: sole Chronicle edge is `chronicle-application` (normal + no dev/build Chronicle edges).
- `scripts/validate.sh fast` and release run the `architecture` check; `scripts/tests/validation/test_architecture_boundaries.py` covers acceptance and rejection cases in temp workspaces.

### Strong: other existing guarantees

- eBPF raw ABI privacy (Aya confined to `chronicle-capture-ebpf`, target-gated, only normalized capture events cross out).
- WAL durability/recovery authority, canonical schema v1, ETL as complete Extract-Transform-Load, replay default-deny — documented in `docs/architecture/crate-boundaries.md` "Reliability boundaries that must not change".
- Timing correctness: `scripts/tests/validation/test_sleep_policy.py` rejects undocumented correctness sleeps (>= 500 ms Python, >= 500 ms Rust integration) and unbounded polling; wired into `validate.sh fast` via the tooling-tests step. This rule is enforced but documented nowhere outside the test file itself.
- Test architecture (functional layers, preflight outcomes, gate semantics) enforced by `validation/test-architecture/test-catalog.toml` + `validation.py catalog`.
- Bounded command execution via `scripts/run-with-timeout.sh` wrapping every gate step.

### Leak: semantic boundary at the application/CLI seam (evidence)

`crates/chronicle-application/src/lib.rs`:

- Line 72: `pub use chronicle_common::{RecordingId, SessionId, Timestamp, escape_control};` — neutral primitives; this re-export is intentional and stays.
- Line 73: `pub use chronicle_protocol::{ProtocolError, TransportErrorCategory};`
- Lines 74-77: `pub use chronicle_replay::{LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayOutcome, Replayability, TimingMode};` (multiline block: opens line 74, symbols 75-76, closes 77)

`crates/chronicle-cli/src/main.rs` (production):

| CLI usage | Line | What leaks |
| --- | --- | --- |
| `use chronicle_application::{LoopbackReplayOptions, ReplayOutcome, TimingMode}` | 22 | names three replay-owned types |
| `ReplayOutcome::Completed | CompletedWithSkips | DryRun` pattern match | 376 | interprets replay outcome taxonomy to decide success |
| `impl From<Timing> for TimingMode` | 418-424 | CLI's clap enum converted into replay's timing policy type |
| `LoopbackReplayOptions { ... }` construction | 1498, 1541 | CLI constructs replay policy options (target, allow_hosts, execute, allow_reads, allow_writes, timing) |
| `replay_exit_code(&result)` -> `replay_outcome_exit_code(&result.outcome)` | 1845-1847 | CLI extracts replay-owned `ReplayOutcome` from the view model to map exit codes |
| `chronicle_application::protocol_registry()` pass-through | 906 | CLI briefly holds a protocol-owned `ProtocolRegistry` only to pass it into an application function (intentional access seam; no registry methods called) |

`crates/chronicle-cli/src/main.rs` (tests, `#[cfg(test)]` module):

- Line 1856: `use chronicle_application::{OperationExecutionState, ProtocolError, TransportErrorCategory};`
- Lines 2178-2274: tests construct `ReplaySessionResult` fixtures naming `Replayability::FullyReplayable`, `ReplayOutcome::*`, `OperationExecutionState::NotAttempted`, and construct `ApplicationError::Protocol(ProtocolError::Transport { category: TransportErrorCategory::Timeout, .. })` (2217-2218). These tests exercise application-owned exit-code mapping by building lower-layer taxonomy.

Application-owned view model embedding replay-owned types (`crates/chronicle-application/src/replay_inspect.rs`):

- `ReplaySessionResult { session_id, replayability: Replayability, outcome: ReplayOutcome, dry_run, preflight_denied, transport_failed, counts: ReplayCounts, operations: Vec<ReplayOperationSummary> }` (lines 484-494).
- `ReplayOperationSummary { ..., state: OperationExecutionState, ... }` (lines 428-436).
- `InspectSessionResult { ..., replayable: bool, replayability: Replayability, ... }` (line 78; compatibility bool alongside) — serialized-only view-model leak: CLI consumes the view model but never names the type, and application render functions own presentation. Classified as an intentional serialized-only contract by explicit decision (see corrections), not a correction target.
- These fields make the application-owned result contract carry replay-owned taxonomy; CLI production code pattern-matches `ReplaySessionResult.outcome` specifically.

### Intentional re-exports vs leakage

Intentional public adapter contracts (defined in application modules, consumed by CLI, must stay): `ReplaySessionResult`, `ReplayCounts`, `ReplayOperationSummary`, `InspectSessionResult`, `CommandReplayResult`, `CleanupOutcome`, `RecorderConfigV1`, `RecorderLease`, `RecorderStatusV1`, `RecordingStatus`, `RecordingEtlCheckpoint`, `render_*` functions, `doctor_*` functions, `application_error_exit_code`, `protocol_registry` (access seam).

Leakage (verbatim re-exports of replay/protocol-defined types): the eight symbols on lib.rs lines 73-76. The `protocol_registry()` seam returns a protocol-owned type but is a deliberate, reviewed translation point (CLI never invokes registry methods); it is acceptable today and an optional cleanup target (application could build the registry internally for `process_and_publish_recording_wal`).

## Boundary model

**Dependency boundary** — which crate may depend on which crate. Enforced by `validation/architecture.toml` + Cargo metadata. Strong today.

**Semantic/API boundary** — which concepts and decisions may cross those dependencies. Newly specified here:

1. Outer adapters (CLI; future external adapters) operate only on application-owned request/result/error/rendering contracts. They do not name, construct, or pattern-match lower-layer vocabulary, even when that vocabulary is reachable through application re-exports.
2. Application must not re-export lower-layer implementation vocabulary as an escape hatch around the dependency policy. Lower-layer re-exports are forbidden except (a) neutral primitives from `chronicle-common` and (b) explicitly allowlisted, reviewed per-symbol contracts with documented rationale.
3. Replay policy (options, timing, target mapping, execution authorization) remains owned by replay/application composition; the CLI passes plain request data and receives application-owned results.
4. Protocol error taxonomy (`ProtocolError`, `TransportErrorCategory`) and replay error taxonomy (`ReplayError`, `ReplayOutcome`, `Replayability`, `OperationExecutionState`) do not leak into outer adapters; application translates them into an application-facing classification where CLI needs one (the existing `application_error_exit_code` pattern is the model).
5. WAL wire types, ETL checkpoint formats, persistence/reconstruction concepts remain owned by their crates; the same rule applies if application ever re-exports them.

**Enforce invariants, not implementations.** The policy forbids vocabulary crossing and wholesale re-exports; it does not prescribe struct hierarchies, request shapes, module layout, or file naming. Multiple correct designs for the application-owned request/classification types remain open to the implementer.

## Required analysis answers

1. **Already-strong boundaries:** Cargo dependency direction (all kinds, optional/target/rename aware, cycles, critical forbids); eBPF raw ABI privacy; WAL durability/recovery authority; canonical schema v1; ETL complete-pipeline; replay default-deny and replay independence from capture/WAL/ETL; CLI Cargo dependency; sleep/unbounded-polling policy; test-architecture catalog/preflight/gates; bounded command execution wrapper.
2. **Boundaries that exist only at Cargo dependency level:** the application/CLI semantic boundary (the leak list above). The session->WAL neutralization edge was already corrected at dependency level previously; no residual leak found there.
3. **Lower-layer types crossing the application/CLI boundary:** `LoopbackReplayOptions`, `ReplayOutcome`, `Replayability`, `TimingMode`, `OperationExecutionState` (replay-owned); `ProtocolError`, `TransportErrorCategory` (protocol-owned); `ReplayError` (replay-owned, embedded in `ApplicationError::Replay` and matched by application's own exit-code mapping); plus the view-model fields `ReplaySessionResult.outcome`/`replayability` and `ReplayOperationSummary.state`, and a `ProtocolRegistry` pass-through.
4. **Intentional re-exports vs leakage:** intentional = application-defined types and functions listed above; leakage = the eight verbatim replay/protocol re-exports; acceptable seam = `protocol_registry()`.
5. **Taste principles already mechanically enforced:** timing correctness (sleep policy); dependency direction (architecture.toml); test classification/gates (catalog); bounded execution (timeout wrapper); deterministic-core (by design: preflight deny-by-default, `--ignored` privileged suites, decision docs — not a lint).
6. **Rules existing only in AGENTS.md or docs:** bounded-command-execution parameter table (AGENTS.md only); macOS/Multipass fallback (AGENTS.md); task-completion rules (AGENTS.md prose); crate ownership detail (docs + AGENTS.md duplication); test architecture (AGENTS.md duplicates the test-architecture README nearly verbatim); "CLI communicates only through application-owned APIs" (AGENTS.md/doc/spec prose, not mechanically enforced — the gap this change closes); sleep policy (code-only; documented nowhere).
7. **Rules worth mechanical enforcement now:** outer-adapter vocabulary ban; application re-export allowlist; validation-error message format. High signal, deterministic, low false positives.
8. **Rules that stay human judgment:** file-size/agent-legibility thresholds (evidence-based review signal only); observability structure (no demonstrated recurring problem); parse-don't-validate at trust boundaries (design guidance); one-owner invariant (duplication detection is fuzzy); whether a future re-export is an intentional contract vs leakage (the allowlist review decision, reviewed once, then machine-enforced).
9. **Is AGENTS.md too large/repetitive?** Yes — ~17 KB, duplicated test-architecture and crate-ownership prose, encyclopedia env tables; target ~5-6 KB navigation map.
10. **How does this improve agent autonomy without over-constraining?** Mechanical checks give immediate, localizable, actionable feedback (invariant/where/why/remediation) so agents self-correct without human review; concise AGENTS.md lowers per-task context overhead; canonical docs provide depth on demand; taste principles encode decision guidance without style rules; invariants constrain vocabulary and ownership, never implementation structure.

## New invariants (with mechanical enforcement)

| # | Invariant | Enforcement classification |
| --- | --- | --- |
| S1 | CLI (and future outer adapters) never names lower-layer vocabulary via application re-exports | mechanically enforced now (source scan) |
| S2 | Application never re-exports lower-layer vocabulary except neutral primitives or reviewed allowlist entries | mechanically enforced now (re-export scan) |
| S3 | Application-owned view models/errors expose application-owned classification for CLI-visible decisions | enforced via S1+S2 plus the required owned-classification correction (name scans cannot see opaque field access); further taxonomy checks are potential future work |
| S4 | Replay policy composition stays in replay/application | enforced via S1 (CLI cannot construct `LoopbackReplayOptions`) |
| T2 | Correctness must not depend on timing guesses | mechanically enforced now (existing sleep policy; now documented in `docs/engineering-taste.md`) |
| V1 | Validation failures state the invariant, location, rationale, and remediation | mechanically enforced now for checks added under this change and future promotions (tooling test on checker output); legacy checks (sleep policy, catalog) name the violation and location and may be upgraded opportunistically |

## Engineering taste principles (7)

Preserving the existing philosophy (low overhead, stable tests, simple replay, behavior over volume, reliability first, portable artifacts, deterministic core), the change adds these golden principles for an agent-maintained codebase:

1. **Outer adapters do not consume lower-layer vocabulary.** CLI and future external adapters operate on application-owned contracts. — mechanically enforceable now (S1/S2).
2. **Correctness must not depend on timing guesses.** No undocumented sleeps or unbounded polling in tests; readiness uses bounded deadlines. — mechanically enforced now (existing sleep policy; newly documented).
3. **Boundaries validate external or durable data.** Network, persisted, configuration, and kernel-derived inputs are validated when crossing ownership boundaries (parse, don't validate); no redundant defensive validation inside trusted internal paths. — document-only now; potential future enforcement via boundary-validation checklists.
4. **Shared invariants have one owner.** Replay/WAL/ETL correctness rules are not duplicated across application, CLI, tests, and scripts. Concrete case: exit-code mapping is application logic, so its unit tests live in application; CLI tests only verify wiring. — document-only now; duplication detection is fuzzy.
5. **Code remains agent-legible.** Evidence from current distribution: `chronicle-wal/src/lib.rs` 4668 lines, `chronicle-application/src/lib.rs` 2994, `chronicle-cli/src/main.rs` 2382, `chronicle-application/src/record.rs` 2227. No hard size limit is introduced: a threshold would have to exceed the current largest file to pass today, so it would not catch anything. Keep as a human review signal; revisit if a file grows far beyond the current maximum. — document-only now; potential future enforcement only with justified evidence-based threshold.
6. **Observability is structured on production paths.** Current state: no `tracing` in library crates; `chronicle-cli` (outer adapter) initializes `tracing-subscriber` once (main.rs 29, 481) — acceptable; sparse `eprintln!` (continuous_recorder 10, bootstrap 2, etl 1, cli main.rs 1); recorder status flows through owned status/checkpoint artifacts. No recurring problem demonstrated, so no new logging abstraction and no enforcement. Guidance: prefer owned status artifacts and structured output; keep payloads redacted. — document-only now.
7. **Prefer deterministic mechanisms over heuristics.** Heuristics/LLM assistance never become correctness authority; already expressed as philosophy and enforced by design (preflight deny-by-default, honest planned registrations, `--ignored` privileged suites). No additional mechanism needed. — already enforced by design; document-only.

## Feedback loop: from recurring judgment to invariant

Documented in `docs/engineering-taste.md`:

1. **Recurring review finding** (a human or agent reviewer flags a pattern more than once, or it touches a critical invariant).
2. **Document the principle** in `docs/engineering-taste.md` with an enforcement classification (document-only / mechanically enforced now / potential future enforcement).
3. **Observe repetition** — count occurrences or criticality before investing in automation.
4. **Promote to structural/lint/test invariant** only when all four promotion criteria hold: the pattern has occurred repeatedly or is a critical invariant; violation is objectively detectable; false positives are acceptably low; remediation is clear.
5. **Give actionable remediation** — every mechanical failure message states: what invariant was violated, where (file:line), why the invariant exists (rationale), and the preferred remediation with a doc pointer.

The repository does not immediately encode every reviewer opinion into lint. Rules without objective detection, low false-positive rates, or clear remediation stay documented guidance.

## Mechanical enforcement design

Keep the existing mechanisms; no new framework, crate, or third-party tool.

**Policy (`validation/architecture.toml` extension).** New `[semantic]` table (additive; dependency-policy schema untouched):

```toml
[semantic]
# Symbols outer adapters MUST NOT name, even when reachable via application
# re-exports. Word-boundary exact matches; entries are reviewed.
forbidden_outer_vocabulary = [
  "LoopbackReplayOptions", "ReplayOutcome", "Replayability", "TimingMode",
  "OperationExecutionState", "ReplayError", "ProtocolError", "TransportErrorCategory",
]
# Lower-layer crates whose public items application MUST NOT re-export
# wholesale. chronicle-common neutral primitives remain allowed.
forbidden_re_export_sources = [
  "chronicle-replay", "chronicle-protocol", "chronicle-wal", "chronicle-etl",
  "chronicle-capture", "chronicle-session", "chronicle-storage",
  "chronicle-canonical", "chronicle-protocol-builtins", "chronicle-capture-ebpf",
]
# Reviewed per-symbol re-export allowlist; each entry requires a rationale and
# documentation update in the same change. Empty today.
allowed_re_exports = []
```

The checker also requires that every `forbidden_outer_vocabulary` entry appears in at least one `forbidden_re_export_sources` crate's source (guards against stale/typo'd policy), and that allowlist entries carry a rationale.

**Checker (`scripts/validation.py`).** Extend the existing `architecture` subcommand (already wired into `validate.sh fast` and release) with semantic checks when the `[semantic]` table is present:

1. Re-export scan: for each forbidden source crate, scan `crates/chronicle-application/src/**` for `pub use <crate>::{...}` blocks (continuation-join multiline blocks), bare `pub use <crate>;`, and glob `pub use <crate>::*;` forms (prefix-match `pub use <forbidden_crate>`); flag symbols not in `allowed_re_exports`, keyed by source symbol. Regex-based line scanning with continuation-join (consistent with the sleep-policy precedent; std-lib only).
2. Vocabulary scan: for each forbidden symbol, scan `crates/chronicle-cli/src/**` and `crates/chronicle-cli/tests/**` with word-boundary exact matching after stripping `//` and `///` comment lines; flag file:line.
3. Policy self-consistency: forbidden symbols resolve to a real re-export source; allowlist entries are well-formed.

Violation messages follow V1 (invariant / location / rationale / remediation), e.g.:

```text
semantic-boundary violation: outer adapter must not name lower-layer vocabulary
location: crates/chronicle-cli/src/main.rs:22
rationale: CLI operates only on application-owned contracts; replay policy and
  outcome taxonomy are owned by replay+application composition
remediation: use the application-owned request/result/exit-code API; see
  docs/architecture/crate-boundaries.md#semantic-boundaries
```

**Tests.** Extend `scripts/tests/validation/test_architecture_boundaries.py` (or add `test_semantic_boundaries.py` alongside it) with temp-workspace fixtures:

- Acceptance: a compliant application-owned result type consumed by CLI passes.
- Rejection: `pub use chronicle_replay::ReplayOutcome;` in application fails; `use chronicle_application::{ReplayOutcome, ...}` in a CLI file fails; construction of `LoopbackReplayOptions` in CLI fails.
- False-positive control: internal application modules using lower-layer types for orchestration (not re-exported) pass; `chronicle_common` primitives re-export passes; the word `ReplayOutcome` in a doc comment or a different identifier (e.g. `ReplayOutcomeMapper`) passes.
- Message format: violations contain invariant, location, rationale, remediation.
- Regression: existing valid Cargo edges continue passing (existing suite).

**Wiring.** The existing `architecture` step in `validate.sh fast` and the release path already executes the extended check; no new step. Tooling self-tests run in the existing tooling-tests step. Portable only — no privileged execution required.

## Application-facing contract corrections (minimal; implementation phase)

Only what the semantic boundary requires. No wholesale wrapper types.

1. **Application re-exports (lib.rs:73-76).** Remove the verbatim `pub use chronicle_replay::{...}` and `pub use chronicle_protocol::{...}` lines. Keep `pub use chronicle_common::{...}` neutral primitives. Any future lower-layer re-export requires an `allowed_re_exports` entry with rationale.
2. **View-model classification (`ReplaySessionResult`, `ReplayOperationSummary`).** The application-owned result contract keeps its shape and serialized JSON (REPLAY_REPORT_VERSION, outcome strings, `dry_run`/`preflight_denied`/`transport_failed` booleans, counts) but exposes an application-owned outcome classification type serializing identically (e.g. an application-owned status enum with serde representation identical to today, plus convenience accessors such as `succeeded()` and `replay_result_exit_code(&ReplaySessionResult)`). The owned classification is REQUIRED, not optional: a name scan cannot catch opaque field access such as `result.outcome`, so an accessor-only variant would leave replay taxonomy in the CLI-visible contract while satisfying the scan. Implementation shape is free; the invariants are: CLI never names or pattern-matches replay taxonomy, and CLI-visible JSON/exit codes are byte-identical. `InspectSessionResult.replayability` remains a documented intentional serialized-only contract (CLI never names `Replayability`; application render functions own presentation; if a future adapter must interpret it, application translates).
3. **CLI request path (`run_replay`, `run_replay_legacy`, `replay_exit_code`, `render_public_replay_human`).** CLI passes plain request data (target, allow_hosts, execute, allow_read/write, timing choice from its own clap enum) to an application-owned request API; application constructs `LoopbackReplayOptions` internally. CLI success determination and exit-code mapping use application-owned classification (`replay_result_exit_code`-style) instead of matching `ReplayOutcome` variants. The `impl From<Timing> for TimingMode` conversion moves behind the application request API.
4. **Test ownership.** Exit-code mapping tests (`replay_outcomes_map_to_stable_exit_codes`, `application_errors_map_to_stable_exit_family`) move into chronicle-application's own tests, where the mapping is owned (taste principle 4). CLI tests keep rendering/wiring checks and build fixtures through application-owned constructors/fixture helpers — no CLI construction of `ReplaySessionResult` with replay-owned enum fields.
5. **ProtocolRegistry pass-through (main.rs:906).** Acceptable today; optional cleanup: application-owned wrapper that builds the registry internally. Not required by the invariants.

## AGENTS.md restructuring (progressive disclosure)

Current: ~17 KB. Target: ~5-6 KB navigation map. All removed detail moves to existing or one new canonical doc; nothing is deleted silently.

| Current AGENTS.md content | Decision |
| --- | --- |
| Chronicle philosophy (7 principles) | keep, lightly compressed |
| CodeGraph / graphify sections | keep (short, operational) |
| Bounded Command Execution (full env-var table) | keep a condensed safety rule (every long-running command needs a bounded timeout; defaults 900s command / 3600s gate; knobs exist); move the full parameter table to CONTRIBUTING.md |
| Reliability boundaries that must not change (now only in crate-boundaries.md) | keep a condensed bullet list directly in AGENTS.md (the developer-onboarding spec delta requires it); detail stays in crate-boundaries.md |
| Canonical validation and acceptance entrypoints | condense to the entrypoint list (`validate.sh` modes, `openspec validate`, `acceptance.sh` profiles) with pointer to CONTRIBUTING.md/validate.sh usage |
| Linux-only validation on macOS | keep (short, safety-relevant) |
| OpenSpec / SDD workflow | keep short workflow pointer; no detail duplication |
| Automated/privileged test responsibilities + task-completion rules | condense to the three task-completion rules and a pointer to validation/test-architecture/README.md |
| Crate Architecture (mandatory) | keep the mandatory summary (ownership one-liners, dependency direction, forbidden patterns, update-in-same-change rule) per the existing spec; drop duplicated prose; pointer to docs/architecture/crate-boundaries.md |
| Test Architecture | replace duplicated prose with a pointer to validation/test-architecture/README.md |
| New | navigation map table (task -> doc/policy), pointer to docs/engineering-taste.md |

Doc moves: bounded-execution parameter table -> CONTRIBUTING.md (new "Validation timeouts" section; CONTRIBUTING.md already lists validation modes). New `docs/engineering-taste.md` (taste principles + classification + feedback loop + current mechanical-invariant inventory). Semantic boundary model section added to `docs/architecture/crate-boundaries.md` (natural owner; mirrors `[semantic]` policy).

## Non-goals

- No new dependency-analysis framework, Rust crate, third-party lint platform, large AST framework, or generated policy system.
- No new logging abstraction; no file-size lint; no generic architecture framework.
- No redesign of WAL, ETL, canonical, storage, replay, capture, or eBPF behavior; no CLI behavior/output/exit-code changes (compatibility-preserving adapter change only).
- No new crates; no directory-layout prescriptions; no prescriptive struct hierarchies.
- The change does not re-litigate already-decided boundaries (e.g. session->WAL neutralization).
- Privileged acceptance semantics and release qualification guarantees are untouched; the new checks are portable.

## Migration

1. Current state (leak inventory, doc duplication, file sizes, sleep-policy documentation gap) is recorded as evidence in this change (tasks 1.x) before edits.
2. Contract corrections (application re-exports, view-model classification, CLI request path, test ownership) are implemented behind the existing behavior contract: JSON, human output, and exit codes unchanged; existing workspace tests keep passing throughout.
3. The `[semantic]` policy table may land with the docs (task 3.1) but remains inert until the checker lands (task 5.1); `validate.sh fast` goes green in one step at 5.x. Contract-correction tasks 4.1/4.4/4.5 land in one commit sequence: removing the re-exports breaks CLI test imports until the relocated mapping tests and fixture helpers land.
4. This change edits acceptance-fingerprint inputs (`crates/**`, `scripts/validation.py`, `scripts/validate.sh`, `validation/**`), invalidating retained live-capture/recorder evidence; evidence must be regenerated before any release gate (task 8.5).
5. `AGENTS.md` is condensed only after the canonical docs (crate-boundaries semantic section, engineering-taste.md, CONTRIBUTING.md timeouts) exist and are linked.
6. Spec deltas are synced to canonical specs and the change is archived per OpenSpec workflow.

## Risks and false positives

- **Regex false positives:** word-boundary matching on distinctive CamelCase names keeps false positives low; identifiers that merely contain a forbidden name (e.g. `ReplayOutcomeMapper`) must not match; the scanner strips `//`/`///` comment lines so doc comments are not flagged; tests pin all three cases.
- **Opaque field-access residual:** a name scan cannot detect reads of a replay-typed field (e.g. `result.outcome`) when the type name never appears; this is why the application-owned classification is required rather than optional for CLI-visible decisions (correction 2).
- **Evidence fingerprint invalidation:** this change edits fingerprint inputs (`crates/**`, `scripts/validation.py`, `scripts/validate.sh`, `validation/**`), which changes the acceptance fingerprint and invalidates retained live-capture/recorder evidence under the content-addressed evidence policy; release planning must regenerate evidence before any release gate (task 8.5).
- **Stale vocabulary list:** if the policy lists a symbol that no longer exists, the self-consistency check (symbol resolves to a re-export source) fails loudly instead of silently passing.
- **Allowlist erosion:** the reviewed allowlist is the escape hatch; adding an entry requires rationale + doc update in the same change (mirrors the dependency-edge convention). Empty today; reviewers control growth.

## Implementation evidence (tasks 1.x, 2.x)

Re-verified on main HEAD `ba3f9c1` (2026-08-15), unchanged since the change was authored.

- **1.1 Leak inventory re-verified**: re-export lines `crates/chronicle-application/src/lib.rs` 72 (common), 73 (protocol), 74-77 (replay block, symbols on 75-76); CLI production usage `crates/chronicle-cli/src/main.rs` 22 (import), 376 (ReplayOutcome match), 418-424 (From<Timing> for TimingMode), 906 (protocol_registry pass-through), 1498 + 1541 (LoopbackReplayOptions construction), 1845-1847 (replay_exit_code via result.outcome); CLI test usage 1856 (import), 2180-2270 (Replayability/ReplayOutcome/OperationExecutionState/ProtocolError/TransportErrorCategory construction); view-model fields `replay_inspect.rs` 78 (InspectSessionResult.replayability), 428-436 (ReplayOperationSummary.state), 484-494 (ReplaySessionResult). Full CLI crate grep (src + tests): only main.rs contains leaked vocabulary; `cli/tests/cli_contract.rs` is clean.
- **1.2 Cargo boundary**: `crates/chronicle-cli/Cargo.toml` sole Chronicle dependency is `chronicle-application`; `python3 scripts/validation.py architecture --root .` → 13 members, cyclic=false, issues=[].
- **1.3 AGENTS.md**: 17025 bytes (~17 KB). Duplication: "functional layer" taxonomy appears in AGENTS.md and 3x in `validation/test-architecture/README.md`; crate-ownership prose duplicated between AGENTS.md and `docs/architecture/crate-boundaries.md`.
- **1.4 Sleep policy**: enforced by `scripts/tests/validation/test_sleep_policy.py`, wired into `validate.sh fast` via the tooling-tests step (line 194); documented in 0 other files.
- **1.5 Source sizes**: `chronicle-wal/src/lib.rs` 4668, `chronicle-application/src/lib.rs` 2994, `chronicle-cli/src/main.rs` 2382, `chronicle-application/src/record.rs` 2227; 0 `tracing::` usages across wal/etl/replay/capture/application library sources.
- **1.6 Exit/JSON baseline**: REPLAY_REPORT_VERSION=1; ReplayOutcome/Replayability/OperationExecutionState serialize snake_case; exit codes: 0 completed/dry-run, 3 cleanup TimedOut (main.rs 1486), 4 policy/invalid-session, 5 transport, 6 verification; error family via `application_error_exit_code`.
- **2.1 Classification** matches design tables (production / test-only / view-model / intentional seams).
- **2.2 Decision**: `InspectSessionResult.replayability` stays a documented intentional serialized-only contract (CLI never names it; application renderers own presentation). `ReplaySessionResult` outcome/replayability and `ReplayOperationSummary` state become application-owned types with identical serde representation.
- **2.3 No other outer adapter exists; no other lower-layer vocabulary crosses the CLI boundary** (grep of the full CLI crate above).

Implementation phase evidence (tasks 4.x-8.x):

- Contract corrections landed: application re-exports removed (lib.rs 73-77); application-owned `ReplayStatus`/`ReplayabilityStatus`/`OperationStatus`/`ReplayRequest` + `replay_result_exit_code` + `ReplaySessionResult::succeeded()`; CLI builds `ReplayRequest` (main.rs 1498/1541), converts `From<Timing> for ReplayTiming` (418-424), uses `replay_result_exit_code` + `succeeded()`; mapping tests moved to chronicle-application (207 application tests incl. both mapping tests green); CLI vocabulary scan clean (only `ReplayabilityStatus` matches the substring, not the word boundary).
- `cargo test --workspace --all-features --locked` all suites green, zero failures; `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` clean.
- Validator: `validation.py` gained `semantic_check` (S1/S2/policy self-consistency), wired into `architecture_check` (`"semantic": true` in output); `validate.sh fast` passed end to end including the extended architecture step.
- Rejection probe (task 7.2): temporary `use chronicle_application::ReplayOutcome;` under `crates/chronicle-cli/src/` failed the architecture step with invariant + location + rationale + remediation; after removal, exit 0 and issues=[].
- Tooling tests: 134 tests OK across all nine validation suites incl. 12 new `SemanticBoundaryTests` (acceptance/rejection/false-positive/message-format fixtures).
- AGENTS.md: 17.0 KB -> 8.6 KB navigation map; duplicated test-architecture prose removed (pointer added); bounded-execution knobs moved to CONTRIBUTING.md; content-addressed evidence policy re-homed in `validation/test-architecture/README.md`; reliability-boundaries bullet list kept directly per the developer-onboarding delta.
- Fingerprint/evidence (task 8.5): this change edits acceptance-fingerprint inputs (`crates/**`, `scripts/validation.py`, `scripts/validate.sh`, `validation/**`, `AGENTS.md`, `CONTRIBUTING.md`, `docs/**`), so retained live-capture/recorder evidence invalidates. Release planning MUST regenerate live-capture/recorder evidence before any release gate; no release gate was run in this change.

- **Contract-correction regression risk:** view-model/exit mapping changes touch CLI-visible behavior; mitigated by the requirement that existing CLI integration tests (cli_contract.rs), smoke, acceptance, and E2E suites pass unchanged, and JSON/exit-code stability is asserted before and after.
- **AGENTS.md over-trim:** mandatory sections (crate architecture summary, safety rules, entrypoints) are preserved; the existing spec requirement that AGENTS.md carry a crate architecture section is kept (summary + pointers, not full ownership prose).
- **Enforcement scope creep:** only S1/S2/V1 are mechanically enforced now; everything else is classified document-only or potential-future, and promotion requires four objective criteria.
