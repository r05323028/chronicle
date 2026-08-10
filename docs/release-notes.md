# Release notes

## 0.1.x: intent-oriented CLI

Chronicle now presents five public commands: `record`, `replay`, `list`, `inspect`, and `doctor`. Start with the [operations guide](operations.md); its migration appendix maps every 0.1.x compatibility form to current syntax.

### Deprecation schedule

Legacy top-level `recorder`, `recorder-status`, and `etl`; `record --source fixture|ebpf`; and session-root `inspect`/`replay` remain hidden, behavior-compatible entrypoints throughout 0.1.x. Each invocation emits one safe deprecation warning. They may be removed only by a later OpenSpec change and not before 0.2.0.

### Artifact and rollback compatibility

This release does not change authoritative WAL v1, canonical session v1, or session-manifest v1 formats. `recording-catalog.json` v1 and per-recording `recording-intent.json` v1 are additive advisory artifacts; older releases ignore them.

Rollback requires coordinated Chronicle binaries/crates, not a CLI-only downgrade. A previous 0.1.x release can still inspect an explicit canonical root and process an explicit WAL directory. It does not understand recording names, `latest`, intent sidecars, catalog reconciliation, or `record --retry`; resolve IDs and finish recovery before rollback when those features matter.

Cross-version check:

```bash
cargo build -p chronicle-cli
CHRONICLE_PREVIOUS_RELEASE_BIN=/path/to/controlled-previous-chronicle \
CHRONICLE_CURRENT_WAL_DIR=/path/to/current-production-wal \
  ./scripts/tests/test-user-intent-cli-rollback.sh
```

Harness proves supported directions: previous reader against current WAL/canonical artifacts, and current reader against previous canonical artifacts. Pre-metadata WAL is not a supported forward-migration input. Harness makes no claim that old binaries consume additive catalog/sidecar state.
