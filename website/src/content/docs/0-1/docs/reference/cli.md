---
title: CLI reference
description: The current intent-oriented Chronicle command surface and global options.
slug: 0-1/docs/reference/cli
---

The public 0.1.x CLI has five intent-oriented commands. Run `chronicle --help` or a command's `--help` for the binary's exact parser output.

## Global options

These options precede the subcommand:

| Option | Purpose |
| --- | --- |
| `--config FILE` | Read a TOML configuration file. Secrets must be referenced through environment variables. |
| `--data-dir DIR` | Select the public data directory. |
| `--format human\|json` | Select human or machine-readable rendering. |

## Public commands

| Command | Use |
| --- | --- |
| `record` | Capture a command, process, or cgroup into a published recording. |
| `replay` | Plan and safely replay a recording against a spawned or already-running application. |
| `list` | List recordings, newest first. |
| `inspect` | Summarize a recording by `latest`, ID, or exact name. |
| `doctor` | Run non-destructive platform, capture, storage, protocol, and replay-policy probes. |

### record

```bash
chronicle record --name checkout -- ./my-app
chronicle record --duration 30s -- ./my-app
chronicle record --pid PID
chronicle record --cgroup /sys/fs/cgroup/my-service
chronicle record --retry checkout
```

Public flags include `--name`, `--duration`, `--retry`, `--pid`, and `--cgroup`. Command arguments follow `--`.

### replay

```bash
chronicle replay checkout -- ./my-app
chronicle replay checkout --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 --allow-read --execute
```

Replay flags include `--target`, repeatable `--allow-host`, `--allow-read`, `--allow-write`, and `--execute`. Command mode and explicit-target mode have different authorization requirements; read [replay safety](../../concepts/replay/) before enabling effects.

### list, inspect, doctor

```bash
chronicle list
chronicle inspect latest
chronicle doctor
chronicle --format json list
chronicle --format json inspect latest
chronicle --format json doctor
```

All three are safe to use for diagnosis. `doctor` is non-destructive; `inspect` avoids printing bodies and arbitrary header values.

## Advanced entrypoints

The hidden `internal` namespace is the operational surface for the continuous recorder, recorder status, standalone ETL, and deterministic fixture recording; it is not the recommended public surface. Docker/Kubernetes commands do not exist.
