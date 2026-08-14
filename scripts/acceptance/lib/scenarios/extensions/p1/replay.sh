#!/usr/bin/env bash
# Central-dispatch scenario: replay.
scenario_p1_replay() {
phase 20 "Prove public inspect remains stable after WAL validation"
"$CHRONICLE" --format json --data-dir "$DATA_DIR" inspect "$RECORDING_REF" >"$ARTIFACT_ROOT/inspect-after-relocation.json" 2>"$ARTIFACT_ROOT/inspect-after-relocation.log"
assert_json "$ARTIFACT_ROOT/inspect-after-relocation.json" 'value["version"] == 1 and value["recording_id"] == "'"$RECORDING_REF"'" and value["integrity_valid"] is True'
if full_mode; then
	F2_9_RESULT=passed
fi

phase 21 "Verify replay target isolation"
python3 - "$ARTIFACT_ROOT/upstream-requests.jsonl" "$ARTIFACT_ROOT/replay-requests.jsonl" <<'PY'
import json
import sys
upstream = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
replay = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8")]
assert len(upstream) == 3, upstream
assert len(replay) >= 2, replay
assert all(item["target"] != "http://127.0.0.1" for item in replay)
PY
if full_mode; then
	F2_10_RESULT=passed
fi

if full_mode; then
	phase 22 "Run privileged signal acceptance"
	# Portable replay transport/verification matrix and cgroup decision matrix
	# are separately selected gate prerequisites (task 8.1); the privileged
	# scenario proves only the real signal/process invariant.
	cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture >"$ARTIFACT_ROOT/signal-tests.log" 2>&1
	grep -q 'test result: ok' "$ARTIFACT_ROOT/signal-tests.log"
	SIGNAL_RESULT="passed"
else
	skip_phase 22 "Run privileged signal acceptance" signal-tests.log
fi


}
