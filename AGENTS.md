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

## Specification, Testing, and Acceptance Responsibilities

### OpenSpec / SDD workflow

OpenSpec MUST be used as the SDD workflow for capturing requirements, documenting design decisions, defining implementation scope, decomposing work into tasks, recording acceptance criteria, validating OpenSpec document structure and consistency, and archiving completed changes.

OpenSpec validation proves only that specification artifacts are structurally and semantically valid according to OpenSpec rules. It MUST NOT be treated as proof that Chronicle production functionality works.

### Automated test responsibilities

Unit, integration, property, and rootless end-to-end tests SHOULD be the primary evidence for normal implementation correctness. They cover deterministic behavior, component contracts, regression protection, failure paths, platform-independent workflows, and behavior that does not require privileged Linux runtime evidence.

### Privileged acceptance responsibilities

Privileged acceptance MUST validate only behavior requiring the supported production-like Linux environment, including as applicable Ubuntu 24.04, supported kernel, cgroup v2, BTF, bpffs, required Linux capabilities, real eBPF load and attachment, real network capture, WAL persistence and recovery through the privileged path, ETL from captured evidence, inspect, isolated replay, process/cgroup/eBPF cleanup, and retained machine-readable evidence tied to a specific commit and environment.

Privileged acceptance MUST NOT become repository lint, documentation consistency checking, an OpenSpec validator, or a duplicate CI suite. OpenSpec validation and repository consistency checks belong to their SDD or repository-validation workflows, not to privileged runtime evidence.

### Task completion rules

- A task MAY be marked complete only from evidence explicitly required by that task.
- OpenSpec task checkboxes are progress records, not proof by themselves.
- A task requiring privileged supported-Linux evidence MUST NOT be checked solely because unit tests, rootless tests, implementation code, or OpenSpec validation pass.
- A privileged task SHOULD be checked only when retained evidence exists for the required environment and scenario.
- A fast development acceptance mode MUST NOT be treated as complete retained evidence unless the task explicitly permits it.
- Removing OpenSpec validation from privileged acceptance does not weaken runtime validation; OpenSpec validation and runtime validation prove different things.
