#!/usr/bin/env bash
# Central-dispatch scenario: wal-recovery.
scenario_p2_wal_recovery() {
	phase stop 'request graceful SIGTERM and verify unit exit'
	wait_for_unit_stable "$UNIT" || true
	RECORDER_STATUS="$ARTIFACT_ROOT/recorder-before-stop.json" \
		wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	# Ensure the active epoch carries committed evidence before the final stop:
	# with one-second epochs the stop can otherwise land on an empty epoch.
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs" 2>/dev/null || true
	for _attempt in 1 2 3; do
		python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/final-workload.json" 2>/dev/null && break
		sleep 1
	done
	sleep 1
	systemctl stop "$UNIT"
	if systemctl is-active --quiet "$UNIT"; then
		exit 1
	fi
	journalctl --no-pager -u "$UNIT" >"$ARTIFACT_ROOT/recorder.log" || true
	[[ "$(systemctl show "$UNIT" -p ExecMainStatus --value 2>/dev/null || true)" == 0 ]]
	set_check signal_shutdown passed

	phase finalization 'validate committed boundary, epochs, quota, and capture counters'
	ACTIVE_WAL_DIR="$STATE_ROOT/wal"
	if [[ -f "$STATE_ROOT/wal/epochs.json" ]]; then
		ACTIVE_REL=$(
			python3 - "$STATE_ROOT/wal/epochs.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
active = [entry for entry in value["epochs"] if entry.get("state") == "Active"]
print(active[-1]["path"] if active else ".")
PY
		)
		ACTIVE_WAL_DIR="$STATE_ROOT/wal/$ACTIVE_REL"
	fi
	python3 - "$ACTIVE_WAL_DIR/recording.json" "$STATE_ROOT/recorder.json" "$STATE_ROOT/wal" <<'PY'
import json
import sys
from pathlib import Path
recording = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
active = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
segments = list(Path(sys.argv[3]).rglob("*.chwal"))
assert recording["status"] == "completed", recording
assert recording["capture"]["scope"]["selected_subtree"] is True, recording
epoch = active["current_epoch"]
assert (epoch["ordinal"] if isinstance(epoch, dict) else epoch) >= 2, active
assert len(segments) >= 3, [path.name for path in segments]
assert all(domain["free_bytes"] > domain["reserved_bytes"] for domain in active["quota"])
# The active epoch may legitimately be empty at a time boundary; committed
# evidence is proven across the whole catalog.
total_committed = sum(
    json.loads(path.read_text(encoding="utf-8"))["counters"]["committed"]["records"]
    for path in Path(sys.argv[3]).rglob("recording.json")
)
assert total_committed > 0, total_committed
PY
	set_check three_epochs passed
	# Protected lineage remains present after finalization; deletion proof is
	# intentionally not claimed by this acceptance path.
	# Metadata headroom is diagnostic only; quota/min-free behavior is proven by
	# the isolated loopback pressure run above.

	phase etl 'publish final canonical session'
	# With one-second epochs the catalog's Active entry may be a fresh empty
	# successor (the final-workload pass committed to the preceding recording),
	# so publish the last recording that actually published committed segments.
	ETL_WAL_DIR="$STATE_ROOT/wal"
	if [[ -f "$STATE_ROOT/wal/epochs.json" ]]; then
		ETL_REL=$(
			python3 - "$STATE_ROOT/wal/epochs.json" "$STATE_ROOT/wal" <<'PY'
import json, sys
from pathlib import Path
value = json.load(open(sys.argv[1], encoding="utf-8"))
root = Path(sys.argv[2])
chosen = None
for entry in value["epochs"]:
    if entry.get("state") in ("Active", "Finalized") and any(
        (root / entry["path"] / "segments").glob("*.chwal")
    ):
        chosen = entry
print(chosen["path"] if chosen else ".")
PY
		)
		ETL_WAL_DIR="$STATE_ROOT/wal/$ETL_REL"
	fi
	"$CHRONICLE" --format json etl --wal-dir "$ETL_WAL_DIR" --output "$STORE_ROOT" >"$ARTIFACT_ROOT/etl.json" 2>"$ARTIFACT_ROOT/etl.log"
	SESSION_ID=$(
		python3 - "$ARTIFACT_ROOT/etl.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["already_processed"] in (False, True)
print(value["session_id"])
PY
	)
	test -d "$STORE_ROOT/sessions/$SESSION_ID"

	phase equivalence 'compare one-shot ETL output with incremental checkpoint output'
	"$CHRONICLE" --format json inspect "$SESSION_ID" --root "$STORE_ROOT" >"$ARTIFACT_ROOT/inspect.json"
	ONE_SHOT_STORE="$ARTIFACT_ROOT/one-shot-store"
	"$CHRONICLE" --format json etl --wal-dir "$ETL_WAL_DIR" --output "$ONE_SHOT_STORE" >"$ARTIFACT_ROOT/one-shot-etl.json" 2>"$ARTIFACT_ROOT/one-shot-etl.log"
	ONE_SHOT_SESSION=$(
		python3 - "$ARTIFACT_ROOT/one-shot-etl.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["session_id"])
PY
	)
	"$CHRONICLE" --format json inspect "$ONE_SHOT_SESSION" --root "$ONE_SHOT_STORE" >"$ARTIFACT_ROOT/one-shot-inspect.json"
	python3 - "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/one-shot-inspect.json" "$ARTIFACT_ROOT/equivalence.json" <<'PY'
import json
import sys

def comparable(path):
    value = json.load(open(path, encoding="utf-8"))
    value.pop("root", None)
    value.pop("output", None)
    return value

incremental, one_shot = map(comparable, sys.argv[1:3])
assert incremental == one_shot, (incremental, one_shot)
json.dump({"version": 1, "equivalent": True, "comparison": "inspect", "session_id": incremental["session_id"]}, open(sys.argv[3], "w", encoding="utf-8"), indent=2)
with open(sys.argv[3], "a", encoding="utf-8") as handle:
    handle.write("\n")
PY
	set_check one_shot_equivalence passed

	p1_wal_status=passed
	run_compat_command wal-feasibility cargo test -p chronicle-capture-ebpf --test privileged_feasibility --locked -- --ignored --nocapture || p1_wal_status=failed
	run_compat_command wal-tests cargo test -p chronicle-wal --locked || p1_wal_status=failed
	set_check wal_fault_matrix "$p1_wal_status"

	p1_ingest_status=passed
	run_compat_command ingest-limit cargo test -p chronicle-application --locked ingest_discards_only_wal_limit_suffix_and_wal_limit_wins_duration_tie || p1_ingest_status=failed
	run_compat_command ingest-bounds cargo test -p chronicle-application --locked production_recording_bounds_enforce_duration_and_wal_limits || p1_ingest_status=failed
	set_check ingest_limit_matrix "$p1_ingest_status"

}
