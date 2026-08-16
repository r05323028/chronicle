# Engineering taste

This document is the canonical home for Chronicle's engineering taste principles and for the
feedback loop that promotes recurring human judgment into mechanically enforced invariants. It is
the counterpart of the philosophy section in `AGENTS.md`; `AGENTS.md` stays a short navigation
map and points here for depth.

## Principles

Chronicle preserves its product philosophy (low production overhead, stable tests over exact
recordings, simple replay, behavior over traffic volume, reliability first, portable artifacts,
deterministic core). The following golden principles additionally guide an agent-maintained
codebase. Each is classified as **document-only**, **mechanically enforced now**, or **potential
future enforcement**. Only rules with strong signal and low false-positive rates are automated.

1. **Outer adapters do not consume lower-layer vocabulary.** CLI and future external adapters
   operate on application-owned request/result/error/rendering contracts; replay policy and
   protocol/replay error taxonomies stay behind the application seam. — *mechanically enforced now*
   (`[semantic]` table in `validation/architecture.toml`, checked by `scripts/validation.py
   architecture`; model in `docs/architecture/crate-boundaries.md`).
2. **Correctness must not depend on timing guesses.** No undocumented correctness sleeps
   (>= 500 ms in Python tests, >= 500 ms in Rust integration tests) and no unbounded polling in
   product test files; readiness and scenario waits use bounded deadlines. — *mechanically enforced
   now* (`scripts/tests/validation/test_sleep_policy.py`, wired into `validate.sh fast` via the
   tooling-tests step). Fixture sleeper workloads under `scripts/tests/**` are exempt because they
   exercise the timeout/infrastructure itself.
3. **Boundaries validate external or durable data.** Network, persisted, configuration, and
   kernel-derived inputs are validated (parse, don't validate) when crossing ownership boundaries:
   WAL recovery validates durable frames, ETL validates metadata, config parsing validates TOML,
   eBPF normalizes kernel observations at the adapter edge. Do not add redundant defensive
   validation inside already trusted internal paths. — *document-only*; potential future
   enforcement via boundary-validation checklists if a recurring pattern emerges.
4. **Shared invariants have one owner.** Protocol/replay/WAL/ETL correctness rules are not
   duplicated across application, CLI, tests, and scripts. Concrete case: exit-code mapping is
   application logic, so its unit tests live in `chronicle-application`; CLI tests only verify
   wiring. — *document-only*; duplication detection is fuzzy and not worth automating yet.
5. **Code remains agent-legible.** Evidence from the current source distribution (largest files:
   `chronicle-wal/src/lib.rs` 4668 lines, `chronicle-application/src/lib.rs` 2994,
   `chronicle-cli/src/main.rs` 2382, `chronicle-application/src/record.rs` 2227). No hard
   file-size limit is introduced: a threshold would have to exceed today's largest file to pass, so
   it would not catch anything. Size distribution stays a human review signal; revisit if a file
   grows far beyond the current maximum. — *document-only*; potential future enforcement only with
   a justified evidence-based threshold.
6. **Observability is structured on production paths.** No `tracing` in library crates today;
   `chronicle-cli` (outer adapter) initializes `tracing-subscriber` once; production
   observability flows through owned status artifacts (recorder status, checkpoints, machine-readable
   reports). Prefer owned status artifacts and structured output; keep payloads redacted. Do not
   introduce a new logging abstraction without a demonstrated recurring problem. — *document-only*.
7. **Prefer deterministic mechanisms over heuristics.** Heuristics or LLM assistance never become
   correctness authority. Already expressed as philosophy and enforced by design: privileged
   preflight deny-by-default, honest planned protocol registrations, `--ignored` privileged suites,
   deterministic replay. — *already enforced by design*; no additional mechanism needed.

## Feedback loop: from recurring judgment to invariant

1. **Recurring review finding** — a human or agent reviewer flags the same pattern more than once,
   or the pattern touches a critical invariant.
2. **Document the principle** here with an enforcement classification (document-only / mechanically
   enforced now / potential future enforcement).
3. **Observe repetition** — count occurrences or judge criticality before investing in automation.
4. **Promote to a structural/lint/test invariant** only when **all four** criteria hold:
   - the pattern has occurred repeatedly **or** is a critical invariant;
   - violation is objectively detectable;
   - false positives are acceptably low;
   - remediation is clear.
5. **Give actionable remediation** — the mechanical failure message states what invariant was
   violated, where (file:line), why it exists (rationale), and the preferred remediation with a
   documentation pointer.

The repository does not encode every reviewer opinion into lint. Rules lacking objective detection,
low false-positive rates, or clear remediation stay documented guidance.

## Validation message format

Checks added or promoted under this document produce failures with four elements:

```text
<invariant>: <what was violated>
location: <file:line>
rationale: <why the invariant exists>
remediation: <preferred fix, with documentation pointer>
```

Older checks (sleep policy, catalog) name the violation and location and are upgraded to the full
format opportunistically. Agents should not need to search a large policy document to understand a
failure.

## Current mechanical invariants

| Invariant | Mechanism | Where it runs |
| --- | --- | --- |
| Dependency direction (normal/dev/build edges, forbids, cycles) | `validation/architecture.toml` + `validation.py architecture` | `validate.sh fast`, release |
| Semantic boundary (outer-adapter vocabulary, application re-exports) | `[semantic]` table + `validation.py architecture` | `validate.sh fast`, release |
| No undocumented correctness sleeps / unbounded polling | `test_sleep_policy.py` | `validate.sh fast` tooling-tests |
| Test classification, gate obligations, preflight contract | `validation/test-architecture/` + `validation.py catalog` | `validate.sh fast` |
| Bounded command execution | `scripts/run-with-timeout.sh` wrapping every gate step | all gates |
| Privileged preflight deny-by-default | `scripts/privileged/preflight.py` | acceptance |
