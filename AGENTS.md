## Chronicle philosophy

Chronicle turns production behavior into reliable, replayable regression tests with minimal production cost.

When making design decisions, prefer:

1. **Low production overhead** — avoid unnecessary capture, processing, storage, and dependencies.
2. **Stable tests over exact recordings** — normalize dynamic values, preserve causal behavior, and minimize flaky replay.
3. **Simple replay** — users operate on applications and recordings, not WALs, ETL stages, cgroups, or storage internals.
4. **Behavior over traffic volume** — retain representative behaviors instead of turning every production interaction into a test.
5. **Reliability first** — WAL durability, recovery, loss accounting, deterministic behavior, and explicit safety boundaries must not be weakened for convenience.
6. **Portable artifacts** — canonical tests should avoid unnecessary dependence on production hosts, ports, timing, and infrastructure.
7. **Deterministic core** — heuristics or LLMs may assist classification or suggestions, but correctness must not depend on them.

Prefer the smallest design that preserves these properties.

<!-- CODEGRAPH_START -->
## CodeGraph

In repositories indexed by CodeGraph (a `.codegraph/` directory exists at the repo root), reach for it BEFORE grep/find or reading files when you need to understand or locate code:

- **MCP tool** (when available): `codegraph_explore` answers most code questions in one call — the relevant symbols' verbatim source plus the call paths between them, including dynamic-dispatch hops grep can't follow. Name a file or symbol in the query to read its current line-numbered source. If it's listed but deferred, load it by name via tool search.
- **Shell** (always works): `codegraph explore "<symbol names or question>"` prints the same output.

If there is no `.codegraph/` directory, skip CodeGraph entirely — indexing is the user's decision.
<!-- CODEGRAPH_END -->

<!-- GRAPHIFY_START -->
## graphify

This project has a graphify knowledge graph at graphify-out/.

Rules:

- Before answering architecture or codebase questions, read graphify-out/GRAPH_REPORT.md for god nodes and community structure
- If graphify-out/wiki/index.md exists, navigate it instead of reading raw files
- After modifying code files in this session, run `graphify update .` to keep the graph current (AST-only, no API cost)
<!-- GRAPHIFY_END -->

## Bounded Command Execution

Do not run potentially blocking commands without a bounded timeout.

This applies to:

- Tests and builds
- Package installation
- Network operations
- Docker, Kubernetes, Multipass, SSH, and other remote commands
- Service start, stop, restart, and readiness checks
- Long-running application processes
- Polling loops
- Acceptance and privileged tests

Choose the timeout based on the expected workload and environment. Use a duration that is long enough for a healthy run, but never allow an unbounded wait.

Suggested ranges:

- Status and readiness checks: 30-120 seconds
- Targeted tests and builds: 5-15 minutes
- Full workspace validation: 15-30 minutes
- VM bootstrap and privileged acceptance: 30-60 minutes

Prefer repository timeout wrapper:

```bash
./scripts/run-with-timeout.sh <duration> <command> [arguments...]
```

Wrapper preserves command status/output; deadline returns 124 after process-tree TERM and `CHRONICLE_TIMEOUT_GRACE_SECONDS` (default 5), then KILL. Hierarchy defaults: command 900s, readiness command/readiness 10s/180s, service command 30s, scenario 300s (600s quota/retention, 900s cargo-heavy), acceptance cleanup 180s, acceptance profile 3300s under `validate.sh`, gate 3600s. Override with `CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS`, `CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS`, `CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS`, and gate timeout variables. Multipass knobs: `CHRONICLE_ACCEPTANCE_GUEST_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_STATUS_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_VM_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_TRANSFER_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_BOOTSTRAP_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_REMOTE_TIMEOUT_SECONDS`; guest and remote deadlines must remain shorter than host profile deadline.

Agent recorder workflow: run `validate.sh fast`, then targeted changed-path/recorder tooling, then bounded scenario or lifecycle validation. Run complete P1/P2 gate only when required. Never start foreground daemon without wrapper and deterministic cleanup.

### Canonical validation and acceptance entrypoints

- `scripts/validate.sh` is the layered validation entrypoint (`fast`, `targeted`, `gate p1|p2`, `release`); `scripts/validation.py` is its selection/orchestration/reporting helper and `scripts/run-with-timeout.sh` wraps every bounded step.
- `scripts/acceptance.sh` is the ONLY user-facing acceptance entrypoint (`--profile p1|p2|all`, `--executor local|multipass`). No deprecated aliases or compatibility wrappers are retained; do not reintroduce one without a concrete external caller or documented compatibility obligation (a self-referential delegation test is not one).
- Scenario implementations live under `scripts/acceptance/lib/scenarios/` with one owner per scenario: `shared/<scenario>.sh` provides `scenario_<name>()` when behavior is identical across profiles; otherwise `p1/<scenario>.sh` / `p2/<scenario>.sh` provide `scenario_p1_<name>()` / `scenario_p2_<name>()`. Never add per-profile extension forwards for identical behavior; `scenario-dispatch.sh` and `report.py` validate this convention, so a missing implementation fails before execution.
- Runtime/environment setup stays in `scripts/acceptance/lib/profile-{p1,p2}.sh`, `lib/multipass.sh`, and `acceptance/recorder-readiness.sh`; scenario files hold assertions only. Scenario sets, order, and timeouts are owned by `scripts/acceptance/scenarios.toml` and must not change casually.

Acceptance evidence is content-addressed, not commit-addressed. Acceptance-sensitive content, fingerprint, or compatibility changes invalidate evidence; commit SHA changes alone do not. Retain commit/tree SHA as provenance and preserve same-run source-mutation checks, but reuse compatible P1/P2 evidence across rebases, equivalent recommits, OpenSpec archives, documentation-only commits, and unrelated changes outside validation fingerprint. Every release request still requires a clean, identifiable current checkout that remains unchanged through validation; never compare current commit/tree with historical evidence identity.

## Linux-only validation on macOS

If the host OS is macOS and the task requires Linux-specific validation:

1. Check for Multipass with `command -v multipass`.
2. Reuse a suitable Ubuntu VM from `multipass list` when available.
3. Otherwise, create an Ubuntu 24.04 VM and mount the repository.
4. Run Linux-only checks inside the VM.
5. Do not mark Linux-only tasks complete based only on macOS results.
6. If Multipass is unavailable, report the blocker and continue with portable checks only.

## Specification, Testing, and Acceptance Responsibilities

### OpenSpec / SDD workflow

OpenSpec MUST be used as the SDD workflow for capturing requirements, documenting design decisions, defining implementation scope, decomposing work into tasks, recording acceptance criteria, validating OpenSpec document structure and consistency, and archiving completed changes.

OpenSpec validation proves only that specification artifacts are structurally and semantically valid according to OpenSpec rules. It MUST NOT be treated as proof that Chronicle production functionality works.

### Automated test responsibilities

Unit, integration, property, and rootless end-to-end tests SHOULD be the primary evidence for normal implementation correctness. They cover deterministic behavior, component contracts, regression protection, failure paths, platform-independent workflows, and behavior that does not require privileged Linux runtime evidence.

### Privileged acceptance responsibilities

Privileged acceptance MUST validate only behavior requiring the supported production-like Linux environment, including as applicable Ubuntu 24.04, supported kernel, cgroup v2, BTF, bpffs, required Linux capabilities, real eBPF load and attachment, real network capture, WAL persistence and recovery through the privileged path, ETL from captured evidence, inspect, isolated replay, process/cgroup/eBPF cleanup, and retained machine-readable evidence recording originating commit provenance and compatible environment.

Privileged acceptance MUST NOT become repository lint, documentation consistency checking, an OpenSpec validator, or a duplicate CI suite. OpenSpec validation and repository consistency checks belong to their SDD or repository-validation workflows, not to privileged runtime evidence.

### Task completion rules

- A task MAY be marked complete only from evidence explicitly required by that task.
- OpenSpec task checkboxes are progress records, not proof by themselves.
- A task requiring privileged supported-Linux evidence MUST NOT be checked solely because unit tests, rootless tests, implementation code, or OpenSpec validation pass.
- A privileged task SHOULD be checked only when retained evidence exists for the required environment and scenario.
- A fast development acceptance mode MUST NOT be treated as complete retained evidence unless the task explicitly permits it.
- Removing OpenSpec validation from privileged acceptance does not weaken runtime validation; OpenSpec validation and runtime validation prove different things.

## Crate Architecture (mandatory)

Chronicle's 13-crate workspace is a dependency-direction contract. Every crate has ONE primary owner (detailed in `docs/architecture/crate-boundaries.md`); `validation/architecture.toml` is the executable mirror of the allowed normal/dev/build edges and critical forbids. When you change any Chronicle Cargo manifest, update the architecture documentation, `AGENTS.md`, and policy in the same change, then run the bounded architecture check.

### Primary ownership

- `chronicle-common`: transport-neutral shared primitives (IDs, timestamps, endpoints).
- `chronicle-canonical`: protocol-independent canonical recording/replay model + validation.
- `chronicle-capture`: protocol-neutral capture evidence, socket/payload/loss metadata, fixtures.
- `chronicle-capture-ebpf`: Linux eBPF/kernel interaction; only normalized capture events cross outward.
- `chronicle-wal`: append-only durable framing, commit authority, recovery, manifests, retention.
- `chronicle-session`: ordered bidirectional stream reconstruction and loss handling.
- `chronicle-protocol`: detector/decoder/canonicalizer/replay/verifier SPI and registry.
- `chronicle-protocol-builtins`: concrete protocol implementations; must never be depended on by protocol core.
- `chronicle-etl`: complete Extract-Transform-Load from evidence through canonical publication (keeps storage).
- `chronicle-storage`: persistence abstractions + filesystem/in-memory implementations.
- `chronicle-replay`: replay planning, execution, verification (no capture/WAL/ETL knowledge).
- `chronicle-application`: user-facing use-case composition (record/recorder/ETL/replay/inspect/doctor).
- `chronicle-cli`: argument parsing, application dispatch, rendering/writing, exit mapping.

### Dependency direction

- Allowlist: see `validation/architecture.toml` `[normal]`/`[dev]`/`[build]`. Unlisted workspace edges are forbidden.
- Forbidden patterns: dependency on `chronicle-cli`; `chronicle-session -> chronicle-wal`; `chronicle-protocol -> chronicle-protocol-builtins`; `chronicle-common` depending upward; any `chronicle-cli` Chronicle edge except `chronicle-application` (in every dependency kind).
- Dev/build edges cannot bypass normal-layer architecture.
- Optional and target-specific declarations are checked independent of host platform.

### Rules

- **Application/CLI**: CLI communicates only through application-owned requests/results/errors/rendering; no protocol decoding, replay policy, WAL scanning, ETL orchestration, or eBPF loading in CLI.
- **ETL is complete Extract-Transform-Load**: it owns storage publication and checkpoint ordering; never refactor it to transform-only.
- **eBPF privacy**: Aya handles/kernel ABI stay private to `chronicle-capture-ebpf`.
- Adding a legitimate dependency requires updating `docs/architecture/crate-boundaries.md`, `AGENTS.md`, and `validation/architecture.toml` together, then running the architecture check.

## Test Architecture

Every significant test is classified as exactly one functional layer, and the
lowest-cost layer that conclusively proves the behavior wins:

- **unit** — local logic (functions, modules, parsers, state machines); colocated under `crates/<crate>/src/**`.
- **integration** — public crate/component contracts; normally `crates/<crate>/tests/`.
- **smoke** — shipped executables start or reject invalid startup sanely (`tests/smoke/`).
- **acceptance** — one documented user-facing feature through public surfaces (`tests/acceptance/`).
- **end-to-end** — important invariants across the complete composed path (`tests/e2e/`).
- **privileged** — only behavior requiring real supported Linux/kernel/environment behavior (real eBPF load/attach, cgroup, BTF, kernel, ring buffer, crash/reboot); always the last resort.

Privileged execution, P1, and P2 are NOT layers: P1/P2 are gate selectors that
map milestone obligations to classified tests. Privileged acceptance proves
only kernel/environment-dependent behavior and does NOT own portable
correctness. Portable assertions (WAL encoding/checksums, deterministic
recovery/corruption, ETL transforms, CLI parsing/rendering, protocol parsing,
replay matching, repository checks) MUST NOT be privileged proof merely
because a Multipass profile can run them.

Root black-box tests live under `tests/{smoke,acceptance,e2e}/`; acceptance
and E2E tests MUST NOT import internal crate implementation details. Shared
black-box infrastructure lives under `tests/support/` (including the HTTP
workload/replay driver at `tests/support/http_driver.py`); root layers never
import another root layer for shared helpers. Internal command/internal
compatibility seams are Integration contracts owned by the appropriate
application/CLI boundary, never Smoke or Acceptance: application ETL behavior
is `integration:etl-contract` owned by chronicle-application, while the
internal ETL CLI seam is covered by the CLI-owned `integration:cli-contract`
(chronicle-cli); `chronicle internal etl` is not public ETL acceptance, and
an internal command is never the sole evidence for a documented user-facing
capability. Privileged execution runs bounded preflight first (`scripts/privileged/preflight.py`,
schema in `validation/test-architecture/privileged-preflight-schema.toml`):
`supported`, `unsupported_environment`, `infrastructure_error`, `not_checked`;
unsupported/infrastructure never count as product regression and never
satisfy gate evidence. Validation scripts (`scripts/validate.sh`,
`scripts/validation.py`) select/orchestrate/report classified tests and contain
no product test logic. Repeated helpers are language-local
(crate-local or `tests/support/`); no new shared workspace crate without
demonstrated multi-crate need plus architecture/policy updates. Crate-boundary
constraints and migrations are tracked in `validation/test-architecture/`
(catalog, ledger, gate coverage, decisions); see its README for exact rules.
