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
	phase 22 "Run mixed replay transport and verification matrix"
	cargo test -p chronicle-replay --locked execution_stops_on_transport_or_verification_and_accounts_for_skips >"$ARTIFACT_ROOT/replay-tests.log" 2>&1
	grep -q 'test result: ok' "$ARTIFACT_ROOT/replay-tests.log"
	REPLAY_MATRIX_RESULT="passed"
else
	skip_phase 22 "Run mixed replay transport and verification matrix" replay-tests.log
fi

if full_mode; then
	phase 23 "Run cgroup scope and identity matrix"
	cargo test -p chronicle-application --locked deduplicates_threads_and_safe_result_is_count_only >"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
	cargo test -p chronicle-application --locked shared_posix_group_and_descendants_need_acknowledgement >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
	cargo test -p chronicle-application --locked pid_requires_exact_direct_tgid_even_with_acknowledgement >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
	cargo test -p chronicle-application --locked recorder_descendant_race_namespace_and_forbidden_scope_fail_closed >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
	grep -q 'test result: ok' "$ARTIFACT_ROOT/cgroup-tests.log"
	CGROUP_MATRIX_RESULT="passed"
else
	skip_phase 23 "Run cgroup scope and identity matrix" cgroup-tests.log
fi

if full_mode; then
	phase 24 "Run graceful signal acceptance"
	cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture >"$ARTIFACT_ROOT/signal-tests.log" 2>&1
	grep -q 'test result: ok' "$ARTIFACT_ROOT/signal-tests.log"
	SIGNAL_RESULT="passed"
else
	skip_phase 24 "Run graceful signal acceptance" signal-tests.log
fi


}
