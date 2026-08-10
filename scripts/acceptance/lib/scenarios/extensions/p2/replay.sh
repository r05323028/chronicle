#!/usr/bin/env bash
# Central-dispatch scenario: replay.
scenario_p2_replay() {
phase inspect_replay 'inspect and replay through public recording identity'
"$CHRONICLE" --format json --data-dir "$P2_DATA_DIR" inspect "$P2_RECORDING_REF" >"$ARTIFACT_ROOT/inspect.json" 2>"$ARTIFACT_ROOT/inspect.log"
python3 - "$ARTIFACT_ROOT/inspect.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["integrity_valid"] is True
assert value["operations"] >= 3
assert value["replayability"] in ("fully_replayable", "partially_replayable")
assert value["executable_operations"] >= 2
PY
python3 "$DRIVER" serve --port-file "$ARTIFACT_ROOT/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 &
REPLAY_PID=$!
for _ in $(seq 1 100); do
	[[ -s "$ARTIFACT_ROOT/replay.port" ]] && break
	sleep 0.1
done
REPLAY_ORIGIN="http://127.0.0.1:$(cat "$ARTIFACT_ROOT/replay.port")"
"$CHRONICLE" --format json --data-dir "$P2_DATA_DIR" replay "$P2_RECORDING_REF" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
kill "$REPLAY_PID" 2>/dev/null || true
python3 - "$ARTIFACT_ROOT/replay.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["result"]["outcome"] in ("completed", "completed_with_skips")
assert all(item["verification"] == "passed" for item in value["result"]["operations"] if item["state"] == "completed")
PY
set_check inspect_replay passed
if run_compat_command replay-matrix cargo test -p chronicle-replay --locked execution_stops_on_transport_or_verification_and_accounts_for_skips; then
	set_check replay_matrix passed
else
	set_check replay_matrix failed
fi

}
