---
title: Quick start
description: Record, inspect, and safely replay a bounded HTTP/1.1 workload.
slug: 0-1/docs/getting-started/quick-start
---

This walkthrough uses command mode on a supported Linux host and a plaintext HTTP/1.1 application. Replace `./my-app` with the application you want to supervise.

## Check the host

```bash
chronicle doctor
```

Fix reported platform, cgroup, BTF, capability, or embedded-program issues before recording. `doctor` does not mutate the host.

## Record behavior

```bash
chronicle record --name checkout -- ./my-app
```

Chronicle attaches capture first, then starts the application. Recording stops when the application exits, you press `Ctrl+C`, or the duration bound is reached. While it runs, send representative requests from another terminal.

The public one-shot default is 600 seconds; the maximum is 3600 seconds. The physical WAL ceiling is 4 GiB.

:::caution
The application must be reachable on a non-loopback address while recording. Command-mode replay starts a supervised copy on loopback and refuses to target the exact recorded destination.
:::

## Find the recording

```bash
chronicle list
chronicle inspect checkout
```

Recordings can be addressed by `latest`, `rec_<uuid>`, a bare UUID, or an exact name. `inspect` summarizes endpoints, operations, loss warnings, and replay eligibility without printing captured bodies or arbitrary header values.

## Replay into a fresh copy

```bash
chronicle replay checkout -- ./my-app
```

Command mode plans before spawning the target, discovers one owned loopback listener, and replays only after target-independent policy checks pass. It is dry-run by default; writes and other effects stay denied unless the relevant policy is explicitly authorized.

For an already-running application, use explicit target mode only with a loopback IP literal and all required gates:

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

Add `--allow-write` only when the recording and target are prepared for write effects. Chronicle never uses the recorded production destination as a fallback.

## Inspect machine-readable results

All public commands accept the global format option:

```bash
chronicle --format json list
chronicle --format json inspect checkout
chronicle --format json replay checkout -- ./my-app
```

JSON output is rendered after the bounded operation completes. Use it for tooling; keep human output for interactive diagnosis.
