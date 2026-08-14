#!/usr/bin/env bash
# Central-dispatch scenario: checkpoint-kill-restart.
scenario_recorder_checkpoint_kill_restart() {
	phase checkpoint_crash_matrix 'exercise production publication crash boundaries'
	wait_for_unit_stable "$UNIT"
	wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
	: >"$CHECKPOINT_PAUSE_FILE"
	# The workload must run inside the recorder's capture cgroup so the
	# checkpoint publication pauses deterministically; uncaptured traffic
	# leaves the worker with no new committed data to publish.
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs" 2>/dev/null || true
	# A body >= GROUP_COMMIT_BYTES forces the WAL group commit inside the
	# append path, so the checkpoint publish pauses deterministically;
	# tiny traffic would sit uncommitted until the next age-expired append
	# and the pause would never fire.
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" --body-bytes 4194304 >"$ARTIFACT_ROOT/checkpoint-crash-workload.json"
	# Leave the capture cgroup: the restart's cgroup sweep would otherwise
	# signal this scenario shell while the recorder converges.
	printf '%s\n' "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null || true
	wait_for_path "$CHECKPOINT_READY_FILE" 30
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
	# The paused recorder never resumed to remove its readiness marker; remove
	# both scenario-owned control files so no stale pause state survives.
	rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
	# The unit restarts itself on failure; a manual restart would race the
	# auto-restart and can double-start the recorder (WAL lock contention).
	if ! wait_for_unit_stable "$UNIT"; then
		# Retain the restart journal and unit state for diagnosis: a crash
		# loop here means startup recovery fails on the killed recording.
		collect_recorder_readiness_diagnostics 2>/dev/null || true
		journalctl --no-pager -u "$UNIT" >"$ARTIFACT_ROOT/checkpoint-restart-journal.log" 2>&1 || true
		systemctl show "$UNIT" >"$ARTIFACT_ROOT/checkpoint-restart-unit.txt" 2>&1 || true
		# Archive the durable recorder state for offline crash-recovery
		# diagnosis (WAL, checkpoint, store, metadata).
		tar -czf "$ARTIFACT_ROOT/crash-recovery-state.tar.gz" \
			-C "$(dirname "$STATE_ROOT")" "$(basename "$STATE_ROOT")" \
			-C "$(dirname "$STORE_ROOT")" "$(basename "$STORE_ROOT")" 2>/dev/null || true
		exit 1
	fi
	wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	wait_for_path_absent "$CHECKPOINT_PAUSE_FILE" 5
	wait_for_path_absent "$CHECKPOINT_READY_FILE" 5
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
