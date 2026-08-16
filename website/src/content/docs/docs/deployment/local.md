---
title: Local Linux
description: The supported deployment shape for live capture and replay.
---

Chronicle's supported live-capture deployment is a local Linux host. Release binaries target `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`; the capture adapter requires kernel and capability support in addition to the binary.

## Readiness checklist

```bash
chronicle doctor
```

Live capture needs:

- Linux 6.1+;
- cgroup v2;
- BTF at `/sys/kernel/btf/vmlinux`;
- little-endian x86_64 or aarch64;
- `CAP_BPF` and `CAP_NET_ADMIN` for the recording process;
- embedded eBPF programs in the binary;
- a workload emitting bounded plaintext HTTP/1.1.

Use `doctor --format json` through the global option when an automation layer needs stable probe data:

```bash
chronicle --format json doctor
```

## Local data and capacity

Recordings, WAL segments, manifests, checkpoints, and canonical payloads remain in the resolved local data directory. Omitted `--duration` means no whole-recording deadline; explicit deadlines are independent from bounded epoch/segment WAL limits. A parent recording may own many epochs without a total-WAL cap. Plan disk and file permissions around captured data, not just binary size.

## Deployment boundaries

Chronicle does not currently ship Docker or Kubernetes packaging, an always-on distributed capture plane, PostgreSQL/S3 persistence, or remote artifact publication. Those are future concerns, not deployment instructions for 0.1.x.

For a supervised command, use:

```bash
chronicle record --name checkout -- ./my-app
```

For an existing process or cgroup, use `--pid PID` or `--cgroup PATH`; Chronicle does not terminate those workloads. Replay remains loopback-only and explicitly authorized.
