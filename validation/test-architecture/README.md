# Test architecture (validation/test-architecture/)

Machine-readable and documentary artifacts for the responsibility-based test
architecture change
(`openspec/changes/reorganize-test-suite-by-responsibility-and-runtime-boundary`).

| Artifact | Purpose |
| --- | --- |
| `inventory.py` | Regenerates `baseline.json` from the current checkout (standard library only) |
| `baseline.json` | Before-state snapshot: Rust tests, root/script tests, acceptance scenarios, embedded commands, CI, groups, evidence fields |
| `migration-ledger.toml` | Per-test/assertion classification: layer, environment, owner, gates, disposition, destination, rationale, verify |
| `gate-coverage.toml` | Scenario, legacy-check, and milestone obligations mapped to classified coverage |
| `test-catalog.toml` | Classified tests + P1/P2/release selectors + scenario/legacy obligations; validated by `scripts/validation.py catalog` |
| `boundary-review.md` | Shared-helper ownership and dependency-direction review |
| `decisions.md` | Deliberate per-suite migration decisions |
| `process-patterns.md` | Lifecycle/process pattern inventory and shared-helper justification |
| `privileged-preflight-schema.toml` | Versioned preflight contract: outcomes, exit codes, probes, remediation |

## Validation

```bash
python3 scripts/validation.py catalog --root .
python3 scripts/tests/validation/test_test_architecture.py
```

The `validation/test-architecture/**` path is owned by the `build_tooling`
validation group, so catalog/ledger changes select tooling validation rather
than privileged gates.

## Rules (exact guidance)

### Functional layers — lowest cost that conclusively proves behavior

Every significant test is exactly one functional layer; privileged execution,
P1, and P2 are NOT layers:

- **unit** — local function/module/parser/serializer/validation/calculation/state-machine logic; colocated under `crates/<crate>/src/**`
- **integration** — public crate/component contracts and meaningful deterministic combinations; normally `crates/<crate>/tests/`
- **smoke** — shipped executables start or reject invalid startup sanely (`tests/smoke/`)
- **acceptance** — one documented user-facing feature through public surfaces (`tests/acceptance/`)
- **end-to-end** — important invariants across the complete composed path (`tests/e2e/`)
- **privileged** — only behavior requiring real Linux/kernel/environment behavior (real eBPF load/attach, cgroup, BTF, kernel, ring buffer, crash/reboot). Always the layer of last resort.

### Location follows responsibility and crate ownership

Colocated unit tests stay in `src/**`; integration tests live in the owning crate's `tests/`; root black-box tests use `tests/{smoke,acceptance,e2e}/`. Acceptance/E2E tests MUST NOT import internal crate implementation details. No aesthetic directory moves.

### Privilege is an independent environment dimension

WAL encoding/checksums, deterministic recovery/corruption, ETL transformations, CLI parsing/rendering, protocol parsing, replay matching, repository format checks, and equivalent portable assertions MUST NOT run as privileged proof merely because a Multipass profile can execute them.

### Preflight outcomes

Privileged execution runs bounded preflight first and emits machine-readable outcomes (schema v1 in `privileged-preflight-schema.toml`, probe at `scripts/privileged/preflight.py`): `supported` (product assertions may run), `unsupported_environment` (probe failures + remediation; never `test_failed`), `infrastructure_error` (VM/transfer/bootstrap/executor failure; never product blame), `not_checked` (treated as unsupported for gates). Exit codes: 0/78/77/79/2 (schema `[exit_mapping]`). Neither unsupported nor infrastructure satisfies or reuses gate evidence; product failure is reported only after supported preflight.

### Gate semantics

P1/P2/release are selectors that map required milestone obligations to classified tests (`test-catalog.toml` `required.*` lists). Gates do NOT require tests to live under P1/P2 directories.

### Helper ownership

Repeated helpers are language-local and crate-local or root-suite-local (`tests/support/`), justified by at least two consumers; one-off support stays local. No new shared workspace crate without demonstrated multi-crate need + architecture/AGENTS/policy updates. Portable Cargo/repository checks and product field assertions stay out of selector/executor tooling.

### Crate-boundary constraints

No forbidden workspace dev edges (see `validation/architecture.toml`); Aya/kernel ABI stays private to `chronicle-capture-ebpf`; CLI tests use application-owned test support/fixtures rather than new CLI dev dependencies.

### Migration ledger

Every move/split/delete records one disposition (`stay`, `move`, `split`, `delete_duplicate`, `deterministic_fixture`, `make_rootless`, `remain_privileged`, `shared_lifecycle_migration`) with responsibility reason, replacement test ID/destination, dependency impact, and verification command. Required gate coverage stays mapped throughout.

## Terminology

P1 and P2 are gate/milestone selectors, not test categories or directories. Privileged acceptance proves only kernel/environment-dependent behavior; it does NOT own portable correctness. Tests are classified by functional layer + environment dimension, and live under owning paths.
