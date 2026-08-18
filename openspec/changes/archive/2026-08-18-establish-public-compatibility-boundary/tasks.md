## 1. Public compatibility policy

- [x] 1.1 Update post-release architecture and WAL documentation to describe the 0.1 compatibility boundary, frozen contracts, reader/writer expectations, unsupported-version failure, migration, deprecation, and future incompatible-change process without freezing advisory/private formats.
- [x] 1.2 Sync the new `public-compatibility-boundary` capability and retire `mvp-schema-versioning`; update active CLI and canonical/session specifications so no active spec names the repository as an unreleased MVP or points to the old freeze gate.

## 2. Released documentation state

- [x] 2.1 Update `README.md` installation/status wording to present the stable GitHub Release installer as supported, while preserving Linux-only live capture, x86_64/aarch64 artifacts, plaintext HTTP/1.1, no TLS decryption, no Docker/Kubernetes packaging, and no PostgreSQL/S3 persistence claims.
- [x] 2.2 Update current/latest and `0-1` English, Traditional Chinese, and Japanese website installation pages to released-state wording with the same support scope and installer/checksum behavior.
- [x] 2.3 Add semantic release-snapshot contradiction checks to `website/scripts/verify-release-docs.mjs` and prove the guard fails on stale English/localized fixtures or controlled text and passes the released snapshots.

## 3. Release workflow hardening

- [x] 3.1 Replace the Pi action version tag with verified full SHA `0698813906fb1f23425bd742510e3080d624840d` for `v2.27.0`.
- [x] 3.2 Change `scripts/tests/validation/test_release_workflow.py` to require a full 40-character commit SHA and assert the intended action commit, while retaining dry-run publication gating, read-only preparation jobs, secret scoping, and one publish write permission.
- [x] 3.3 Run focused workflow/documentation tests and inspect the workflow's prepared-release contract for both target archives, `SHA256SUMS`, and `release-notes.md`.

## 4. Small release-boundary cleanup

- [x] 4.1 Remove unused `ApplicationError::NotImplemented` and verify no dead callers or tests remain.
- [x] 4.2 Align `install.sh` header, preflight checks, installer spec, and portable tests for `uname`, `grep`, `head`, `cut`, `awk`, `mktemp`, and SHA-256 dependencies without adding a new installation feature.
- [x] 4.3 Check `deny.toml` enforcement; if `cargo-deny` is not already available and pinned, record the portable/release validation decision without adding fragile installation wiring.

## 5. Support-matrix evidence

- [x] 5.1 Run fresh full production eBPF acceptance on x86_64 Linux, including the documented user lifecycle and release-sensitive scenarios, with `--no-reuse` and retained bounded evidence.
  - Evidence: workflow run `32175100105`; clean candidate `c751e1d6f07f83b38fce1415c648c1d4c610d1fe`; Ubuntu 24.04 Multipass guest, kernel `6.8.0-137-generic`, x86_64; 14/14 scenarios passed with `release_eligible=true`. Retained at `target/validation-evidence/release-final-x86/` with fingerprint `ebe6ce75c710b1130515e30ebae6e6b302c05be802e029162f4a1d28e2fc6dcf`.
- [x] 5.2 Classify aarch64/arm64 and kernel/userspace evidence against the documented Linux 6.1+ matrix; validate a representative older userspace/kernel when practical, or narrow claims without fabricating support evidence. (Fresh release proof: Ubuntu 24.04, Linux 6.8, aarch64; Linux 6.1 minimum and other architectures remain separately classified.)

## 6. Final release validation

- [x] 6.1 Run `graphify update .` after source changes and execute focused tests plus fmt, warnings-denied Clippy, all-feature locked workspace tests, strict OpenSpec validation, ownership, architecture, and website localization/version/build checks.
- [x] 6.2 Run `./scripts/validate.sh fast` and fresh `./scripts/validate.sh release` from the final candidate checkout; do not reuse stale privileged evidence unless fingerprints prove validity. (Fast passed; clean candidate `d019b77ea90bebc1812bc6e7cf363c0392c691e1` release passed with fresh release-eligible aarch64 evidence; gated workflow `32175100105` passed with fresh x86_64 evidence.)
- [x] 6.3 Validate the existing workflow-dispatch dry-run preparation path and inspect the prepared bundle/checksum manifest/archive contents without creating a tag or GitHub Release. (Final gated run `32175100105` passed; publish skipped; both archives, SHA256SUMS, and release-notes.md verified.)
- [x] 6.4 Verify final diff, active-spec/documentation consistency, acceptance evidence provenance, and OpenSpec completeness; run change verification and archive the completed change. Final x86 and aarch64 release evidence retained; active and archived OpenSpec validation complete.
