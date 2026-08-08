## 1. Evidence Identity Audit

- [x] 1.1 Classify every commit/tree equality check as historical reuse/release identity, same-run integrity, or provenance-only.
- [x] 1.2 Review and narrowly strengthen validation fingerprint inputs for source, build, acceptance scenarios/runner, schema, and configuration.

## 2. Content-Addressed Reuse

- [x] 2.1 Remove commit-SHA equality from historical evidence reuse and release eligibility while retaining provenance metadata.
- [x] 2.2 Preserve clean start and unchanged end source identity checks for freshly executed release runs.
- [x] 2.3 Require compatible schema, scenario set, privileged environment, valid complete manifest, passed required checks, and no timeout/incomplete/`not_checked` evidence.

## 3. Contracts and Guidance

- [x] 3.1 Update layered-validation and P2 completion requirements to distinguish provenance/in-run integrity from reuse identity.
- [x] 3.2 Update `AGENTS.md` with content-addressed acceptance evidence rule and SHA-only rerun guidance.

## 4. Tests and Validation

- [x] 4.1 Add regression tests for cross-SHA, equivalent-content, and OpenSpec archive-only evidence reuse.
- [x] 4.2 Add rejection tests for fingerprint/scenario/environment/schema/manifest/check/timeout/incomplete incompatibility and same-run source mutation.
- [x] 4.3 Run focused acceptance/validation tests, strict OpenSpec validation, bounded `./scripts/validate.sh fast`, and graphify update.
- [x] 4.4 Inspect retained P1/P2 evidence under final compatibility rules and report whether reuse remains valid.
- [x] 4.5 Validate every release checkout before evidence lookup, cover clean cross-SHA reuse, dirty-current rejection, historical dirty provenance, development reuse, and same-run mutation, then sync and archive final semantics.
