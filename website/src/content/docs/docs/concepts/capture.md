---
title: Capture
description: How Chronicle observes application behavior without application instrumentation.
---

Capture is the evidence boundary. It observes socket lifecycle events and ordered payload fragments for the selected workload; it does not pretend those events are already application operations.

## Select a workload

The public `record` command supports three scopes:

```bash
chronicle record -- ./my-app
chronicle record --pid 12345
chronicle record --cgroup /sys/fs/cgroup/my-service
```

Command mode supervises the command. PID and cgroup modes observe an existing workload and do not terminate it. Use `--name` to give a recording a stable human reference and `--duration` to set a shorter bounded window.

## What crosses the boundary

The Linux adapter keeps Aya and kernel ABI details private. The application layer receives normalized capture events with socket identity, endpoint evidence, direction, and payload fragments. Endpoint and active/passive role evidence arrives before endpoint-free payload fragments are interpreted.

Only plaintext TCP payloads are currently useful to the HTTP/1.1 decoder. TLS ciphertext remains opaque. Capture loss is represented as evidence with a temporal loss window; Chronicle does not invent complete operations across ambiguous loss.

## Why no instrumentation?

Chronicle attaches around a command, process, or cgroup. The application does not need Chronicle SDK calls, patches, a restart into a special mode, or a protocol-specific test hook. This keeps production integration small, while the normalized evidence boundary lets fixture and eBPF capture use the same downstream pipeline.

## Bounds are part of the contract

One-shot recording defaults to 600 seconds and is capped at 3600 seconds. The physical WAL ceiling is 4 GiB. Capture queues are bounded; when evidence cannot be admitted, the loss is visible rather than silently dropped.

Read [WAL](../wal/), [ETL](../../architecture/etl/), and [local deployment](../../deployment/local/) for the next boundaries.
