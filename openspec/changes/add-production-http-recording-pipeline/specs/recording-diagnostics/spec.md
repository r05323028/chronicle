## ADDED Requirements

### Requirement: Typed diagnostic status model
Doctor SHALL mark each probe required or optional and represent it as `supported`, `supported_with_warnings`, `unsupported`, or `not_checked`, with stable code, safe message, and remediation. Aggregate precedence SHALL be: any required unsupported -> unsupported; else any required not-checked -> not-checked; else any warning or optional unsupported/not-checked -> supported-with-warnings; else supported. Required unsupported/not-checked SHALL exit 4; supported/warnings SHALL exit 0.

#### Scenario: Fully supported environment
- **WHEN** every required P1 probe passes without warning
- **THEN** doctor reports supported and exits success

#### Scenario: Warning-only environment
- **WHEN** required features pass but disk space or optional evidence triggers warning
- **THEN** doctor reports supported-with-warnings and exits success with warning summary

#### Scenario: Unsupported environment
- **WHEN** required recording feature is absent
- **THEN** doctor reports unsupported with non-success exit and remediation

#### Scenario: Required probe not checked
- **WHEN** permissions prevent required non-destructive probe from deciding support
- **THEN** probe and aggregate are not-checked and doctor exits 4 rather than claiming support

#### Scenario: Optional probe not checked
- **WHEN** optional probe cannot run but all required probes pass
- **THEN** aggregate is supported-with-warnings and doctor exits 0

### Requirement: Platform and eBPF probes
Doctor SHALL check operating system, architecture, kernel version, cgroup v2, BTF, required eBPF hooks/helpers, effective privileges/capabilities, capture object/backend availability, and attach feasibility where safely testable. It SHALL NOT leave programs attached after probe.

#### Scenario: Missing BTF
- **WHEN** `/sys/kernel/btf/vmlinux` is unavailable or unusable
- **THEN** doctor reports stable BTF unsupported code and expected remediation

#### Scenario: Insufficient capability
- **WHEN** kernel supports capture but caller lacks attach/load privilege
- **THEN** doctor reports privilege unsupported/not-checked separately from kernel support

#### Scenario: Non-Linux development host
- **WHEN** doctor runs on macOS
- **THEN** live capture is unsupported while portable ETL/inspect/replay checks still run

### Requirement: WAL and filesystem probes
Doctor SHALL check requested/default WAL path creation/writability, private permission support, advisory locking, available space warning, file/data sync, and atomic no-replace session publication behavior using private temporary data. It SHALL remove probe artifacts best-effort and SHALL not overwrite user files.

#### Scenario: Writable private filesystem
- **WHEN** directory supports required permissions, sync, lock, and atomic rename
- **THEN** storage probe reports supported

#### Scenario: Low disk space
- **WHEN** filesystem is writable but available bytes are below documented warning threshold
- **THEN** doctor reports supported-with-warnings and required free-space guidance

#### Scenario: Atomic publication unavailable
- **WHEN** filesystem cannot provide required no-replace atomic publication behavior
- **THEN** doctor reports storage unsupported

### Requirement: Protocol and replay policy probes
Doctor SHALL report registered protocol capability availability and validate configured replay safety shape without making network connections. It SHALL confirm P1 plaintext HTTP/1.1 capability, loopback-only explicit target policy, timeout, and no-redirect behavior; missing target SHALL not be an error because replay target must be supplied per command.

#### Scenario: HTTP capability available
- **WHEN** built-in registry has HTTP detect/decode/canonicalize/replay/verify implementations
- **THEN** doctor reports plaintext HTTP/1.1 supported and all other protocols at actual status

#### Scenario: Unsafe replay config
- **WHEN** configuration attempts to provide implicit execution or broaden target authorization
- **THEN** doctor reports warning/unsupported policy code and states CLI gates cannot be supplied by config

### Requirement: Safe human and JSON output
Doctor human and JSON output SHALL include aggregate status, probe statuses/codes, detected non-sensitive versions/capabilities, and remediation in deterministic order. It SHALL not print environment values, credentials, process command lines, captured data, or arbitrary file contents.

#### Scenario: JSON diagnostics
- **WHEN** user requests doctor JSON output
- **THEN** valid versioned machine-readable structure contains every probe and aggregate status

#### Scenario: Sensitive environment present
- **WHEN** runtime credential environment variable exists
- **THEN** doctor reports only presence/check status and never value

### Requirement: Doctor is diagnostic only
Doctor SHALL not create recording metadata, start capture, mutate WAL/session data, send replay traffic, repair files, or change system configuration. Failed probe SHALL not prevent independent safe probes from running unless prerequisite makes them not-checkable.

#### Scenario: One probe fails
- **WHEN** eBPF privilege probe fails but filesystem checks are independent
- **THEN** doctor continues filesystem/protocol checks and reports both results

#### Scenario: No side effects
- **WHEN** doctor completes
- **THEN** no eBPF link, recording, replay connection, or persistent probe artifact remains
