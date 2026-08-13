#!/usr/bin/env bash
# Central-dispatch scenario: recorder-readiness.
scenario_p2_recorder_readiness() {
	phase incremental 'prove recorder-owned incremental worker processes live committed WAL'
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/incremental-workload.json"
	processing_ready() {
		if ! "$CHRONICLE" --format json internal recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS" 2>/dev/null; then
			printf 'recorder status unavailable\n'
			return 1
		fi
		python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
print(f"state={value.get('state')} processing={value.get('processing_readiness')} checkpoint={value.get('checkpoint')}")
if value.get("state") == "failed":
    raise SystemExit(2)
raise SystemExit(0 if value.get("processing_readiness") == "ready" else 1)
PY
	}
	wait_until 60 0.5 'incremental ETL processing readiness' processing_ready
	set_check incremental_worker passed

	phase live_etl 'prove standalone ETL rejects live WAL ownership'
	set +e
	"$CHRONICLE" --format json internal etl --wal-dir "$STATE_ROOT/wal" --output "$ARTIFACT_ROOT/live-etl" >"$ARTIFACT_ROOT/live-etl.json" 2>"$ARTIFACT_ROOT/live-etl.log"
	live_etl_status=$?
	set -e
	[[ $live_etl_status -ne 0 ]]
	set_check standalone_etl_live_rejection passed

}
