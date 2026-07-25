# Replay Safety

Replay is primary capability and highest-risk boundary.

## Current safeguards

- Target mapping is mandatory per connection. Missing mapping fails.
- Recorded production destination is never a fallback.
- Mapping back to recorded endpoint is blocked by default.
- Policy defaults to dry-run with reads, writes, authentication, publication, and unknown operations denied.
- CLI execution requires explicit `http://<loopback-ip>:<port>` target, matching repeated `--allow-host`, effect authorization, and `--execute`; configuration cannot supply these gates.
- HTTP replay removes every captured Host and emits one target Host; removes Connection tokens, hop-by-hop fields, Authorization, Proxy-Authorization, Cookie, Forwarded, X-Forwarded-*, Expect, and Transfer-Encoding; then emits one recomputed Content-Length. It never follows redirects.
- Optional Authorization comes only from configured environment-variable name; captured credentials are never fallback and control bytes are rejected.
- Protocol adapters receive replay-environment `ReplayContext`; secret bytes render as `<redacted>` in debug output.
- Verification compares status, body SHA-256/size, and ordered non-ignored headers, producing Passed, Failed, Skipped, Inconclusive, or Unsupported without body/header values in details.

Only bounded plaintext HTTP/1.1 loopback replay is implemented. TLS, DNS targets, preserve timing, connection reuse, and pipelined replay remain unsupported.

## Planned MVP controls

- explicit confirmation/flag for non-dry-run;
- operation filters and protocol-specific classification;
- authentication/token/header replacement from external runtime configuration;
- blocklists for production networks and service identities;
- audit records containing IDs and policy decisions, never payloads or secrets;
- protocol-specific normalization policies with no broad hidden exclusions;
- response capture and persisted verification summaries.

Recorded credentials must never be replayed blindly. Adapters establish fresh sessions using replay-environment credentials and restore only permitted session state. Secret management integration remains outside MVP.

Database writes and message publication can be irreversible. Unknown operations stay denied until a protocol canonicalizer classifies them or operator creates an explicit narrow policy.
