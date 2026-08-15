# engineering-taste Specification

## Purpose
TBD - created by archiving change 2026-08-15-strengthen-semantic-boundaries-and-systematize-engineering-taste. Update Purpose after archive.

## Requirements

### Requirement: Chronicle defines and classifies engineering taste

Chronicle SHALL document a small set (at most 7) of engineering taste principles in `docs/engineering-taste.md`, preserving the existing product philosophy (low production overhead, stable tests over exact recordings, simple replay, behavior over traffic volume, reliability first, portable artifacts, deterministic core). Each taste principle SHALL be classified as one of: document-only, mechanically enforced now, or potential future enforcement. The seven principles SHALL be:

1. Outer adapters do not consume lower-layer vocabulary (CLI and future external adapters operate on application-owned contracts) — mechanically enforced now.
2. Correctness must not depend on timing guesses (no undocumented correctness sleeps or unbounded polling in product tests; bounded deadlines for readiness) — mechanically enforced now.
3. Boundaries validate external or durable data (network, persisted, configuration, and kernel-derived inputs are validated when crossing ownership boundaries; no redundant defensive validation inside trusted internal paths) — document-only.
4. Shared invariants have one owner (replay/WAL/ETL correctness rules are not duplicated across application, CLI, tests, and scripts; e.g. exit-code mapping is owned and tested by application) — document-only.
5. Code remains agent-legible (source-size and public-surface distribution are monitored as a human review signal; no arbitrary file-size limits without evidence-based justification from Chronicle's actual distribution) — document-only.
6. Observability is structured on production paths (prefer owned status artifacts and structured output over scattered ad-hoc logging; keep payloads redacted; no new logging abstraction without a demonstrated recurring problem) — document-only.
7. Prefer deterministic mechanisms over heuristics (heuristics or LLM assistance never become correctness authority) — already enforced by design; document-only.

The documentation SHALL also inventory the current mechanical invariants (dependency architecture policy, semantic-boundary policy, sleep/unbounded-polling policy, test catalog/preflight, bounded command execution) and SHALL record the no-magic-sleep rule so it is no longer code-only.

#### Scenario: Agent designs a new feature

- **WHEN** an agent or maintainer makes a design decision in record, recorder, ETL, replay, inspect, or doctor work
- **THEN** `docs/engineering-taste.md` states which principles apply and which are mechanically enforced, without prescribing implementation structure

#### Scenario: Sleep-policy lookup

- **WHEN** an agent asks why sleeps above 500 ms or unbounded polling are rejected in product tests
- **THEN** the documented taste principle and its mechanical enforcement are discoverable in `docs/engineering-taste.md`, and the checker's violation message names the rule and its location

### Requirement: Recurring review judgment promotes to mechanical invariants

Chronicle SHALL follow a documented promotion ladder for turning recurring human judgment into mechanically enforced invariants: (1) a recurring review finding, (2) document the principle with an enforcement classification, (3) observe repetition, (4) promote to a structural/lint/test invariant, (5) give actionable remediation. A rule SHALL be promoted only when all four criteria hold: the pattern has occurred repeatedly or represents a critical invariant; violation is objectively detectable; false positives are acceptably low; and remediation is clear. Rules lacking objective detection, low false-positive rates, or clear remediation SHALL remain documented guidance.

Every mechanically enforced validation failure produced by checks added or promoted under this requirement SHALL explain in the failure message: what invariant was violated, where it was violated (file and line), why the invariant exists (rationale), and the preferred remediation (including a documentation pointer). Legacy checks (for example the sleep/unbounded-polling policy) name the violation and location and SHALL be upgraded to the full format opportunistically. Agents SHALL NOT need to search a large policy document to understand a validation failure. The feedback loop and promotion criteria SHALL be documented in `docs/engineering-taste.md`.

#### Scenario: Review finding becomes a rule

- **WHEN** the same review finding recurs across changes and is objectively detectable with low false positives
- **THEN** it is documented, promoted to a mechanical check with remediation, and added to the mechanical-invariant inventory

#### Scenario: Review opinion stays guidance

- **WHEN** a reviewer opinion lacks objective detection or a clear remediation (for example a stylistic preference)
- **THEN** it remains documented guidance and is not encoded into lint
