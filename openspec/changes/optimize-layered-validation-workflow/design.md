## Context

P1 and P2 already have authoritative privileged scripts and Multipass wrappers, but wrappers clone/build per run and transfer broad artifact trees. Portable checks and OpenSpec checks are mixed with runtime acceptance. The new workflow must orchestrate existing scripts without weakening their exact-commit, environment, cleanup, or report assertions.

## Goals / Non-Goals

**Goals:**

- Select the smallest valid validation set from changed paths.
- Keep explicit complete P1/P2 and release paths.
- Use one VM instance, VM-local Cargo target/cache, and one build per invocation.
- Make evidence compact by default and preserve minimal failure reproducers.
- Reuse only valid gate evidence identified by an input fingerprint.

**Non-Goals:**

- Rewriting capture, WAL, ETL, replay, or acceptance semantics.
- Treating OpenSpec validation or rootless checks as privileged runtime proof.
- Uploading build caches or changing Cargo compilation semantics.

## Decisions

### Single shell entry point

`scripts/validate.sh` parses mode and options, runs portable checks, prints selection/reuse decisions, and delegates complete gates to existing `p1-multipass.sh` and `p2-multipass.sh`. `release` requires both gate obligations to be executed or reused; `gate p1` and `gate p2` run only the named obligation.

### Declarative dependency map

`validation/groups.toml` defines groups, path globs, commands, and gate ownership. The standalone Python standard-library helper at `scripts/validation.py`, invoked by `validate.sh`, evaluates Git paths and emits selected/skipped reasons. Documentation-only paths select no privileged group. Unknown source paths conservatively select the portable group and both affected gates only when the map marks them gate-owned.

### Fingerprints

A fingerprint is SHA-256 over canonical JSON containing gate name, sorted hashes of mapped source/acceptance/build-input paths, Cargo.lock, rustc version, target architecture, Ubuntu/kernel/BTF/cgroup capability values collected inside the privileged VM, and validation contract/config. It excludes unrelated repository paths and commit hash. Missing or changed inputs produce a different fingerprint; evidence manifests record original commit and covered checks.

### Evidence retention

Each run writes `summary.json`, `environment.json`, `manifest.json`, and `checksums.txt` only when artifacts are enabled. Failure handling copies failed logs, kernel log, reproducer, and only failure-related WAL/session paths, compressing bulky data with zstd when available. `--no-artifact` disables all copying; `--artifact-on-failure` is default outside release. Release retains successful full evidence. Cache directories never live below evidence roots.

### Multipass and caches

Wrappers reuse a named existing Ubuntu VM, verify or create the source mount at `/mnt/chronicle` before cloning, and set `CARGO_TARGET_DIR=/home/ubuntu/chronicle-target`; Cargo home remains `/home/ubuntu/.cargo`. Bootstrap installs missing packages only, with a marker for completed setup. Source is not copied onto the host mount for build output. Existing wrapper behavior remains the source of privileged coverage.

### Recorder readiness contract

The existing recorder status payload remains authority and gains a derived `state` field. It combines lifecycle, capture/processing readiness, health, and stale-owner detection into `starting`, `recovering`, `loading_ebpf`, `ready`, `degraded`, or `failed`. P2 polls this state with a bounded configurable timeout and interval. Status-command failures are logged as unavailable rather than treated as readiness; terminal failure stops immediately. Timeout diagnostics collect only status, service/journal, kernel capability, cgroup/BTF/eBPF, WAL listing, checkpoint summary, process, and disk files.

### Reuse contract

`--reuse-evidence` searches `evidence/privileged/<gate>/manifest.json` for a matching valid fingerprint and verifies manifest/checksum integrity. Output labels reused checks separately from executed checks. Reuse is forbidden for missing manifests, failed/incomplete status, acceptance-script changes, relevant source/build changes, compiler/kernel/architecture changes, or validation-contract changes.

## Risks / Trade-offs

- [Risk] A dependency-map omission could under-select validation. → Keep unknown paths conservative, require explicit release gate coverage, and test representative path classes.
- [Risk] VM-local cache can become stale. → Cargo fingerprints remain authoritative; bootstrap marker only avoids repeated package installation.
- [Risk] Failure extraction could omit useful data. → Keep failure summary and reproducer commands, select paths by failure manifest, and allow `--keep-workdir` for debugging.
- [Risk] Reused evidence may be mistaken for fresh execution. → Require explicit `reused` status and report original commit/date/environment/check list.

## Migration Plan

Add entry point and tests first, then route CI/developer documentation to `fast` or `targeted`; retain direct wrappers for compatibility. Gate and release remain opt-in until selection and fingerprint tests pass. Rollback is deleting the new entry point/config and continuing to use existing wrappers; no production data migration is required.

## Open Questions

None blocking implementation. CI provider-specific upload syntax may vary; generic retention guidance and the existing CI workflow will be updated without uploading cache directories.
