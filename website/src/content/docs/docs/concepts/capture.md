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

Command mode supervises the command. PID and cgroup modes observe an existing workload and do not terminate it. Use `--name` to give a recording a stable human reference and `--duration` for an optional whole-recording deadline. Omit it to run until source completion, explicit stop, or fatal safety failure. Capture rolls over bounded epochs without detaching the source.

## What crosses the boundary

The Linux adapter keeps Aya and kernel ABI details private. The application layer receives normalized capture events with socket identity, endpoint evidence, direction, and payload fragments. Endpoint and active/passive role evidence arrives before endpoint-free payload fragments are interpreted.

Only plaintext TCP payloads are currently useful to the HTTP/1.1 decoder. TLS ciphertext remains opaque. Capture loss is represented as evidence with a temporal loss window; Chronicle does not invent complete operations across ambiguous loss.

## Why no instrumentation?

Chronicle attaches around a command, process, or cgroup. The application does not need Chronicle SDK calls, patches, a restart into a special mode, or a protocol-specific test hook. This keeps production integration small, while the normalized evidence boundary lets fixture and eBPF capture use the same downstream pipeline.

## Bounds are part of the contract

Whole-recording duration is optional; checked values such as `10m` and `24h` set a deadline that is not reset at epoch rollover. Each epoch retains bounded WAL/segment limits, while the parent has no total-WAL cap. Capture queues remain bounded; when evidence cannot be admitted, the loss is visible rather than silently dropped.

Read [WAL](../wal/), [ETL](../../architecture/etl/), and [local deployment](../../deployment/local/) for the next boundaries.
