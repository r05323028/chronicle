## ADDED Requirements

### Requirement: Doctor data-directory probing and actionable remediation

Doctor SHALL resolve the public data directory per `recording-identity`, report its source, and non-destructively assess existing-directory access or prospective creation/private-mode support from the nearest existing ancestor. Results SHALL be `supported`, `unsupported`, or `not_checked`; doctor SHALL NOT claim conclusive writability/private-mode support when metadata alone cannot prove it. Every probe SHALL carry a stable code, status, safe message, and an actionable remediation string that states what the user can do — for example installing capabilities, mounting cgroup v2, enabling BTF, fixing directory permissions, or adjusting replay configuration. Doctor SHALL remain strictly non-destructive: it SHALL NOT start capture, create recording metadata, repair state, send replay traffic, mutate WAL/session data, or change system configuration; a probe failure SHALL NOT prevent independent probes from running.

#### Scenario: Data directory probed

- **WHEN** doctor runs with no explicit paths
- **THEN** it reports resolved path/source plus supported/unsupported/not-checked access status without creating it

#### Scenario: Actionable remediation

- **WHEN** a required probe fails, such as missing BTF or insufficient capability
- **THEN** the probe report includes a remediation string naming the concrete fix and doctor exits per the required-probe status

#### Scenario: No side effects

- **WHEN** doctor completes on any environment
- **THEN** no capture, repair, replay traffic, metadata creation, or persistent probe artifact remains

#### Scenario: Probe independence

- **WHEN** one required probe fails but other probes are independent
- **THEN** doctor continues and reports every independent probe result

#### Scenario: Prospective access is uncertain

- **WHEN** nonexistent path access/private-mode support cannot be proved from nearest existing ancestor
- **THEN** doctor reports `not_checked` with remediation and creates no probe artifact
