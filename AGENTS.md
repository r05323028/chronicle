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
