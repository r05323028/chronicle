#!/usr/bin/env bash
# shellcheck disable=SC2034 # runtime variables are consumed by sourced scenario modules
# live-capture runtime setup/cleanup. Assertions run through scenario-dispatch.sh.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
if [[ ${CHRONICLE_ACCEPTANCE_GATE_WRAPPED:-0} != 1 ]]; then
	gate_timeout=${CHRONICLE_ACCEPTANCE_GATE_TIMEOUT_SECONDS:-3600}
	[[ $gate_timeout =~ ^[1-9][0-9]*$ ]] || {
		printf '%s\n' 'CHRONICLE_ACCEPTANCE_GATE_TIMEOUT_SECONDS must be a positive integer' >&2
		exit 2
	}
	cleanup_grace=${CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS:-180}
	command_grace=${CHRONICLE_TIMEOUT_GRACE_SECONDS:-5}
	[[ $cleanup_grace =~ ^[1-9][0-9]*$ ]] || {
		printf '%s\n' 'CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS must be a positive integer' >&2
		exit 2
	}
	timeout_env=(CHRONICLE_ACCEPTANCE_GATE_WRAPPED=1 CHRONICLE_TIMEOUT_LAYER=acceptance_gate CHRONICLE_TIMEOUT_NAME=live-capture CHRONICLE_TIMEOUT_GRACE_SECONDS="$cleanup_grace")
	if [[ -n ${CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT:-} ]]; then
		timeout_env+=(
			CHRONICLE_TIMEOUT_EVIDENCE_FILE="$CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT/gate-timeout.json"
			CHRONICLE_TIMEOUT_PHASE_FILE="$CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT/current-phase.txt"
		)
	fi
	exec env "${timeout_env[@]}" "$ROOT/scripts/run-with-timeout.sh" "$gate_timeout" env CHRONICLE_TIMEOUT_GRACE_SECONDS="$command_grace" bash "$0" "$@"
fi
# shellcheck source=scripts/lib/common.sh
source "$ROOT/scripts/lib/common.sh"
# shellcheck source=scripts/lib/env.sh
source "$ROOT/scripts/lib/env.sh"
# shellcheck source=scripts/lib/assertions.sh
source "$ROOT/scripts/lib/assertions.sh"

RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
DEFAULT_ARTIFACT_ROOT="$ROOT/target/live-capture-acceptance/$RUN_ID"
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
RELEASE_MODE=${CHRONICLE_ACCEPTANCE_RELEASE:-0}
CHRONICLE="$TARGET_DIR/release/chronicle"
DRIVER="$ROOT/tests/support/http_driver.py"
DATA_DIR="$ARTIFACT_ROOT/data"
RECORDING_REF=""
WAL_DIR="$ARTIFACT_ROOT/wal"
VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
SESSION_ROOT="$DATA_DIR"
TMP_DIR=""
DEDICATED_CGROUP=""
SHARED_CGROUP=""
CGROUP_PARENT=""
RECORDER_PID=""
UPSTREAM_PID=""
REPLAY_PID=""
SHARED_ONE_PID=""
SHARED_TWO_PID=""
SELECTOR_TARGET_PID=""
CURRENT_PHASE="initializing"
SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
TOTAL_PHASES=29
EBPF_OBJECT_SHA256="not_checked"
SIGNAL_RESULT="not_checked"
SIGNAL_SHARED_RUNTIME_RESULT="not_checked"
SHARED_RUNTIME_RESULT="not_checked"
USER_INTENT_RESULT="not_checked"
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

separated_chronicle() {
	local uid gid
	uid=$(stat -c '%u' "$ROOT")
	gid=$(stat -c '%g' "$ROOT")
	[[ $uid -ne 0 ]] || die "separated supervisor requires non-root source owner"
	python3 "$ROOT/scripts/acceptance/separated-supervisor.py" "$uid" "$gid" "$@"
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
	printf '%s\n' 'chronicle-live-capture-acceptance-root-v1' >"$ARTIFACT_ROOT/.chronicle-acceptance-root"
}

skip() {
	log "SKIP: $*" >&2
	exit 77
}

phase() {
	CURRENT_PHASE="$1/$TOTAL_PHASES"
	printf '%s\n' "$CURRENT_PHASE" >"$ARTIFACT_ROOT/current-phase.txt"
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
	python3 - "$SUMMARY" "$status" "$CURRENT_PHASE" "$commit_sha" "$kernel" "$architecture" "$cgroup_status" "$btf_status" "$cap_eff" "$EBPF_OBJECT_SHA256" "$working_tree_dirty" "$COMMAND_LOG" "$ACCEPTANCE_MODE" "$SIGNAL_RESULT" "$USER_INTENT_RESULT" "$RELEASE_MODE" "$EXPECTED_SHA" "$START_SHA" "$END_SHA" "$TREE_CLEAN" "$SIGNAL_SHARED_RUNTIME_RESULT" <<'PY'
import json
import sys
from pathlib import Path

summary = Path(sys.argv[1])
status, phase = sys.argv[2:4]
commit, kernel, architecture, cgroup, btf, cap_eff, object_sha, dirty, command_log = sys.argv[4:13]
mode, signal = sys.argv[13:15]
user_intent = sys.argv[15]
release, expected, start_sha, end_sha, tree_clean = sys.argv[16:21]
signal_shared_runtime = sys.argv[21]
summary.parent.mkdir(parents=True, exist_ok=True)
result = "passed" if status == "0" else ("not_checked" if status == "77" else "failed")
commands = []
if Path(command_log).is_file():
    commands = [line[2:] if line.startswith("+ ") else line for line in Path(command_log).read_text(encoding="utf-8").splitlines()]
scenarios = {"signal_and_shared_runtime": signal_shared_runtime}
identity_ok = (
    release != "1"
    or (expected and expected == commit == start_sha == end_sha and tree_clean == "true")
)
retained = (
    mode == "full"
    and result == "passed"
    and identity_ok
    and user_intent == "passed"
    and all(value == "passed" for value in scenarios.values())
)
# Portable WAL/ingest/replay/cgroup/fmt/workspace matrices are separately
# selected gate prerequisites (task 8.1); the privileged scenario gates only
# on its own privileged invariants.
required_matrix = {
    "privileged_signal": signal,
    "user_intent_lifecycle": user_intent,
}
if result == "passed" and (mode == "full" and (
    not retained or any(value != "passed" for value in required_matrix.values())
)):
    result = "not_checked"
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
    "release_mode": release == "1",
    "ebpf_object_sha256": object_sha,
    "commands_executed": commands,
    "checks": {
        "privileged_acceptance": result,
        "live_capture_retained_acceptance": "complete" if retained else "not_checked",
        "privileged_scenarios": scenarios,
        "privileged_signal": signal,
        "user_intent_lifecycle": user_intent,
    },
    "status": result,
    "exit_code": int(status),
    "phase": phase,
    "artifacts": {
        "root": ".",
        "wal": "data/recordings",
        "sessions": "data/sessions",
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
	stop_process "$SELECTOR_TARGET_PID"
	RECORDER_PID=""
	UPSTREAM_PID=""
	REPLAY_PID=""
	SHARED_ONE_PID=""
	SHARED_TWO_PID=""
	SELECTOR_TARGET_PID=""
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
	declare -F stop_scenario_watchdog >/dev/null && stop_scenario_watchdog
	declare -F finalize_scenario_state >/dev/null && finalize_scenario_state "$status" || true
	local cleanup_status=0
	cleanup_resources || cleanup_status=$?
	END_SHA=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf '%s' 'not_checked')
	if [[ $status -eq 0 && $cleanup_status -ne 0 ]]; then
		status=1
	fi
	if full_mode; then
		if [[ $RELEASE_MODE == 1 && ($END_SHA != "$START_SHA" || -n $(git -C "$ROOT" status --porcelain --untracked-files=all)) ]]; then
			TREE_CLEAN=false
			status=1
		fi
		if [[ $status -eq 0 && $SIGNAL_RESULT == passed && $SHARED_RUNTIME_RESULT == passed ]]; then
			SIGNAL_SHARED_RUNTIME_RESULT=passed
		fi
	fi
	write_summary "$status"
	declare -F stop_scenario_watchdog >/dev/null && stop_scenario_watchdog true
	exit "$status"
}
trap on_exit EXIT

if ! prepare_artifact_root; then
	ARTIFACT_ROOT_ERROR="unsafe or non-dedicated artifact root: $REQUESTED_ARTIFACT_ROOT"
	ARTIFACT_ROOT="$DEFAULT_ARTIFACT_ROOT"
	DATA_DIR="$ARTIFACT_ROOT/data"
	WAL_DIR="$ARTIFACT_ROOT/wal"
	VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
	SESSION_ROOT="$DATA_DIR"
	SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
	COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
	prepare_artifact_root || die "cannot initialize fallback artifact root: $ARTIFACT_ROOT"
fi
DATA_DIR="$ARTIFACT_ROOT/data"
WAL_DIR="$ARTIFACT_ROOT/wal"
VALIDATION_ROOT="$ARTIFACT_ROOT/wal-validation"
SESSION_ROOT="$DATA_DIR"
SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
: >"$COMMAND_LOG"
printf '%s\n' "$CURRENT_PHASE" >"$ARTIFACT_ROOT/current-phase.txt"
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
if full_mode && [[ $RELEASE_MODE == 1 ]]; then
	[[ -n $EXPECTED_SHA ]] || die "release acceptance requires CHRONICLE_ACCEPTANCE_EXPECTED_SHA"
	[[ $START_SHA == "$EXPECTED_SHA" ]] || die "expected commit $EXPECTED_SHA, found $START_SHA"
	[[ $TREE_CLEAN == true ]] || die "release acceptance requires a clean working tree"
fi
export CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR"
log "Acceptance mode: $ACCEPTANCE_MODE"

# shellcheck source=scripts/acceptance/lib/wait.sh
source "$ROOT/scripts/acceptance/lib/wait.sh"
# shellcheck source=scripts/acceptance/lib/scenario-dispatch.sh
source "$ROOT/scripts/acceptance/lib/scenario-dispatch.sh"
run_scenario_plan live-capture
