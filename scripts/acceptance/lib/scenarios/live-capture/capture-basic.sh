#!/usr/bin/env bash
# shellcheck disable=SC2034 # scenario globals are consumed by profile cleanup/report
# Central-dispatch scenario: capture-basic.
scenario_live_capture_capture_basic() {
	phase 1 "Validate environment"
	[[ $(uname -s) == Linux ]] || skip "privileged runtime requires Linux; rootless fixture and eBPF compile profiles remain available"
	validate_environment

	phase 2 "Build isolated eBPF object"
	(
		cd "$ROOT/ebpf" || exit
		CARGO_TARGET_DIR="$EBPF_TARGET_DIR" \
			CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" \
			cargo build --release --locked
	) >"$ARTIFACT_ROOT/build.log" 2>&1
	assert_file "$EBPF_OBJECT"

	phase 3 "Build release Chronicle CLI"
	CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" cargo build --release -p chronicle-cli --locked >>"$ARTIFACT_ROOT/build.log" 2>&1
	assert_file "$CHRONICLE"

	phase 4 "Prepare artifacts and cgroups"
	install -d -m 0700 "$DATA_DIR"
	mkdir -p "$VALIDATION_ROOT"
	CGROUP_PARENT=$(current_cgroup_path) || die "cannot resolve current cgroup v2 path"
	[[ -w $CGROUP_PARENT/cgroup.procs ]] || die "current cgroup is not writable: $CGROUP_PARENT"
	DEDICATED_CGROUP="$CGROUP_PARENT/chronicle-live-capture-dedicated-$RUN_ID"
	SHARED_CGROUP="$CGROUP_PARENT/chronicle-live-capture-shared-$RUN_ID"
	mkdir "$DEDICATED_CGROUP" "$SHARED_CGROUP"
	sleep 600 >"$ARTIFACT_ROOT/shared-one.log" 2>&1 &
	SHARED_ONE_PID=$!
	sleep 600 >"$ARTIFACT_ROOT/shared-two.log" 2>&1 &
	SHARED_TWO_PID=$!
	printf '%s\n' "$SHARED_ONE_PID" >"$SHARED_CGROUP/cgroup.procs"
	printf '%s\n' "$SHARED_TWO_PID" >"$SHARED_CGROUP/cgroup.procs"
	if full_mode; then
		if "$CHRONICLE" --format json --data-dir "$ARTIFACT_ROOT/shared-rejected-data" record --cgroup "$SHARED_CGROUP" --duration 1 >"$ARTIFACT_ROOT/shared-rejected.json" 2>"$ARTIFACT_ROOT/shared-rejected.log"; then
			die "shared cgroup recording unexpectedly succeeded without acknowledgement"
		fi
		grep -qi 'cgroup selection invalid' "$ARTIFACT_ROOT/shared-rejected.log"
		"$CHRONICLE" --format json --data-dir "$ARTIFACT_ROOT/shared-accepted-data" record --cgroup "$SHARED_CGROUP" --allow-shared-cgroup --duration 1 >"$ARTIFACT_ROOT/shared-accepted.json" 2>"$ARTIFACT_ROOT/shared-accepted.log"
		assert_json "$ARTIFACT_ROOT/shared-accepted.json" 'value["status"] == "completed" and value["shutdown_reason"] == "duration_limit"'
		SHARED_RUNTIME_RESULT=passed
	fi

	phase 5 "Run Chronicle doctor"
	"$CHRONICLE" --format json --data-dir "$DATA_DIR" doctor >"$ARTIFACT_ROOT/doctor.json" 2>"$ARTIFACT_ROOT/doctor.log"
	assert_json "$ARTIFACT_ROOT/doctor.json" 'value["version"] == 1 and value["status"] in ("supported", "supported_with_warnings") and len(value["probes"]) >= 1'

	phase 6 "Start upstream service"
	python3 "$DRIVER" serve --port-file "$TMP_DIR/upstream.port" --requests "$ARTIFACT_ROOT/upstream-requests.jsonl" >"$ARTIFACT_ROOT/upstream.log" 2>&1 &
	UPSTREAM_PID=$!
	wait_for_file "$TMP_DIR/upstream.port" 5 || die "upstream did not become ready; see $ARTIFACT_ROOT/upstream.log"
	UPSTREAM_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/upstream.port")"

	phase 7 "Start public Chronicle recorder"
	"$CHRONICLE" --format json --data-dir "$DATA_DIR" record --cgroup "$DEDICATED_CGROUP" --duration 60 >"$ARTIFACT_ROOT/record.json" 2>"$ARTIFACT_ROOT/record.log" &
	RECORDER_PID=$!
	RECORDING_METADATA=""
	recording_metadata_ready() {
		RECORDING_METADATA=$(find "$DATA_DIR/recordings" -mindepth 2 -maxdepth 2 -name recording.json -print -quit 2>/dev/null || true)
		if [[ -z $RECORDING_METADATA ]]; then
			printf 'recording metadata absent under %s\n' "$DATA_DIR/recordings"
			return 1
		fi
		python3 - "$RECORDING_METADATA" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
print(f"metadata={sys.argv[1]} status={value.get('status')}")
raise SystemExit(0 if value.get("status") == "recording" else 1)
PY
	}
	wait_until 15 0.1 "public recorder to reach recording metadata state" recording_metadata_ready ||
		die "recorder did not reach recording status; see $ARTIFACT_ROOT/record.log"
	RECORDING_METADATA=$(find "$DATA_DIR/recordings" -mindepth 2 -maxdepth 2 -name recording.json -print -quit)
	WAL_DIR=$(dirname "$RECORDING_METADATA")

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
	assert_json "$ARTIFACT_ROOT/record.json" 'value["version"] == 1 and value["status"] == "completed" and value["shutdown_reason"] == "user_interrupt" and value["operations"] >= 3 and value["recording_id"].startswith("rec_")'
	RECORDING_REF=$(json_value "$ARTIFACT_ROOT/record.json" 'value["recording_id"]')
	assert_json "$WAL_DIR/recording.json" 'value["status"] == "completed" and value["shutdown_reason"] == "user_interrupt" and value["last_valid_commit"] is not None'

	phase 10 "Validate WAL"
	assert_dir "$WAL_DIR/segments"
	find "$WAL_DIR/segments" -type f -name '*.chwal' -print -quit | grep -q . || die "WAL has no segment files"
	"$CHRONICLE" --format json internal etl --wal-dir "$WAL_DIR" --output "$VALIDATION_ROOT" >"$ARTIFACT_ROOT/wal-validation.json" 2>"$ARTIFACT_ROOT/wal-validation.log"
	assert_json "$ARTIFACT_ROOT/wal-validation.json" 'value["version"] == 1 and value["already_processed"] is False'

	phase 11 "Verify publication and public catalog"
	"$CHRONICLE" --format json --data-dir "$DATA_DIR" list >"$ARTIFACT_ROOT/list.json" 2>"$ARTIFACT_ROOT/list.log"
	assert_json "$ARTIFACT_ROOT/list.json" 'value["version"] == 1 and any(item["recording_id"] == "'"$RECORDING_REF"'" and item["status"] == "published" for item in value["recordings"])'
	"$CHRONICLE" --format json internal etl --wal-dir "$WAL_DIR" --output "$SESSION_ROOT" >"$ARTIFACT_ROOT/etl.json" 2>"$ARTIFACT_ROOT/etl.log"
	assert_json "$ARTIFACT_ROOT/etl.json" 'value["version"] == 1 and value["already_processed"] is True'
	SESSION_ID=$(json_value "$ARTIFACT_ROOT/etl.json" 'value["session_id"]')
	assert_dir "$SESSION_ROOT/sessions/$SESSION_ID"

	phase 12 "Validate canonical session through public inspect"
	"$CHRONICLE" --format json --data-dir "$DATA_DIR" inspect "$RECORDING_REF" >"$ARTIFACT_ROOT/inspect.json" 2>"$ARTIFACT_ROOT/inspect.log"
	assert_json "$ARTIFACT_ROOT/inspect.json" 'value["version"] == 1 and value["recording_id"] == "'"$RECORDING_REF"'" and value["session_id"] == "'"$SESSION_ID"'" and value["operations"] >= 3 and value["replayability"] in ("fully_replayable", "partially_replayable") and value["executable_operations"] >= 2'
	CANONICAL_OPERATION_COUNT=$(json_value "$ARTIFACT_ROOT/inspect.json" 'value["operations"]')

	phase 13 "Replay against dedicated local target"
	python3 "$DRIVER" serve --port-file "$TMP_DIR/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 &
	REPLAY_PID=$!
	wait_for_file "$TMP_DIR/replay.port" 5 || die "replay target did not become ready; see $ARTIFACT_ROOT/replay-target.log"
	REPLAY_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/replay.port")"
	"$CHRONICLE" --format json --data-dir "$DATA_DIR" replay "$RECORDING_REF" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
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
		phase 15 "Run privileged kernel-behavior acceptance"
		# The retired feasibility harness proved kernel semantics with a
		# separate experiment probe. The production eBPF adapter is the only
		# validation authority now: this runs the production-object kernel
		# suite (hook matrix, identity, truncation, ring/loss accounting).
		cargo test -p chronicle-capture-ebpf --test privileged_kernel --locked -- --ignored --nocapture >"$ARTIFACT_ROOT/privileged-kernel.log" 2>&1
		grep -q 'test result: ok' "$ARTIFACT_ROOT/privileged-kernel.log"
	else
		skip_phase 15 "Run privileged kernel-behavior acceptance" privileged-kernel.log
	fi

}
