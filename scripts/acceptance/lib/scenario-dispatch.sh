#!/usr/bin/env bash
# Single scenario dispatcher. Profile files provide setup/cleanup; scenario files provide assertions.
set -euo pipefail

SCENARIO_ROOT="$ROOT/scripts/acceptance/lib/scenarios"
SCENARIO_WATCHDOG_PID=""
SCENARIO_TIMEOUT_PROFILE=""
SCENARIO_TIMEOUT_NAME=""
SCENARIO_TIMEOUT_SECONDS=""
SCENARIO_TIMED_OUT=0
SCENARIO_STATE_FILE=""

# Privileged environment classification before any scenario assertion runs
# (tasks 7.2/7.3 wiring). Writes preflight.json into the evidence artifacts;
# on an unsupported/infrastructure environment the profile exits 77
# (not_checked) so the run never masquerades as a product failure. The
# runner records the preflight outcome in the acceptance evidence.
run_preflight() {
	local tests=$1 outcome
	if [[ ! -f "$ROOT/scripts/privileged/preflight.py" ]]; then
		return 0
	fi
	python3 "$ROOT/scripts/privileged/preflight.py" --tests "$tests" \
		>"$ARTIFACT_ROOT/preflight.json" 2>>"$ARTIFACT_ROOT/preflight.err" || true
	outcome=$(python3 -c 'import json,sys;print(json.load(open(sys.argv[1])).get("outcome",""))' \
		"$ARTIFACT_ROOT/preflight.json" 2>/dev/null || printf '%s' '')
	case "$outcome" in
	supported) return 0 ;;
	unsupported_environment | not_checked | infrastructure_error)
		log "PREFLIGHT: $outcome - privileged scenarios not run (see preflight.json)" >&2
		return 1
		;;
	*)
		log "PREFLIGHT: unreadable or unknown outcome - treating as not_checked" >&2
		return 1
		;;
	esac
}

write_scenario_state() {
	local action=$1 profile=$2 scenario=${3:-}
	shift 3 || true
	python3 - "${SCENARIO_STATE_FILE:-${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/scenario-state.json}" "$action" "$profile" "$scenario" "$@" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
action, profile, scenario = sys.argv[2:5]
if action == "init":
    selected = sys.argv[5:]
    value = {
        "version": 1, "profile": profile, "selected": selected,
        "current": None, "completed": [], "failed": None,
        "statuses": {name: "not_checked" for name in selected},
    }
else:
    value = json.loads(path.read_text(encoding="utf-8"))
    if action == "start":
        value["current"] = scenario
    elif action == "pass":
        value["statuses"][scenario] = "passed"
        if scenario not in value["completed"]:
            value["completed"].append(scenario)
        value["current"] = None
    elif action == "fail" and scenario in value["statuses"]:
        value["statuses"][scenario] = "failed"
        value["failed"] = scenario
        value["current"] = scenario
    else:
        raise SystemExit(f"invalid scenario state action: {action}")
temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(temporary, path)
PY
}

finalize_scenario_state() {
	local status=${1:-1}
	[[ $status -eq 0 || -z ${SCENARIO_STATE_FILE:-} || ! -f $SCENARIO_STATE_FILE ]] && return 0
	local current
	current=$(
		python3 - "$SCENARIO_STATE_FILE" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], encoding="utf-8")).get("current") or "")
PY
	)
	[[ -z $current ]] || write_scenario_state fail "${SCENARIO_TIMEOUT_PROFILE:-unknown}" "$current"
}

scenario_timeout_handler() {
	trap - TERM
	local marker="${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/scenario-timeout.json"
	local phase_file="${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/current-phase.txt"
	local timed_out_phase=${CURRENT_PHASE:-$(cat "$phase_file" 2>/dev/null || printf unknown)}
	SCENARIO_TIMED_OUT=1
	write_scenario_state fail "$SCENARIO_TIMEOUT_PROFILE" "$SCENARIO_TIMEOUT_NAME" || true
	CURRENT_PHASE="scenario_timeout:${SCENARIO_TIMEOUT_PROFILE}/${SCENARIO_TIMEOUT_NAME}"
	# Watchdog writes marker before signalling. Update phase only when profile
	# observed a newer phase than persisted phase file.
	python3 - "$marker" "$SCENARIO_TIMEOUT_PROFILE" "$SCENARIO_TIMEOUT_NAME" "$timed_out_phase" "$SCENARIO_TIMEOUT_SECONDS" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
try:
    value = json.loads(path.read_text(encoding="utf-8"))
except (OSError, json.JSONDecodeError):
    value = {
        "timeout_layer": "scenario", "profile": sys.argv[2],
        "scenario": sys.argv[3], "configured_timeout_seconds": int(sys.argv[5]),
        "exit_code": 124,
    }
value["phase"] = sys.argv[4]
tmp = path.with_name(path.name + ".handler.tmp")
tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(tmp, path)
PY
	printf 'ERROR: scenario %s/%s timed out after %ss (phase=%s)\n' "$SCENARIO_TIMEOUT_PROFILE" "$SCENARIO_TIMEOUT_NAME" "$SCENARIO_TIMEOUT_SECONDS" "$timed_out_phase" >&2
	if declare -F collect_recorder_readiness_diagnostics >/dev/null; then
		collect_recorder_readiness_diagnostics || true
		python3 - "$marker" "$ARTIFACT_ROOT" <<'PY'
import json, os, sys
from pathlib import Path
path, root = map(Path, sys.argv[1:])
value = json.loads(path.read_text(encoding="utf-8"))
names = [
    "recorder-service-status.txt", "recorder-journal.log",
    "process-list.txt", "disk-space.txt", "readiness-transitions.log",
]
value["diagnostics"] = [name for name in names if (root / name).is_file()]
tmp = path.with_name(path.name + ".diagnostics.tmp")
tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(tmp, path)
PY
	fi
	exit 124
}

stop_scenario_watchdog() {
	local force=${1:-false}
	[[ ${SCENARIO_TIMED_OUT:-0} == 0 || $force == true ]] || return 0
	[[ -n ${SCENARIO_WATCHDOG_PID:-} ]] || return 0
	kill -KILL "$SCENARIO_WATCHDOG_PID" 2>/dev/null || true
	wait "$SCENARIO_WATCHDOG_PID" 2>/dev/null || true
	SCENARIO_WATCHDOG_PID=""
}

terminate_scenario_descendants() {
	local parent=$1 watchdog=$2 first_signal=$3 grace=$4
	python3 - "$parent" "$watchdog" "$first_signal" "$grace" <<'PY'
import os, signal, subprocess, sys, time
from pathlib import Path

parent, watchdog = map(int, sys.argv[1:3])
first_signal = getattr(signal, "SIG" + sys.argv[3])
grace = int(sys.argv[4])

def process_table():
    rows = []
    proc = Path("/proc")
    if proc.is_dir():
        for entry in proc.iterdir():
            if not entry.name.isdigit():
                continue
            try:
                text = (entry / "stat").read_text(encoding="utf-8")
                tail = text[text.rfind(")") + 2:].split()
                rows.append((int(entry.name), int(tail[1])))
            except (OSError, ValueError, IndexError):
                pass
        return rows
    try:
        output = subprocess.check_output(
            ["ps", "-axo", "pid=,ppid="],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=2,
        )
        return [tuple(map(int, line.split())) for line in output.splitlines()]
    except (OSError, ValueError, subprocess.SubprocessError):
        return rows

def descendants(root, rows):
    children = {}
    for pid, ppid in rows:
        children.setdefault(ppid, []).append(pid)
    found, pending = set(), [root]
    while pending:
        direct = children.get(pending.pop(), [])
        found.update(direct)
        pending.extend(direct)
    return found

def targets():
    rows = process_table()
    excluded = descendants(watchdog, rows) | {watchdog, os.getpid()}
    return descendants(parent, rows) - excluded

def send(pids, sig):
    for pid in sorted(pids, reverse=True):
        try:
            os.kill(pid, sig)
        except ProcessLookupError:
            pass

tracked = targets()
send(tracked, first_signal)
deadline = time.monotonic() + grace
while time.monotonic() < deadline:
    tracked.update(targets())
    time.sleep(0.05)
tracked.update(targets())
send(tracked, signal.SIGKILL)
PY
}

start_scenario_watchdog() {
	local profile=$1 scenario=$2 timeout=$3 command_grace=$4 cleanup_grace=$5 parent=$$
	local marker="${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/scenario-timeout.json"
	local phase_file="${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/current-phase.txt"
	(
		trap '' TERM INT HUP
		local deadline=$((SECONDS + timeout)) phase watchdog
		watchdog=$(python3 -c 'import os; print(os.getppid())')
		while ((SECONDS < deadline)); do
			kill -0 "$parent" 2>/dev/null || exit 0
			sleep 0.1
		done
		kill -0 "$parent" 2>/dev/null || exit 0
		phase=$(cat "$phase_file" 2>/dev/null || printf 'scenario:%s/%s' "$profile" "$scenario")
		mkdir -p "$(dirname "$marker")"
		python3 - "$marker" "$profile" "$scenario" "$phase" "$timeout" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
value = {
    "timeout_layer": "scenario",
    "profile": sys.argv[2],
    "scenario": sys.argv[3],
    "phase": sys.argv[4],
    "configured_timeout_seconds": int(sys.argv[5]),
    "exit_code": 124,
}
tmp = path.with_name(path.name + ".watchdog.tmp")
tmp.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(tmp, path)
PY
		# Bash can defer TERM traps while a foreground command runs. Signal the
		# profile, then independently end its descendants (but never watchdog
		# subtree) so control returns to the timeout handler and EXIT cleanup.
		kill -TERM "$parent" 2>/dev/null || true
		terminate_scenario_descendants "$parent" "$watchdog" TERM "$command_grace"
		sleep "$cleanup_grace"
		if kill -0 "$parent" 2>/dev/null; then
			terminate_scenario_descendants "$parent" "$watchdog" KILL 0
			kill -KILL "$parent" 2>/dev/null || true
		fi
	) &
	SCENARIO_WATCHDOG_PID=$!
}

source_scenarios() {
	local profile=$1 scenario
	shift
	for scenario in "$@"; do
		if [[ -f "$SCENARIO_ROOT/shared/$scenario.sh" ]]; then
			# Single-owner convention: the shared implementation runs in every profile.
			# shellcheck source=/dev/null
			source "$SCENARIO_ROOT/shared/$scenario.sh"
		else
			[[ -f "$SCENARIO_ROOT/$profile/$scenario.sh" ]] || die "scenario $scenario is not implemented for $profile"
			# shellcheck source=/dev/null
			source "$SCENARIO_ROOT/$profile/$scenario.sh"
		fi
	done
}

scenario_timeout_from_toml() {
	local config="$ROOT/scripts/acceptance/scenarios.toml"
	[[ -f $config ]] || {
		printf '300\n'
		return
	}
	python3 - "$config" "$1" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as handle:
    value = tomllib.load(handle)
print(value["scenarios"].get(sys.argv[2], {}).get("timeout_seconds", 300))
PY
}

scenario_order_from_toml() {
	local profile=$1
	python3 - "$ROOT/scripts/acceptance/scenarios.toml" "$profile" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as handle:
    value = tomllib.load(handle)
print(",".join(value["profiles"][sys.argv[2]].get("execution_order", value["profiles"][sys.argv[2]]["scenarios"])))
PY
}

run_scenario_plan() {
	local profile=$1 scenario function scenario_timeout
	local command_grace=${CHRONICLE_TIMEOUT_GRACE_SECONDS:-5}
	local cleanup_grace=${CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS:-180}
	if [[ -n ${CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS:-} ]]; then
		[[ $CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS =~ ^[1-9][0-9]*$ ]] || die "CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS must be a positive integer"
	fi
	[[ $command_grace =~ ^[0-9]+$ ]] || die "CHRONICLE_TIMEOUT_GRACE_SECONDS must be a non-negative integer"
	[[ $cleanup_grace =~ ^[1-9][0-9]*$ ]] || die "CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS must be a positive integer"
	local -a selected
	if [[ -n ${CHRONICLE_ACCEPTANCE_SCENARIOS:-} ]]; then
		IFS=',' read -r -a selected <<<"$CHRONICLE_ACCEPTANCE_SCENARIOS"
	else
		IFS=',' read -r -a selected <<<"$(scenario_order_from_toml "$profile")"
	fi
	[[ ${#selected[@]} -gt 0 ]] || die 'central scenario dispatcher received no scenarios'
	local unique_count
	unique_count=$(printf '%s\n' "${selected[@]}" | sort -u | wc -l | tr -d ' ')
	[[ "$unique_count" == "${#selected[@]}" ]] || die "scenario selection is duplicated for $profile"
	SCENARIO_STATE_FILE="${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/scenario-state.json"
	if ! run_preflight "${selected[*]}"; then
		exit 77
	fi
	write_scenario_state init "$profile" "" "${selected[@]}"
	source_scenarios "$profile" "${selected[@]}"
	for scenario in "${selected[@]}"; do
		if [[ -f "$SCENARIO_ROOT/shared/$scenario.sh" ]]; then
			function="scenario_${scenario//-/_}"
		else
			function="scenario_${profile}_${scenario//-/_}"
		fi
		declare -F "$function" >/dev/null || die "scenario function missing: $function"
		if declare -F log >/dev/null; then
			log "[scenario] $profile/$scenario"
		else
			printf '[scenario] %s/%s\n' "$profile" "$scenario"
		fi
		scenario_timeout=${CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS:-$(scenario_timeout_from_toml "$scenario")}
		[[ $scenario_timeout =~ ^[1-9][0-9]*$ ]] || die "scenario timeout for $scenario must be a positive integer"
		SCENARIO_TIMED_OUT=0
		SCENARIO_TIMEOUT_PROFILE=$profile
		SCENARIO_TIMEOUT_NAME=$scenario
		SCENARIO_TIMEOUT_SECONDS=$scenario_timeout
		printf 'scenario:%s/%s\n' "$profile" "$scenario" >"${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}/current-phase.txt"
		write_scenario_state start "$profile" "$scenario"
		trap scenario_timeout_handler TERM
		start_scenario_watchdog "$profile" "$scenario" "$scenario_timeout" "$command_grace" "$cleanup_grace"
		"$function"
		write_scenario_state pass "$profile" "$scenario"
		stop_scenario_watchdog
		trap - TERM
	done
}
