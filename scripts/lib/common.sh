#!/usr/bin/env bash

log() { printf '%s\n' "$*"; }
die() {
	log "ERROR: $*" >&2
	exit 1
}
require_command() { command -v "$1" >/dev/null 2>&1 || die "required command missing: $1"; }

wait_for_file() {
	local path=$1 timeout=$2
	local deadline=$((SECONDS + timeout))
	while ((SECONDS < deadline)); do
		[[ -s $path ]] && return 0
		sleep 0.05
	done
	return 1
}

wait_for_json() {
	local path=$1 expression=$2 timeout=$3
	local deadline=$((SECONDS + timeout))
	while ((SECONDS < deadline)); do
		if [[ -s $path ]] && python3 - "$path" "$expression" <<'PY'; then
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    value = json.load(source)
if not eval(sys.argv[2], {"__builtins__": {}}, {"value": value}):
    raise SystemExit(1)
PY
			return 0
		fi
		sleep 0.05
	done
	return 1
}

wait_for_process_exit() {
	local pid=${1:?pid required} timeout=${2:?timeout required}
	local deadline=$((SECONDS + timeout))
	while kill -0 "$pid" 2>/dev/null; do
		((SECONDS < deadline)) || return 1
		sleep 0.05
	done
	wait "$pid"
}

stop_process() {
	local pid=${1:-}
	[[ -n $pid ]] && kill -0 "$pid" 2>/dev/null || return 0
	kill -TERM "$pid" 2>/dev/null || true
	local deadline=$((SECONDS + 5))
	while kill -0 "$pid" 2>/dev/null && ((SECONDS < deadline)); do sleep 0.05; done
	kill -KILL "$pid" 2>/dev/null || true
	wait "$pid" 2>/dev/null || true
}
