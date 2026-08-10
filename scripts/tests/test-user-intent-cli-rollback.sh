#!/usr/bin/env bash
# Controlled cross-version check for authoritative 0.1.x WAL/canonical artifacts.
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
[[ -f "$CURRENT_WAL/recording.json" ]] || { echo "current production WAL lacks recording.json: $CURRENT_WAL" >&2; exit 2; }
"$PREVIOUS" --format json etl --wal-dir "$CURRENT_WAL" --output "$WORK/previous-from-current" >"$WORK/previous-etl-current.json"

# Current release reads canonical output produced by previous release. Pre-metadata
# WAL is intentionally outside current forward-migration support.
PREVIOUS_SESSION=$("$PREVIOUS" --format json record --source fixture --input "$FIXTURE" --root "$WORK/previous" | session_id)
"$CURRENT" --format json inspect "$PREVIOUS_SESSION" --root "$WORK/previous" >"$WORK/current-inspect-previous.json" 2>"$WORK/current-inspect-previous.warning"
python3 - "$WORK" "$CURRENT_SESSION" "$PREVIOUS_SESSION" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
for name in (
    "previous-inspect-current.json",
    "previous-etl-current.json",
    "current-inspect-previous.json",
):
    value = json.loads((root / name).read_text(encoding="utf-8"))
    assert value["version"] == 1, (name, value)
assert all(value for value in sys.argv[2:4]), sys.argv[2:4]
print(json.dumps({
    "version": 1,
    "status": "passed",
    "directions": ["previous_reads_current_wal_and_canonical", "current_reads_previous_canonical"],
    "authoritative_formats": ["wal_v1", "canonical_session_v1", "session_manifest_v1"],
    "excluded_additive_artifacts": ["recording_catalog_v1", "recording_intent_v1", "names", "latest", "retry"],
}, sort_keys=True))
PY
