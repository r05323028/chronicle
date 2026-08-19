# AGENTS.md

Chronicle turns production behavior into reliable, replayable regression tests with minimal
production cost. This file is a navigation map; canonical detail lives in the linked documents.
Prefer the smallest design that preserves the properties below.

## Chronicle philosophy

1. **Low production overhead** — avoid unnecessary capture, processing, storage, dependencies.
2. **Stable tests over exact recordings** — normalize dynamic values, preserve causal behavior, minimize flaky replay.
3. **Simple replay** — users operate on applications and recordings, not WALs/ETL stages/cgroups/storage internals.
4. **Behavior over traffic volume** — retain representative behaviors, not every interaction.
5. **Reliability first** — durability, recovery, loss accounting, determinism, safety boundaries never weakened for convenience.
6. **Portable artifacts** — canonical tests avoid dependence on production hosts, ports, timing, infrastructure.
7. **Deterministic core** — heuristics/LLMs may assist classification or suggestions, never correctness.

Engineering taste: 7 golden principles plus the review-finding-to-invariant feedback loop live in
`docs/engineering-taste.md`.

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

<!-- GRAPHIFY_START -->
## graphify

This project has a generated knowledge graph in `graphify-out/`.

Rules:

- Before answering architecture or codebase questions, read `graphify-out/GRAPH_REPORT.md`, then use `graphify query "<question>"` to target source reads and verify exact behavior in current code.
- Use `graphify path "<source>" "<target>"` for connection chains and `graphify explain "<node>"` for one symbol or concept.
- Treat `graphify-out/graph.json` as the current machine-readable graph and `graphify-out/GRAPH_REPORT.md` as its generated architecture report.
- Other files and directories under `graphify-out/` are Graphify-managed generated artifacts. Do not infer their semantics from their names; verify with Graphify tooling or documentation when their meaning matters.
- If `graphify-out/wiki/index.md` exists, use it as the navigation entry point before broad raw-source exploration.
- After modifying code files in this session, run `graphify update .` to keep the graph current using the AST-only update path.
<!-- GRAPHIFY_END -->

## Navigation map

| Need | Go to |
| --- | --- |
| High-level runtime/service architecture | `docs/architecture/overview.md` |
| Runtime/operator guidance | `docs/operations/overview.md` |
| Internal recorder runtime | `docs/operations/recorder.md` |
| Recorder runbook | `docs/operations/recorder-runbook.md` |
| Crate ownership, dependency direction, semantic boundaries | `docs/architecture/crate-boundaries.md`; policy `validation/architecture.toml` |
| Engineering taste + feedback loop | `docs/engineering-taste.md` |
| Test layers, gates, preflight, content-addressed evidence | `validation/test-architecture/README.md` |
| Validation modes + timeouts | `CONTRIBUTING.md` |
| Replay safety / WAL format | `docs/replay-safety.md` / `docs/wal-format.md` |
| Product/operations docs | `docs/operations/overview.md`, `docs/operations/recorder.md`, `docs/operations/recorder-runbook.md` |
| OpenSpec SDD workflow | `openspec/` |

## Mandatory workflow entrypoints

- `./scripts/validate.sh fast` — local feedback: fmt, warnings-denied Clippy, workspace tests,
  strict OpenSpec validation, ownership/architecture/catalog checks, tooling meta-tests.
- `./scripts/validate.sh targeted --changed-since <ref>` — changed-path validation;
  `live-capture|recorder` — privileged Linux acceptance; `release` — release qualification.
- `./scripts/acceptance.sh` — the ONLY user-facing acceptance entrypoint (`--profile
  live-capture|recorder|all`, `--executor local|multipass`). No deprecated aliases or wrappers.
- Before pushing: the repository pre-push hook runs act-based CI checks locally
  (`scripts/pre-push-validation.sh`, jobs `checks` and website `validate-build`;
  install with `scripts/install-pre-push-hook.sh`).
  Pre-push act = local CI parity; GitHub CI = authoritative remote validation; release gates = full qualification.
- `openspec validate --all --strict --no-interactive` — proves SDD artifact structure only,
  never product correctness. OpenSpec is the SDD workflow for requirements, design, scope,
  acceptance criteria, validation, and archiving (see `openspec/`).

## Documentation contract

Documentation is part of product contract. Before completing any change, review whether it affects:

- user-facing behavior, CLI commands/flags/defaults/output, or configuration;
- installation, deployment, or operations;
- supported capabilities;
- architecture, crate/service/module boundaries, dependency direction, or cross-crate contracts;
- durable engineering invariants or conventions.

If affected, update relevant documentation in same change. User-facing behavior or canonical English documentation changes require review and update of corresponding `zh-tw` and `ja` pages; run `cd website && npm run verify:localization`. Terminology and freshness rules live in `website/TERMINOLOGY.md`. Targets include:

- `README.md` for repository usage and entry-point information;
- `docs/` for canonical product, architecture, operations, and runbook guidance;
- `website/` for user-facing product documentation;
- `AGENTS.md` for durable agent-facing rules and conventions;
- `openspec/` for behavior and contract specifications.

Record only durable, repository-wide guidance in `AGENTS.md`; do not add temporary implementation details or one-off decisions.

For architecture or boundary changes, review `AGENTS.md`, canonical `docs/architecture/crate-boundaries.md`, and `validation/architecture.toml`; update affected contracts without duplicating canonical detail, then run architecture validation.

For OpenSpec changes: implementation → tests → documentation impact review → validation → complete/archive. Do not complete or archive while affected documentation is stale.

Before declaring complete, review implementation diff, determine documentation impact, update affected documentation, or explicitly verify documentation is unaffected, then validate.

## Bounded command execution

Never run potentially blocking commands without a bounded timeout:
`./scripts/run-with-timeout.sh <duration> <command>...` (deadline 124, then KILL). Defaults:
command 900s, acceptance profile 3300s, gate 3600s; all override knobs + Multipass deadlines in
`CONTRIBUTING.md` "Validation timeouts". Ranges: readiness 30-120s; targeted builds 5-15 min;
full workspace 15-30 min; VM/privileged 30-60 min.

Recorder/agent workflow: `validate.sh fast` → targeted changed-path tooling → bounded
scenario/lifecycle; run a complete `live-capture`/`recorder` gate only when required. Never start
a foreground daemon without the wrapper and deterministic cleanup.

## Linux-only validation on macOS

If the host is macOS and the task requires Linux-specific validation, run the checks inside a
Multipass Ubuntu 24.04 VM (reuse one from `multipass list` or create one with the repository
mounted); never mark Linux-only tasks complete from macOS results. If Multipass is unavailable,
report the blocker and continue with portable checks only.

## Reliability boundaries that must not change

Kernel ABI/Aya privacy (`chronicle-capture-ebpf`); WAL commit-marker durability/recovery
authority; canonical schema compatibility; persisted checkpoint formats; ETL as complete
Extract-Transform-Load; replay default-deny safety; deterministic replay/test behavior; layered
test architecture; privileged acceptance semantics; release qualification guarantees. Detail:
`docs/architecture/crate-boundaries.md`.

Public 0.1 compatibility rules:

- Keep frozen WAL, intentionally persisted Capture Event, Canonical Session, Session Manifest, replay-safety, public CLI, and declared public JSON contracts stable within 0.1.x.
- Do not add migration adapters for unsupported or repository-only formats, compatibility aliases, or implicit lineage fallbacks.
- Introduce any incompatible public change only through an explicit OpenSpec change with version, reader/writer, migration, deprecation, test, and documentation policy.
- Do not add feasibility-only implementations when production validation paths exist.
- Historical behavior belongs in Git history and archived OpenSpec changes.
- Runtime lineage must be explicit; never infer from identifiers.
- The hidden `internal` namespace is current operational surface; hidden compatibility entrypoints must not return (the legacy-names guard in `scripts/validation.py` covers removed identifiers, retired paths, and scenario ids).

Recording lifetime may be unbounded while epochs and segments remain bounded. A WAL epoch boundary is not a protocol reconstruction boundary; cross-epoch state requires bounded, versioned, checksummed, lineage-verified continuation evidence.

## Crate Architecture (mandatory)

13-crate workspace; every crate has one primary owner (see `docs/architecture/crate-boundaries.md`).
Dependency direction is a maximum-coupling allowlist in `validation/architecture.toml`, enforced by
`scripts/validation.py architecture`, which also enforces the semantic boundary: outer adapters
operate only on application-owned contracts (no lower-layer vocabulary via re-exports).

- Primary ownership: `chronicle-common` primitives; `chronicle-canonical` canonical model;
  `chronicle-capture` evidence; `chronicle-capture-ebpf` Linux adapter; `chronicle-wal`
  durability; `chronicle-session` reconstruction; `chronicle-protocol` SPI/registry;
  `chronicle-protocol-builtins` implementations; `chronicle-storage` persistence;
  `chronicle-replay` replay; `chronicle-etl` complete Extract-Transform-Load;
  `chronicle-application` use-case composition; `chronicle-cli` parsing/dispatch/rendering/exit.
- Forbidden patterns (every dependency kind): dependency on `chronicle-cli`; `session -> wal`;
  `protocol -> protocol-builtins`; `common` upward; any `chronicle-cli` Chronicle edge except
  `chronicle-application`.
- **Application/CLI**: CLI communicates only through application-owned requests/results/errors/
  rendering; no protocol decoding, replay policy, WAL scanning, ETL orchestration, or eBPF loading
  in CLI.
- **ETL is complete Extract-Transform-Load**: it owns storage publication and checkpoint ordering.
- **eBPF privacy**: Aya handles/kernel ABI stay private to `chronicle-capture-ebpf`.
- Adding a legitimate dependency requires updating `docs/architecture/crate-boundaries.md`,
  `AGENTS.md`, and `validation/architecture.toml` together, then running the architecture check.

## Service architecture (durable)

Chronicle's long-term production pipeline separates Recorder, Local WAL, Durable Evidence Store, ETL, and Canonical Store. The current local runtime co-locates Recorder and incremental ETL in one process over filesystem storage; that is an implementation/deployment choice, not a correctness requirement.

- **Local WAL is the capture durability and recovery authority.** Remote evidence stores are durable handoff/distribution boundaries and never replace local WAL durability in the capture hot path.
- **Recorder and ETL are separate logical boundaries.** Correctness must not depend on sharing a process, memory, capture ownership, or a local filesystem namespace; ETL must remain independently deployable in the architecture.
- **ETL owns canonical publication, publication verification, and checkpoint advancement ordering.** It consumes recovery-authoritative evidence through a durable evidence contract, not by owning the capture runtime.
- Architecture changes affecting these service boundaries require architecture documentation and validation-policy review together.

## Test Architecture

Every significant test is exactly one functional layer (unit/integration/smoke/acceptance/e2e/
privileged); privileged execution, live-capture, and recorder are gate selectors, not layers.
Portable assertions MUST NOT become privileged proof merely because a Multipass profile can run
them. Normative details: `validation/test-architecture/README.md`.

## Pull-request workflow

- Prepare work as a pull-request-targeted change; do not assume direct pushes to `main`.
- Squash merging makes the final PR title the canonical commit title on `main`; choose a
  Conventional Commit-compatible title before presenting or creating a PR.
- Intermediate branch commits may be `wip`, `fix tests`, or `address review`; do not rewrite
  branch history merely to make temporary commits conventional.
- Keep each PR conceptually coherent so its squash commit describes shipped intent, not
  implementation noise. Required GitHub CI checks must pass before merge.
- Release/changelog automation treats the resulting linear `main` history as deterministic input.
  Detailed workflow guidance lives in `CONTRIBUTING.md#pull-request-workflow`.

## Task completion rules

- A task is complete only from evidence that task explicitly requires; OpenSpec checkboxes are
  progress records, not proof.
- Privileged tasks require retained evidence for the required environment/scenario; `fast`
  development acceptance is not complete retained evidence.
- Privileged acceptance proves only kernel/environment-dependent behavior — never repository lint,
  doc consistency, an OpenSpec validator, or a duplicate CI suite.
