#!/usr/bin/env bash
# Central-dispatch scenario: checkpoint-kill-restart.
scenario_p2_checkpoint_kill_restart() {
phase checkpoint_crash_matrix 'exercise production publication crash boundaries'
rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
: >"$CHECKPOINT_PAUSE_FILE"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/checkpoint-crash-workload.json"
for _ in $(seq 1 120); do
	[[ -f "$CHECKPOINT_READY_FILE" ]] && break
	sleep 0.25
done
test -f "$CHECKPOINT_READY_FILE"
PENDING_FILES=()
while IFS= read -r -d '' pending; do
	PENDING_FILES+=("$pending")
done < <(find "$STATE_ROOT/wal" -type f -name '*.pending' -print0)
if ((${#PENDING_FILES[@]} != 1)); then
	printf 'expected one pending checkpoint, found %s:\n' "${#PENDING_FILES[@]}" >&2
	printf '%s\n' "${PENDING_FILES[@]}" >&2
	exit 1
fi
PENDING_BEFORE=${PENDING_FILES[0]}
DELTA_BEFORE=$(
	python3 - "$PENDING_BEFORE" "$STORE_ROOT" <<'PY'
import json, sys
from pathlib import Path
pending = json.load(open(sys.argv[1], encoding="utf-8"))
key = pending["outputs"][-1]["key"]
path = Path(sys.argv[2], "artifacts", *key.split("/"))
print(path)
PY
)
test -f "$DELTA_BEFORE"
PID_BEFORE=$(systemctl show "$UNIT" -p ExecMainPID --value)
python3 - "$PENDING_BEFORE" "$DELTA_BEFORE" <<'PY'
import json, sys
pending = json.load(open(sys.argv[1], encoding="utf-8"))
artifact = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(output["key"] == artifact["key"] and output["digest"] == artifact["checksum"] for output in pending["outputs"])
PY
systemctl kill --kill-who=main -s SIGKILL "$UNIT"
rm -f "$CHECKPOINT_PAUSE_FILE"
systemctl restart "$UNIT"
wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
PID_AFTER=$(systemctl show "$UNIT" -p ExecMainPID --value)
test "$PID_BEFORE" != "$PID_AFTER"
if find "$STATE_ROOT/wal" -type f -name '*.pending' -print -quit | grep -q .; then
	echo 'checkpoint pending artifact survived restart' >&2
	exit 1
fi
CHECKPOINT_AFTER="$(dirname "$PENDING_BEFORE")/incremental-etl-checkpoint.json"
test -f "$CHECKPOINT_AFTER"
python3 - "$CHECKPOINT_AFTER" "$DELTA_BEFORE" "$STORE_ROOT/artifacts" <<'PY'
import json, sys
checkpoint = json.load(open(sys.argv[1], encoding="utf-8"))
artifact = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(output["key"] == artifact["key"] and output["digest"] == artifact["checksum"] for output in checkpoint["outputs"])
from pathlib import Path
artifact_path = Path(sys.argv[3], *artifact["key"].split("/"))
assert artifact_path.is_file(), artifact_path
PY
set_check checkpoint_output_committed_checkpoint_missing passed
set_check checkpoint_committed_status_missing passed


}
