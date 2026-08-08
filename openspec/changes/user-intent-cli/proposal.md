## Why

Chronicle 0.1.x CLI exposes internal implementation mechanics — recorder daemon, WAL directories, ETL, `--source`, `--root`, and replay authorization flags — as the normal user workflow. That makes record/replay feel like operating machinery instead of using a tool. Chronicle must be usable by intent: `chronicle record -- ./my-app`, `chronicle replay latest -- ./my-app`.

## What Changes

- Replace the public surface with five intent-oriented commands: `record`, `replay`, `list`, `inspect`, `doctor`.
- `chronicle record -- COMMAND...` becomes the primary record form: supervised scope, capture of the process and in-scope descendants, internal WAL, automatic ETL/finalization, publication, stable recording ID. `--pid`/`--cgroup` remain for already-running workloads and are mutually exclusive with command execution. `chronicle record --retry RECORDING` retries recovery/finalization/publication for a recoverable recording without rerunning capture or the target. **BREAKING** relative to today's public `--source fixture|ebpf --wal-dir ...` form, which survives only as a hidden 0.1.x compatibility entrypoint.
- `chronicle replay <RECORDING> -- COMMAND...` becomes symmetrical with record: Chronicle spawns the target, infers the loopback target, replays, verifies, and prints PASS/FAIL. Explicit `--target URL` mode remains for already-running applications and is mutually exclusive with command mode. Replay safety gates are preserved; only safe inferences are automated.
- Introduce stable recording identity (`rec_<uuid>`), `latest` resolution, optional human-readable names, and a predictable default data directory — no more session ID + `--root` pairs for ordinary use.
- Add `chronicle list`; simplify `chronicle inspect <RECORDING>` / `inspect latest`; keep `chronicle doctor` first-class with actionable remediation.
- Move `recorder`, `recorder-status`, `etl`, fixture recording, and legacy record/replay flag forms to hidden deprecated compatibility entrypoints routed into the same application services (no duplicate business logic), with removal targeted for 0.2. **BREAKING** at the 0.2 boundary.
- Do not redesign authoritative WAL, ETL, canonical-session, session-manifest, recovery, or replay-core formats. Add only private/advisory recording-intent and catalog v1 files around the proven architecture.

## Capabilities

### New Capabilities

- `user-intent-cli`: Public five-command surface, record/replay/list/inspect execution models, deprecation and compatibility behavior, human/JSON output and exit contracts.
- `recording-identity`: Stable recording ID, default data-directory resolution, application-owned recording catalog, and deterministic `latest`/ID/name resolution.

### Modified Capabilities

- `runnable-http-cli`: Record/inspect/replay CLI contracts narrow to hidden compatibility entrypoints; exit/output contract extends to new commands.
- `safe-local-http-replay`: Target and effect authorization add safe inference for Chronicle-spawned local targets; explicit target mode and deny-by-default for dangerous effects unchanged.
- `recording-diagnostics`: Doctor adds default data-directory probing and actionable remediation; no side-effect guarantee strengthened.
- `continuous-recorder-lifecycle`: Domain ownership is clarified as one exact configured Chronicle data-domain lock path; device equality or subordinate state lock alone is not mutual exclusion.

Unchanged capabilities: `recording-store`, `recoverable-recording-wal`, `restartable-recording-etl`, `production-http-capture`, `local-session-artifacts`, `http11-operations`, `endpoint-provenance-reconstruction`, `fixture-recording-pipeline`, `mvp-schema-versioning`, `ebpf-capture-adapter`, `production-recorder-operation`, `p2-completion`, `layered-validation`, `bounded-validation-execution`.

## Impact

Affected crates: `chronicle-cli` (new hierarchy, thin dispatch/rendering, hidden compat entrypoints), `chronicle-application` (recording resolver and catalog, default data-directory resolution, exact shared domain-lock path resolution, command-scope orchestration, listener discovery/readiness, recovery retry, high-level record/replay workflows wrapping existing services), `chronicle-storage` (bounded session-summary reads for catalog reconciliation only; `FilesystemSessionStore` publication authority unchanged), `chronicle-common` (ID formatting/parsing helpers only), `chronicle-replay` (core planner/executor/verification unchanged; narrow policy-construction API only if required).

Affected non-code: `docs/operations.md`, `docs/replay-safety.md`, `docs/continuous-recorder.md`, `docs/continuous-recorder-runbook.md`, `docs/systemd/chronicle-recorder.service`, acceptance scenario scripts under `scripts/acceptance/lib/scenarios/`, and CLI contract tests in `crates/chronicle-cli/tests/`.

No new external dependencies. No GUI, TLS decryption, new protocols, remote orchestration, Kubernetes/Docker-specific implementation, or authoritative storage-backend redesign. New persisted catalog/sidecar formats are additive and private/advisory. Docker/Kubernetes packaging may be mentioned as future follow-up only.
