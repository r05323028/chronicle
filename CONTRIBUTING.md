# Contributing

Chronicle is early architecture-first software. Keep changes narrow, formats versioned, payloads redacted in logs, and capability claims honest. Rust toolchain and required components are pinned in `rust-toolchain.toml`; no external services are required for tests.

```bash
cargo fmt --all --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
```

## Commit messages

Chronicle uses [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/)
to keep history readable and usable by release tooling.

```text
<type>[optional scope][!]: <description>

[optional body]

[optional footer(s)]
```

- `feat` — new feature; `fix` — bug fix.
- Common additional types: `build`, `chore`, `ci`, `docs`, `perf`, `refactor`, `style`, and
  `test`. The specification allows other types.
- Scope is optional and names the affected area: `fix(wal): reject truncated records`.
- Body and footers follow the subject after one blank line. Use trailers such as
  `Refs: #123` when useful.
- Mark breaking changes with `!` (`feat(api)!: change response shape`) or an uppercase
  `BREAKING CHANGE: ...` footer. Breaking changes can use any type.

Examples:

```text
feat(replay): add deterministic session filtering
fix(wal): reject truncated commit markers
docs: clarify release qualification
```

Commit types describe intent; they do not replace Chronicle's explicit versioning and release
qualification policy.

## Validation timeouts

Never run potentially blocking commands without a bounded timeout. Use the repository wrapper:

```bash
./scripts/run-with-timeout.sh <duration> <command> [arguments...]
```

The wrapper preserves command status/output; deadline returns 124 after process-tree TERM and
`CHRONICLE_TIMEOUT_GRACE_SECONDS` (default 5), then KILL. Suggested ranges: status/readiness
30-120s; targeted tests and builds 5-15 minutes; full workspace validation 15-30 minutes; VM
bootstrap and privileged acceptance 30-60 minutes.

Hierarchy defaults: command 900s, readiness command/readiness 10s/180s, service command 30s,
scenario 300s (600s quota/retention, 900s cargo-heavy), acceptance cleanup 180s, acceptance profile
3300s under `validate.sh`, gate 3600s. Override with
`CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS`,
`CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS`,
`CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS`,
`CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS`,
`CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS`,
`CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS`,
`CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS`, and gate timeout variables. Multipass knobs:
`CHRONICLE_ACCEPTANCE_GUEST_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_STATUS_TIMEOUT_SECONDS`,
`CHRONICLE_MULTIPASS_VM_READINESS_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_TRANSFER_TIMEOUT_SECONDS`,
`CHRONICLE_MULTIPASS_BOOTSTRAP_TIMEOUT_SECONDS`, `CHRONICLE_MULTIPASS_REMOTE_TIMEOUT_SECONDS`;
guest and remote deadlines must remain shorter than the host profile deadline.

## Pre-push validation with act

`git push` can run a fast local CI parity check before anything reaches GitHub: a
repository-managed pre-push hook invokes `act` to execute the existing portable
`checks` job from `.github/workflows/ci.yml` (fast layered validation plus
portable smoke/acceptance/rootless coverage). This reuses the GitHub Actions
workflow as the single source of truth — it is not a second validation
implementation.

```
pre-push + act      = fast local CI parity check (catch obvious regressions)
GitHub CI           = authoritative validation
release gates        = full qualification (validate.sh live-capture|recorder|release)
```

Pre-push never runs release, privileged Multipass, or eBPF runtime validation;
those stay in GitHub CI and the acceptance gates.

**Required local dependencies**

- `act` — <https://github.com/nektos/act> (`brew install act` on macOS)
- Docker with a running daemon (act executes the job in a container)
- `prek` for hook management (<https://github.com/j178/prek>), optional but
  preferred; without it the installer falls back to a symlink

**Install once**

```bash
./scripts/install-pre-push-hook.sh   # prek install --hook-type pre-push, or symlink
prek list                            # verify the push-stage hook is configured
```

Every `git push` then runs `act -j checks` (bounded by
`CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS`, default 900s). Override the job with
`CHRONICLE_PRE_PUSH_JOB`. Uninstall with `prek uninstall` or by removing the
`.git/hooks/pre-push` symlink.

**Troubleshooting**

- Missing `act` or Docker: the hook aborts the push with an actionable message
  naming the dependency, why it is required, and install guidance.
- GitHub-hosted runners preinstall Rust, but act containers do not; the hook maps
  `ubuntu-latest` to `catthehacker/ubuntu:rust-latest` (`CHRONICLE_PRE_PUSH_IMAGE`
  overrides). The first run pulls that image and rebuilds Cargo artifacts inside
  it — allow a few extra minutes; later runs reuse `~/.act` and the action cache.
- The `checks` job needs network (it installs the OpenSpec CLI) and a writable
  Cargo target directory inside the container.
- eBPF changes are compiled by the `ebpf-compile` CI job only; pre-push does not
  cover nightly/bpf-linker toolchain setup.
- When full local feedback is needed, run `./scripts/validate.sh fast` or
  `targeted --changed-since origin/main` directly; `release` remains the
  qualification gate.

The canonical local validation entry point is `./scripts/validate.sh fast` (formatting, warnings-denied Clippy, workspace tests, strict OpenSpec validation, and repository consistency checks); use `./scripts/validate.sh targeted --changed-since origin/main` for focused changed-path validation and `live-capture|recorder` / `release` for complete or release evidence. Real eBPF runtime coverage is opt-in privileged acceptance (`./scripts/acceptance.sh --profile live-capture|recorder --executor local|multipass`) on supported Linux; see the [operations guide](docs/operations/overview.md) and [architecture](docs/architecture/overview.md) for details.

## Website

Website CI/CD, GitHub Pages deployment, and committed documentation version preparation live in [website/MAINTAINERS.md](website/MAINTAINERS.md).

Protocol work belongs behind `chronicle-protocol` interfaces. Add fixtures containing no real credentials or production data. Replay examples must preserve the safety semantics of the mode being documented: command mode automatically executes permitted reads only against the Chronicle-owned supervised target; explicit-target mode remains dry-run until `--execute` and all required target/effect gates are supplied; writes always require explicit authorization. Reference environment variable names instead of embedding connection credentials. Bounded plaintext HTTP/1.1 fixture record/inspect/loopback replay is functional alongside fake, including bounded chunked responses and trusted close-delimited responses; fixture capture is one configured WAL segment with no restart repair. eBPF capture runs as opt-in privileged acceptance on supported Linux; other real protocols, PostgreSQL/S3 adapters, TLS, and broad replay remain planned.
