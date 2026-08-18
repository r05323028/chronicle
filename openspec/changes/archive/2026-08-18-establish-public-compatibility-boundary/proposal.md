## Why

Chronicle is crossing its first public-release boundary, but active architecture and OpenSpec policy still describe schemas as unreleased mutable MVP artifacts and leave the compatibility freeze implicit. Release consumers need an explicit 0.1 contract for durable recordings, replay safety, CLI behavior, and release tooling before `v0.1.0` is tagged.

## What Changes

- Establish the `0.1` public compatibility boundary and define frozen contract scope for WAL v1 framing/commit markers, intentionally persisted Capture Event v1, Canonical Session v1, Session Manifest v1, replay safety/authorization, documented public CLI behavior, and documented machine-readable output schemas.
- Define 0.1.x reader/writer compatibility, fail-closed unsupported-version behavior, migration requirements, deprecation policy, and the OpenSpec process for future incompatible contract lines.
- Retire `mvp-schema-versioning` as an active capability and update dependent specifications and architecture wording to use the released compatibility policy.
- Convert current and 0-1 English, Traditional Chinese, and Japanese installation documentation to released-state wording without widening support claims; add a semantic guard that rejects contradictory pre-release release-state text in committed snapshots.
- Pin `shaftoe/pi-coding-agent-action` to the verified full commit SHA for `v2.27.0` and require full SHA immutability in workflow validation while preserving dry-run and least-privilege publication gates.
- Remove the unused scaffold `ApplicationError` variant, make installer dependency documentation/preflight checks truthful, and leave dependency-policy wiring unchanged when `cargo-deny` is unavailable rather than adding fragile setup.
- Obtain and retain fresh release acceptance evidence for the documented support matrix, or narrow claims when an environment cannot be demonstrated.

## Capabilities

### New Capabilities

- `public-compatibility-boundary`: Public 0.1 contract scope, compatibility behavior, unsupported-version handling, migration, deprecation, and future incompatible-change policy.

### Modified Capabilities

- `mvp-schema-versioning`: Retire the unreleased single-model/freeze-gate capability after replacing it with the public 0.1 policy.
- `runnable-http-cli`: Reference the released public command and JSON compatibility policy instead of an unreleased freeze gate.
- `http11-operations`: Rename Canonical MVP schema terminology and state Canonical Session v1 compatibility behavior.
- `local-session-artifacts`: Rename Canonical MVP terminology and align persisted session compatibility language.
- `shell-installer`: Make required command dependencies and preflight behavior explicit.
- `layered-validation`: Require semantic release-snapshot state validation alongside version-directory checks.
- `release-packaging`: Define immutable third-party action pinning as a full commit-SHA requirement.

## Impact

Affected documentation and specs include `README.md`, `docs/architecture/overview.md`, current/latest and `0-1` website documentation in all supported locales, and active OpenSpec specifications. Affected tooling includes `website/scripts/verify-release-docs.mjs`, `.github/workflows/release.yml`, `scripts/tests/validation/test_release_workflow.py`, `install.sh`, validation wiring, and the application error enum. No new product command, storage backend, package channel, replay capability, or compatibility adapter is introduced. Privileged validation may require an x86_64 Linux environment; evidence is retained through the existing acceptance fingerprint/evidence model.
