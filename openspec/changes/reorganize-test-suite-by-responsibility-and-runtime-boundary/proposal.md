## Why

Chronicle has strong lower-level Rust coverage and durable privileged evidence, but current test ownership is inconsistent: root `tests/e2e/` contains only acceptance workload tooling, crate integration suites include complete vertical slices and executable contracts, and P1/P2 privileged scenarios rerun portable Cargo tests and assert WAL, ETL, replay, CLI, corruption, and lifecycle behavior that often does not itself require privilege. This makes milestone gates act like test categories, increases Multipass cost, and obscures which layer conclusively proves each behavior.

Chronicle needs one responsibility-based test architecture aligned with crate ownership, with privilege treated as an execution requirement rather than a substitute for unit, integration, smoke, acceptance, or end-to-end classification.

## What Changes

- Define normative purposes and boundaries for unit, integration, smoke, acceptance, end-to-end, and privileged testing, including the rule to use the lowest-cost layer that conclusively proves behavior.
- Require a complete classification ledger for significant existing tests and assertions, with explicit stay, move, split, rootless, retain-privileged, helper-migration, or delete decisions based on responsibility and reliability rather than directory aesthetics.
- Separate user-facing acceptance from complete-system E2E coverage, and define a deterministic synthetic-capture rootless E2E path alongside a very small real-eBPF privileged E2E path.
- Treat unsupported privileged environments as a distinct preflight result, not a failed product assertion, while preserving supported Ubuntu 24.04/kernel evidence and content-addressed P1/P2 release obligations.
- Move product correctness assertions out of validation and VM orchestration code; validation selects tests, gates select required coverage, and environment tooling prepares/runs/collects evidence.
- Define minimal shared process/readiness/lifecycle helper ownership for black-box suites and retain specialized recorder readiness where domain semantics exceed a generic condition wait.
- Align test helpers and dev dependencies with the 13-crate ownership/allowlist contract; test code cannot create a dependency-direction escape hatch.
- Recast P1/P2 as milestone selectors over required layers and environment-qualified scenarios, not structural test-suite categories.
- Preserve existing product behavior, test coverage obligations, timeout containment, scenario attribution, and evidence compatibility while migrating in audited slices.

## Capabilities

### New Capabilities

- `test-architecture`: Responsibility-based test layers, runtime/privilege classification, helper ownership, migration rules, and coverage non-duplication policy.

### Modified Capabilities

- `layered-validation`: Define mode composition in terms of test layers, keep validation as selector/orchestrator only, and make P1/P2 gates select coverage rather than own test categories.
- `p2-completion`: Restrict privileged P2 assertions to behavior requiring supported Linux/kernel conditions while preserving complete release evidence and required lower-layer coverage.

## Impact

Future implementation affects test placement under crate `src/` and `tests/`, root `tests/{smoke,acceptance,e2e,privileged}`, acceptance scenario ownership, shared test support, privileged preflight/executor/evidence scripts, `scripts/validate.sh`, `scripts/validation.py`, `validation/groups.toml`, CI invocation, testing documentation, and `AGENTS.md`.

No production behavior, public API, persisted format, crate architecture, dependency allowlist, protocol, CLI design, or eBPF behavior changes are authorized by this change. This planning task creates OpenSpec artifacts only; test moves, helper implementation, script refactors, and flaky-test fixes belong to later implementation tasks.

## Acceptance Criteria

- Every significant current test and product assertion has one functional layer, one environment classification, one owner, one migration disposition, and mapped gate obligations.
- Portable correctness no longer runs as privileged proof; every remaining privileged assertion names real supported-environment dependency.
- Smoke, black-box acceptance, deterministic rootless E2E, and minimal privileged coverage are independently selectable.
- Validation and privileged executor code select, bound, and report tests without owning product correctness assertions.
- Unsupported privileged environment, infrastructure error, timeout/incomplete, and supported-environment product failure are machine-distinct and cannot satisfy release evidence.
- Process tests use bounded condition-based readiness and deterministic cleanup while preserving specialized recorder-readiness semantics.
- P1/P2 remain complete milestone selectors, content-addressed evidence remains valid under compatible contracts, and no required coverage disappears.
- Test helpers and dev dependencies pass existing crate architecture policy.
- `AGENTS.md` and detailed testing documentation contain final layer rules and lowest-cost conclusive-proof rule after implementation.
- No production behavior, persisted format, public CLI/API, protocol, eBPF behavior, P1/P2 feature requirement, or crate architecture changes.
