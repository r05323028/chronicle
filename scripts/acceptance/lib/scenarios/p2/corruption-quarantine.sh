#!/usr/bin/env bash
# Central-dispatch scenario: corruption-quarantine.
scenario_p2_corruption_quarantine() {
phase corruption 'corrupt committed WAL copy and prove fail-closed preservation'
CORRUPT_WAL="$ARTIFACT_ROOT/corrupt-wal"
CORRUPT_STORE="$ARTIFACT_ROOT/corrupt-store"
cp -a "$STATE_ROOT/wal" "$CORRUPT_WAL"
SOURCE_WAL_DIGEST=$(sha256sum "$STATE_ROOT"/wal/segments/*.chwal | sha256sum | awk '{print $1}')
CORRUPT_SEGMENT=$(find "$CORRUPT_WAL/segments" -name '*.chwal' -print -quit)
python3 - "$CORRUPT_SEGMENT" <<'PY'
import sys
from pathlib import Path
path = Path(sys.argv[1])
data = bytearray(path.read_bytes())
if len(data) <= 64:
    raise SystemExit('sealed WAL segment too short to corrupt')
offset = min(len(data) - 1, max(64, len(data) // 2))
data[offset] ^= 0x01
path.write_bytes(data)
PY
CORRUPT_WAL_DIGEST=$(sha256sum "$CORRUPT_WAL"/segments/*.chwal | sha256sum | awk '{print $1}')
set +e
"$CHRONICLE" --format json etl --wal-dir "$CORRUPT_WAL" --output "$CORRUPT_STORE" >"$ARTIFACT_ROOT/corruption.json" 2>"$ARTIFACT_ROOT/corruption.log"
corruption_status=$?
"$CHRONICLE" --format json etl --wal-dir "$CORRUPT_WAL" --output "$CORRUPT_STORE-retry" >"$ARTIFACT_ROOT/corruption-retry.json" 2>"$ARTIFACT_ROOT/corruption-retry.log"
corruption_retry_status=$?
set -e
[[ $corruption_status -ne 0 && $corruption_retry_status -ne 0 ]]
[[ "$(sha256sum "$STATE_ROOT"/wal/segments/*.chwal | sha256sum | awk '{print $1}')" == "$SOURCE_WAL_DIGEST" ]]
[[ "$(sha256sum "$CORRUPT_WAL"/segments/*.chwal | sha256sum | awk '{print $1}')" == "$CORRUPT_WAL_DIGEST" ]]
test -f "$CORRUPT_SEGMENT"
set_check corruption_fail_closed passed
set_check corruption_evidence_preservation passed


}
