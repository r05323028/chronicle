#!/usr/bin/env bash
# Central-dispatch scenario: recorder-readiness.
scenario_p2_recorder_readiness() {
phase incremental 'prove recorder-owned incremental worker processes live committed WAL'
printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/incremental-workload.json"
for attempt in $(seq 1 120); do
	if "$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS" 2>/dev/null &&
		grep -q '"processing_readiness"[[:space:]]*:[[:space:]]*"ready"' "$RECORDER_STATUS"; then
		break
	fi
	if ((attempt % 10 == 0)); then
		printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
		python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/incremental-workload-$attempt.json" || true
	fi
	sleep 0.5
done
grep -q '"processing_readiness"[[:space:]]*:[[:space:]]*"ready"' "$RECORDER_STATUS"
set_check incremental_worker passed

phase live_etl 'prove standalone ETL rejects live WAL ownership'
set +e
"$CHRONICLE" --format json etl --wal-dir "$STATE_ROOT/wal" --output "$ARTIFACT_ROOT/live-etl" >"$ARTIFACT_ROOT/live-etl.json" 2>"$ARTIFACT_ROOT/live-etl.log"
live_etl_status=$?
set -e
[[ $live_etl_status -ne 0 ]]
set_check standalone_etl_live_rejection passed


}
