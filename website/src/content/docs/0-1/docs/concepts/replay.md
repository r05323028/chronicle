---
title: Replay
description: Plan and execute recordings with explicit loopback safety gates.
slug: 0-1/docs/concepts/replay
---

Replay is the highest-risk Chronicle boundary. It consumes canonical sessions and protocol interfaces; it does not reconnect to the recorded production destination.

## Safe defaults

* Dry-run is the default.
* Reads, writes, authentication, publication, and unknown effects are denied until policy allows them.
* Every canonical connection needs a target mapping.
* Recorded destinations are never a fallback.
* Incomplete, malformed, unmatched, unsupported, pipelined, or ambiguous-loss operations remain visible and are not attempted.

## Command mode

Command mode starts a supervised copy of the application and discovers one unique loopback listener owned by that scope:

```bash
chronicle replay checkout -- ./my-app
```

Planning and denial checks finish before the target is spawned. Command mode can infer a loopback target and matching host for the supervised copy, but it does not grant write, authentication, publication, or unknown effects.

## Explicit-target mode

For an already-running application, provide a loopback IP-literal target and all necessary gates:

```bash
chronicle replay checkout \
  --target http://127.0.0.1:8080 \
  --allow-host 127.0.0.1 \
  --allow-read \
  --execute
```

Explicit-target mode requires:

* `http://` with a loopback IP literal;
* a matching repeated `--allow-host` value;
* effect authorization such as `--allow-read` or `--allow-write`;
* `--execute`.

Writes additionally require `--allow-write`. Configuration cannot silently supply these execution gates.

## HTTP request handling

For bounded plaintext HTTP/1.1, replay removes captured `Host`, hop-by-hop fields, `Authorization`, `Proxy-Authorization`, `Cookie`, forwarding headers, `Expect`, and `Transfer-Encoding`. It emits one target `Host` and a recomputed `Content-Length`; it never follows redirects. Optional authorization comes only from a configured environment-variable name, never from captured credentials.

## Verification

Verification compares status, body SHA-256/size, and ordered non-ignored headers. Details do not print bodies or arbitrary header values. Outcomes distinguish passed, failed, skipped, inconclusive, and unsupported operations.

:::caution
Do not point replay at a production destination. A database write or message publication can be irreversible. Unknown operations remain denied until a protocol canonicalizer classifies them or an operator creates a narrow explicit policy.
:::
