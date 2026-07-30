# ADR 0001: Compile-time protocol registry

- Status: Accepted
- Date: 2026-07-24

## Context

MVP needs several protocol boundaries, honest partial capability status, and dispatch without protocol branches in shared core. Rust has no stable trait-object ABI. Dynamic plugin loading would add deployment, safety, and versioning complexity before independently distributed implementations are needed.

## Decision

Use explicit compile-time `ProtocolRegistration` values with separate detector, decoder factory, canonicalizer, replay adapter, and verifier traits. Consolidate built-in protocol modules in one builtins crate. Keep fake protocol as a boundary-test implementation.

## Consequences

Registry remains simple, testable, and statically checked. Shared ETL/replay code does not know protocol variants. Adding a built-in requires recompilation. Independently distributed plugins are deferred to an explicit stable ABI/process/component boundary; Rust trait objects will not cross shared-library boundaries.
