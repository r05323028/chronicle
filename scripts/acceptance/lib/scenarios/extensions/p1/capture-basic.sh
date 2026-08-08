#!/usr/bin/env bash
# shellcheck disable=SC2034 # scenario globals are consumed by profile cleanup/report
# Central-dispatch scenario: capture-basic.
scenario_p1_capture_basic() {
	phase 1 "Validate environment"
	[[ $(uname -s) == Linux ]] || skip "privileged runtime requires Linux; rootless fixture and eBPF compile profiles remain available"
	validate_environment

	phase 2 "Build isolated eBPF object"
	(
		cd "$ROOT/ebpf"
		CARGO_TARGET_DIR="$EBPF_TARGET_DIR" \
			CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" \
			cargo build --release --locked
	) >"$ARTIFACT_ROOT/build.log" 2>&1
	assert_file "$EBPF_OBJECT"

	phase 3 "Build Chronicle CLI with linux-ebpf"
	CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" cargo build -p chronicle-cli --features linux-ebpf --locked >>"$ARTIFACT_ROOT/build.log" 2>&1
	assert_file "$CHRONICLE"

	phase 4 "Prepare artifacts and cgroups"
	mkdir -p "$SESSION_ROOT" "$VALIDATION_ROOT"
	CGROUP_PARENT=$(current_cgroup_path) || die "cannot resolve current cgroup v2 path"
	[[ -w $CGROUP_PARENT/cgroup.procs ]] || die "current cgroup is not writable: $CGROUP_PARENT"
	DEDICATED_CGROUP="$CGROUP_PARENT/chronicle-p1-dedicated-$RUN_ID"
	SHARED_CGROUP="$CGROUP_PARENT/chronicle-p1-shared-$RUN_ID"
	mkdir "$DEDICATED_CGROUP" "$SHARED_CGROUP"
	sleep 600 >"$ARTIFACT_ROOT/shared-one.log" 2>&1 &
	SHARED_ONE_PID=$!
	sleep 600 >"$ARTIFACT_ROOT/shared-two.log" 2>&1 &
	SHARED_TWO_PID=$!
	printf '%s\n' "$SHARED_ONE_PID" >"$SHARED_CGROUP/cgroup.procs"
	printf '%s\n' "$SHARED_TWO_PID" >"$SHARED_CGROUP/cgroup.procs"
	if full_mode; then
		if "$CHRONICLE" --format json record --source ebpf --cgroup "$SHARED_CGROUP" --wal-dir "$ARTIFACT_ROOT/shared-rejected-wal" --duration-seconds 1 --segment-bytes 16777216 --max-wal-bytes 16777216 >"$ARTIFACT_ROOT/shared-rejected.json" 2>"$ARTIFACT_ROOT/shared-rejected.log"; then
			die "shared cgroup recording unexpectedly succeeded without acknowledgement"
		fi
		grep -qi 'cgroup selection invalid' "$ARTIFACT_ROOT/shared-rejected.log"
		"$CHRONICLE" --format json record --source ebpf --cgroup "$SHARED_CGROUP" --allow-shared-cgroup --wal-dir "$ARTIFACT_ROOT/shared-accepted-wal" --duration-seconds 1 --segment-bytes 16777216 --max-wal-bytes 16777216 >"$ARTIFACT_ROOT/shared-accepted.json" 2>"$ARTIFACT_ROOT/shared-accepted.log"
		assert_json "$ARTIFACT_ROOT/shared-accepted.json" 'value["status"] == "completed" and value["shutdown_reason"] == "duration_limit"'
		SHARED_RUNTIME_RESULT=passed
	fi

	phase 5 "Run Chronicle doctor"
	"$CHRONICLE" --format json doctor --wal-dir "$ARTIFACT_ROOT/doctor-wal" --output "$ARTIFACT_ROOT/doctor-output" >"$ARTIFACT_ROOT/doctor.json" 2>"$ARTIFACT_ROOT/doctor.log"
	assert_json "$ARTIFACT_ROOT/doctor.json" 'value["version"] == 1 and value["status"] in ("supported", "supported_with_warnings")'

	phase 6 "Start upstream service"
	python3 "$DRIVER" serve --port-file "$TMP_DIR/upstream.port" --requests "$ARTIFACT_ROOT/upstream-requests.jsonl" >"$ARTIFACT_ROOT/upstream.log" 2>&1 &
	UPSTREAM_PID=$!
	wait_for_file "$TMP_DIR/upstream.port" 5 || die "upstream did not become ready; see $ARTIFACT_ROOT/upstream.log"
	UPSTREAM_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/upstream.port")"

	phase 7 "Start Chronicle recorder"
	"$CHRONICLE" --format json record --source ebpf --cgroup "$DEDICATED_CGROUP" --wal-dir "$WAL_DIR" --duration-seconds 60 --segment-bytes 16777216 --max-wal-bytes 16777216 >"$ARTIFACT_ROOT/record.json" 2>"$ARTIFACT_ROOT/record.log" &
	RECORDER_PID=$!
	wait_for_json "$WAL_DIR/recording.json" 'value["status"] == "recording"' 15 || die "recorder did not become ready; see $ARTIFACT_ROOT/record.log"

	phase 8 "Generate real HTTP workload"
	bash -c 'printf "%s\n" "$$" >"$1/cgroup.procs"; exec "$2" "$3" workload --origin "$4"' bash "$DEDICATED_CGROUP" python3 "$DRIVER" "$UPSTREAM_ORIGIN" >"$ARTIFACT_ROOT/workload.json" 2>"$ARTIFACT_ROOT/workload.log"
	assert_json "$ARTIFACT_ROOT/workload.json" 'value["status"] == "ok" and value["requests"] >= 3 and value["connections"] >= 2'
	python3 - "$ARTIFACT_ROOT/upstream-requests.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    requests = [json.loads(line) for line in source]
assert [(item["method"], item["target"]) for item in requests] == [
    ("GET", "/content-length"), ("GET", "/chunked"), ("POST", "/echo"),
]
PY

	phase 9 "Stop recording and flush WAL"
	kill -INT "$RECORDER_PID"
	if ! wait_for_process_exit "$RECORDER_PID" 30; then
		printf 'recorder did not stop within 30s after SIGINT\n' >&2
		stop_process "$RECORDER_PID"
		exit 1
	fi
	RECORDER_PID=""
	assert_json "$ARTIFACT_ROOT/record.json" 'value["status"] == "completed" and value["shutdown_reason"] == "user_interrupt" and value["last_valid_commit"] is not None and value["physical_wal_bytes"] > 0 and value["physical_wal_bytes"] <= value["effective_bounds"]["max_wal_bytes"]'
	assert_json "$WAL_DIR/recording.json" 'value["status"] == "completed" and value["shutdown_reason"] == "user_interrupt" and value["last_valid_commit"] is not None'

	phase 10 "Validate WAL"
	assert_dir "$WAL_DIR/segments"
	find "$WAL_DIR/segments" -type f -name '*.chwal' -print -quit | grep -q . || die "WAL has no segment files"
	"$CHRONICLE" --format json etl --wal-dir "$WAL_DIR" --output "$VALIDATION_ROOT" >"$ARTIFACT_ROOT/wal-validation.json" 2>"$ARTIFACT_ROOT/wal-validation.log"
	assert_json "$ARTIFACT_ROOT/wal-validation.json" 'value["version"] == 1 and value["already_processed"] is False'

	phase 11 "Run ETL"
	"$CHRONICLE" --format json etl --wal-dir "$WAL_DIR" --output "$SESSION_ROOT" >"$ARTIFACT_ROOT/etl.json" 2>"$ARTIFACT_ROOT/etl.log"
	assert_json "$ARTIFACT_ROOT/etl.json" 'value["version"] == 1 and value["already_processed"] is False'
	SESSION_ID=$(json_value "$ARTIFACT_ROOT/etl.json" 'value["session_id"]')
	assert_dir "$SESSION_ROOT/sessions/$SESSION_ID"

	phase 12 "Validate canonical session and inspect"
	"$CHRONICLE" --format json inspect "$SESSION_ID" --root "$SESSION_ROOT" >"$ARTIFACT_ROOT/inspect.json" 2>"$ARTIFACT_ROOT/inspect.log"
	assert_json "$ARTIFACT_ROOT/inspect.json" 'value["version"] == 1 and len(value["connections"]) >= 1 and sum(len(connection["operations"]) for connection in value["connections"]) >= 3 and all("client" in connection and "server" in connection for connection in value["connections"]) and all(operation["method"] and operation["target"] and operation["response_status"] for connection in value["connections"] for operation in connection["operations"])'
	CANONICAL_OPERATION_COUNT=$(
		python3 - "$ARTIFACT_ROOT/inspect.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
print(sum(len(connection["operations"]) for connection in value["connections"]))
PY
	)

	phase 13 "Replay against dedicated local target"
	python3 "$DRIVER" serve --port-file "$TMP_DIR/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 &
	REPLAY_PID=$!
	wait_for_file "$TMP_DIR/replay.port" 5 || die "replay target did not become ready; see $ARTIFACT_ROOT/replay-target.log"
	REPLAY_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/replay.port")"
	"$CHRONICLE" --format json replay "$SESSION_ID" --root "$SESSION_ROOT" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
	assert_json "$ARTIFACT_ROOT/replay.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips") and len(value["result"]["operations"]) == int("'"$CANONICAL_OPERATION_COUNT"'") and all(item["verification"] == "passed" for item in value["result"]["operations"] if item["state"] == "completed")'
	python3 - "$ARTIFACT_ROOT/replay-requests.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    requests = [json.loads(line) for line in source]
assert len(requests) >= 2, requests
PY
	python3 - "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/replay.json" <<'PY'
import json, sys
canonical = json.load(open(sys.argv[1], encoding="utf-8"))
replay = json.load(open(sys.argv[2], encoding="utf-8"))
canonical_operations = [
    operation
    for connection in canonical["connections"]
    for operation in connection["operations"]
]
results = replay["result"]["operations"]
assert len(results) == len(canonical_operations), (canonical_operations, replay)
assert len(results) == len(canonical_operations), (canonical_operations, results)
assert all(
    item["verification"] == "passed"
    for item in results
    if item["state"] == "completed"
)
PY

	phase 14 "Verify active recording metadata"
	assert_json "$WAL_DIR/recording.json" 'value["version"] == 1 and value["status"] == "completed" and value["capture"]["scope"]["selected_subtree"] is True and value["capture"]["scope"]["direct_tgid_count"] >= 0 and value["capture"]["scope"]["descendant_cgroup_count"] == 0'

	if full_mode; then
		phase 15 "Run privileged capture feasibility and automated WAL fault matrix"
		cargo test -p chronicle-capture-ebpf --test privileged_feasibility --locked -- --ignored --nocapture >"$ARTIFACT_ROOT/privileged-feasibility.log" 2>&1
		grep -q 'test result: ok' "$ARTIFACT_ROOT/privileged-feasibility.log"
		cargo test -p chronicle-wal --locked >"$ARTIFACT_ROOT/wal-tests.log" 2>&1
		assert_file "$ARTIFACT_ROOT/wal-tests.log"
		grep -q 'test result: ok' "$ARTIFACT_ROOT/wal-tests.log"
		WAL_MATRIX_RESULT="passed"
	else
		skip_phase 15 "Run privileged capture feasibility and automated WAL fault matrix" privileged-feasibility.log wal-tests.log
	fi

	if full_mode; then
		phase 16 "Run bounded ingest and WAL-limit matrix"
		cargo test -p chronicle-application --locked ingest_discards_only_wal_limit_suffix_and_wal_limit_wins_duration_tie >"$ARTIFACT_ROOT/ingest-limit-tests.log" 2>&1
		cargo test -p chronicle-application --locked production_recording_bounds_enforce_duration_and_wal_limits >>"$ARTIFACT_ROOT/ingest-limit-tests.log" 2>&1
		grep -q 'test result: ok' "$ARTIFACT_ROOT/ingest-limit-tests.log"
		INGEST_MATRIX_RESULT="passed"
	else
		skip_phase 16 "Run bounded ingest and WAL-limit matrix" ingest-limit-tests.log
	fi

}
