#!/usr/bin/env bash
# Bounded eventual-condition waits for acceptance assertions.

_wait_evidence() {
	local path=$1 status=$2 description=$3 timeout=$4 last=$5
	python3 - "$path" "$status" "$description" "$timeout" "${SCENARIO_TIMEOUT_NAME:-unknown}" "${CURRENT_PHASE:-unknown}" "$last" <<'PY'
import json, os, sys
from pathlib import Path
path = Path(sys.argv[1])
value = {
    "version": 1,
    "status": sys.argv[2],
    "condition": sys.argv[3],
    "configured_timeout_seconds": int(sys.argv[4]),
    "scenario": sys.argv[5],
    "phase": sys.argv[6],
    "last_observation": sys.argv[7],
}
path.parent.mkdir(parents=True, exist_ok=True)
temporary = path.with_name(path.name + f".{os.getpid()}.tmp")
temporary.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
os.replace(temporary, path)
PY
}

wait_until() {
	local timeout=${1:?timeout required} interval=${2:?interval required} description=${3:?description required}
	shift 3
	[[ $timeout =~ ^[1-9][0-9]*$ ]] || {
		printf 'wait timeout must be a positive integer, got %q\n' "$timeout" >&2
		return 2
	}
	[[ $interval =~ ^[0-9]+([.][0-9]+)?$ ]] || {
		printf 'wait interval must be a non-negative number, got %q\n' "$interval" >&2
		return 2
	}
	(($# > 0)) || {
		printf 'wait condition command is required\n' >&2
		return 2
	}
	local root=${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}
	local current="$root/current-wait.json" failure="$root/wait-failure-${SCENARIO_TIMEOUT_NAME:-unknown}.json"
	local observation_file deadline=$((SECONDS + timeout)) rc last='' previous=''
	observation_file=$(mktemp "$root/.wait-observation.XXXXXX") || return 1
	_wait_evidence "$current" waiting "$description" "$timeout" 'not observed'
	while :; do
		if "$@" >"$observation_file" 2>&1; then
			rc=0
		else
			rc=$?
		fi
		last=$(tail -c 4096 "$observation_file" 2>/dev/null || true)
		[[ -n $last ]] || last="condition returned $rc without observation"
		if [[ $last != "$previous" ]]; then
			_wait_evidence "$current" waiting "$description" "$timeout" "$last"
			previous=$last
		fi
		if ((rc == 0)); then
			rm -f "$observation_file" "$current" "$failure"
			return 0
		fi
		if ((rc != 1)); then
			_wait_evidence "$failure" terminal "$description" "$timeout" "$last"
			printf 'condition failed terminally (status=%s): %s; last observation: %s\n' "$rc" "$description" "$last" >&2
			rm -f "$observation_file" "$current"
			return 1
		fi
		if ((SECONDS >= deadline)); then
			_wait_evidence "$failure" timed_out "$description" "$timeout" "$last"
			printf 'condition timed out after %ss: %s; last observation: %s\n' "$timeout" "$description" "$last" >&2
			rm -f "$observation_file" "$current"
			return 1
		fi
		sleep "$interval"
	done
}

wait_for_elapsed_time() {
	local seconds=${1:?seconds required} description=${2:?description required}
	[[ $seconds =~ ^[1-9][0-9]*$ ]] || {
		printf 'elapsed-time wait must be a positive integer, got %q\n' "$seconds" >&2
		return 2
	}
	local root=${ARTIFACT_ROOT:-${TMPDIR:-/tmp}}
	local current="$root/current-wait.json"
	local started=$SECONDS deadline=$((SECONDS + seconds)) elapsed
	_wait_evidence "$current" elapsed_time "$description" "$seconds" 'elapsed=0'
	while ((SECONDS < deadline)); do
		sleep 0.1
	done
	elapsed=$((SECONDS - started))
	_wait_evidence "$current" elapsed_time "$description" "$seconds" "elapsed=$elapsed"
	rm -f "$current"
}

_path_present_condition() {
	if [[ -e $1 ]]; then
		printf 'present=%s\n' "$1"
		return 0
	fi
	printf 'absent=%s\n' "$1"
	return 1
}

_path_absent_condition() {
	if [[ ! -e $1 ]]; then
		printf 'absent=%s\n' "$1"
		return 0
	fi
	printf 'present=%s\n' "$1"
	return 1
}

wait_for_path() {
	local path=${1:?path required} timeout=${2:?timeout required} interval=${3:-0.1}
	wait_until "$timeout" "$interval" "path to appear: $path" _path_present_condition "$path"
}

wait_for_path_absent() {
	local path=${1:?path required} timeout=${2:?timeout required} interval=${3:-0.1}
	wait_until "$timeout" "$interval" "path to disappear: $path" _path_absent_condition "$path"
}
