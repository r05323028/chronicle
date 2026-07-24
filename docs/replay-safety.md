# Replay Safety

Replay is primary capability and highest-risk boundary.

## Current safeguards

- Target mapping is mandatory per connection. Missing mapping fails.
- Recorded production destination is never a fallback.
- Mapping back to recorded endpoint is blocked by default.
- Policy defaults to dry-run with reads, writes, authentication, publication, and unknown operations denied.
- Dry-run plans cannot execute.
- Protocol adapters receive replay-environment `ReplayContext`; secret bytes render as `<redacted>` in debug output.
- Verification distinguishes Passed, Failed, Skipped, Inconclusive, and Unsupported.

Enabling reads or writes in configuration is not wired to execution yet; CLI is a service skeleton. Tests opt into fake read replay against a distinct fake target.

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
