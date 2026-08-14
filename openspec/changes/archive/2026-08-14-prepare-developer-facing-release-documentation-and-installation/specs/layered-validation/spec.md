## ADDED Requirements

### Requirement: Documentation and installation command truthfulness

Portable validation SHALL run a rootless documentation-command contract that exercises the built binary's documented public surface: `--version`, `--help` (five public commands, internal mechanics hidden), `doctor` (non-destructive with actionable output), `list` empty contract, public record-syntax preflight failure with no mutation on a non-feature build, and fixture-recorded public inspect/replay forms where rootless-compatible. The contract SHALL execute commands and assert contracts; it SHALL NOT match README prose. The contract SHALL run in the `cli_docs` targeted group and in CI; README relative links SHALL resolve. Privileged P1/P2 acceptance SHALL include a quick-start scenario executing the exact documented command-mode forms (`chronicle doctor`, `chronicle record --name checkout -- ./my-app`, `chronicle list`, `chronicle inspect checkout`, `chronicle replay checkout -- ./my-app`) against a release-built `linux-ebpf` binary, asserting the published recording, catalog entry, inspectable session, replay verification, and no recorded-destination contact. Documentation-only changes SHALL still select no privileged gate.

#### Scenario: Rootless documentation contract

- **WHEN** the portable layer runs the documentation-command contract
- **THEN** version/help/doctor/list contracts pass, record-syntax preflight fails safely with typed error and no mutation on a non-feature build, and README relative links resolve

#### Scenario: Privileged quick-start scenario

- **WHEN** the P1 or P2 gate runs the quick-start scenario on supported Linux with a release-built binary
- **THEN** the documented record -> list -> inspect -> replay forms succeed and produce retained machine-readable evidence

#### Scenario: Docs-only change stays portable

- **WHEN** only documentation paths change
- **THEN** targeted mode selects the documentation-command contract and no privileged gate, preserving existing evidence-economy rules
