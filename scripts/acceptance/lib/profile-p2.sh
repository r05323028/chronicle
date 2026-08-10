#!/usr/bin/env bash
# shellcheck disable=SC2034 # runtime variables are consumed by sourced scenario modules
# P2 runtime setup/cleanup. Assertions run through scenario-dispatch.sh.
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
	timeout_env=(CHRONICLE_ACCEPTANCE_GATE_WRAPPED=1 CHRONICLE_TIMEOUT_LAYER=acceptance_gate CHRONICLE_TIMEOUT_NAME=p2 CHRONICLE_TIMEOUT_GRACE_SECONDS="$cleanup_grace")
	if [[ -n ${CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT:-} ]]; then
		timeout_env+=(
			CHRONICLE_TIMEOUT_EVIDENCE_FILE="$CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT/gate-timeout.json"
			CHRONICLE_TIMEOUT_PHASE_FILE="$CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT/current-phase.txt"
			CHRONICLE_TIMEOUT_DIAGNOSTICS="recorder-service-status.txt:recorder-journal.log:process-list.txt:disk-space.txt:readiness-transitions.log"
		)
	fi
	exec env "${timeout_env[@]}" "$ROOT/scripts/run-with-timeout.sh" "$gate_timeout" env CHRONICLE_TIMEOUT_GRACE_SECONDS="$command_grace" bash "$0" "$@"
fi
MODE=${CHRONICLE_ACCEPTANCE_MODE:-full}
RELEASE_MODE=${CHRONICLE_ACCEPTANCE_RELEASE:-0}
EXPECTED_SHA=${CHRONICLE_ACCEPTANCE_EXPECTED_SHA:-}
REBOOT_RESUME=${CHRONICLE_ACCEPTANCE_REBOOT_RESUME:-0}
CRASH_MODE=${CHRONICLE_ACCEPTANCE_CRASH_MODE:-0}
PRE_REBOOT=${CHRONICLE_ACCEPTANCE_PRE_REBOOT:-0}
SHA=$(git -C "$ROOT" rev-parse HEAD)
RUN_ID="$(date -u +%Y%m%dT%H%M%SZ)-$$"
ARTIFACT_ROOT=${CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT:-"$ROOT/target/p2-acceptance/$RUN_ID"}
TARGET_DIR=${CARGO_TARGET_DIR:-"$ROOT/target"}
CHRONICLE="$TARGET_DIR/debug/chronicle"
DRIVER="$ROOT/tests/e2e/http_acceptance_driver.py"
UNIT="chronicle-p2-${RANDOM}-$$"
CGROUP="/sys/fs/cgroup/$UNIT"
STATE_ROOT=${CHRONICLE_ACCEPTANCE_STATE_ROOT:-"$ARTIFACT_ROOT/state"}
STORE_ROOT=${CHRONICLE_ACCEPTANCE_STORE_ROOT:-"$ARTIFACT_ROOT/store"}
DOMAIN_ROOT=${CHRONICLE_ACCEPTANCE_DOMAIN_ROOT:-"$ARTIFACT_ROOT"}
CONFIG="$ARTIFACT_ROOT/recorder.toml"
SUMMARY="$ARTIFACT_ROOT/acceptance-report.json"
COMMAND_LOG="$ARTIFACT_ROOT/commands.log"
RECORDER_STATUS="$ARTIFACT_ROOT/recorder-status.json"
CHECKS_JSON="$ARTIFACT_ROOT/checks.json"
CHECKPOINT_PAUSE_FILE="$STATE_ROOT/wal/checkpoint-pause.control"
CHECKPOINT_READY_FILE="${CHECKPOINT_PAUSE_FILE%.*}.ready"
UPSTREAM_PID=""
REPLAY_PID=""
SELECTOR_TARGET_PID=""
TMP_DIR=""
QUOTA_UNIT=""
QUOTA_CGROUP=""
QUOTA_MOUNT=""
QUOTA_IMAGE=""
CURRENT_PHASE="initializing"
READINESS_TIMEOUT=${CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS:-180}
READINESS_INTERVAL=${CHRONICLE_ACCEPTANCE_READINESS_INTERVAL_SECONDS:-1}
SERVICE_COMMAND_TIMEOUT=${CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS:-30}
TIMEOUT_WRAPPER="$ROOT/scripts/run-with-timeout.sh"
[[ $SERVICE_COMMAND_TIMEOUT =~ ^[1-9][0-9]*$ ]] || {
	printf '%s\n' 'CHRONICLE_ACCEPTANCE_SERVICE_COMMAND_TIMEOUT_SECONDS must be a positive integer' >&2
	exit 2
}
systemctl() { "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" systemctl "$@"; }
systemd-run() { "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" systemd-run "$@"; }
journalctl() { "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" journalctl "$@"; }

separated_chronicle() {
	local uid gid
	uid=$(stat -c '%u' "$ROOT")
	gid=$(stat -c '%g' "$ROOT")
	[[ $uid -ne 0 ]] || die "separated supervisor requires non-root source owner"
	python3 "$ROOT/scripts/acceptance/separated-supervisor.py" "$uid" "$gid" "$@"
}
# shellcheck source=scripts/lib/common.sh
source "$ROOT/scripts/lib/common.sh"
# shellcheck source=scripts/lib/env.sh
source "$ROOT/scripts/lib/env.sh"
# shellcheck source=scripts/lib/assertions.sh
source "$ROOT/scripts/lib/assertions.sh"
source "$ROOT/scripts/acceptance/recorder-readiness.sh"

mkdir -p "$ARTIFACT_ROOT"
rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
printf '%s\n' 'chronicle-p2-acceptance-root-v1' >"$ARTIFACT_ROOT/.chronicle-acceptance-root"
printf '%s\n' "$UNIT" >"$ARTIFACT_ROOT/active-units.txt"
printf '%s\n' "$CURRENT_PHASE" >"$ARTIFACT_ROOT/current-phase.txt"
exec > >(tee -a "$COMMAND_LOG") 2>&1

phase() {
	CURRENT_PHASE="$1"
	printf '%s\n' "$CURRENT_PHASE" >"$ARTIFACT_ROOT/current-phase.txt"
	printf '[%s] %s\n' "$1" "$2"
}

# Waits until the unit is active, its restart counter is stable, and the
# recorder status is fresh (written after the unit's latest start). Restart
# attempts can transiently fail closed on WAL lock contention while systemd
# auto-restart converges, and the status file of a killed predecessor must
# not satisfy readiness.
wait_for_unit_stable() {
	local unit=${1:?unit required}
	local status_root=${2:-$STATE_ROOT}
	local timeout=${3:-90}
	local deadline=$((SECONDS + timeout)) restarts_before restarts_after stable=0 state=unknown
	restarts_before=$(systemctl show "$unit" -p NRestarts --value 2>/dev/null || printf '0')
	while ((SECONDS < deadline)); do
		state=$(systemctl is-active "$unit" 2>/dev/null || true)
		[[ $state != failed ]] || {
			printf 'unit %s entered terminal failed state; restarts=%s\n' "$unit" "$restarts_before" >&2
			return 1
		}
		if [[ $state == active ]]; then
			restarts_after=$(systemctl show "$unit" -p NRestarts --value 2>/dev/null || printf '0')
			if [[ "$restarts_after" == "$restarts_before" ]]; then
				stable=$((stable + 1))
			else
				stable=0
				restarts_before=$restarts_after
			fi
			if ((stable >= 2)); then
				local start_epoch=0 status_epoch=0
				start_epoch=$(systemd_active_epoch "$SERVICE_COMMAND_TIMEOUT" "$unit")
				status_epoch=$(recorder_metadata_epoch "$status_root/recorder.json")
				if ((start_epoch == 0)) || ((status_epoch >= start_epoch)); then
					return 0
				fi
			fi
		fi
		sleep 1
	done
	printf 'unit %s did not stabilize after %ss; last state=%s restarts=%s\n' "$unit" "$timeout" "$state" "$restarts_before" >&2
	return 1
}

wait_for_unit_inactive() {
	local unit=${1:?unit required} timeout=${2:-30}
	local deadline=$((SECONDS + timeout)) state=unknown
	while ((SECONDS < deadline)); do
		state=$(systemctl is-active "$unit" 2>/dev/null || true)
		[[ $state == inactive ]] && return 0
		[[ $state != failed ]] || break
		sleep 0.1
	done
	printf 'unit %s did not become cleanly inactive after %ss; last state=%s\n' "$unit" "$timeout" "$state" >&2
	return 1
}

wait_for_unit_stopped() {
	local unit=${1:?unit required} timeout=${2:-30}
	local deadline=$((SECONDS + timeout)) state=unknown
	while ((SECONDS < deadline)); do
		state=$(systemctl is-active "$unit" 2>/dev/null || true)
		case $state in inactive | failed | unknown) return 0 ;; esac
		sleep 0.1
	done
	printf 'unit %s did not stop after %ss; last state=%s\n' "$unit" "$timeout" "$state" >&2
	return 1
}

stop_managed_unit() {
	local unit=${1:-}
	[[ -n $unit ]] || return 0
	systemctl stop --no-block "$unit" 2>/dev/null || true
	if ! wait_for_unit_stopped "$unit" 35; then
		systemctl kill --kill-who=all --signal=SIGKILL "$unit" 2>/dev/null || true
		wait_for_unit_stopped "$unit" 5 || return 1
	fi
	systemctl reset-failed "$unit" 2>/dev/null || true
}

wait_for_unit_active() {
	local unit=${1:?unit required} timeout=${2:-30}
	local deadline=$((SECONDS + timeout)) state=unknown
	while ((SECONDS < deadline)); do
		state=$(systemctl is-active "$unit" 2>/dev/null || true)
		[[ $state == active ]] && return 0
		[[ $state == failed ]] && break
		sleep 0.1
	done
	printf 'unit %s did not become active after %ss; last state=%s\n' "$unit" "$timeout" "$state" >&2
	return 1
}

wait_for_path() {
	local path=${1:?path required} timeout=${2:?timeout required}
	local deadline=$((SECONDS + timeout))
	while ((SECONDS < deadline)); do
		[[ -e $path ]] && return 0
		sleep 0.1
	done
	printf 'path did not appear after %ss: %s\n' "$timeout" "$path" >&2
	return 1
}

set_check() {
	python3 - "$CHECKS_JSON" "$1" "$2" <<'PY'
import json, sys
from pathlib import Path
path = Path(sys.argv[1])
checks = json.loads(path.read_text(encoding="utf-8")) if path.exists() else {}
checks[sys.argv[2]] = sys.argv[3]
path.write_text(json.dumps(checks, sort_keys=True) + "\n", encoding="utf-8")
PY
}

run_compat_command() {
	local name=$1
	shift
	set +e
	"$@" >"$ARTIFACT_ROOT/p1-$name.log" 2>&1
	local status=$?
	set -e
	return "$status"
}

write_report() {
	local status=$1
	python3 - "$SUMMARY" "$status" "$CURRENT_PHASE" "$SHA" "$EXPECTED_SHA" "$MODE" "$RELEASE_MODE" "$CHECKS_JSON" <<'PY'
import json, subprocess, sys
from pathlib import Path
summary = Path(sys.argv[1])
status, phase, commit, expected, mode, release, checks_path = sys.argv[2:9]
checks = {
    "privileged_acceptance": "not_checked",
    "wal_fault_matrix": "not_checked",
    "ingest_limit_matrix": "not_checked",
    "replay_matrix": "not_checked",
    "cgroup_matrix": "not_checked",
    "privileged_signal": "not_checked",
    "format_check": "not_checked",
    "workspace_check": "not_checked",
    "systemd_type_simple": "not_checked",
    "three_epochs": "not_checked",
    "segment_age_rotation": "not_checked",
    "segment_size_rotation": "not_checked",
    "epoch_size_rollover": "not_checked",
    "incremental_worker": "not_checked",
    "one_shot_equivalence": "not_checked",
    "standalone_etl_live_rejection": "not_checked",
    "signal_shutdown": "not_checked",
    "process_restart_readiness": "not_checked",
    "checkpoint_output_committed_checkpoint_missing": "not_checked",
    "checkpoint_committed_status_missing": "not_checked",
    "host_reboot_recovery": "not_checked",
    "pre_reboot_persistence": "not_checked",
    "quota_exhaustion_admission": "not_checked",
    "quota_restart_accounting": "not_checked",
    "quota_recovery_after_space": "not_checked",
    "minimum_free_enforcement": "not_checked",
    "minimum_free_recovery": "not_checked",
    "corruption_fail_closed": "not_checked",
    "corruption_evidence_preservation": "not_checked",
    "cleanup_interrupted_recovery": "not_checked",
    "cleanup_protected_lineage": "not_checked",
    "cleanup_restart_consistency": "not_checked",
    "inspect_replay": "not_checked",
    "resource_cleanup": "not_checked",
}
check_file = Path(checks_path)
if check_file.exists():
    checks.update(json.loads(check_file.read_text(encoding="utf-8")))
not_checked = sorted(name for name, value in checks.items() if value == "not_checked")
failed = sorted(name for name, value in checks.items() if value not in {"passed", "not_checked"})
end_commit = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
dirty = bool(subprocess.check_output(
    ["git", "status", "--porcelain", "--untracked-files=all"], text=True
).strip())
identity_ok = release != "1" or (end_commit == commit and not dirty and expected == commit)
report_status = "passed" if status == "0" and identity_ok and not not_checked and not failed else ("not_checked" if status in {"0", "77"} and identity_ok and not failed else "failed")
summary.write_text(json.dumps({
    "version": 1,
    "gate": "p2",
    "task": "8.4",
    "git_commit_sha": commit,
    "end_git_commit_sha": end_commit,
    "expected_git_commit_sha": expected or None,
    "working_tree_dirty": dirty,
    "kernel_version": __import__("platform").release(),
    "architecture": __import__("platform").machine(),
    "acceptance_mode": mode,
    "release_mode": release == "1",
    "status": report_status,
    "exit_code": int(status),
    "phase": phase,
    "checks": checks,
    "not_checked": not_checked,
}, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
}

cleanup() {
	set +e
	local failed=0 loop loops quota_mounted=false
	rm -f "$CHECKPOINT_PAUSE_FILE" "$CHECKPOINT_READY_FILE"
	stop_process "$UPSTREAM_PID"
	UPSTREAM_PID=""
	if type -P systemctl >/dev/null 2>&1; then
		stop_managed_unit "$QUOTA_UNIT" || failed=1
		stop_managed_unit "$UNIT" || failed=1
	fi
	printf '%s\n' "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null || true
	if [[ -n $QUOTA_CGROUP && -d $QUOTA_CGROUP ]]; then
		rmdir "$QUOTA_CGROUP" 2>/dev/null || failed=1
	fi
	if [[ -d $CGROUP ]]; then
		rmdir "$CGROUP" 2>/dev/null || failed=1
	fi
	if [[ -n $QUOTA_MOUNT ]] && "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" mountpoint -q "$QUOTA_MOUNT" 2>/dev/null; then
		if ! "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" umount "$QUOTA_MOUNT" 2>/dev/null; then
			failed=1
			quota_mounted=true
		fi
	fi
	if [[ $quota_mounted == false && -n $QUOTA_IMAGE ]] && type -P losetup >/dev/null 2>&1; then
		loops=$("$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" losetup -j "$QUOTA_IMAGE" 2>/dev/null | cut -d: -f1) || failed=1
		while read -r loop; do
			[[ -z $loop ]] || "$TIMEOUT_WRAPPER" "$SERVICE_COMMAND_TIMEOUT" losetup -d "$loop" 2>/dev/null || failed=1
		done <<<"$loops"
	fi
	if [[ $quota_mounted == false ]]; then
		[[ -z $QUOTA_MOUNT ]] || rm -rf -- "$QUOTA_MOUNT"
		[[ -z $QUOTA_IMAGE ]] || rm -f -- "$QUOTA_IMAGE"
	fi
	# Atomic metadata writers may leave transient files while systemd drains;
	# never include disappearing temporary files in retained evidence.
	find "$ARTIFACT_ROOT" -type f -name '.tmp-*' -delete 2>/dev/null || true
	return "$failed"
}
on_exit() {
	local rc=$? cleanup_status=0
	declare -F stop_scenario_watchdog >/dev/null && stop_scenario_watchdog
	cleanup || cleanup_status=$?
	if [[ $rc -eq 0 && $cleanup_status -ne 0 ]]; then
		rc=1
		CURRENT_PHASE="cleanup_failed"
	fi
	local end_sha
	end_sha=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unavailable')
	if [[ "$rc" == 0 && "$RELEASE_MODE" == 1 ]] && { [[ "$end_sha" != "$SHA" ]] || [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]]; }; then
		rc=1
		CURRENT_PHASE="identity_validation"
	fi
	write_report "$rc"
	if [[ -d "$ARTIFACT_ROOT" && "$PRE_REBOOT" != 1 ]]; then
		find "$ARTIFACT_ROOT" -type f ! -name artifact-manifest.sha256 ! -name '.tmp-*' -print0 |
			sort -z | xargs -0r sha256sum >"$ARTIFACT_ROOT/artifact-manifest.sha256"
	fi
	declare -F stop_scenario_watchdog >/dev/null && stop_scenario_watchdog true
	exit "$rc"
}
trap on_exit EXIT

skip() {
	printf 'NOT CHECKED: %s\n' "$*" >&2
	exit 77
}

[[ "$MODE" == full ]] || skip 'P2 privileged acceptance requires full mode'
[[ "$(uname -s)" == Linux ]] || skip 'requires Linux'
[[ "$(id -u)" == 0 ]] || skip 'requires root'
type -P systemctl >/dev/null || skip 'requires systemd'
[[ -r /sys/fs/cgroup/cgroup.controllers ]] || skip 'requires cgroup v2'
[[ -r /sys/kernel/btf/vmlinux ]] || skip 'requires BTF'
if [[ "$RELEASE_MODE" == 1 ]]; then
	[[ -n "$EXPECTED_SHA" && "$EXPECTED_SHA" == "$SHA" ]] || {
		printf 'release expected exact commit %s, got %s\n' "$EXPECTED_SHA" "$SHA" >&2
		exit 1
	}
fi
if [[ "$RELEASE_MODE" == 1 ]]; then
	[[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]] || {
		printf '%s\n' 'release acceptance requires a clean working tree' >&2
		exit 1
	}
fi
TMP_DIR=$(mktemp -d)
# Invoke the profile's on_exit from this final trap so it actually runs
# (re-registering it via eval never fires it again on EXIT). Preserve the
# script's real exit status for on_exit, which reads \$?.
p2_final() {
	local rc=$?
	rm -rf -- "$TMP_DIR"
	trap - EXIT
	(exit "$rc")
	on_exit
}
trap p2_final EXIT

# shellcheck source=scripts/acceptance/lib/scenario-dispatch.sh
source "$ROOT/scripts/acceptance/lib/scenario-dispatch.sh"
run_scenario_plan p2
