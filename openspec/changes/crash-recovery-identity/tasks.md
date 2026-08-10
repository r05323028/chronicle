# Tasks

- [x] 1. Make `finish_reconstructed` deduplicate deterministic operation identities session-globally. Acceptance: the only dedup site that is per-connection becomes global; incremental batch and `finalize` paths are already global and unchanged.
- [x] 2. Add a deterministic regression test where two reconstruction connections carry the same envelope sequence + payload. Acceptance: the ETL output keeps the duplicate identity exactly once and `session.validate()` passes; the test fails before the fix (asserts unique ids and valid session).
- [x] 3. Run bounded fast/targeted validation. Acceptance: `validate.sh fast` green (fmt, clippy, unit/contract/integration suites).
- [x] 4. Re-run the full retained P2 privileged gate (pre-reboot and post-reboot phases) on supported Linux. Acceptance: both phases pass, all 36 P2 checks green including checkpoint-kill-restart crash matrix, with retained evidence at `target/validation-evidence/acceptance/p2/a21b3b1b…/20260810T143000Z-968b3dc`.
