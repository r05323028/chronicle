---
title: Troubleshooting
description: Diagnose readiness, capture, finalization, and replay failures
  without guessing.
slug: 0-1/docs/troubleshooting
---

Start with the non-destructive readiness report:

```bash
chronicle doctor
chronicle --format json doctor
```

Read the probe code and remediation before changing the host.

## Live capture is unavailable

Check:

* the host is Linux 6.1+;
* cgroup v2 is mounted and the selected workload is in the intended subtree;
* `/sys/kernel/btf/vmlinux` exists;
* the binary contains the capture object and programs;
* the recording process has `CAP_BPF` and `CAP_NET_ADMIN`;
* the architecture is little-endian x86\_64 or aarch64.

Non-Linux builds still support fixture recording, listing, inspection, replay planning and verification, and doctor. They do not provide live eBPF capture.

## No operations appear

The current decoder supports bounded plaintext HTTP/1.1 only. TLS ciphertext, HTTP/2+, upgrades, pipelining, chunked requests, and unsupported protocol traffic do not become replayable HTTP operations. Confirm that the workload was reachable on a non-loopback address during recording and that traffic arrived inside the recording window.

## Finalization stopped or the WAL is near its limit

Public recording has no implicit time deadline; a bounded epoch WAL (4 GiB physical ceiling by default) rolls over rather than terminating the recording. Inspect disk space and the recording directory. If the recording is recoverable, retry finalization without recapturing:

```bash
chronicle record --retry checkout
```

Do not delete segments or manifests while recovery is diagnosing a recording. Complete corruption, identity mismatch, sequence gaps, and invalid commit references fail closed.

## Replay is denied

Dry-run and denial are expected defaults. Check:

* every connection has a target mapping;
* command mode can discover one unique owned loopback listener;
* explicit target is an `http://` loopback IP literal;
* `--allow-host` exactly matches the target host;
* `--allow-read` or `--allow-write` authorizes the intended effect;
* `--execute` is present for explicit-target execution.

Writes, authentication, publication, and unknown effects remain denied unless explicitly supported and authorized. Recorded production destinations are never a fallback.

## Data looks missing

Chronicle carries temporal loss windows and completeness states. An operation overlapping ambiguous loss may be incomplete, truncated, unmatched, or not replayable. Inspect reports loss warnings and replay eligibility; it does not fabricate missing endpoints or bodies.

## Artifacts contain sensitive data

WAL and payload files may contain captured credentials, headers, bodies, and personal data. Treat the data directory as sensitive, use filesystem permissions, and do not share artifacts without an independent review. Chronicle does not currently promise encryption at rest or comprehensive redaction.
