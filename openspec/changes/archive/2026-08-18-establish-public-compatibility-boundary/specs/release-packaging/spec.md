## ADDED Requirements

### Requirement: Third-party release actions use immutable commit references

The release workflow SHALL reference the third-party `shaftoe/pi-coding-agent-action` at the verified full 40-character commit SHA corresponding to the intended stable `v2.27.0` release. A semantic-version tag, branch, abbreviated SHA, or other moving reference SHALL fail workflow validation. The action step SHALL retain least privilege and receive `OPENCODE_GO_API_KEY` only where release-note generation requires it.

#### Scenario: Verified action pin

- **WHEN** the release workflow is checked before publication
- **THEN** the Pi action reference is a full commit SHA and resolves to the intended `v2.27.0` release commit

#### Scenario: Version tag is rejected

- **WHEN** a maintainer changes the Pi action reference to `v2.27.0`, `v2`, a branch, or an abbreviated SHA
- **THEN** the release workflow validation fails before release publication
