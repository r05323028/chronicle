## Why

Chronicle currently retains fingerprinted privileged evidence but still uses exact Git commit identity in release eligibility, causing equivalent content to rerun expensive P1/P2 acceptance after rebases, archive commits, or unrelated changes. Evidence identity must follow validation-relevant content and compatibility contracts while preserving commit metadata for audit and detecting source mutation within a run.

## What Changes

- Make privileged evidence reuse and release eligibility depend on validation fingerprint, scenario contract, evidence schema, compatible environment, complete artifact manifest, and passed required checks.
- Retain Git commit/tree, dirty state, and start/end identity as provenance and same-run integrity metadata without using historical commit equality as reuse identity.
- Strengthen acceptance fingerprint inputs where needed so acceptance behavior, scenarios, schema, source/build inputs, and validation configuration remain content-addressed.
- Add regression coverage for reuse across different/equivalent commits and rejection on material content, scenario, environment, schema, manifest, timeout, incomplete, failed, or `not_checked` changes.
- Update agent guidance so SHA-only changes do not trigger privileged reruns.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

- `layered-validation`: Define content-addressed evidence identity and release reuse independently from Git commit SHA while retaining same-run source integrity checks.
- `p2-completion`: Replace exact historical commit release eligibility with compatible, complete content-addressed evidence requirements.

## Impact

Affected surfaces: `scripts/validation.py`, unified acceptance runner/report and Multipass handoff, legacy profile release checks where still authoritative, acceptance and layered-validation tests, `validation/groups.toml` fingerprint inputs, acceptance/P2 OpenSpec requirements, and `AGENTS.md`. No production runtime API or dependency change.
