#!/usr/bin/env bash
# Central-dispatch scenario: reboot-recovery.
scenario_p2_reboot_recovery() {
if [[ "$PRE_REBOOT" == 1 ]]; then
		phase pre_reboot 'retain live recorder state for VM reboot'
		"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$ARTIFACT_ROOT/pre-reboot-status.json"
		test -f "$STATE_ROOT/wal/recording.json"
		# Drain the recorder before the VM reboot so the reboot never lands in a
		# partial publication window; restart continuity is then deterministic.
		systemctl stop "$UNIT" 2>/dev/null || true
		for _ in $(seq 1 60); do
				systemctl is-active --quiet "$UNIT" || break
				sleep 0.25
		done
		! systemctl is-active --quiet "$UNIT"
		exit 0
fi

	phase reboot_recovery 'prove WAL, checkpoint, catalog, and counter continuity after reboot'
	PRE_STATUS="$ARTIFACT_ROOT/../pre-reboot/pre-reboot-status.json"
	test -f "$PRE_STATUS"
	test -f "$STATE_ROOT/wal/recording.json"
	find "$STATE_ROOT/wal" -type f -name '*.chwal' -print -quit | grep -q .
	find "$STATE_ROOT/wal" -type f -name 'incremental-etl-checkpoint.json' -print -quit | grep -q .
	test -f "$STATE_ROOT/wal/epochs.json"
	python3 - "$PRE_STATUS" "$RECORDER_STATUS" "$STATE_ROOT" "$ARTIFACT_ROOT/reboot-continuity.json" <<'PY'
import json, sys
from pathlib import Path
pre = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
post = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
state_root = Path(sys.argv[3])

# Status counters describe the active epoch only; after the reboot the
# recorder continues in a fresh successor epoch, so compare total committed
# records across every epoch recording on disk against the pre-reboot active
# epoch (its records must survive the reboot).
def total_committed(root: Path) -> int:
    total = 0
    for path in root.glob("wal/epochs/*/recording.json"):
        total += json.loads(path.read_text(encoding="utf-8"))["counters"]["committed"]["records"]
    root_recording = root / "wal/recording.json"
    if root_recording.is_file():
        total += json.loads(root_recording.read_text(encoding="utf-8"))["counters"]["committed"]["records"]
    return total

def committed(value):
    counters = value.get("counters", {})
    nested = counters.get("committed", {})
    return counters.get("committed_records", nested.get("records", 0))

def epoch(value):
    current = value.get("current_epoch", 0)
    return current.get("ordinal", 0) if isinstance(current, dict) else current
post_total = total_committed(state_root)
assert epoch(post) >= epoch(pre), (pre, post)
assert post_total >= committed(pre), (pre, post_total)
json.dump({
    "version": 1,
    "pre_active_committed_records": committed(pre),
    "post_active_committed_records": committed(post),
    "post_total_committed_records": post_total,
    "pre_epoch": epoch(pre),
    "post_epoch": epoch(post),
    "wal_recording": True,
    "checkpoint": True,
    "epoch_catalog": True,
}, open(sys.argv[4], "w", encoding="utf-8"), indent=2)
PY
	set_check pre_reboot_persistence passed
	set_check host_reboot_recovery passed

}
