# Gate A kernel capture feasibility

Validated on Ubuntu 24.04, Linux 6.8.0-136-generic, aarch64, cgroup v2, BTF, Aya 0.14.0/aya-ebpf 0.2.1, privileged root.

```bash
(cd ebpf-feasibility && cargo build --release)
sudo -E cargo test -p chronicle-capture-ebpf --test privileged_feasibility -- --ignored --nocapture
```

Retained machine evidence: `gate-a-ubuntu-24.04-kernel-6.8-aarch64.json`.

## Endpoint-evidence acceptance

Verified 2026-07-29 in Multipass VM `chronicle-ubuntu`: Ubuntu 24.04.4 LTS (`ubuntu:24.04`), Linux `6.8.0-136-generic`, `aarch64`, cgroup v2 (`cgroup2fs`), readable `/sys/kernel/btf/vmlinux`, privileged root, bpftool 7.4.0, Cargo 1.97.1, and rustc 1.97.1. Embedded eBPF object SHA-256: `db025f8055b27b7281cf6b6271c4ec0b1040452d8a4afe28be2cbeebd14ef32bc`.

```bash
CARGO_TARGET_DIR=/tmp/chronicle-target-ubuntu \
  cargo test -p chronicle-capture-ebpf --features linux-ebpf \
  --test privileged_adapter --no-run
sudo "$(find /tmp/chronicle-target-ubuntu/debug/deps -type f -executable \
  -name 'privileged_adapter-*' | head -1)" \
  --ignored --nocapture --test-threads=1
```

Result: 3 passed—IPv4 active establishment, IPv4 passive establishment, and IPv6 active establishment. This adds no compatibility claim for any other environment.

## Verified matrix

Only `ubuntu:24.04`, Linux `6.8.0-136-generic`, `aarch64`, cgroup v2, and BTF is verified. The report encodes this in `verified_environment` and marks every other target, including `x86_64`, `not_verified` in `target_compatibility_matrix`.

Not verified: other distributions, kernel minor versions, cloud-provider kernels, physical NIC behavior, and production offload characteristics. This report does not claim Linux 6.1+ support.

## Proven

- `cgroup/connect4`, `cgroup/connect6`, `sockops`, and cgroup-skb ingress/egress attach and detach cleanly.
- Socket cookie correlates IPv4/IPv6 connect, lifecycle, request, and response evidence.
- Connect hooks provide host-visible PID/TGID and stable descendant cgroup ID.
- Cgroup-skb provides direction, tuple, TCP sequence, and plaintext payload.
- Veth traffic retains direct GSO/GRO metadata (`gso_size=1448`, up to 10 segments) and proves `bpf_skb_load_bytes` reaches nonlinear bytes beyond 52-byte linear heads. Separate 32 KiB loopback aggregates reconstruct exactly as two correlation-bearing 16 KiB continuations; four continuations bound observations through 64 KiB.
- Eight MiB ring saturation increments cumulative per-CPU loss counters. Complete four-CPU snapshots use boot-monotonic 100 ms-or-later intervals plus mandatory final sample.
- Selected parent cgroup covers descendant workload while direct TGID and descendant counts remain separate.

## Unsupported or non-authoritative

- `bpf_get_current_pid_tgid` is unavailable to sock-ops and is not meaningful for cgroup-skb; join process identity from connect evidence by socket cookie.
- `bpf_get_current_cgroup_id` from cgroup-skb execution is not socket-owner identity; use connect evidence.
- Sock-ops state changes alone do not distinguish reset from orderly close or prove directional half-close. Preserve raw state and report unknown unless another proven signal exists.
- TLS plaintext is not visible.
- Report proves tested matrix only; other Linux kernels/architectures require same privileged test.
