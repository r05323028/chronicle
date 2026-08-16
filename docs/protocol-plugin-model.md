# Protocol Plugin Model

## Current: compile-time registry

`ProtocolRegistry` owns explicit `ProtocolRegistration` values. Each registration independently advertises detector, decoder factory, canonicalizer, replay adapter, verifier, metadata, and per-capability status. Missing implementations remain `None`; stubs are never reported available.

Small object-safe traits separate responsibilities:

- detector returns Confirmed, Probable, NeedMoreData, Rejected, or Unknown;
- decoder accepts ordered directional frames and supports finish;
- canonicalizer owns protocol correlation and replay intent;
- replay adapter establishes a fresh target connection using replay-environment context;
- verifier compares recorded and observed protocol outcomes.

Core ETL/replay dispatches through registration capabilities and contains no protocol-specific `match` branches. Explicit user override bypasses heuristic detection only when registered.

`chronicle-protocol-builtins` contains HTTP/1.1, PostgreSQL, MySQL-family, MySQL, MariaDB, Oracle, MongoDB, Kafka, NATS, and fake module boundaries. Fake and bounded plaintext HTTP/1.1 have all five capabilities available; every other protocol retains its declared non-Available status.

## Planned

Real implementations remain ordinary Rust modules/crates linked at compile time. Detectors combine ports, signatures, validated framing, metadata, confidence, and need-more-data states. Malformed/truncated/unknown bytes produce issues and opaque preservation rather than process crashes.

Oracle code must not invent undocumented semantics. Opaque TNS frames remain valid canonical evidence when semantic decode is unavailable.

## Future distribution

Separately distributed plugins require an explicit versioned ABI, subprocess/RPC, WebAssembly component boundary, or another safe contract. Rust trait-object layouts are not stable ABI and must never cross a dynamic-library boundary directly. Dynamic loading is not part of the current scope.

## Decision history

The compile-time registry decision (formerly ADR 0001) is captured here and in `docs/architecture/crate-boundaries.md`; the historical decision record lives in OpenSpec archives and Git history.
