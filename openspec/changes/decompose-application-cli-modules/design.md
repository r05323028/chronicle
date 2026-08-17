## Context

The application crate already exposes a broad intentional API from `lib.rs`, but its error/exit-code helpers and tests sit beside the module registry. `continuous_recorder.rs` contains startup, metadata, incremental processing, continuation, rollover, shutdown, and tests in one module. The CLI combines clap definitions, command dispatch, signal handling, rendering, and internal commands in `main.rs`. The public surface and crate graph are stable and must remain unchanged.

## Goals / Non-Goals

**Goals:**

- Make lifecycle ownership visible through private child modules and narrow `use super` boundaries.
- Leave `ContinuousRecorderService`, application exports, CLI commands, defaults, output, schemas, and exit codes unchanged.
- Keep extraction mechanical and validate after each responsibility group.

**Non-Goals:**

- No new crate, dependency, public symbol, visibility broadening, or behavior change.
- No redesign of recorder/ETL runtime semantics; runtime fixes belong to the separate incremental-controls change.
- No unrelated formatting or dead-code cleanup.

## Decisions

### 1. Convert the recorder file into a private module tree

Move the current file to `continuous_recorder/mod.rs` and extract cohesive private modules for startup/recovery, incremental processing, continuation, rollover, finalization/shutdown, and metadata helpers. Keep the service struct and error enum in `mod.rs`; child modules use `super` private access where needed. Do not make child implementation details public.

### 2. Keep application `lib.rs` as an API/composition surface

Move `ApplicationError`, exit-code helpers, and their unit tests to a private `application_error` module if extraction reduces the root file. Keep module declarations and deliberate `pub use` exports in `lib.rs`; do not split the public API into an abstraction layer.

### 3. Split CLI by stable responsibility

Keep clap structs/enums and parsing in `args.rs`, dispatch/public command execution in `dispatch.rs`, human/JSON rendering in `render.rs`, signal watcher code in `signals.rs`, and hidden internal command handling in `internal.rs`. Use private functions and `pub(super)` only where Rust module privacy requires it. `main.rs` retains process startup and top-level dispatch only.

### 4. Use mechanical equivalence gates

Before and after each extraction, run focused application/CLI tests, CLI contract tests, and architecture/semantic-boundary checks. Compare `--help`, JSON fixtures, exit codes, and command registration mechanically where existing tests cover them. Revert an extraction rather than introduce visibility or behavior drift.

## Risks / Trade-offs

- **Private-module import churn** → extract one responsibility at a time and compile/test after each move.
- **Large move diff obscures behavior** → preserve source text mechanically first; format only after tests pass.
- **Rust privacy pressure** → prefer child modules accessing parent-private items through `super` and small `pub(super)` helpers; no public API expansion.

## Migration Plan

No runtime or artifact migration. The resulting module paths are compile-time implementation details; public paths and binaries remain unchanged.
