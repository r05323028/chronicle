---
title: Introduction
description: Turn real application behavior into deterministic, replayable regression-test evidence.
---

Chronicle records real application traffic and turns it into deterministic, replayable regression-test evidence.

It attaches around a supervised command, a running process, or a cgroup instead of requiring application instrumentation. Captured evidence is written to a local write-ahead log (WAL) before interpretation, reconstructed into a protocol-independent canonical session, and replayed only against an explicitly authorized loopback target.

:::caution
Captured traffic can contain credentials and personal data. Replay can have side effects. Chronicle defaults to dry-run with every effect denied, and never falls back to the recorded production destination.
:::

## What is supported now?

The current 0.1.x surface is intentionally narrow:

- Live eBPF capture on Linux for bounded plaintext HTTP/1.1 traffic.
- Recording around a command, an existing process, or a cgroup.
- A segmented, crash-recoverable WAL with in-WAL commit markers.
- ETL that publishes one deterministic canonical session to local filesystem storage.
- Safe command-mode and explicit-target replay with loopback authorization.
- Fixture recording, inspection, catalog listing, and non-destructive readiness checks on any platform.

TLS decryption, HTTP/2+, other protocol implementations, remote persistence, encryption at rest, comprehensive redaction, Docker packaging, and Kubernetes packaging are not implemented.

## The path

```text
application behavior
        │
        ▼
eBPF capture evidence
        │
        ▼
segmented WAL ── durable commit boundary
        │
        ▼
ETL ── recover, decode, account for loss
        │
        ▼
canonical session ── inspect and store
        │
        ▼
loopback replay ── verify, never production fallback
```

Start with [installation](./getting-started/installation/), then follow [quick start](./getting-started/quick-start/). When you need the model behind the commands, read [capture](./concepts/capture/), [WAL](./concepts/wal/), [canonical model](./concepts/canonical-model/), and [replay](./concepts/replay/).

## Documentation status

English is the canonical source. Traditional Chinese and Japanese pages are translated incrementally; when a page has no translation yet, Starlight falls back to the English page instead of creating a stale parallel definition. Keep command names, format versions, and flags unchanged across locales. See [terminology](./reference/terminology/) for recurring terms.
