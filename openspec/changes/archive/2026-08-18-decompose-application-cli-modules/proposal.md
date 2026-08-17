## Why

`chronicle-application/src/continuous_recorder.rs`, `chronicle-application/src/lib.rs`, and `chronicle-cli/src/main.rs` contain several unrelated responsibilities in single files, raising change risk and obscuring ownership even though their public behavior is already stable. Internal module extraction can reduce navigation cost without changing crate boundaries, public contracts, or runtime semantics.

## What Changes

- Split continuous-recorder implementation by lifecycle responsibility while keeping the existing service type and private ownership boundaries.
- Move implementation-heavy application helpers out of `lib.rs`, retaining intentional application API/composition exports.
- Split CLI parsing, dispatch, rendering, signals, and internal command handling into private modules without changing commands, defaults, schemas, output, or exit codes.
- Preserve dependency direction, visibility, replay safety, WAL formats, and all existing tests.
- Add no new crates and perform no unrelated formatting or behavior changes.

## Capabilities

### New Capabilities

<!-- None: this is a behavior-preserving internal refactor. -->

### Modified Capabilities

<!-- None: public behavior and contracts remain unchanged. -->

## Impact

- `chronicle-application` internal module layout and private imports.
- `chronicle-cli` internal module layout and private imports.
- Existing unit, integration, CLI contract, architecture, and semantic-boundary validation.
- `.openspec.yaml` will opt out of spec deltas because this change has no behavior contract change.
- No new crates, dependencies, public APIs, schemas, formats, or validation-policy changes.
