## Why

Chronicle's crate-level dependency boundary is already strong: `validation/architecture.toml` mechanically allowlists every normal/dev/build workspace edge, and `chronicle-cli` depends only on `chronicle-application`. But the boundary currently exists only at the Cargo level. `chronicle-application` re-exports lower-layer vocabulary verbatim, and `chronicle-cli` consumes it:

- `crates/chronicle-application/src/lib.rs:73` `pub use chronicle_protocol::{ProtocolError, TransportErrorCategory};`
- `crates/chronicle-application/src/lib.rs:74-77` `pub use chronicle_replay::{LoopbackReplayOptions, OperationExecutionState, ReplayError, ReplayOutcome, Replayability, TimingMode};` (multiline block; the view model also embeds `ReplayOutcome`/`Replayability`/`OperationExecutionState` via `ReplaySessionResult`, `InspectSessionResult` (serialized-only), and `ReplayOperationSummary`)
- `crates/chronicle-cli/src/main.rs:22` imports `LoopbackReplayOptions, ReplayOutcome, TimingMode` from `chronicle_application`; production code constructs `LoopbackReplayOptions` twice (lines 1498, 1541), pattern-matches `ReplayOutcome` variants (line 376), and implements `From<Timing> for TimingMode` (lines 418-424). Tests additionally construct `ProtocolError`, `TransportErrorCategory`, `OperationExecutionState`, and `Replayability` (lines 1856, 2178-2274).

The CLI therefore understands and constructs replay policy and interprets protocol/replay error taxonomies merely because application re-exports them — exactly the leak the intended boundary (`CLI -> application-owned contracts -> internal concepts`) forbids. The Cargo graph is clean; the semantic boundary is not.

Separately, agent-maintained engineering guidance has drifted: `AGENTS.md` is ~17 KB, duplicates the test-architecture README nearly verbatim, repeats crate-ownership detail already owned by `docs/architecture/crate-boundaries.md`, and carries encyclopedia-level timeout parameter tables. Engineering taste rules exist partly as philosophy, partly as undocumented code (the sleep policy is enforced by `scripts/tests/validation/test_sleep_policy.py` but documented nowhere else), and partly as prose that is never promoted to mechanical checks. The OpenAI "Harness engineering" article argues for exactly the fixes proposed here: strict semantic boundaries mechanically enforced, taste codified with classification, recurring review judgment promoted to invariants, and a concise AGENTS.md acting as a navigation map with progressive disclosure.

## What Changes

- **Semantic boundary model.** Extend the boundary model beyond Cargo edges: outer adapters (CLI today, future external adapters) operate only on application-owned request/result/error/rendering contracts; application must not re-export lower-layer implementation vocabulary as an escape hatch; replay policy and protocol error taxonomy remain owned by replay/application composition; the application-owned view model exposes application-owned classification instead of raw replay/protocol taxonomy. Documented in `docs/architecture/crate-boundaries.md`.
- **Mechanical enforcement.** Extend `validation/architecture.toml` with a `[semantic]` section (forbidden outer-adapter vocabulary, forbidden re-export sources, reviewed per-symbol allowlist) and extend the existing `scripts/validation.py architecture` check to enforce it: source-level scans of application re-export lines and CLI source. No new framework, crate, or third-party lint platform.
- **Minimal application-facing contract corrections.** Application stops re-exporting the leaked vocabulary; CLI stops naming/constructing replay and protocol types, using application-owned request APIs and application-owned outcome/exit classification instead; exit-code mapping tests move to application where the mapping is owned. JSON output, exit codes, and CLI behavior remain unchanged (compatibility-preserving adapter change).
- **Engineering taste.** New `docs/engineering-taste.md` documenting 7 taste principles, each classified as document-only, mechanically enforced now, or potential future enforcement, plus the promotion ladder from recurring review finding to mechanical invariant and the required validation-error message format.
- **AGENTS.md as navigation map.** Condense `AGENTS.md` from ~17 KB to a short principles + navigation entrypoint: philosophy, mandatory workflow entrypoints, critical safety rules, and pointers to canonical docs. Remove duplication with existing docs; move the bounded-command-execution parameter table to `CONTRIBUTING.md`.
- **Spec deltas.** New `engineering-taste` capability spec; extended `workspace-dependency-boundaries` (semantic boundaries) and `developer-onboarding-documentation` (AGENTS.md navigation map) specs.

## Capabilities

### New Capabilities

- `engineering-taste`: Chronicle defines a small set of engineering taste principles with explicit enforcement classification, and converts recurring review judgment into mechanical invariants through a documented promotion ladder with objective criteria and actionable remediation messages.

### Modified Capabilities

- `workspace-dependency-boundaries`: semantic/API boundaries are enforced beyond Cargo edges — outer adapters operate only on application-owned contracts; application re-exports of lower-layer vocabulary are forbidden except a reviewed allowlist; a portable source-level check wired into `validate.sh fast` and release validation enforces the policy with tests for acceptance and rejection cases.
- `developer-onboarding-documentation`: `AGENTS.md` is a concise agent navigation map (~5-6 KB target) with progressive disclosure to canonical docs, no duplication of policy prose, and mandatory safety/workflow sections preserved.

## Impact

Affected code: `crates/chronicle-application/src/lib.rs` (re-export removal, view-model classification, exit-code mapping ownership), `crates/chronicle-cli/src/main.rs` (request API usage, outcome classification usage, test fixture ownership), `crates/chronicle-cli/tests/cli_contract.rs` (only where it names lower-layer vocabulary).

Affected non-code: `validation/architecture.toml` (`[semantic]` section), `scripts/validation.py` (architecture check extension), `scripts/tests/validation/test_architecture_boundaries.py` or new `test_semantic_boundaries.py`, `scripts/validate.sh` (unchanged wiring; existing architecture step now covers semantic checks), `AGENTS.md` (condensed), `CONTRIBUTING.md` (timeout details), `docs/architecture/crate-boundaries.md` (semantic boundary section), new `docs/engineering-taste.md`, OpenSpec specs.

No change to WAL durability/recovery authority, canonical schema, persisted checkpoint formats, replay default-deny safety, ETL as complete Extract-Transform-Load, eBPF raw ABI privacy, CLI behavior/output/exit codes, deterministic replay/test behavior, layered test architecture, privileged acceptance semantics, or release qualification guarantees. This is harness/architecture quality work, not a product redesign.
