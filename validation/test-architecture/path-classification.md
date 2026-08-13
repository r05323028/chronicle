# P1/P2 path classification (task 5.4) + privileged E2E definition (task 5.3)

Every existing full P1/P2 scenario is classified as privileged capture
integration, privileged acceptance, privileged E2E, or split coverage.
Classification confirms the 5.4 acceptance: real eBPF attach/capture, cgroup
filtering, process attribution, bpftool cleanup, supported Ubuntu
compatibility, and reboot remain privileged; portable replay/CLI/ETL/WAL
assertions move out (executed in tasks 3.2-3.6, 9.1-9.4).

| Scenario | Capabilities | Classification | Privileged core | Portable moved out |
| --- | --- | --- | --- | --- |
| capture-basic | ebpf, privileged | privileged capture integration + acceptance (split) | real eBPF load/attach, ring-buffer capture, process attribution | WAL/ingest cargo invocations -> unit (3.2); fmt/build -> fast/CI |
| checkpoint-kill-restart | crash-recovery | privileged acceptance | real recorder kill/restart crash recovery | checkpoint matrix -> etl_contract + checkpoint unit |
| cli-compatibility | compatibility, ebpf | split | privileged_signal (real signal handling) | CLI grammar/exit mapping -> cli_contract integration |
| corruption-quarantine | corruption | privileged acceptance | real WAL corruption quarantine on recorded evidence | corruption matrices -> etl_contract + wal_matrix |
| incremental-etl | etl | privileged acceptance | real recorded-WAL ETL | incremental/one-shot/checkpoint matrices -> etl_contract + incremental unit |
| quota-pressure | quota | privileged acceptance | real quota enforcement under pressure | recorder_quota unit (portable) |
| reboot-recovery | persistent-state, reboot | privileged acceptance | real reboot handoff + evidence sealing | (no portable duplicate; reboot is genuinely non-simulatable) |
| recorder-readiness | btf, cgroup-v2, systemd | privileged acceptance | real systemd activation, cgroup v2 placement, readiness specialization | recorder_lease unit (portable) + generic-wait non-reinterpretation test |
| replay | replay | split + **privileged E2E candidate** | real replay of captured session, cgroup scope enforcement | replay matrix -> chronicle-replay unit (3.4); cgroup decision -> cgroup_selection unit |
| resource-cleanup | cleanup | split | bpftool/kernel/process/cgroup cleanup on real environment | fmt/check -> fast; cgroup matrix -> cgroup_selection unit |
| retention-interruption | crash-recovery, retention | privileged acceptance | real retention interruption on recorded WAL | retention fault matrix -> chronicle-wal unit + wal_matrix |
| user-intent-lifecycle | cgroup-v2, ebpf, replay | privileged acceptance + **privileged E2E candidate** | real capture + cgroup filtering + replay intent lifecycle | user-intent CLI contracts -> rootless suites + cli_contract |
| wal-recovery | wal | split | real WAL recovery after reboot/crash | wal_fault_matrix + recovery/corruption/retention -> chronicle-wal unit + wal_matrix |

## Privileged E2E definition (task 5.3, definition part)

Small privileged E2E candidates: the `replay` and `user-intent-lifecycle`
scenarios' composition portions. Unique privileged invariant (per 5.3
acceptance): a REAL workload through REAL eBPF capture feeds the replayable
pipeline (WAL -> ETL -> canonical -> replay -> verification), and
kernel/process/cgroup resources clean up. Lower-layer downstream matrices
(WAL encode/checksum, ETL transforms, replay matching, CLI rendering) are NOT
rerun inside the privileged E2E; they are proven by the rootless crate suites
and referenced by gate coverage. Implementation requires supported
privileged Linux (CAP_NET_ADMIN/SYS_ADMIN/BPF + tooling); recorded as
pending under task 5.3/10.4.

## Supported-environment evidence (task 7.4, portable verification)

Preflight probe (`scripts/privileged/preflight.py`) run inside the supported
Ubuntu 24.04 VM (kernel 6.8.0-136, cgroup v2, BTF, bpffs) classified 8/10
probes supported; `privileges` (non-root, no caps) and `hooks_tooling` (no
cargo/bpf-linker in the runtime VM) correctly reported unsupported with
remediation; outcome `unsupported_environment` (exit 78), never product
failure. Evidence retained (non-reusable) at
`validation/test-architecture/evidence/preflight-ubuntu-2404-vm.json` with
commit provenance. This validates the supported-environment compatibility
contract on a real supported OS; running privileged gates additionally needs
privileges + build tooling in the VM (10.4).
