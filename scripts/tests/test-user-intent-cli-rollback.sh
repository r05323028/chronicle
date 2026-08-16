#!/usr/bin/env bash
# Controlled cross-version check for the intentionally stable WAL v1 and
# canonical session v1 artifacts: a controlled older binary must read current
# artifacts (the rollback direction). Current binaries are not required to read
# artifacts produced by unreleased older builds - the pre-0.1 single-model
# policy (mvp-schema-versioning) allows replacing unreleased formats.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
CURRENT=${CHRONICLE_CURRENT_BIN:-$ROOT/target/debug/chronicle}
PREVIOUS=${CHRONICLE_PREVIOUS_RELEASE_BIN:?set CHRONICLE_PREVIOUS_RELEASE_BIN to controlled previous 0.1.x chronicle binary}
FIXTURE="$ROOT/fixtures/http/basic-session.json"
CURRENT_WAL=${CHRONICLE_CURRENT_WAL_DIR:?set CHRONICLE_CURRENT_WAL_DIR to a controlled current production WAL directory}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

session_id() {
    python3 -c 'import json,sys; print(json.load(sys.stdin)["session_id"])'
}
# Previous release reads canonical and WAL produced by current release.
CURRENT_SESSION=$("$CURRENT" --format json internal record-fixture --input "$FIXTURE" --root "$WORK/current" | session_id)
"$PREVIOUS" --format json inspect "$CURRENT_SESSION" --root "$WORK/current" >"$WORK/previous-inspect-current.json"
[[ -f "$CURRENT_WAL/recording.json" ]] || {
    echo "current production WAL lacks recording.json: $CURRENT_WAL" >&2
    exit 2
}
"$PREVIOUS" --format json etl --wal-dir "$CURRENT_WAL" --output "$WORK/previous-from-current" >"$WORK/previous-etl-current.json"

python3 - "$WORK" "$CURRENT_SESSION" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
for name in (
    "previous-inspect-current.json",
    "previous-etl-current.json",
):
    value = json.loads((root / name).read_text(encoding="utf-8"))
    assert value["version"] == 1, (name, value)
assert sys.argv[2]
print(json.dumps({
    "version": 1,
    "status": "passed",
    "directions": ["previous_reads_current_wal_and_canonical"],
    "authoritative_formats": ["wal_v1", "canonical_session_v1", "session_manifest_v1"],
    "excluded_additive_artifacts": ["recording_catalog_v1", "recording_intent_v1", "names", "latest", "retry"],
}, sort_keys=True))
PY
