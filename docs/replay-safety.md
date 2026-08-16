# Replay Safety

Replay is primary capability and highest-risk boundary.

Parent replay aggregates only verified terminal canonical operations from ordered epoch sessions. Continuation-only predecessor evidence remains inspectable completeness metadata and never becomes executable traffic. A WAL epoch boundary is not a protocol reconstruction boundary; missing or invalid continuation fails closed rather than synthesizing a request.

## Current safeguards

Replay planner preserves one result per canonical operation. Complete supported operations can execute while incomplete, truncated, malformed, unmatched, unsupported, pipelined, or ambiguous-loss operations remain visible as not attempted. Aggregate outcomes are `completed`, `completed_with_skips`, `dry_run`, `stopped_policy`, `stopped_invalid_session`, `stopped_transport`, and `stopped_verification`; a transport or verification stop retains prior, current, and later unattempted results.

- Target mapping is mandatory per connection. Missing mapping fails.
- Recorded production destination is never a fallback.
- Mapping back to recorded endpoint is blocked by default.
- Policy defaults to dry-run with reads, writes, authentication, publication, and unknown operations denied.
- Command mode starts target only after target-independent planning and scope setup, then requires one unique loopback listener owned by supervised cgroup members and their open socket inode.
- Explicit-target execution requires `http://<loopback-ip>:<port>`, matching repeated `--allow-host`, effect authorization, and `--execute`; configuration cannot supply these gates.
- HTTP replay removes every captured Host and emits one target Host; removes Connection tokens, hop-by-hop fields, Authorization, Proxy-Authorization, Cookie, Forwarded, X-Forwarded-*, Expect, and Transfer-Encoding; then emits one recomputed Content-Length. It never follows redirects.
- Optional Authorization comes only from configured environment-variable name; captured credentials are never fallback and control bytes are rejected.
- Protocol adapters receive replay-environment `ReplayContext`; secret bytes render as `<redacted>` in debug output.
- Verification compares status, body SHA-256/size, and ordered non-ignored headers, producing Passed, Failed, Skipped, Inconclusive, or Unsupported without body/header values in details.

Only bounded plaintext HTTP/1.1 loopback replay is implemented. TLS, DNS targets, preserve timing, connection reuse, pipelined replay, upgrades, and automatic redirects remain unsupported. Replay never falls back to the recorded destination.

## Inferred versus explicit target modes

| Property | `replay RECORDING -- COMMAND...` | `replay RECORDING --target URL` |
| --- | --- | --- |
| Target lifecycle | Chronicle spawns and boundedly cleans supervised cgroup | Caller owns already-running target; Chronicle never terminates it |
| Planning | Target-independent plan completes before spawn | Existing planner evaluates explicit target and policy |
| Target selection | Unique owned loopback listener from stable `(pid,start_time,fd,socket_inode)` evidence | Exact user-supplied loopback IP-literal origin |
| Automatically granted | Execution intent, inferred target/host match, read effects | Nothing |
| Still explicit | `--allow-write` | `--execute`, matching `--allow-host`, `--allow-read`, `--allow-write` |
| Recorded destination | Forbidden | Forbidden |

Writes, authentication, publication, and unknown effects are denied by default in both modes. Command inference never grants them. Optional fresh Authorization still comes only from configured environment-variable name; captured credentials are never fallback.

## Command and output contract

Command mode executes only after target-independent denial checks and owned-listener discovery. Explicit-target mode is dry-run until full gates are supplied. Exit 0 covers dry-run and successful complete/partial execution; 3 covers cleanup/orphan or launch failure; 4 covers policy, invalid-session, unsupported scope, or listener-readiness denial; 5 transport stop; 6 executed verification failure. Human output retains detailed plan/result and adds operation counts, `✓ passed`/`✗ failed`, and final `Replay passed.`/`Replay failed.`. JSON is rendered once after bounded result completes, so render failure never retries network traffic.

## Planned controls

- explicit confirmation/flag for non-dry-run;
- operation filters and protocol-specific classification;
- authentication/token/header replacement from external runtime configuration;
- blocklists for production networks and service identities;
- audit records containing IDs and policy decisions, never payloads or secrets;
- protocol-specific normalization policies with no broad hidden exclusions;
- response capture and persisted verification summaries.

Recorded credentials must never be replayed blindly. Adapters establish fresh sessions using replay-environment credentials and restore only permitted session state. Secret management integration remains outside the current scope.

Database writes and message publication can be irreversible. Unknown operations stay denied until a protocol canonicalizer classifies them or operator creates an explicit narrow policy.
