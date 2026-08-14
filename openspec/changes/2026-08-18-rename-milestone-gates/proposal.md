## Why

Chronicle's validation vocabulary still used development-phase milestone names: `P1` and `P2` (plus `gate p1|p2`, `--profile p1|p2`, selectors, evidence directories, and `Gate A` for the kernel capture feasibility harness). A contributor had to learn Chronicle's project history to know what `./scripts/validate.sh gate p2` validated. As the project approaches its first public release, the validation architecture should read as designed around enduring system responsibilities, not the order in which those responsibilities were implemented.

## What Changes

- Rename validation profile/gate/selector `p1` to `live-capture`: privileged Linux acceptance proving real eBPF capture through the public record/replay surface (capture-basic, WAL recovery, replay, user-intent lifecycle, CLI compatibility, documented quick start, resource cleanup).
- Rename `p2` to `recorder`: privileged Linux acceptance proving the continuous recorder's durable operation — systemd supervision and readiness, epoch rollover/rotation, incremental ETL, checkpoint crash recovery, quota pressure, retention interruption, corruption quarantine, and host reboot recovery. `--profile all` runs the recorder superset (it includes the live-capture scenarios).
- The validation CLI becomes `./scripts/validate.sh fast|targeted|live-capture|recorder|release` (the `gate` subcommand wrapper is removed; modes are the semantic scopes).
- `acceptance.sh --profile live-capture|recorder|all`; scenario implementations under `scenarios/live-capture/` and `scenarios/recorder/`; profile scripts `profile-live-capture.sh`/`profile-recorder.sh`; evidence directories and env vars follow the same names.
- Rename the active `Gate A` kernel capture feasibility harness to `capture-feasibility` terminology (feasibility test output, report path `target/capture-feasibility/`, docs). Gates B/C/D existed only in archived change artifacts and stay historical.
- `ETL_PIPELINE_VERSION` value `"p1"` becomes `"etl-v1"` (self-describing persisted format version; pre-release, no compatibility commitment; no fixtures carry the old value).
- Update README, operations, architecture, contributing, AGENTS.md (one-sentence scope descriptions), validation/test-architecture docs, CI, and the normative OpenSpec specs to the canonical vocabulary.

## Capabilities

### Modified Capabilities

- `layered-validation`: validation modes and acceptance profiles use semantic scopes (`live-capture`, `recorder`) instead of milestone gates; selectors and evidence naming follow.
- `developer-onboarding-documentation`, `production-recorder-operation`, `recording-diagnostics`, `recording-store`, `recoverable-recording-wal`, `restartable-recording-etl`, `runnable-http-cli`, and other specs drop milestone prefixes in favor of component/capability wording.
- `recorder-durability` (renamed from `p2-completion`): the durable recorder completion requirements (startup recovery, crash-safe rollover, quota, retention, corruption, privileged acceptance).

## Impact

Renamed machinery: `scripts/validate.sh`, `scripts/validation.py`, `scripts/acceptance/{runner,report}.py`, `lib/multipass.sh`, `lib/profile-{live-capture,recorder}.sh`, `lib/scenarios/{live-capture,recorder}/**`, `scenarios.toml`, `validation/groups.toml`, `validation/test-architecture/test-catalog.toml`, validation tests, `.github/workflows/ci.yml`, `test-recorder-readiness.sh`. Docs: README, CONTRIBUTING, AGENTS.md, docs/operations.md, docs/architecture.md, docs/feasibility. Code: ETL pipeline version value, crate comments, feasibility test strings. OpenSpec: specs renamed in place; `p2-completion` directory renamed to `recorder-durability`.

No validation semantics change: the live-capture and recorder gates exercise exactly the scenarios and checks the P1 and P2 gates did; evidence fingerprint/content changes only because acceptance-sensitive configuration and scenario definitions changed. No compatibility aliases are introduced (internal tooling, no external consumers, pre-1.0); the previous deprecation-wrappers policy already forbade them.

Historical references intentionally retained: archived OpenSpec changes, the retained feasibility evidence file `docs/feasibility/gate-a-ubuntu-24.04-kernel-6.8-aarch64.json` (content records the historical verification), and historical-context lines in active change design documents.
