## Why

The current README is a P1 implementation report, not a developer onboarding document. A first-time developer cannot quickly learn what Chronicle is, what it actually supports, how to install it, or how to complete a minimal record → inspect → replay workflow. Concretely:

- The primary installation path shown is the contributor workflow `cargo run -p chronicle-cli -- ...`, and every live-recording example omits the `linux-ebpf` feature, so the documented commands do not enable live eBPF capture on a default build and fail the capture preflight on Linux.
- The README leads with internal implementation detail: "Current implementation" describes hidden 0.1.x compatibility entrypoints, WAL-format and schema-version policy, sidecar/catalog mechanics, and fixture-based demo forms routed through deprecated syntax. None of that is required before a new user's first working example.
- There is no release packaging. The repository produces no downloadable binaries (only `ci.yml` exists under `.github/workflows/`, `publish = false`, no tags), so no truthful binary installation path can be documented until a minimal release workflow exists.
- The CLI has no `--version` reporting, which is required for a release-quality install experience.

The intended direction — user-intent CLI, bounded plaintext HTTP/1.1 capture through eBPF → WAL → ETL → canonical session → storage → replay, deny-by-default safety — is correct and must be preserved. This change prepares that surface for a public developer-facing release by restructuring the README and making installation and Quick Start truthful and validated.

## What Changes

- Rewrite `README.md` as the primary developer onboarding entry point, organized What → Install → Quick Start → Architecture → Details, with the intended positioning that Chronicle records real application traffic with eBPF and turns it into deterministic, replayable regression-test evidence.
- Define a public installation contract: primary installation from a released `chronicle` binary; a documented build-from-source path that explicitly enables `linux-ebpf`; and explicit platform/runtime requirements with `chronicle doctor` as the readiness check.
- Add minimal release packaging (new scope): a GitHub Actions release workflow producing `x86_64`/`aarch64` Linux binaries with the eBPF object embedded, tar.gz archives, SHA-256 checksums, GitHub Release artifacts, and `chronicle --version` reporting. No package managers, Docker, Homebrew, or crates.io publishing.
- Provide a short Quick Start on an installed binary covering `doctor` → `record` → `list` → `inspect` → `replay` command-mode; advanced forms (PID/cgroup selectors, explicit replay targets, effect gates, retry, recorder internals, hidden compatibility commands) move out of the primary path and link to `docs/operations.md`.
- Present a developer-friendly architecture overview that preserves the boundary that replay consumes canonical sessions and protocol interfaces and does not depend on capture, eBPF, WAL, or ETL.
- Replace the internal protocol matrix with a compact capability table distinguishing supported, planned, and research status; keep internal test scaffolds such as Fake out of the public-facing table.
- Keep concise safety and limitations sections and link detailed semantics to `docs/replay-safety.md` and `docs/operations.md`; reclassify the current README's P1-specific, compatibility, WAL-format, schema-version, sidecar/catalog, and migration detail into the appropriate detailed documents.
- Add validation that keeps documented installation and Quick Start commands truthful: a rootless documentation-command contract test (doctor, help, version, public CLI forms), a privileged quick-start acceptance scenario executing the exact documented command-mode flow against a release-built `linux-ebpf` binary, and release-artifact checks (version output, embedded eBPF object, checksums). No brittle prose-grep documentation tests.

## Capabilities

### New Capabilities

- `developer-onboarding-documentation`: README role and information hierarchy, product positioning, installation contract (binary primary, source build), Quick Start contract, platform requirements presentation, high-level architecture presentation with the replay boundary, protocol support presentation, safety and limitations presentation, documentation separation, and validation of documented commands.
- `release-packaging`: minimal release workflow and artifact contract — supported targets, binary naming, archive format, checksums, embedded eBPF object, version reporting, GitHub Release artifact generation, and release-artifact validation.

### Modified Capabilities

- `layered-validation`: portable validation runs a rootless documentation-command contract alongside existing checks; privileged P1/P2 acceptance includes a quick-start scenario executing the documented command-mode forms; documentation-only changes still select no privileged gate.

Unchanged capabilities: `user-intent-cli`, `runnable-http-cli`, `recording-identity`, `safe-local-http-replay`, `recording-diagnostics`, `continuous-recorder-lifecycle`, `recording-store`, `recoverable-recording-wal`, `restartable-recording-etl`, `production-http-capture`, `local-session-artifacts`, `http11-operations`, `endpoint-provenance-reconstruction`, `fixture-recording-pipeline`, `mvp-schema-versioning`, `ebpf-capture-adapter`, `production-recorder-operation`, `p2-completion`, `bounded-validation-execution`.

## Impact

Affected code: `crates/chronicle-cli` (add clap `version` so `chronicle --version` reports the workspace version; no new commands, no behavioral change to existing commands).

Affected non-code: `README.md` (restructured onboarding document), `.github/workflows/release.yml` (new minimal release workflow), `.github/workflows/ci.yml` (run the documentation-command contract and eBPF-object drift check), `tests/smoke/` (new rootless documentation-command contract test), `scripts/acceptance/lib/scenarios/` (new quick-start scenario), `validation/groups.toml` + `scripts/validate.sh` (extend the `cli_docs` targeted case with the documentation-command contract), `docs/operations.md` or `docs/architecture.md` only where README content moves there (no semantics rewrite).

No new external Rust dependencies. No changes to WAL, ETL, canonical session, storage, replay, capture, or recorder behavior. No protocol implementations, TLS decryption, HTTP/2/3, PostgreSQL/S3 runtime implementation, Kubernetes/Docker packaging, package managers, crates.io publishing, UI, arbitrary remote replay, CLI redesign, or crate-boundary changes.

Docker and Kubernetes packaging remain future follow-up only.
