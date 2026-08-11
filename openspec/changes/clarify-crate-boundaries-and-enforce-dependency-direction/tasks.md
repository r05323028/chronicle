## 1. Baseline and architecture contract

- [x] 1.1 Capture the implementation-start workspace graph with bounded `cargo metadata --format-version 1 --no-deps`, including normal/dev/build, optional, renamed, and target-specific Chronicle edges; compare it with the 43-normal/4-dev/no-cycle assessment in `proposal.md` and record any drift before editing. Acceptance: reviewed edge inventory names every root-workspace crate and explains every drift caused by concurrent work.
- [x] 1.2 Add `docs/architecture/crate-boundaries.md` with one primary responsibility, owned concepts/public seam, allowed dependencies, forbidden knowledge, current graph, target graph, and “must not change” reliability boundaries for all 13 crates. Acceptance: every crate appears exactly once as primary owner and ETL is explicitly complete Extract-Transform-Load with storage.
- [x] 1.3 Add `validation/architecture.toml` version 1 with target normal/dev/build allowlists and explicit critical forbids; include optional Linux application→eBPF and ETL dev→builtins, allow no workspace build edges, and permit CLI→application only for every dependency kind. Acceptance: config represents design matrix without blessing current session→WAL or CLI lower-layer edges.

## 2. Portable dependency enforcement

- [x] 2.1 Extend existing standard-library validation helper with bounded Cargo-metadata architecture checking: workspace/package identity, dependency kind, optional flag, target condition, package rename, cycle detection, policy-member validation, forbidden/unclassified-edge diagnostics, deterministic output. Acceptance: no external dependency; deleting an allowed edge passes; source/target/kind are printed on failure.
- [x] 2.2 Add focused standard-library tests beside layered-validation tooling for accepted graph, forbidden normal/dev/build edges, Linux-only optional edge detected on non-Linux, renamed package identity, dependency on CLI, session→WAL, protocol→builtins, unknown/missing policy members, duplicate/conflicting policy, and cycles. Acceptance: tests execute without Cargo network access and fail if any bypass becomes accepted.
- [x] 2.3 Add architecture policy/config/helper paths to targeted validation ownership without assigning privileged P1/P2 runtime proof solely for architecture hygiene. Acceptance: dry-run selection explains portable/build-tooling checks and makes no eBPF runtime claim.

## 3. Session boundary cleanup

- [x] 3.1 Define transport-neutral persistence-loss evidence in `chronicle-capture` using existing clock/interval/count/ambiguity concepts; keep `chronicle-wal::TerminalWalLoss` and its v1 codec as WAL-owned wire types. Acceptance: neutral type names no WAL implementation and WAL terminal-loss codec fixtures remain byte-identical.
- [x] 3.2 Convert recovered terminal WAL-loss records to neutral evidence inside ETL extraction before pushing reconstruction input. Acceptance: conversion preserves interval, clock identity, counts, reason, and conservative ambiguity exactly; malformed WAL still fails at WAL/ETL boundary.
- [x] 3.3 Change `chronicle-session` reconstruction input/state to consume neutral evidence and remove `chronicle-wal` from its Cargo manifest. Preserve serialized checkpoint-compatible fields/names where renaming would change format. Acceptance: `cargo metadata` shows no session→WAL edge and session source imports no `chronicle_wal` type.
- [x] 3.4 Add/update capture, WAL, session, and ETL tests proving reconstruction equivalence for persisted, metadata-only, overlapping, outside-window, clock-mismatch, and unavailable-timestamp loss evidence. Acceptance: focused crate tests pass and canonical completeness/provenance output remains unchanged.

## 4. Application use-case ownership cleanup

- [ ] 4.1 Inventory every workspace consumer of `chronicle-application` public items and classify each export under record, recorder, ETL, replay, inspect, doctor, or internal implementation. Acceptance: no public export is moved/removed without a named consumer decision; active `user-intent-cli` changes are preserved.
- [ ] 4.2 Move root-level doctor, inspect, ETL, replay, and record implementations into focused existing/new internal modules, while grouping existing recorder lifecycle modules under clear recorder ownership where a move reduces ambiguity. Exact directory nesting is optional. Acceptance: `lib.rs` becomes composition/re-export surface rather than home for unrelated large implementations; no new crate, service trait, or factory is added.
- [ ] 4.3 Curate application exports around user-facing requests/results/errors and documented extension seams; make low-level scope/quota/transition/persistence helpers crate-private when no real cross-crate consumer exists. Remove the mostly unimplemented `ChronicleApplication` facade if still unused rather than completing speculative methods. Acceptance: workspace builds after each bounded move and external behavior tests remain unchanged.
- [ ] 4.4 Move existing one-shot `FilesystemSessionStore` final publication, no-replace verification, and recording-local checkpoint advancement from application behind one ETL-owned API; keep domain lock, quota policy/reservation, use-case selection, and presentation in application. Acceptance: ETL retains `chronicle-storage`; application no longer calls concrete session publication or advances ETL checkpoints; deterministic IDs, quota accounting, final session/checkpoint bytes, and publication-before-checkpoint fault behavior remain identical.

## 5. CLI outer-adapter cleanup

- [ ] 5.1 Add the smallest application-owned request constructors and stable outcome/error classification methods needed for explicit replay, record, inspect, doctor, ETL compatibility paths, rendering, and exit mapping. Acceptance: no duplicate replay planner/policy/error hierarchy and no CLI behavior change.
- [ ] 5.2 Replace CLI imports/matches of common, protocol, protocol-builtins, replay, capture, and WAL types with application APIs; retain only Clap grammar, binary runtime/signal wiring, checked output writes, and final exit. Acceptance: production CLI source imports no Chronicle crate except `chronicle_application`.
- [ ] 5.3 Move capture/WAL fixture construction out of CLI tests into application services/test support and remove CLI dev dependencies on capture/WAL. Remove CLI normal dependencies on common/protocol/protocol-builtins/replay. Acceptance: all CLI normal/dev/build Chronicle dependencies target application only.
- [ ] 5.4 Run CLI golden/contract tests for help, five public commands, hidden compatibility forms, human/JSON output, exit matrix, replay safety, and signal behavior. Acceptance: outputs and exits match pre-cleanup expectations byte-for-byte where contracts require exact output.

## 6. Agent and architecture documentation

- [ ] 6.1 Add mandatory `AGENTS.md` crate architecture section covering all primary owners, dependency direction, critical forbidden patterns, complete ETL/storage rule, private eBPF boundary, application organization, CLI application-only rule, and required architecture validation after manifest changes. Acceptance: guidance states `AGENTS.md` is normative and `validation/architecture.toml` is executable mirror.
- [ ] 6.2 Link `docs/architecture/crate-boundaries.md` from existing architecture documentation and reconcile existing diagrams with actual/target manifest edges. Acceptance: no diagram describes WAL as dependency-standalone or omits known direct edges while claiming exact compile-time graph.
- [ ] 6.3 Review crate/module docs and public API comments touched by cleanup so responsibility statements match `AGENTS.md`, architecture docs, and policy. Acceptance: searches find no contradictory claim that ETL is transform-only, CLI may reach lower layers, or protocol core owns built-ins.

## 7. Validation integration and completion evidence

- [ ] 7.1 After session/CLI dependency cleanup, wire architecture check into bounded `scripts/validate.sh fast` and release paths beside existing source-ownership validation; update CI only if existing fast invocation does not already cover it. Acceptance: injecting one forbidden fixture edge makes fast validation fail, removing it restores pass.
- [ ] 7.2 Run proactive diagnostics and focused portable checks: architecture helper tests, session/capture/WAL/ETL tests, application tests, CLI contract tests, and strict OpenSpec validation. Acceptance: every command is bounded, exit status recorded, and no structural validation is reported as privileged runtime proof.
- [ ] 7.3 Run `./scripts/validate.sh fast` with repository timeout hierarchy. Acceptance: fmt, Clippy, workspace tests, OpenSpec strict validation, source ownership, and architecture validation pass; unrelated pre-existing failures are reported rather than hidden.
- [ ] 7.4 Verify final Cargo graph is acyclic and matches target maximum coupling: session has no WAL edge; CLI has only application across normal/dev/build; protocol has no built-ins edge; common remains leaf; ETL retains storage; optional target-specific edges are classified. Acceptance: retain deterministic graph output as completion evidence.
- [ ] 7.5 Verify no persisted or user-visible contract changed by comparing relevant fixture bytes, canonical/checkpoint outputs, CLI golden outputs, and public command behavior. Acceptance: no recording/WAL/canonical/checkpoint/storage format or CLI behavior delta; any discovered runtime delta blocks completion and requires scope review.
- [ ] 7.6 Run `graphify update .` after code-file modifications and review resulting graph for unexpected community/coupling changes. Acceptance: graph update succeeds and architecture docs still match Cargo policy.
- [ ] 7.7 Mark acceptance criteria complete only from explicit evidence: every crate documented, direction documented, validator exists/rejects forbidden edges, CLI application-only, application internally separated, session WAL-independent, `AGENTS.md` updated, tests passing, and no user-visible behavior change. Privileged acceptance remains not required unless implementation exceeds architecture-only scope.

## Implementation Evidence

### 1.1 Workspace graph at implementation start (2026-08-11)

Bounded `cargo metadata --format-version 1 --no-deps` from the workspace root captured 13 Chronicle root-workspace crates: common, canonical, capture, capture-ebpf, wal, session, protocol, protocol-builtins, storage, replay, etl, application, cli. Edge inventory: 43 normal, 4 dev, 0 build, no dependency cycle (DFS).

Drift vs `proposal.md`/design assessment (43-normal/4-dev/no-cycle): **none**. The working tree already contains the archived user-intent-cli surface; no manifest edge differs from the assessed graph. Known problem edges confirmed present: `session -> wal` (normal), `cli -> {protocol, protocol-builtins, replay, common}` (normal), `cli -> {capture, wal}` (dev).
