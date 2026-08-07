#!/usr/bin/env bash
# Central-dispatch scenario: incremental-etl.
scenario_p2_incremental_etl() {
phase size_rotation 'force deterministic segment and epoch byte rollover'
SIZE_SEGMENTS_BEFORE=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
SIZE_BYTES_BEFORE=$(du -sb "$STATE_ROOT/wal" | awk '{print $1}')
EPOCH_BEFORE=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
for _ in $(seq 1 12); do
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" --body-bytes 2097152 >"$ARTIFACT_ROOT/size-workload-$_.json"
done
sleep 1
SIZE_SEGMENTS_AFTER=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
SIZE_BYTES_AFTER=$(du -sb "$STATE_ROOT/wal" | awk '{print $1}')
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
EPOCH_AFTER=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
[[ $SIZE_SEGMENTS_AFTER -gt $SIZE_SEGMENTS_BEFORE ]]
[[ $((SIZE_BYTES_AFTER - SIZE_BYTES_BEFORE)) -ge 524288 ]]
[[ $EPOCH_AFTER -gt $EPOCH_BEFORE ]]
python3 - "$ARTIFACT_ROOT/rotation-proof.json" <<PY
import json
json.dump({
    "version": 1,
    "segment_count_before": $SIZE_SEGMENTS_BEFORE,
    "segment_count_after": $SIZE_SEGMENTS_AFTER,
    "segment_bytes_before": $SIZE_BYTES_BEFORE,
    "segment_bytes_after": $SIZE_BYTES_AFTER,
    "epoch_ordinal_before": $EPOCH_BEFORE,
    "epoch_ordinal_after": $EPOCH_AFTER,
    "fixed_post_body_bytes": 2097152,
    "workload_iterations": 12,
}, open("$ARTIFACT_ROOT/rotation-proof.json", "w", encoding="utf-8"), indent=2)
PY
set_check segment_size_rotation passed
set_check epoch_size_rollover passed

if [[ "$CRASH_MODE" == 1 ]]; then
	phase crash_restart 'kill recorder process and verify systemd recovery'
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/pre-crash-workload.json"
	# Pause publication at the post-delta commit boundary so the SIGKILL
	# lands deterministically in a recoverable state: the pending checkpoint
	# already references a published delta, so restart reconciliation adopts
	# it instead of failing closed on a partial publication.
	: >"$CHECKPOINT_PAUSE_FILE"
	for _ in $(seq 1 60); do
		[[ -f "$CHECKPOINT_READY_FILE" ]] && break
		sleep 0.25
	done
	systemctl kill --kill-who=main -s SIGKILL "$UNIT"
	rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
	# The unit restarts itself on failure; a manual restart would race the
	# auto-restart and can double-start the recorder (WAL lock contention).
	wait_for_unit_stable "$UNIT"
	RECORDER_STATUS="$ARTIFACT_ROOT/recorder-restart-status.json" \
		wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	systemctl is-active --quiet "$UNIT"
	set_check process_restart_readiness passed
fi

phase segment_age_rotation 'force age rotation with delayed low-traffic append'
AGE_SEGMENTS_BEFORE=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
AGE_EPOCH_BEFORE=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
sleep 2
printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/age-workload.json"
AGE_SEGMENTS_AFTER=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
AGE_EPOCH_AFTER=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
[[ $AGE_SEGMENTS_AFTER -gt $AGE_SEGMENTS_BEFORE || $AGE_EPOCH_AFTER -gt $AGE_EPOCH_BEFORE ]]
python3 - "$ARTIFACT_ROOT/age-rotation-proof.json" <<PY
import json
json.dump({
    "version": 1,
    "segment_count_before": $AGE_SEGMENTS_BEFORE,
    "segment_count_after": $AGE_SEGMENTS_AFTER,
    "epoch_before": $AGE_EPOCH_BEFORE,
    "epoch_after": $AGE_EPOCH_AFTER,
    "idle_seconds": 2,
}, open("$ARTIFACT_ROOT/age-rotation-proof.json", "w", encoding="utf-8"), indent=2)
PY
set_check segment_age_rotation passed

phase epochs 'generate traffic across at least three short epochs'
for _ in 1 2 3 4 5 6; do
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/workload-$_.json"
	sleep 1
done


}
