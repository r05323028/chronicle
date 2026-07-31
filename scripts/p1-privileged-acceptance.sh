#!/usr/bin/env bash
# Privileged Ubuntu 24.04 production acceptance. Retains artifacts; never uses fixtures.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=scripts/lib/common.sh
source "$ROOT/scripts/lib/common.sh"
# shellcheck source=scripts/lib/env.sh
source "$ROOT/scripts/lib/env.sh"
# shellcheck source=scripts/lib/assertions.sh
source "$ROOT/scripts/lib/assertions.sh"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ARTIFACT_ROOT=${CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT:-"$ROOT/target/p1-acceptance/$RUN_ID"}
TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
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
TOTAL_PHASES=30
OPENSPEC_RESULT="not_checked"
EBPF_OBJECT_SHA256="not_checked"
COMMAND_LOG="$ARTIFACT_ROOT/commands.log"

skip() {
  log "SKIP: $*" >&2
  exit 77
}

phase() {
  CURRENT_PHASE="$1/$TOTAL_PHASES"
  log "[$1/$TOTAL_PHASES] $2"
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
  if [[ -f "$ROOT/ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture" ]]; then
    EBPF_OBJECT_SHA256=$(sha256sum "$ROOT/ebpf/target/bpfel-unknown-none/release/chronicle-ebpf-capture" | awk '{print $1}')
  fi
  python3 - "$SUMMARY" "$status" "$CURRENT_PHASE" "$ARTIFACT_ROOT" "$WAL_DIR" "$SESSION_ROOT" "$VALIDATION_ROOT" "$commit_sha" "$kernel" "$architecture" "$cgroup_status" "$btf_status" "$cap_eff" "$EBPF_OBJECT_SHA256" "$OPENSPEC_RESULT" "$working_tree_dirty" "$COMMAND_LOG" <<'PY'
import json
import sys
from pathlib import Path

summary = Path(sys.argv[1])
status, phase = sys.argv[2:4]
artifacts, wal, sessions, validation = map(Path, sys.argv[4:8])
commit, kernel, architecture, cgroup, btf, cap_eff, object_sha, openspec, dirty, command_log = sys.argv[8:]
summary.parent.mkdir(parents=True, exist_ok=True)
result = "passed" if status == "0" else ("not_checked" if status == "77" else "failed")
commands = []
if Path(command_log).is_file():
    commands = [line[2:] if line.startswith("+ ") else line for line in Path(command_log).read_text(encoding="utf-8").splitlines()]
summary.write_text(json.dumps({
    "version": 1,
    "git_commit_sha": commit,
    "working_tree_dirty": dirty == "true",
    "kernel_version": kernel,
    "architecture": architecture,
    "cgroup_v2": cgroup,
    "btf": btf,
    "capabilities": {
        "effective_hex": cap_eff,
        "required": ["CAP_BPF", "CAP_NET_ADMIN", "CAP_SYS_ADMIN"],
    },
    "ebpf_object_sha256": object_sha,
    "commands_executed": commands,
    "checks": {
        "privileged_acceptance": result,
        "openspec_validation": openspec,
    },
    "status": result,
    "exit_code": int(status),
    "phase": phase,
    "artifacts": {
        "root": str(artifacts),
        "wal": str(wal),
        "sessions": str(sessions),
        "wal_validation": str(validation),
        "commands": str(Path(command_log)),
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
    (( SECONDS < deadline )) || return 1
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
  RECORDER_PID=""; UPSTREAM_PID=""; REPLAY_PID=""
  SHARED_ONE_PID=""; SHARED_TWO_PID=""
  if ! remove_cgroup "$DEDICATED_CGROUP"; then
    log "ERROR: could not remove dedicated cgroup" >&2
    failed=1
  fi
  if ! remove_cgroup "$SHARED_CGROUP"; then
    log "ERROR: could not remove shared cgroup" >&2
    failed=1
  fi
  [[ -z $TMP_DIR ]] || rm -rf "$TMP_DIR"
  return "$failed"
}

on_exit() {
  local status=$?
  set +e
  local cleanup_status=0
  cleanup_resources || cleanup_status=$?
  if [[ $status -eq 0 && $cleanup_status -ne 0 ]]; then
    status=1
  fi
  write_summary "$status"
  exit "$status"
}
trap on_exit EXIT

mkdir -p "$ARTIFACT_ROOT"
: >"$COMMAND_LOG"
trap 'printf "%s\\n" "$BASH_COMMAND" >>"$COMMAND_LOG"' DEBUG
TMP_DIR=$(mktemp -d)

phase 1 "Validate environment"
[[ $(uname -s) == Linux ]] || skip "privileged runtime requires Linux; rootless fixture and eBPF compile profiles remain available"
validate_environment

phase 2 "Build rootless workspace"
cargo build --workspace --locked >"$ARTIFACT_ROOT/build.log" 2>&1

phase 3 "Build isolated eBPF object"
(cd "$ROOT/ebpf" && env -u CARGO_TARGET_DIR cargo build --release --locked) >>"$ARTIFACT_ROOT/build.log" 2>&1
cargo build -p chronicle-cli --features linux-ebpf --locked >>"$ARTIFACT_ROOT/build.log" 2>&1
assert_file "$CHRONICLE"

phase 4 "Prepare artifacts and cgroups"
mkdir -p "$SESSION_ROOT" "$VALIDATION_ROOT"
CGROUP_PARENT=$(current_cgroup_path) || die "cannot resolve current cgroup v2 path"
[[ -w $CGROUP_PARENT/cgroup.procs ]] || die "current cgroup is not writable: $CGROUP_PARENT"
DEDICATED_CGROUP="$CGROUP_PARENT/chronicle-p1-dedicated-$RUN_ID"
SHARED_CGROUP="$CGROUP_PARENT/chronicle-p1-shared-$RUN_ID"
mkdir "$DEDICATED_CGROUP" "$SHARED_CGROUP"
sleep 600 >"$ARTIFACT_ROOT/shared-one.log" 2>&1 & SHARED_ONE_PID=$!
sleep 600 >"$ARTIFACT_ROOT/shared-two.log" 2>&1 & SHARED_TWO_PID=$!
printf '%s\n' "$SHARED_ONE_PID" >"$SHARED_CGROUP/cgroup.procs"
printf '%s\n' "$SHARED_TWO_PID" >"$SHARED_CGROUP/cgroup.procs"

phase 5 "Run Chronicle doctor"
"$CHRONICLE" --format json doctor --wal-dir "$ARTIFACT_ROOT/doctor-wal" --output "$ARTIFACT_ROOT/doctor-output" >"$ARTIFACT_ROOT/doctor.json" 2>"$ARTIFACT_ROOT/doctor.log"
assert_json "$ARTIFACT_ROOT/doctor.json" 'value["version"] == 1 and value["status"] in ("supported", "supported_with_warnings")'

phase 6 "Start upstream service"
python3 "$DRIVER" serve --port-file "$TMP_DIR/upstream.port" --requests "$ARTIFACT_ROOT/upstream-requests.jsonl" >"$ARTIFACT_ROOT/upstream.log" 2>&1 & UPSTREAM_PID=$!
wait_for_file "$TMP_DIR/upstream.port" 5 || die "upstream did not become ready; see $ARTIFACT_ROOT/upstream.log"
UPSTREAM_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/upstream.port")"

phase 7 "Start Chronicle recorder"
"$CHRONICLE" --format json record --source ebpf --cgroup "$DEDICATED_CGROUP" --wal-dir "$WAL_DIR" --duration-seconds 60 --segment-bytes 16777216 --max-wal-bytes 16777216 >"$ARTIFACT_ROOT/record.json" 2>"$ARTIFACT_ROOT/record.log" & RECORDER_PID=$!
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
python3 "$DRIVER" serve --port-file "$TMP_DIR/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 & REPLAY_PID=$!
wait_for_file "$TMP_DIR/replay.port" 5 || die "replay target did not become ready; see $ARTIFACT_ROOT/replay-target.log"
REPLAY_ORIGIN="http://127.0.0.1:$(<"$TMP_DIR/replay.port")"
"$CHRONICLE" --format json replay "$SESSION_ID" --root "$SESSION_ROOT" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
assert_json "$ARTIFACT_ROOT/replay.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips") and len(value["result"]["operations"]) >= 3 and all(item["verification"] == "passed" for item in value["result"]["operations"] if item["state"] == "completed")'
python3 - "$ARTIFACT_ROOT/replay-requests.jsonl" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    requests = [json.loads(line) for line in source]
assert len(requests) == 3, requests
PY

phase 14 "Verify active recording metadata"
assert_json "$WAL_DIR/recording.json" 'value["version"] == 1 and value["status"] == "completed" and value["capture"]["scope"]["selected_subtree"] is True and value["capture"]["scope"]["direct_tgid_count"] >= 0 and value["capture"]["scope"]["descendant_cgroup_count"] == 0'

phase 15 "Run WAL commit and recovery fault matrix"
cargo test -p chronicle-wal --locked >"$ARTIFACT_ROOT/wal-tests.log" 2>&1
assert_file "$ARTIFACT_ROOT/wal-tests.log"
grep -q 'test result: ok' "$ARTIFACT_ROOT/wal-tests.log"

phase 16 "Run bounded ingest and WAL-limit matrix"
cargo test -p chronicle-application --locked ingest_discards_only_wal_limit_suffix_and_wal_limit_wins_duration_tie >"$ARTIFACT_ROOT/ingest-limit-tests.log" 2>&1
cargo test -p chronicle-application --locked production_recording_bounds_enforce_duration_and_wal_limits >>"$ARTIFACT_ROOT/ingest-limit-tests.log" 2>&1
grep -q 'test result: ok' "$ARTIFACT_ROOT/ingest-limit-tests.log"

phase 17 "Verify recovery report and categorized counters"
for key in received accepted_into_queue written_not_committed committed discarded_from_queue_due_to_wal_limit kernel_or_backend_dropped rejected_after_stop etl_checkpointed; do
  grep -q "\"$key\"" "$ARTIFACT_ROOT/record.json"
  grep -q "\"$key\"" "$WAL_DIR/recording.json"
done

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

phase 21 "Verify replay target isolation"
python3 - "$ARTIFACT_ROOT/upstream-requests.jsonl" "$ARTIFACT_ROOT/replay-requests.jsonl" <<'PY'
import json
import sys
upstream = [json.loads(line) for line in open(sys.argv[1], encoding="utf-8")]
replay = [json.loads(line) for line in open(sys.argv[2], encoding="utf-8")]
assert len(upstream) == 3, upstream
assert len(replay) == 3, replay
assert all(item["target"] != "http://127.0.0.1" for item in replay)
PY

phase 22 "Run mixed replay transport and verification matrix"
cargo test -p chronicle-replay --locked execution_stops_on_transport_or_verification_and_accounts_for_skips >"$ARTIFACT_ROOT/replay-tests.log" 2>&1
grep -q 'test result: ok' "$ARTIFACT_ROOT/replay-tests.log"

phase 23 "Run cgroup scope and identity matrix"
cargo test -p chronicle-application --locked deduplicates_threads_and_safe_result_is_count_only >"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
cargo test -p chronicle-application --locked shared_posix_group_and_descendants_need_acknowledgement >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
cargo test -p chronicle-application --locked pid_requires_exact_direct_tgid_even_with_acknowledgement >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
cargo test -p chronicle-application --locked recorder_descendant_race_namespace_and_forbidden_scope_fail_closed >>"$ARTIFACT_ROOT/cgroup-tests.log" 2>&1
grep -q 'test result: ok' "$ARTIFACT_ROOT/cgroup-tests.log"

phase 24 "Run graceful signal acceptance"
cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture >"$ARTIFACT_ROOT/signal-tests.log" 2>&1
grep -q 'test result: ok' "$ARTIFACT_ROOT/signal-tests.log"

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

phase 27 "Validate stale-language guard"
if command -v openspec >/dev/null 2>&1; then
  openspec validate add-production-http-recording-pipeline --strict >"$ARTIFACT_ROOT/openspec-validation.log" 2>&1
  OPENSPEC_RESULT="passed"
else
  printf '%s\n' 'OpenSpec CLI not installed in VM; validation runs in repository CI.' >"$ARTIFACT_ROOT/openspec-validation.log"
fi
if grep -R -nE 'no eBPF program exists|scaffold-only|future eBPF work' README.md docs/ >/dev/null; then
  die "stale P0 language remains in docs"
fi

phase 28 "Run final workspace checks"
cargo fmt --all --check >"$ARTIFACT_ROOT/fmt.log" 2>&1
cargo check --workspace --all-targets --locked >"$ARTIFACT_ROOT/check.log" 2>&1

phase 29 "Collect artifacts"
for artifact in "$ARTIFACT_ROOT/record.log" "$ARTIFACT_ROOT/etl.log" "$ARTIFACT_ROOT/replay.log" "$ARTIFACT_ROOT/doctor.json" "$ARTIFACT_ROOT/record.json" "$ARTIFACT_ROOT/etl.json" "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/replay.json" "$ARTIFACT_ROOT/wal-tests.log" "$ARTIFACT_ROOT/signal-tests.log" "$WAL_DIR/recording.json"; do
  assert_file "$artifact"
done
write_summary 0

phase 30 "Cleanup processes, cgroups, and temporary files"
cleanup_resources
log "PASS: privileged acceptance artifacts retained at $ARTIFACT_ROOT"
