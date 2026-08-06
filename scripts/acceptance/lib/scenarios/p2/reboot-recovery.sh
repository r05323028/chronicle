#!/usr/bin/env bash
# Central-dispatch scenario: reboot-recovery.
scenario_p2_reboot_recovery() {
if [[ "$PRE_REBOOT" == 1 ]]; then
	phase pre_reboot 'retain live recorder state for VM reboot'
	"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$ARTIFACT_ROOT/pre-reboot-status.json"
	test -f "$STATE_ROOT/wal/recording.json"
	exit 0
fi

phase reboot_recovery 'prove WAL, checkpoint, catalog, and counter continuity after reboot'
PRE_STATUS="$ARTIFACT_ROOT/../pre-reboot/pre-reboot-status.json"
test -f "$PRE_STATUS"
test -f "$STATE_ROOT/wal/recording.json"
find "$STATE_ROOT/wal" -type f -name '*.chwal' -print -quit | grep -q .
find "$STATE_ROOT/wal" -type f -name 'incremental-etl-checkpoint.json' -print -quit | grep -q .
test -f "$STATE_ROOT/wal/epochs.json"
python3 - "$PRE_STATUS" "$RECORDER_STATUS" "$ARTIFACT_ROOT/reboot-continuity.json" <<'PY'
import json, sys
from pathlib import Path
pre = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
post = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))

def committed(value):
    counters = value.get("counters", {})
    nested = counters.get("committed", {})
    return counters.get("committed_records", nested.get("records", 0))

def epoch(value):
    current = value.get("current_epoch", 0)
    return current.get("ordinal", 0) if isinstance(current, dict) else current
assert committed(post) >= committed(pre), (pre, post)
assert epoch(post) >= epoch(pre), (pre, post)
json.dump({
    "version": 1,
    "pre_committed_records": committed(pre),
    "post_committed_records": committed(post),
    "pre_epoch": epoch(pre),
    "post_epoch": epoch(post),
    "wal_recording": True,
    "checkpoint": True,
    "epoch_catalog": True,
}, open(sys.argv[3], "w", encoding="utf-8"), indent=2)
PY
set_check pre_reboot_persistence passed
set_check host_reboot_recovery passed

}
