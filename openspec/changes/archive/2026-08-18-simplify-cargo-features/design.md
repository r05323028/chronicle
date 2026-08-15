## Context

Current feature chain:

```text
chronicle-cli/linux-ebpf
    ↓
chronicle-application/linux-ebpf
    ↓
chronicle-capture-ebpf/linux-ebpf
    ↓
aya / aya-obj / libc
```

Problems: (1) user-facing `--features linux-ebpf` requirement; (2) `chronicle-capture-ebpf/linux-ebpf` is redundant (the crate already is the eBPF implementation); (3) `linux` is expressible through Cargo target configuration; (4) application layer should care about a capability (live capture), not the technology; (5) README build command is surprising.

Target structure after the change:

```text
User-facing CLI (chronicle-cli)           — no features
    ↓  (target-specific dep, no feature)
Application capability: live capture      — no feature; chronicle-capture-ebpf
    ↓  (target-specific dep, no feature)     is a plain Linux dependency
Linux eBPF capture implementation         — aya/aya-obj/libc are plain Linux deps
    ↓
Aya / kernel-specific details
```

## Decision 1: remove every `linux-ebpf` Cargo feature

- `chronicle-capture-ebpf`: `[target.'cfg(target_os = "linux")'.dependencies] aya = "0.14"; aya-obj = "0.3"; libc = "0.2"` (non-optional). The crate still compiles on non-Linux: the stub `EbpfCaptureSource` and the `unavailable` preflight path are `#[cfg(not(target_os = "linux"))]`, and `abi`/`adapter` keep their existing dead-code allowance.
- `chronicle-application`: `[target.'cfg(target_os = "linux")'.dependencies] chronicle-capture-ebpf = { path = ... }` (non-optional). No `[features]` section.
- `chronicle-cli`: no `[features]` section. `tokio/signal` (used only by the Linux signal watchers) is added via `[target.'cfg(target_os = "linux")'.dependencies] tokio = { workspace = true, features = ["signal"] }`; Cargo merges features with the base dependency entry.

Rationale for not keeping an internal `live-capture` feature in the application: there is no consumer that wants a Linux build without live capture, and the application's own test builds compile the Linux modules unconditionally on Linux (the codebase already does this for `listener_discovery` under `cfg(any(target_os = "linux", test))`). A capability feature with exactly one consumer and no alternative implementation would be an abstraction added merely to rename — the task forbids that.

`chronicle-wal` keeps `test-support`: it is scoped, named appropriately, and used by application dev-dependencies.

## Decision 2: cfg expression rewrite (mechanical)

| Old | New |
| --- | --- |
| `all(target_os = "linux", feature = "linux-ebpf")` | `target_os = "linux"` |
| `not(all(target_os = "linux", feature = "linux-ebpf"))` | `not(target_os = "linux")` |
| `any(test, all(target_os = "linux", feature = "linux-ebpf"))` | `any(test, target_os = "linux")` |
| `all(target_os = "linux", feature = "linux-ebpf", target_endian = "little")` | `all(target_os = "linux", target_endian = "little")` |
| `not(all(target_os = "linux", feature = "linux-ebpf", target_endian = "little"))` | `not(all(target_os = "linux", target_endian = "little"))` |
| `all(target_os = "linux", feature = "linux-ebpf", target_endian = "big")` | `all(target_os = "linux", target_endian = "big")` |
| `any(not(target_os = "linux"), not(feature = "linux-ebpf"))` | `not(target_os = "linux")` |

The big-endian Linux guard stays a target expression: a big-endian Linux build compiles the capture crate but reports the embedded object unsupported, exactly as today.

## Decision 3: error surface

`EbpfCaptureError::FeatureDisabled` becomes unreachable (on Linux the feature is always compiled; on non-Linux the stub returns `UnsupportedPlatform`). Remove the variant; the non-Linux stub `load` drops its inner `cfg!(target_os = "linux")` branch.

## Decision 4: rootless record-failure contract

Previously a Linux build without the feature produced `record -- ...` failing with `UnsupportedLivePreflight` (exit 4). That build variant disappears. On Linux the live-capture binary fails in `preflight_command_record` → `preflight_embedded_ebpf` when the host lacks prerequisites (rootless: CAP_BPF/CAP_NET_ADMIN missing) with `ProductionPreflight` (exit 3), before any data-dir mutation or target spawn. Non-Linux builds keep exit 4. `tests/support/process.py::require_rootless_record_failure` and `tests/smoke/test_documented_commands.py` accept the platform-appropriate code while keeping the stronger assertions: empty stdout, typed JSON error, target never touched, data dir never mutated.

## Decision 5: canonical build command

`cargo build --release --locked` at the workspace root (13 crates; `ebpf` and `ebpf-feasibility` are excluded standalone workspaces). Equivalent package-specific form: `cargo build --release --locked -p chronicle-cli`. Release workflow and privileged scenarios use `-p chronicle-cli` (faster, binary-only); README shows the workspace command.

## Non-goals

No new abstraction layer for capture sources; no user-facing `live-capture` feature; no changes to `chronicle-wal` features; no rewrite of archived OpenSpec changes; no change to the eBPF program, its toolchain, or the embedded-object build path.
