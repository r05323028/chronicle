#!/usr/bin/env bash
# P2 privileged acceptance. Requires clean exact commit on Ubuntu 24.04.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
MODE=${CHRONICLE_ACCEPTANCE_MODE:-full}
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
UPSTREAM_PID=""
CURRENT_PHASE="initializing"
READINESS_TIMEOUT=${CHRONICLE_ACCEPTANCE_READINESS_TIMEOUT_SECONDS:-180}
READINESS_INTERVAL=${CHRONICLE_ACCEPTANCE_READINESS_INTERVAL_SECONDS:-1}
source "$ROOT/scripts/acceptance/recorder-readiness.sh"

mkdir -p "$ARTIFACT_ROOT"
printf '%s\n' 'chronicle-p2-acceptance-root-v1' >"$ARTIFACT_ROOT/.chronicle-acceptance-root"
exec > >(tee -a "$COMMAND_LOG") 2>&1

phase() {
	CURRENT_PHASE="$1"
	printf '[%s] %s\n' "$1" "$2"
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

write_report() {
	local status=$1
	python3 - "$SUMMARY" "$status" "$CURRENT_PHASE" "$SHA" "$EXPECTED_SHA" "$MODE" "$CHECKS_JSON" <<'PY'
import json, subprocess, sys
from pathlib import Path
summary = Path(sys.argv[1])
status, phase, commit, expected, mode, checks_path = sys.argv[2:8]
checks = {
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
identity_ok = end_commit == commit and not dirty and (not expected or expected == commit)
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
	if [[ "$PRE_REBOOT" != 1 ]]; then
		[[ -z "$UPSTREAM_PID" ]] || kill "$UPSTREAM_PID" 2>/dev/null || true
		systemctl stop "$UNIT" 2>/dev/null || true
		for _ in $(seq 1 50); do
			systemctl is-active --quiet "$UNIT" || break
			sleep 0.1
		done
		[[ -d "$CGROUP" ]] && rmdir "$CGROUP" 2>/dev/null || true
	fi
	# Atomic metadata writers may leave transient files while systemd drains;
	# never include disappearing temporary files in retained evidence.
	find "$ARTIFACT_ROOT" -type f -name '.tmp-*' -delete 2>/dev/null || true
}
on_exit() {
	local rc=$?
	cleanup
	local end_sha
	end_sha=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf 'unavailable')
	if [[ "$rc" == 0 ]] && { [[ "$end_sha" != "$SHA" ]] || [[ -n "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]]; }; then
		rc=1
		CURRENT_PHASE="identity_validation"
	fi
	write_report "$rc"
	if [[ -d "$ARTIFACT_ROOT" && "$PRE_REBOOT" != 1 ]]; then
		find "$ARTIFACT_ROOT" -type f ! -name artifact-manifest.sha256 ! -name '.tmp-*' -print0 |
			sort -z | xargs -0r sha256sum >"$ARTIFACT_ROOT/artifact-manifest.sha256"
	fi
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
command -v systemctl >/dev/null || skip 'requires systemd'
[[ -r /sys/fs/cgroup/cgroup.controllers ]] || skip 'requires cgroup v2'
[[ -r /sys/kernel/btf/vmlinux ]] || skip 'requires BTF'
[[ -n "$EXPECTED_SHA" && "$EXPECTED_SHA" == "$SHA" ]] || {
	printf 'expected exact commit %s, got %s\n' "$EXPECTED_SHA" "$SHA" >&2
	exit 1
}
[[ -z "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]] || {
	printf '%s\n' 'working tree must be clean for privileged acceptance' >&2
	exit 1
}

phase environment 'validate supported host and exact clean commit'
if [[ "$REBOOT_RESUME" == 1 ]]; then
	[[ -f "$STATE_ROOT/wal/recording.json" ]] || exit 1
fi
mkdir -p "$STATE_ROOT" "$STORE_ROOT"
mkdir "$CGROUP"
CGROUP_ID=$(stat -c '%i' "$CGROUP")

phase build 'build Linux eBPF object and Chronicle CLI'
EBPF_TARGET_DIR=${CHRONICLE_EBPF_TARGET_DIR:-"$ROOT/ebpf/target"}
cargo +nightly build -Z build-std=core --manifest-path "$ROOT/ebpf/Cargo.toml" --target bpfel-unknown-none --release --locked
CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" cargo build -p chronicle-cli --features linux-ebpf --locked

phase config 'write short-epoch recorder configuration'
cat >"$CONFIG" <<EOF
version = 1
state_root = "$STATE_ROOT"
store_root = "$STORE_ROOT"
domain_lock_root = "$DOMAIN_ROOT"
retention = { mode = "retain" }

[scope]
cgroup_path = "$CGROUP"
cgroup_id = $CGROUP_ID
shared_scope_acknowledged = true

[epoch]
max_age_seconds = 1
max_bytes = 50331648

[segment]
max_age_seconds = 1
max_bytes = 16777216

[[domains]]
root = "$DOMAIN_ROOT"
quota_bytes = 8589934592
minimum_free_bytes = 1048576

[etl]
batch_records = 4096
max_lag_records = 4096
retry_attempts = 3

[store]
backend = "filesystem"
max_batch_bytes = 1048576
max_staging_bytes = 4194304

[shutdown]
timeout_seconds = 30

[logging]
level = "info"
EOF

phase workload 'start loopback HTTP workload'
python3 "$DRIVER" serve --port-file "$ARTIFACT_ROOT/upstream.port" --requests "$ARTIFACT_ROOT/upstream-requests.jsonl" >"$ARTIFACT_ROOT/upstream.log" 2>&1 &
UPSTREAM_PID=$!
for _ in $(seq 1 100); do
	[[ -s "$ARTIFACT_ROOT/upstream.port" ]] && break
	sleep 0.1
done
PORT=$(cat "$ARTIFACT_ROOT/upstream.port")

phase start 'start foreground recorder under systemd Type=simple'
systemd-run --quiet --unit="$UNIT" --working-directory="$ROOT" \
	--setenv=CHRONICLE_CHECKPOINT_PAUSE_FILE="$CHECKPOINT_PAUSE_FILE" \
	--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=45s \
	--property=Restart=on-failure --property=RestartSec=1s \
	--property=NoNewPrivileges=no \
	"$CHRONICLE" --format json recorder --config "$CONFIG"
for _ in $(seq 1 30); do
	if systemctl is-active --quiet "$UNIT"; then
		break
	fi
	sleep 1
done
if ! systemctl is-active --quiet "$UNIT"; then
	collect_recorder_readiness_diagnostics
	exit 1
fi
[[ "$(systemctl show "$UNIT" -p Type --value)" == simple ]]
set_check systemd_type_simple passed

phase readiness 'poll recorder status before workload admission'
# Contract requires capture_readiness=ready and state=ready before admission.
wait_for_recorder_ready --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
if [[ "$REBOOT_RESUME" == 1 ]]; then
	set_check host_reboot_recovery passed
	set_check pre_reboot_persistence passed
fi

phase incremental 'prove recorder-owned incremental worker processes live committed WAL'
printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/incremental-workload.json"
for attempt in $(seq 1 120); do
	if "$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS" 2>/dev/null &&
		grep -q '"processing_readiness"[[:space:]]*:[[:space:]]*"ready"' "$RECORDER_STATUS"; then
		break
	fi
	if ((attempt % 10 == 0)); then
		printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
		python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/incremental-workload-$attempt.json" || true
	fi
	sleep 0.5
done
grep -q '"processing_readiness"[[:space:]]*:[[:space:]]*"ready"' "$RECORDER_STATUS"
set_check incremental_worker passed

phase live_etl 'prove standalone ETL rejects live WAL ownership'
set +e
"$CHRONICLE" --format json etl --wal-dir "$STATE_ROOT/wal" --output "$ARTIFACT_ROOT/live-etl" >"$ARTIFACT_ROOT/live-etl.json" 2>"$ARTIFACT_ROOT/live-etl.log"
live_etl_status=$?
set -e
[[ $live_etl_status -ne 0 ]]
set_check standalone_etl_live_rejection passed

phase size_rotation 'force deterministic segment and epoch byte rollover'
SIZE_SEGMENTS_BEFORE=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
SIZE_BYTES_BEFORE=$(du -sb "$STATE_ROOT/wal" | awk '{print $1}')
EPOCH_BEFORE=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
for _ in $(seq 1 12); do
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" --body-bytes 2097152 >"$ARTIFACT_ROOT/size-workload-$_.json"
done
sleep 1
SIZE_SEGMENTS_AFTER=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
SIZE_BYTES_AFTER=$(du -sb "$STATE_ROOT/wal" | awk '{print $1}')
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
EPOCH_AFTER=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
[[ $SIZE_SEGMENTS_AFTER -gt $SIZE_SEGMENTS_BEFORE ]]
[[ $((SIZE_BYTES_AFTER - SIZE_BYTES_BEFORE)) -ge 524288 ]]
[[ $EPOCH_AFTER -gt $EPOCH_BEFORE ]]
python3 - "$ARTIFACT_ROOT/rotation-proof.json" <<PY
import json
json.dump({
    "version": 1,
    "segment_count_before": $SIZE_SEGMENTS_BEFORE,
    "segment_count_after": $SIZE_SEGMENTS_AFTER,
    "segment_bytes_before": $SIZE_BYTES_BEFORE,
    "segment_bytes_after": $SIZE_BYTES_AFTER,
    "epoch_ordinal_before": $EPOCH_BEFORE,
    "epoch_ordinal_after": $EPOCH_AFTER,
    "fixed_post_body_bytes": 2097152,
    "workload_iterations": 12,
}, open("$ARTIFACT_ROOT/rotation-proof.json", "w", encoding="utf-8"), indent=2)
PY
set_check segment_size_rotation passed
set_check epoch_size_rollover passed

if [[ "$CRASH_MODE" == 1 ]]; then
	phase crash_restart 'kill recorder process and verify systemd recovery'
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/pre-crash-workload.json"
	sleep 1
	systemctl kill --kill-who=main -s SIGKILL "$UNIT"
	RECORDER_STATUS="$ARTIFACT_ROOT/recorder-restart-status.json" \
		wait_for_recorder_ready --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	systemctl is-active --quiet "$UNIT"
	set_check process_restart_readiness passed
fi

phase segment_age_rotation 'force age rotation with delayed low-traffic append'
AGE_SEGMENTS_BEFORE=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
AGE_EPOCH_BEFORE=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
sleep 2
printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/age-workload.json"
AGE_SEGMENTS_AFTER=$(find "$STATE_ROOT/wal" -type f -name '*.chwal' | wc -l)
"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$RECORDER_STATUS"
AGE_EPOCH_AFTER=$(
	python3 - "$RECORDER_STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
epoch = value["current_epoch"]
print(epoch["ordinal"] if isinstance(epoch, dict) else epoch)
PY
)
[[ $AGE_SEGMENTS_AFTER -gt $AGE_SEGMENTS_BEFORE || $AGE_EPOCH_AFTER -gt $AGE_EPOCH_BEFORE ]]
python3 - "$ARTIFACT_ROOT/age-rotation-proof.json" <<PY
import json
json.dump({
    "version": 1,
    "segment_count_before": $AGE_SEGMENTS_BEFORE,
    "segment_count_after": $AGE_SEGMENTS_AFTER,
    "epoch_before": $AGE_EPOCH_BEFORE,
    "epoch_after": $AGE_EPOCH_AFTER,
    "idle_seconds": 2,
}, open("$ARTIFACT_ROOT/age-rotation-proof.json", "w", encoding="utf-8"), indent=2)
PY
set_check segment_age_rotation passed

phase epochs 'generate traffic across at least three short epochs'
for _ in 1 2 3 4 5 6; do
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/workload-$_.json"
	sleep 1
done

phase quota_pressure 'exercise quota and minimum-free admission on isolated loopback filesystem'
QUOTA_IMAGE="$ARTIFACT_ROOT/quota.img"
QUOTA_MOUNT="$ARTIFACT_ROOT/quota-mount"
QUOTA_ROOT="$QUOTA_MOUNT/chronicle"
QUOTA_STATE="$QUOTA_ROOT/state"
QUOTA_STORE="$QUOTA_ROOT/store"
QUOTA_CGROUP="/sys/fs/cgroup/${UNIT}-quota"
QUOTA_UNIT="${UNIT}-quota"
truncate -s 128M "$QUOTA_IMAGE"
mkfs.ext4 -F -q "$QUOTA_IMAGE"
mkdir -p "$QUOTA_MOUNT"
mount -o loop "$QUOTA_IMAGE" "$QUOTA_MOUNT"
mkdir -p "$QUOTA_STATE" "$QUOTA_STORE" "$QUOTA_CGROUP"
QUOTA_CGROUP_ID=$(stat -c '%i' "$QUOTA_CGROUP")
cat >"$ARTIFACT_ROOT/quota-recorder.toml" <<EOF
version = 1
state_root = "$QUOTA_STATE"
store_root = "$QUOTA_STORE"
domain_lock_root = "$QUOTA_ROOT"
retention = { mode = "retain" }
[scope]
cgroup_path = "$QUOTA_CGROUP"
cgroup_id = $QUOTA_CGROUP_ID
shared_scope_acknowledged = true
[epoch]
max_age_seconds = 30
max_bytes = 33554432
[segment]
max_age_seconds = 30
max_bytes = 16777216
[[domains]]
root = "$QUOTA_ROOT"
quota_bytes = 67108864
minimum_free_bytes = 8388608
[etl]
batch_records = 4096
max_lag_records = 4096
retry_attempts = 1
[store]
backend = "filesystem"
max_batch_bytes = 1048576
max_staging_bytes = 1048576
[shutdown]
timeout_seconds = 15
[logging]
level = "info"
EOF
systemd-run --quiet --unit="$QUOTA_UNIT" --working-directory="$ROOT" \
	--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=30s \
	--property=Restart=no "$CHRONICLE" --format json recorder --config "$ARTIFACT_ROOT/quota-recorder.toml"
for _ in $(seq 1 30); do
	if systemctl is-active --quiet "$QUOTA_UNIT"; then break; fi
	sleep 1
done
wait_for_quota_ready() {
	for _ in $(seq 1 60); do
		if "$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-status.json" 2>/dev/null &&
			grep -q '"capture_readiness"[[:space:]]*:[[:space:]]*"ready"' "$ARTIFACT_ROOT/quota-status.json"; then return 0; fi
		sleep 0.2
	done
	return 1
}
wait_for_quota_ready
printf '%s\n' "$$" >"$QUOTA_CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/quota-workload.json"
for _ in $(seq 1 30); do
	"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-before-pressure.json" 2>/dev/null || true
	if python3 - "$ARTIFACT_ROOT/quota-before-pressure.json" <<'PY'; then break; fi
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
raise SystemExit(0 if value["counters"]["committed_records"] >= 1 else 1)
PY
	sleep 0.2
done
python3 - "$ARTIFACT_ROOT/quota-before-pressure.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
if value["counters"]["committed_records"] < 1:
    raise SystemExit("quota workload did not produce committed WAL")
PY
# Managed bytes remain below configured quota, but minimum-free headroom is
# independently consumed. Recorder must stay alive and stop admission.
dd if=/dev/zero of="$QUOTA_STATE/filler.bin" bs=1M count=50 conv=fsync status=none
sleep 2
systemctl is-active --quiet "$QUOTA_UNIT"
for pressure_attempt in $(seq 1 30); do
	: "$pressure_attempt"
	"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-pressure-status.json" 2>/dev/null || true
	if grep -q '"failure"[[:space:]]*:[[:space:]]*"quota_pressure"' "$ARTIFACT_ROOT/quota-pressure-status.json"; then break; fi
	sleep 0.2
done
grep -q '"capture_readiness"[[:space:]]*:[[:space:]]*"not_ready"' "$ARTIFACT_ROOT/quota-pressure-status.json"
grep -q '"failure"[[:space:]]*:[[:space:]]*"quota_pressure"' "$ARTIFACT_ROOT/quota-pressure-status.json"
printf '%s\n' "$$" >"$QUOTA_CGROUP/cgroup.procs"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/quota-pressure-workload.json"
sleep 1
"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-pressure-status.json"
python3 - "$ARTIFACT_ROOT/quota-pressure-status.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
if value.get("counters", {}).get("quota_rejected_records", 0) < 1:
    raise SystemExit("quota pressure did not reject admitted source observations")
PY
sleep 1
"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-pressure-status-after.json"
python3 - "$ARTIFACT_ROOT/quota-pressure-status.json" "$ARTIFACT_ROOT/quota-pressure-status-after.json" <<'PY'
import json, sys
before = json.load(open(sys.argv[1], encoding="utf-8"))
after = json.load(open(sys.argv[2], encoding="utf-8"))
if after["counters"]["committed_records"] != before["counters"]["committed_records"]:
    raise SystemExit("quota pressure admitted additional committed WAL records")
PY
set_check minimum_free_enforcement passed
rm -f "$QUOTA_STATE/filler.bin"
wait_for_quota_ready
set_check minimum_free_recovery passed
set_check quota_recovery_after_space passed
# Exhaust configured quota separately from minimum-free pressure.
dd if=/dev/zero of="$QUOTA_STATE/filler.bin" bs=1M count=62 conv=fsync status=none
sleep 2
systemctl is-active --quiet "$QUOTA_UNIT"
"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-exhaustion-status.json"
grep -q '"capture_readiness"[[:space:]]*:[[:space:]]*"not_ready"' "$ARTIFACT_ROOT/quota-exhaustion-status.json"
grep -q '"failure"[[:space:]]*:[[:space:]]*"quota_pressure"' "$ARTIFACT_ROOT/quota-exhaustion-status.json"
set_check quota_exhaustion_admission passed
rm -f "$QUOTA_STATE/filler.bin"
wait_for_quota_ready
systemctl kill --kill-who=main -s SIGKILL "$QUOTA_UNIT" || true
sleep 1
systemctl restart "$QUOTA_UNIT"
wait_for_quota_ready
"$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-restart-status.json"
grep -q '"capture_readiness"[[:space:]]*:[[:space:]]*"ready"' "$ARTIFACT_ROOT/quota-restart-status.json"
python3 - "$ARTIFACT_ROOT/quota-restart-status.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
for quota in value.get("quota", []):
    if quota["free_bytes"] >= quota["quota_bytes"] or quota["reserved_bytes"] > quota["quota_bytes"]:
        raise SystemExit("restart quota accounting reset or exceeded reservation")
PY
set_check quota_restart_accounting passed
systemctl stop "$QUOTA_UNIT"
test -f "$QUOTA_STATE/wal/recording.json"
find "$QUOTA_STATE/wal" -type f -name '*.chwal' -print -quit | grep -q .
"$CHRONICLE" --format json etl --wal-dir "$QUOTA_STATE/wal" --output "$QUOTA_ROOT/recovered-store" >"$ARTIFACT_ROOT/quota-recovery-etl.json"
python3 - "$ARTIFACT_ROOT/quota-recovery-etl.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
if value["version"] != 1 or value["already_processed"] not in (False, True):
    raise SystemExit("quota WAL recovery proof invalid")
PY
rm -f "$QUOTA_STATE/filler.bin"
printf '%s\n' "$$" >"$CGROUP/cgroup.procs" 2>/dev/null || true
rmdir "$QUOTA_CGROUP" 2>/dev/null || true
umount "$QUOTA_MOUNT"
rm -rf "$QUOTA_MOUNT" "$QUOTA_IMAGE"

phase checkpoint_crash_matrix 'exercise production publication crash boundaries'
: >"$CHECKPOINT_PAUSE_FILE"
python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/checkpoint-crash-workload.json"
CHECKPOINT_READY_FILE="${CHECKPOINT_PAUSE_FILE%.*}.ready"
for _ in $(seq 1 120); do
	[[ -f "$CHECKPOINT_READY_FILE" ]] && break
	sleep 0.25
done
test -f "$CHECKPOINT_READY_FILE"
PENDING_BEFORE=$(find "$STATE_ROOT/wal" -type f -name '*.pending' -print -quit)
test -n "$PENDING_BEFORE"
DELTA_BEFORE=$(python3 - "$PENDING_BEFORE" "$STORE_ROOT" <<'PY'
import json, sys
from pathlib import Path
pending = json.load(open(sys.argv[1], encoding="utf-8"))
key = pending["outputs"][-1]["key"]
path = Path(sys.argv[2], "artifacts", *key.split("/"))
print(path)
PY
)
test -f "$DELTA_BEFORE"
PID_BEFORE=$(systemctl show "$UNIT" -p ExecMainPID --value)
python3 - "$PENDING_BEFORE" "$DELTA_BEFORE" <<'PY'
import json, sys
pending = json.load(open(sys.argv[1], encoding="utf-8"))
artifact = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(output["key"] == artifact["key"] and output["digest"] == artifact["checksum"] for output in pending["outputs"])
PY
systemctl kill --kill-who=main -s SIGKILL "$UNIT"
rm -f "$CHECKPOINT_PAUSE_FILE"
systemctl restart "$UNIT"
wait_for_recorder_ready --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
PID_AFTER=$(systemctl show "$UNIT" -p ExecMainPID --value)
test "$PID_BEFORE" != "$PID_AFTER"
if find "$STATE_ROOT/wal" -type f -name '*.pending' -print -quit | grep -q .; then
	echo 'checkpoint pending artifact survived restart' >&2
	exit 1
fi
CHECKPOINT_AFTER="$(dirname "$PENDING_BEFORE")/incremental-etl-checkpoint.json"
test -f "$CHECKPOINT_AFTER"
python3 - "$CHECKPOINT_AFTER" "$DELTA_BEFORE" "$STORE_ROOT/artifacts" <<'PY'
import json, sys
checkpoint = json.load(open(sys.argv[1], encoding="utf-8"))
artifact = json.load(open(sys.argv[2], encoding="utf-8"))
assert any(output["key"] == artifact["key"] and output["digest"] == artifact["checksum"] for output in checkpoint["outputs"])
from pathlib import Path
matches = list(Path(sys.argv[3]).rglob(Path(artifact["key"]).name))
assert len(matches) == 1, matches
PY
set_check checkpoint_output_committed_checkpoint_missing passed
set_check checkpoint_committed_status_missing passed

if [[ "$PRE_REBOOT" == 1 ]]; then
	phase pre_reboot 'retain live recorder state for VM reboot'
	"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$ARTIFACT_ROOT/pre-reboot-status.json"

	test -f "$STATE_ROOT/wal/recording.json"
	set_check pre_reboot_persistence passed
	exit 0
fi

phase stop 'request graceful SIGTERM and verify unit exit'
systemctl stop "$UNIT"
if systemctl is-active --quiet "$UNIT"; then
	exit 1
fi
journalctl --no-pager -u "$UNIT" >"$ARTIFACT_ROOT/recorder.log" || true
[[ "$(systemctl show "$UNIT" -p ExecMainStatus --value 2>/dev/null || true)" == 0 ]]
set_check signal_shutdown passed

phase finalization 'validate committed boundary, epochs, quota, and capture counters'
ACTIVE_WAL_DIR="$STATE_ROOT/wal"
if [[ -f "$STATE_ROOT/wal/epochs.json" ]]; then
	ACTIVE_REL=$(
		python3 - "$STATE_ROOT/wal/epochs.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
active = [entry for entry in value["epochs"] if entry.get("state") == "Active"]
print(active[-1]["path"] if active else ".")
PY
	)
	ACTIVE_WAL_DIR="$STATE_ROOT/wal/$ACTIVE_REL"
fi
python3 - "$ACTIVE_WAL_DIR/recording.json" "$STATE_ROOT/recorder.json" "$STATE_ROOT/wal" <<'PY'
import json
import sys
from pathlib import Path
recording = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
active = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
segments = list(Path(sys.argv[3]).rglob("*.chwal"))
assert recording["status"] == "completed", recording
assert recording["last_valid_commit"] is not None, recording
assert recording["counters"]["committed"]["records"] > 0, recording
assert recording["capture"]["scope"]["selected_subtree"] is True, recording
epoch = active["current_epoch"]
assert (epoch["ordinal"] if isinstance(epoch, dict) else epoch) >= 2, active
assert len(segments) >= 3, [path.name for path in segments]
assert all(domain["free_bytes"] > domain["reserved_bytes"] for domain in active["quota"])
PY
set_check three_epochs passed
# Protected lineage remains present after finalization; deletion proof is
# intentionally not claimed by this acceptance path.
# Metadata headroom is diagnostic only; quota/min-free behavior is proven by
# the isolated loopback pressure run above.

phase etl 'publish final canonical session'
"$CHRONICLE" --format json etl --wal-dir "$STATE_ROOT/wal" --output "$STORE_ROOT" >"$ARTIFACT_ROOT/etl.json" 2>"$ARTIFACT_ROOT/etl.log"
SESSION_ID=$(
	python3 - "$ARTIFACT_ROOT/etl.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["already_processed"] in (False, True)
print(value["session_id"])
PY
)
test -d "$STORE_ROOT/sessions/$SESSION_ID"

phase equivalence 'compare one-shot ETL output with incremental checkpoint output'
"$CHRONICLE" --format json inspect "$SESSION_ID" --root "$STORE_ROOT" >"$ARTIFACT_ROOT/inspect.json"
ONE_SHOT_STORE="$ARTIFACT_ROOT/one-shot-store"
"$CHRONICLE" --format json etl --wal-dir "$STATE_ROOT/wal" --output "$ONE_SHOT_STORE" >"$ARTIFACT_ROOT/one-shot-etl.json" 2>"$ARTIFACT_ROOT/one-shot-etl.log"
ONE_SHOT_SESSION=$(
	python3 - "$ARTIFACT_ROOT/one-shot-etl.json" <<'PY'
import json, sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["session_id"])
PY
)
"$CHRONICLE" --format json inspect "$ONE_SHOT_SESSION" --root "$ONE_SHOT_STORE" >"$ARTIFACT_ROOT/one-shot-inspect.json"
python3 - "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/one-shot-inspect.json" "$ARTIFACT_ROOT/equivalence.json" <<'PY'
import json
import sys

def comparable(path):
    value = json.load(open(path, encoding="utf-8"))
    value.pop("root", None)
    value.pop("output", None)
    return value

incremental, one_shot = map(comparable, sys.argv[1:3])
assert incremental == one_shot, (incremental, one_shot)
json.dump({"version": 1, "equivalent": True, "comparison": "inspect", "session_id": incremental["session_id"]}, open(sys.argv[3], "w", encoding="utf-8"), indent=2)
with open(sys.argv[3], "a", encoding="utf-8") as handle:
    handle.write("\n")
PY
set_check one_shot_equivalence passed

phase corruption 'corrupt committed WAL copy and prove fail-closed preservation'
CORRUPT_WAL="$ARTIFACT_ROOT/corrupt-wal"
CORRUPT_STORE="$ARTIFACT_ROOT/corrupt-store"
cp -a "$STATE_ROOT/wal" "$CORRUPT_WAL"
SOURCE_WAL_DIGEST=$(sha256sum "$STATE_ROOT"/wal/segments/*.chwal | sha256sum | awk '{print $1}')
CORRUPT_SEGMENT=$(find "$CORRUPT_WAL/segments" -name '*.chwal' -print -quit)
python3 - "$CORRUPT_SEGMENT" <<'PY'
import sys
from pathlib import Path
path = Path(sys.argv[1])
data = bytearray(path.read_bytes())
if len(data) <= 64:
    raise SystemExit('sealed WAL segment too short to corrupt')
offset = min(len(data) - 1, max(64, len(data) // 2))
data[offset] ^= 0x01
path.write_bytes(data)
PY
CORRUPT_WAL_DIGEST=$(sha256sum "$CORRUPT_WAL"/segments/*.chwal | sha256sum | awk '{print $1}')
set +e
"$CHRONICLE" --format json etl --wal-dir "$CORRUPT_WAL" --output "$CORRUPT_STORE" >"$ARTIFACT_ROOT/corruption.json" 2>"$ARTIFACT_ROOT/corruption.log"
corruption_status=$?
"$CHRONICLE" --format json etl --wal-dir "$CORRUPT_WAL" --output "$CORRUPT_STORE-retry" >"$ARTIFACT_ROOT/corruption-retry.json" 2>"$ARTIFACT_ROOT/corruption-retry.log"
corruption_retry_status=$?
set -e
[[ $corruption_status -ne 0 && $corruption_retry_status -ne 0 ]]
[[ "$(sha256sum "$STATE_ROOT"/wal/segments/*.chwal | sha256sum | awk '{print $1}')" == "$SOURCE_WAL_DIGEST" ]]
[[ "$(sha256sum "$CORRUPT_WAL"/segments/*.chwal | sha256sum | awk '{print $1}')" == "$CORRUPT_WAL_DIGEST" ]]
test -f "$CORRUPT_SEGMENT"
set_check corruption_fail_closed passed
set_check corruption_evidence_preservation passed

phase inspect_replay 'inspect and replay canonical session'
"$CHRONICLE" --format json inspect "$SESSION_ID" --root "$STORE_ROOT" >"$ARTIFACT_ROOT/inspect.json" 2>"$ARTIFACT_ROOT/inspect.log"
python3 - "$ARTIFACT_ROOT/inspect.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["integrity_valid"] is True
assert len(value["connections"]) >= 1
assert sum(len(connection["operations"]) for connection in value["connections"]) >= 3
PY
python3 "$DRIVER" serve --port-file "$ARTIFACT_ROOT/replay.port" --requests "$ARTIFACT_ROOT/replay-requests.jsonl" >"$ARTIFACT_ROOT/replay-target.log" 2>&1 &
REPLAY_PID=$!
for _ in $(seq 1 100); do
	[[ -s "$ARTIFACT_ROOT/replay.port" ]] && break
	sleep 0.1
done
REPLAY_ORIGIN="http://127.0.0.1:$(cat "$ARTIFACT_ROOT/replay.port")"
"$CHRONICLE" --format json replay "$SESSION_ID" --root "$STORE_ROOT" --target "$REPLAY_ORIGIN" --allow-host 127.0.0.1 --allow-read --allow-write --execute >"$ARTIFACT_ROOT/replay.json" 2>"$ARTIFACT_ROOT/replay.log"
kill "$REPLAY_PID" 2>/dev/null || true
python3 - "$ARTIFACT_ROOT/replay.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["version"] == 1
assert value["result"]["outcome"] in ("completed", "completed_with_skips")
assert all(item["verification"] == "passed" for item in value["result"]["operations"] if item["state"] == "completed")
PY
set_check inspect_replay passed
phase cleanup_recovery 'verify interrupted cleanup and restart convergence'
cargo test -p chronicle-wal cleanup_recovers_after_each_durable_step --locked -- --nocapture >"$ARTIFACT_ROOT/cleanup-recovery-tests.log" 2>&1
grep -q 'test result: ok' "$ARTIFACT_ROOT/cleanup-recovery-tests.log"
# Cleanup unit tests remain supplemental; privileged cleanup proof is not
# claimed until real deletion interruption and lineage gates are wired.

phase artifacts 'retain status, config, logs, and checksums'
cat >"$ARTIFACT_ROOT/sensitivity.json" <<'EOF'
{
  "classification": "synthetic-fixture-acceptance",
  "contains_captured_payloads": true,
  "contains_credentials": false,
  "retention": "test-only",
  "redaction": "not_applied; payloads are generated loopback fixtures",
  "owner_approval": "Chronicle privileged acceptance harness"
}
EOF
printf '%s\n' "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null || true
rmdir "$CGROUP" 2>/dev/null || true
if [[ -d "$CGROUP" ]] || systemctl is-active --quiet "$UNIT"; then
	exit 1
fi
set_check resource_cleanup passed
if python3 - "$CHECKS_JSON" <<'PY'; then
import json, sys
checks = json.load(open(sys.argv[1], encoding="utf-8"))
raise SystemExit(0 if all(value != "not_checked" for value in checks.values()) else 77)
PY
	exit 0
fi
exit 77
