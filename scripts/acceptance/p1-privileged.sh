#!/usr/bin/env bash
# Privileged Ubuntu 24.04 production acceptance. Retains artifacts; never uses fixtures.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=scripts/lib/common.sh
source "$ROOT/scripts/lib/common.sh"
# shellcheck source=scripts/lib/env.sh
source "$ROOT/scripts/lib/env.sh"
# shellcheck source=scripts/lib/assertions.sh
source "$ROOT/scripts/lib/assertions.sh"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
DEFAULT_ARTIFACT_ROOT="$ROOT/target/p1-acceptance/$RUN_ID"
if [[ ${CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT+x} ]]; then
	REQUESTED_ARTIFACT_ROOT=$CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT
else
	REQUESTED_ARTIFACT_ROOT="$DEFAULT_ARTIFACT_ROOT"
fi
ARTIFACT_ROOT="$REQUESTED_ARTIFACT_ROOT"
canonical_path() {
	if realpath -m -- "$1" 2>/dev/null; then
		return 0
	fi
	python3 - "$1" <<'PY'
import os
import sys
print(os.path.realpath(sys.argv[1]))
PY
}

TARGET_DIR=$(canonical_path "${CARGO_TARGET_DIR:-"$ROOT/target"}")
EBPF_TARGET_DIR=$(canonical_path "${CHRONICLE_EBPF_TARGET_DIR:-"$ROOT/ebpf/target"}")
EBPF_OBJECT="$EBPF_TARGET_DIR/bpfel-unknown-none/release/chronicle-ebpf-capture"
if [[ ${CHRONICLE_ACCEPTANCE_MODE+x} ]]; then
	ACCEPTANCE_MODE=$CHRONICLE_ACCEPTANCE_MODE
else
	ACCEPTANCE_MODE=full
fi
CHRONICLE="$TARGET_DIR/debug/chronicle"
DRIVER="$ROOT/tests/e2e/http_acceptance_driver.py"
WAL_DIR="$ARTIFACT_ROOT/wal"
VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
SESSION_ROOT="$ARTIFACT_ROOT/sessions"
TMP_DIR=""
DEDICATED_CGROUP=""
SHARED_CGROUP=""
CGROUP_PARENT=""
RECORDER_PID=""
UPSTREAM_PID=""
REPLAY_PID=""
SHARED_ONE_PID=""
SHARED_TWO_PID=""
CURRENT_PHASE="initializing"
SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
TOTAL_PHASES=29
EBPF_OBJECT_SHA256="not_checked"
WAL_MATRIX_RESULT="not_checked"
INGEST_MATRIX_RESULT="not_checked"
REPLAY_MATRIX_RESULT="not_checked"
CGROUP_MATRIX_RESULT="not_checked"
SIGNAL_RESULT="not_checked"
FMT_RESULT="not_checked"
WORKSPACE_CHECK_RESULT="not_checked"
F2_8_RESULT="not_checked"
F2_9_RESULT="not_checked"
F2_10_RESULT="not_checked"
F2_11_RESULT="not_checked"
SHARED_RUNTIME_RESULT="not_checked"
EXPECTED_SHA=${CHRONICLE_ACCEPTANCE_EXPECTED_SHA:-}
START_SHA="not_checked"
END_SHA="not_checked"
TREE_CLEAN=false
COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
ARTIFACT_ROOT_ERROR=""

full_mode() {
	[[ $ACCEPTANCE_MODE == full ]]
}

smoke_mode() {
	[[ $ACCEPTANCE_MODE == smoke ]]
}

valid_acceptance_mode() {
	[[ $ACCEPTANCE_MODE == full || $ACCEPTANCE_MODE == fast || $ACCEPTANCE_MODE == smoke ]]
}

safe_artifact_root() {
	local path=$1 resolved home
	[[ -n $path && $path == /* && ! -L $path ]] || return 1
	resolved=$(canonical_path "$path") || return 1
	home=${HOME:-}
	[[ $resolved != / && $resolved != "$ROOT" && $resolved != "$home" ]] || return 1
	[[ $resolved != "$ROOT/target" && $resolved != "$ROOT/ebpf" ]] || return 1
	if [[ -e $path && ! -d $path ]]; then
		return 1
	fi
	if [[ -d $path && ! -f "$path/.chronicle-acceptance-root" ]]; then
		return 1
	fi
	return 0
}

prepare_artifact_root() {
	safe_artifact_root "$ARTIFACT_ROOT" || return 1
	mkdir -p -- "$ARTIFACT_ROOT"
	find "$ARTIFACT_ROOT" -mindepth 1 -maxdepth 1 -exec rm -rf -- {} +
	printf '%s\n' 'chronicle-p1-acceptance-root-v1' >"$ARTIFACT_ROOT/.chronicle-acceptance-root"
}

skip() {
	log "SKIP: $*" >&2
	exit 77
}

phase() {
	CURRENT_PHASE="$1/$TOTAL_PHASES"
	log "[$1/$TOTAL_PHASES] $2"
}

skip_phase() {
	local number=$1 description=$2 artifact
	shift 2
	phase "$number" "$description"
	for artifact; do
		printf '%s\n' 'Skipped in fast mode; covered by full privileged acceptance.' >"$ARTIFACT_ROOT/$artifact"
	done
	log "[$number/$TOTAL_PHASES] SKIPPED in fast mode; covered by full privileged acceptance."
}

write_summary() {
	local status=$1
	mkdir -p "$ARTIFACT_ROOT"
	local commit_sha kernel architecture cgroup_status btf_status cap_eff working_tree_dirty
	commit_sha=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf '%s' 'not_checked')
	working_tree_dirty=false
	if [[ -n $(git -C "$ROOT" status --porcelain --untracked-files=all) ]]; then
		working_tree_dirty=true
	fi
	kernel=$(uname -r 2>/dev/null || printf '%s' 'not_checked')
	architecture=$(uname -m 2>/dev/null || printf '%s' 'not_checked')
	cgroup_status="unavailable"
	btf_status="unavailable"
	cap_eff="unavailable"
	if [[ -r /proc/self/cgroup ]]; then
		cgroup_status=$(grep -q '^0::' /proc/self/cgroup && printf '%s' 'available' || printf '%s' 'unavailable')
	fi
	[[ -r /sys/kernel/btf/vmlinux ]] && btf_status="available" || true
	if [[ -r /proc/self/status ]]; then
		cap_eff=$(awk '/^CapEff:/ { print $2; exit }' /proc/self/status)
		[[ -n $cap_eff ]] || cap_eff="unavailable"
	fi
	if [[ -f "$EBPF_OBJECT" ]]; then
		EBPF_OBJECT_SHA256=$(sha256sum "$EBPF_OBJECT" | awk '{print $1}')
	fi
	python3 - "$SUMMARY" "$status" "$CURRENT_PHASE" "$commit_sha" "$kernel" "$architecture" "$cgroup_status" "$btf_status" "$cap_eff" "$EBPF_OBJECT_SHA256" "$working_tree_dirty" "$COMMAND_LOG" "$ACCEPTANCE_MODE" "$WAL_MATRIX_RESULT" "$INGEST_MATRIX_RESULT" "$REPLAY_MATRIX_RESULT" "$CGROUP_MATRIX_RESULT" "$SIGNAL_RESULT" "$FMT_RESULT" "$WORKSPACE_CHECK_RESULT" "$EXPECTED_SHA" "$START_SHA" "$END_SHA" "$TREE_CLEAN" "$F2_8_RESULT" "$F2_9_RESULT" "$F2_10_RESULT" "$F2_11_RESULT" <<'PY'
import json
import sys
from pathlib import Path

summary = Path(sys.argv[1])
status, phase = sys.argv[2:4]
commit, kernel, architecture, cgroup, btf, cap_eff, object_sha, dirty, command_log = sys.argv[4:13]
mode, wal_matrix, ingest_matrix, replay_matrix, cgroup_matrix, signal, fmt, workspace_check = sys.argv[13:21]
expected, start_sha, end_sha, tree_clean = sys.argv[21:25]
f2_8, f2_9, f2_10, f2_11 = sys.argv[25:29]
summary.parent.mkdir(parents=True, exist_ok=True)
result = "passed" if status == "0" else ("not_checked" if status == "77" else "failed")
commands = []
if Path(command_log).is_file():
    commands = [line[2:] if line.startswith("+ ") else line for line in Path(command_log).read_text(encoding="utf-8").splitlines()]
scenarios = {"F2.8": f2_8, "F2.9": f2_9, "F2.10": f2_10, "F2.11": f2_11}
retained = (
    mode == "full"
    and result == "passed"
    and expected
    and expected == commit == start_sha == end_sha
    and tree_clean == "true"
    and all(value == "passed" for value in scenarios.values())
)
summary.write_text(json.dumps({
    "version": 2,
    "git_commit_sha": commit,
    "expected_git_commit_sha": expected or None,
    "start_git_commit_sha": start_sha,
    "end_git_commit_sha": end_sha,
    "working_tree_dirty": dirty == "true",
    "tree_clean_for_entire_run": tree_clean == "true",
    "kernel_version": kernel,
    "architecture": architecture,
    "cgroup_v2": cgroup,
    "btf": btf,
    "capabilities": {
        "effective_hex": cap_eff,
        "required": ["CAP_BPF", "CAP_NET_ADMIN", "CAP_SYS_ADMIN"],
    },
    "acceptance_mode": mode,
    "ebpf_object_sha256": object_sha,
    "commands_executed": commands,
    "checks": {
        "privileged_acceptance": result,
        "p1_retained_acceptance": "complete" if retained else "not_checked",
        "privileged_scenarios": scenarios,
        "wal_fault_matrix": wal_matrix,
        "ingest_limit_matrix": ingest_matrix,
        "replay_matrix": replay_matrix,
        "cgroup_matrix": cgroup_matrix,
        "privileged_signal": signal,
        "format_check": fmt,
        "workspace_check": workspace_check,
    },
    "status": result,
    "exit_code": int(status),
    "phase": phase,
    "artifacts": {
        "root": ".",
        "wal": "wal",
        "sessions": "sessions",
        "wal_validation": "wal-validation",
        "commands": "commands.log",
    },
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

remove_cgroup() {
	local path=$1
	[[ -n $path && -d $path ]] || return 0
	local pid
	while read -r pid; do
		[[ -n $pid ]] && printf '%s\n' "$pid" >"$CGROUP_PARENT/cgroup.procs" 2>/dev/null || true
	done <"$path/cgroup.procs"
	local deadline=$((SECONDS + 5))
	while ! rmdir "$path" 2>/dev/null; do
		((SECONDS < deadline)) || return 1
		sleep 0.05
	done
}

cleanup_resources() {
	local failed=0
	stop_process "$RECORDER_PID"
	stop_process "$UPSTREAM_PID"
	stop_process "$REPLAY_PID"
	stop_process "$SHARED_ONE_PID"
	stop_process "$SHARED_TWO_PID"
	RECORDER_PID=""
	UPSTREAM_PID=""
	REPLAY_PID=""
	SHARED_ONE_PID=""
	SHARED_TWO_PID=""
	if ! remove_cgroup "$DEDICATED_CGROUP"; then
		log "ERROR: could not remove dedicated cgroup" >&2
		failed=1
	fi
	if ! remove_cgroup "$SHARED_CGROUP"; then
		log "ERROR: could not remove shared cgroup" >&2
		failed=1
	fi
	[[ -z $TMP_DIR ]] || rm -rf -- "$TMP_DIR"
	return "$failed"
}

on_exit() {
	local status=$?
	set +e
	local cleanup_status=0
	cleanup_resources || cleanup_status=$?
	END_SHA=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf '%s' 'not_checked')
	if [[ $status -eq 0 && $cleanup_status -ne 0 ]]; then
		status=1
	fi
	if full_mode; then
		if [[ $END_SHA != "$START_SHA" || -n $(git -C "$ROOT" status --porcelain --untracked-files=all) ]]; then
			TREE_CLEAN=false
			status=1
		fi
		if [[ $status -eq 0 && $SIGNAL_RESULT == passed && $CGROUP_MATRIX_RESULT == passed && $SHARED_RUNTIME_RESULT == passed ]]; then
			F2_11_RESULT=passed
		fi
	fi
	write_summary "$status"
	exit "$status"
}
trap on_exit EXIT

if ! prepare_artifact_root; then
	ARTIFACT_ROOT_ERROR="unsafe or non-dedicated artifact root: $REQUESTED_ARTIFACT_ROOT"
	ARTIFACT_ROOT="$DEFAULT_ARTIFACT_ROOT"
	WAL_DIR="$ARTIFACT_ROOT/wal"
	VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
	SESSION_ROOT="$ARTIFACT_ROOT/sessions"
	SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
	COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
	prepare_artifact_root || die "cannot initialize fallback artifact root: $ARTIFACT_ROOT"
fi
WAL_DIR="$ARTIFACT_ROOT/wal"
VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
SESSION_ROOT="$ARTIFACT_ROOT/sessions"
SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
: >"$COMMAND_LOG"
trap 'printf "%s\\n" "$BASH_COMMAND" >>"$COMMAND_LOG"' DEBUG
TMP_DIR=$(mktemp -d)

if [[ -n $ARTIFACT_ROOT_ERROR ]]; then
	die "$ARTIFACT_ROOT_ERROR; refusing to remove it; artifacts retained at $ARTIFACT_ROOT"
fi
valid_acceptance_mode || die "unsupported CHRONICLE_ACCEPTANCE_MODE=$ACCEPTANCE_MODE (expected full, smoke, or fast)"
[[ $TARGET_DIR != "$EBPF_TARGET_DIR" ]] || die "CARGO_TARGET_DIR and CHRONICLE_EBPF_TARGET_DIR must remain isolated"
START_SHA=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || die "repository HEAD unavailable")
END_SHA=$START_SHA
TREE_CLEAN=true
if [[ -n $(git -C "$ROOT" status --porcelain --untracked-files=all) ]]; then
	TREE_CLEAN=false
fi
if full_mode; then
	[[ -n $EXPECTED_SHA ]] || die "full acceptance requires CHRONICLE_ACCEPTANCE_EXPECTED_SHA"
	[[ $START_SHA == "$EXPECTED_SHA" ]] || die "expected commit $EXPECTED_SHA, found $START_SHA"
	[[ $TREE_CLEAN == true ]] || die "full acceptance requires a clean working tree"
fi
export CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR"
log "Acceptance mode: $ACCEPTANCE_MODE"

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
wait "$RECORDER_PID"
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

phase 13 "Replay against dedicated local target"
python3 "$DRIVER" serve --port-file "$TMP_DIR/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 &
REPLAY_PID=$!
wait_for_file "$TMP_DIR/replay.port" 5 || die "replay target did not become ready; see $ARTIFACT_ROOT/replay-target.log"
REPLAY_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/replay.port")"
"$CHRONICLE" --format json replay "$SESSION_ID" --root "$SESSION_ROOT" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
assert_json "$ARTIFACT_ROOT/replay.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips") and len(value["result"]["operations"]) >= 3 and all(item["verification"] == "passed" for item in value["result"]["operations"] if item["state"] == "completed")'
python3 - "$ARTIFACT_ROOT/replay-requests.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    requests = [json.loads(line) for line in source]
assert len(requests) >= 2, requests
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

phase 17 "Verify recovery report and categorized counters"
for key in received accepted_into_queue written_not_committed committed discarded_from_queue_due_to_wal_limit kernel_or_backend_dropped rejected_after_stop etl_checkpointed; do
	grep -q "\"$key\"" "$ARTIFACT_ROOT/record.json"
	grep -q "\"$key\"" "$WAL_DIR/recording.json"
done

STALE_WAL="$ARTIFACT_ROOT/stale-recovery-wal"
STALE_OUTPUT="$ARTIFACT_ROOT/stale-recovery-output"
cp -a "$WAL_DIR" "$STALE_WAL"
python3 - "$STALE_WAL/recording.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
metadata = json.loads(path.read_text(encoding="utf-8"))
metadata["status"] = "recording"
metadata["shutdown_reason"] = None
path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
"$CHRONICLE" --format json etl --wal-dir "$STALE_WAL" --output "$STALE_OUTPUT" >"$ARTIFACT_ROOT/stale-recovery-etl.json" 2>"$ARTIFACT_ROOT/stale-recovery-etl.log"
assert_json "$ARTIFACT_ROOT/stale-recovery-etl.json" 'value["status"] == "aborted" and value["shutdown_reason"] == "process_crash_recovered" and value["checkpoint"]["status"] == "aborted"'
assert_json "$STALE_WAL/etl/recovery-report.json" 'value["status"] == "aborted" and value["shutdown_reason"] == "process_crash_recovered"'
if full_mode; then
	F2_8_RESULT=passed
fi

phase 18 "Rerun ETL in same output root"
"$CHRONICLE" --format json etl --wal-dir "$WAL_DIR" --output "$SESSION_ROOT" >"$ARTIFACT_ROOT/etl-rerun.json" 2>"$ARTIFACT_ROOT/etl-rerun.log"
assert_json "$ARTIFACT_ROOT/etl-rerun.json" 'value["already_processed"] is True and value["session_id"] == "'"$SESSION_ID"'"'

phase 19 "Publish ETL into second output root"
SECOND_ROOT="$ARTIFACT_ROOT/second-sessions"
"$CHRONICLE" --format json etl --wal-dir "$WAL_DIR" --output "$SECOND_ROOT" >"$ARTIFACT_ROOT/etl-second-root.json" 2>"$ARTIFACT_ROOT/etl-second-root.log"
assert_json "$ARTIFACT_ROOT/etl-second-root.json" 'value["already_processed"] is False and value["session_id"] == "'"$SESSION_ID"'"'
assert_dir "$SECOND_ROOT/sessions/$SESSION_ID"

phase 20 "Prove inspect survives source relocation"
mv "$WAL_DIR" "$ARTIFACT_ROOT/wal-relocated"
"$CHRONICLE" --format json inspect "$SESSION_ID" --root "$SESSION_ROOT" >"$ARTIFACT_ROOT/inspect-after-relocation.json" 2>"$ARTIFACT_ROOT/inspect-after-relocation.log"
assert_json "$ARTIFACT_ROOT/inspect-after-relocation.json" 'value["version"] == 1 and value["integrity_valid"] is True'
WAL_DIR="$ARTIFACT_ROOT/wal-relocated"
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

phase 25 "Verify doctor has no persistent side effects"
[[ ! -e "$ARTIFACT_ROOT/doctor-wal/recording.json" ]] || die "doctor created recording metadata"
[[ ! -e "$ARTIFACT_ROOT/doctor-output/sessions" ]] || die "doctor published session"
assert_json "$ARTIFACT_ROOT/doctor.json" 'value["version"] == 1 and value["status"] in ("supported", "supported_with_warnings")'

phase 26 "Verify safe command JSON artifacts"
python3 - "$ARTIFACT_ROOT" <<'PY'
import json
import sys
from pathlib import Path
root = Path(sys.argv[1])
for path in [root / "record.json", root / "etl.json", root / "etl-rerun.json", root / "etl-second-root.json", root / "inspect.json", root / "replay.json", root / "doctor.json"]:
    value = json.loads(path.read_text(encoding="utf-8"))
    assert value.get("version") == 1, path
    assert "command line" not in path.read_text(encoding="utf-8").lower()
PY

if full_mode; then
	phase 27 "Run final workspace checks"
	cargo fmt --all --check >"$ARTIFACT_ROOT/fmt.log" 2>&1
	FMT_RESULT="passed"
	cargo check --workspace --all-targets --locked >"$ARTIFACT_ROOT/check.log" 2>&1
	WORKSPACE_CHECK_RESULT="passed"
else
	skip_phase 27 "Run final workspace checks" fmt.log check.log
fi

phase 28 "Collect artifacts"
for artifact in "$ARTIFACT_ROOT/record.log" "$ARTIFACT_ROOT/etl.log" "$ARTIFACT_ROOT/replay.log" "$ARTIFACT_ROOT/doctor.json" "$ARTIFACT_ROOT/record.json" "$ARTIFACT_ROOT/etl.json" "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/replay.json" "$ARTIFACT_ROOT/wal-tests.log" "$ARTIFACT_ROOT/privileged-feasibility.log" "$ARTIFACT_ROOT/signal-tests.log" "$WAL_DIR/recording.json"; do
	assert_file "$artifact"
done

phase 29 "Cleanup processes, cgroups, and temporary files"
cleanup_resources
log "PASS: privileged acceptance artifacts retained at $ARTIFACT_ROOT"
