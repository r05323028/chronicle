#!/usr/bin/env bash
# Shared machine-readable recorder readiness polling and bounded diagnostics.
set -euo pipefail

TIMEOUT_WRAPPER=${TIMEOUT_WRAPPER:-"$ROOT/scripts/run-with-timeout.sh"}
READINESS_COMMAND_TIMEOUT=${CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS:-10}
[[ $READINESS_COMMAND_TIMEOUT =~ ^[1-9][0-9]*$ ]] || {
	printf '%s\n' 'CHRONICLE_ACCEPTANCE_READINESS_COMMAND_TIMEOUT_SECONDS must be a positive integer' >&2
	return 2 2>/dev/null || exit 2
}

bounded_readiness_command() {
	local timeout=$1
	shift
	"$TIMEOUT_WRAPPER" "$timeout" "$@"
}

systemd_active_epoch() {
	local timeout=$1 unit=$2 value
	value=$(bounded_readiness_command "$timeout" systemctl --timestamp=unix show "$unit" -p ActiveEnterTimestamp --value 2>/dev/null || printf '')
	value=${value#@}
	value=${value%%.*}
	[[ $value =~ ^[0-9]+$ ]] && printf '%s\n' "$value" || printf '0\n'
}

recorder_metadata_epoch() {
	python3 - "$1" <<'PY'
import json, os, sys
try:
    value = json.load(open(sys.argv[1], encoding="utf-8"))
    print(int(value.get("updated_at_unix_seconds") or os.stat(sys.argv[1]).st_mtime))
except (OSError, ValueError, TypeError):
    print(0)
PY
}

recorder_status_state() {
	python3 - "$1" <<'PY'
import json
import sys
from pathlib import Path

try:
    value = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
except (OSError, ValueError):
    print("unavailable|unknown|unknown|unknown|unknown|false")
    raise SystemExit
state = value.get("state")
if state not in {"starting", "recovering", "loading_ebpf", "ready", "degraded", "failed"}:
    lifecycle = value.get("lifecycle")
    capture = value.get("capture_readiness")
    processing = value.get("processing_readiness")
    health = value.get("health")
    if lifecycle == "failed" or health == "failed" or value.get("stale_owner"):
        state = "failed"
    elif capture == "ready" and processing == "ready" and health == "healthy":
        state = "ready"
    elif lifecycle == "starting":
        state = "starting"
    elif lifecycle == "recovering":
        state = "recovering"
    elif lifecycle == "running" and capture != "ready":
        state = "loading_ebpf"
    else:
        state = "degraded"
print("|".join([
    str(state),
    *(str(value.get(key, "unknown")) for key in ("lifecycle", "capture_readiness", "processing_readiness", "health")),
    "true" if value.get("stale_owner") else "false",
]))
PY
}

collect_recorder_readiness_diagnostics() {
	local root=${ARTIFACT_ROOT:?ARTIFACT_ROOT must be set}
	local state_root=${STATE_ROOT:?STATE_ROOT must be set}
	mkdir -p "$root"
	[[ -f "$RECORDER_STATUS" ]] || printf '%s\n' '{"state":"unavailable"}' >"$RECORDER_STATUS"
	printf '%s\n' "$(uname -r 2>/dev/null || printf unknown)" >"$root/kernel-version.txt"
	python3 - "$root/kernel-capabilities.json" "$root/btf-information.txt" <<'PY'
import json
import platform
import sys
from pathlib import Path

capabilities = {}
status = Path("/proc/self/status")
if status.is_file():
    for line in status.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith(("CapPrm:", "CapEff:", "CapBnd:")):
            key, value = line.split(":", 1)
            capabilities[key] = value.strip()
btf = Path("/sys/kernel/btf/vmlinux")
Path(sys.argv[1]).write_text(json.dumps({
    "uname": platform.uname()._asdict(),
    "capabilities": capabilities,
    "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(),
    "btf": btf.is_file(),
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
Path(sys.argv[2]).write_text(json.dumps({
    "path": str(btf),
    "available": btf.is_file(),
    "bytes": btf.stat().st_size if btf.is_file() else 0,
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
	{
		cat /proc/self/cgroup 2>/dev/null || true
		printf '\ncontrollers:\n'
		cat /sys/fs/cgroup/cgroup.controllers 2>/dev/null || true
		printf '\nselected:\n%s\n' "$CGROUP"
		cat "$CGROUP/cgroup.procs" 2>/dev/null || true
	} >"$root/cgroup-information.txt"
	if command -v bpftool >/dev/null 2>&1; then
		bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" bpftool prog show 2>&1 | head -n 200 >"$root/bpftool-programs.txt" || true
		bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" bpftool link show 2>&1 | head -n 200 >"$root/bpftool-links.txt" || true
	else
		printf '%s\n' 'bpftool unavailable' >"$root/bpftool-programs.txt"
		printf '%s\n' 'bpftool unavailable' >"$root/bpftool-links.txt"
	fi
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" systemctl status "$UNIT" --no-pager >"$root/recorder-service-status.txt" 2>&1 || true
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" journalctl --no-pager -n 200 -u "$UNIT" >"$root/recorder-journal.log" 2>&1 || true
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" ps -eo pid,ppid,stat,etimes,cmd --forest 2>&1 | head -n 200 >"$root/process-list.txt" || true
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" df -P "$state_root" "$root" >"$root/disk-space.txt" 2>&1 || true
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" find "$state_root/wal" -maxdepth 3 -type f -printf '%p %s bytes\n' 2>/dev/null | head -n 200 | sort >"$root/wal-directory-listing.txt" || true
	bounded_readiness_command "$READINESS_COMMAND_TIMEOUT" python3 - "$state_root/wal/incremental-etl-checkpoint.json" "$root/checkpoint-metadata.json" <<'PY'
import json
import sys
from pathlib import Path
source, destination = map(Path, sys.argv[1:])
try:
    value = json.loads(source.read_text(encoding="utf-8"))
    value = {
        key: value.get(key)
        for key in ("version", "owner", "lifecycle", "recording_id", "epoch_ordinal", "config_digest", "pipeline_version", "canonical_schema_version", "marker", "predecessor_digest")
    }
    decoder = json.loads(source.read_text(encoding="utf-8")).get("decoder", {})
    value["decoder_state_bytes"] = len(decoder.get("state", []))
    value["segment_lineage_count"] = len(json.loads(source.read_text(encoding="utf-8")).get("segment_lineage", []))
except (OSError, ValueError):
    value = {"status": "unavailable"}
destination.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

wait_for_recorder_ready() {
	local timeout=180 interval=1 allow_stale_owner=false
	while (($#)); do
		case $1 in
		--timeout)
			timeout=${2:?missing timeout}
			shift 2
			;;
		--interval)
			interval=${2:?missing interval}
			shift 2
			;;
		--allow-stale-owner)
			allow_stale_owner=true
			shift
			;;
		*)
			printf 'unknown readiness option: %s\n' "$1" >&2
			return 2
			;;
		esac
	done
	[[ $timeout =~ ^[1-9][0-9]*$ ]] || {
		printf 'readiness timeout must be a positive integer, got %q\n' "$timeout" >&2
		return 2
	}
	[[ $interval =~ ^[0-9]+([.][0-9]+)?$ ]] || {
		printf 'readiness interval must be a non-negative number, got %q\n' "$interval" >&2
		return 2
	}
	local deadline=$((SECONDS + timeout)) state unit_state last_state=none tmp="$RECORDER_STATUS.tmp"
	local remaining command_timeout
	: >"$ARTIFACT_ROOT/readiness-transitions.log"
	rm -f "$RECORDER_STATUS" "$tmp"
	while ((SECONDS < deadline)); do
		remaining=$((deadline - SECONDS))
		command_timeout=$READINESS_COMMAND_TIMEOUT
		((command_timeout <= remaining)) || command_timeout=$remaining
		if bounded_readiness_command "$command_timeout" "$CHRONICLE" --format json internal recorder-status --state-root "$STATE_ROOT" >"$tmp" 2>"$ARTIFACT_ROOT/recorder-status.stderr"; then
			mv -f "$tmp" "$RECORDER_STATUS"
			state=$(recorder_status_state "$RECORDER_STATUS" || printf 'unavailable|unknown|unknown|unknown|unknown|false')
		else
			state='unavailable|unknown|unknown|unknown|unknown|false'
			rm -f "$tmp"
			if [[ -n ${UNIT:-} ]]; then
				unit_state=$(bounded_readiness_command "$command_timeout" systemctl is-active "$UNIT" 2>/dev/null || true)
				if [[ $unit_state == failed ]]; then
					printf 'recorder unit entered terminal failed state while status unavailable\n' >&2
					collect_recorder_readiness_diagnostics
					return 1
				fi
			fi
		fi
		if [[ $state != "$last_state" ]]; then
			printf '[%s] recorder-state=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$state" | tee -a "$ARTIFACT_ROOT/readiness-transitions.log"
			last_state=$state
		fi
		case ${state%%|*} in
		ready)
			# Reject a stale status written by a killed predecessor: the
			# recorder's state file must be newer than the unit's latest start,
			# otherwise the recorder is still mid-startup.
			if [[ -n "${UNIT:-}" ]]; then
				local start_epoch=0 status_epoch=0
				start_epoch=$(systemd_active_epoch "$command_timeout" "$UNIT")
				status_epoch=$(recorder_metadata_epoch "$STATE_ROOT/recorder.json")
				if ((start_epoch != 0)) && ((status_epoch < start_epoch)); then
					sleep "$interval"
					continue
				fi
			fi
			return 0
			;;
		failed)
			if [[ $allow_stale_owner == true && ${state##*|} == true ]]; then
				printf '[%s] stale owner while recorder restarts; continuing\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee -a "$ARTIFACT_ROOT/readiness-transitions.log"
			else
				printf 'recorder reported terminal failed state\n' >&2
				collect_recorder_readiness_diagnostics
				return 1
			fi
			;;
		esac
		sleep "$interval"
	done
	printf 'recorder readiness timeout after %ss\n' "$timeout" >&2
	collect_recorder_readiness_diagnostics
	return 1
}
