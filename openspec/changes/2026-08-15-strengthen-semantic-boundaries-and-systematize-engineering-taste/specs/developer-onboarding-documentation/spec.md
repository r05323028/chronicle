## ADDED Requirements

### Requirement: AGENTS.md is a concise agent navigation map

`AGENTS.md` SHALL be a short (~5-6 KB target) navigation entrypoint for human and agent readers, structured as principles plus navigation with progressive disclosure: short core principles and mandatory safety/workflow sections first, then a navigation map pointing to focused canonical documentation, then machine-enforced policy. `AGENTS.md` SHALL NOT duplicate policy prose that has a canonical owner; duplicated content SHALL be replaced with pointers to that owner.

The following SHALL remain directly in `AGENTS.md`: the Chronicle philosophy (compressed), critical safety/reliability rules (bounded command execution in condensed form, reliability boundaries that must not change, Linux-only validation note), mandatory workflow entrypoints (`./scripts/validate.sh` modes, `openspec validate`, `./scripts/acceptance.sh` profiles), the mandatory crate-architecture summary (primary ownership one-liners, dependency direction, forbidden patterns, update-in-same-change rule), and navigation pointers to `docs/architecture/crate-boundaries.md`, `validation/test-architecture/README.md`, `docs/engineering-taste.md`, `docs/replay-safety.md`, `docs/wal-format.md`, `CONTRIBUTING.md`, and OpenSpec documentation.

Detailed content SHALL move to canonical owners: crate ownership detail to `docs/architecture/crate-boundaries.md`; test architecture prose to `validation/test-architecture/README.md`; engineering taste and the feedback loop to `docs/engineering-taste.md`; bounded-command-execution parameters and validation-mode details to `CONTRIBUTING.md`. Removed detail SHALL remain discoverable in a linked canonical location; no guidance SHALL be deleted silently. The canonical validation entrypoint description in `CONTRIBUTING.md` SHALL remain truthful about `validate.sh` modes and timeouts.

#### Scenario: Agent starts a task

- **WHEN** an agent reads `AGENTS.md` at the start of a task
- **THEN** it finds principles, mandatory workflow entrypoints, safety rules, and a navigation map in a small file, and follows pointers for depth rather than reading policy encyclopedically

#### Scenario: Test-architecture lookup

- **WHEN** an agent needs test-layer or gate rules
- **THEN** `AGENTS.md` points to `validation/test-architecture/README.md` rather than duplicating its prose

#### Scenario: Guidance remains complete

- **WHEN** `AGENTS.md` is condensed
- **THEN** every removed detail exists in a linked canonical document and the mandatory crate-architecture section remains present per the workspace-dependency-boundaries spec
