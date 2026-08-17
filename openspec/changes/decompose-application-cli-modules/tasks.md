## 1. Application module surface

- [x] 1.1 Extract application error/exit-code implementation and tests from `lib.rs`; keep intentional module declarations and public re-exports stable.
- [x] 1.2 Run application unit tests and semantic API/architecture checks; confirm no public symbol or dependency edge changes.

## 2. Continuous recorder decomposition

- [x] 2.1 Move `continuous_recorder.rs` to a private `continuous_recorder/` module tree without changing the service/error public paths.
- [x] 2.2 Extract startup/recovery and metadata helpers with private ownership preserved.
- [x] 2.3 Extract incremental ETL polling/publication, continuation, rollover, and finalization/shutdown responsibilities in separate private modules.
- [x] 2.4 Run focused recorder, restart/recovery, publication/checkpoint, rollover, continuation, and metadata/readiness tests after each extraction.

## 3. CLI decomposition

- [x] 3.1 Extract clap arguments, commands, enums, and validation into private `args.rs` while preserving definitions and defaults.
- [x] 3.2 Extract dispatch and hidden internal commands into private modules; preserve command registration, exit mapping, and application-only dependency direction.
- [x] 3.3 Extract human/JSON renderers and signal watchers; preserve exact output/schema and cleanup behavior.
- [x] 3.4 Run CLI contract, documented-command, smoke, and targeted application tests; compare help/JSON/exit behavior.

## 4. Final structural validation

- [x] 4.1 Confirm no new crate, dependency, public API, schema, WAL format, replay policy, or validation-policy change.
- [x] 4.2 Run `openspec validate --all --strict --no-interactive` and `./scripts/validate.sh fast`; run graphify update after code moves.
