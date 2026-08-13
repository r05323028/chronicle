#!/usr/bin/env bash
# Central-dispatch scenario: quota-pressure.
scenario_p2_quota_pressure() {
	phase quota_pressure 'exercise quota and minimum-free admission on isolated loopback filesystem'
	QUOTA_IMAGE="$ARTIFACT_ROOT/quota.img"
	QUOTA_MOUNT="$ARTIFACT_ROOT/quota-mount"
	QUOTA_ROOT="$QUOTA_MOUNT/chronicle"
	QUOTA_STATE="$QUOTA_ROOT/state"
	QUOTA_STORE="$QUOTA_ROOT/store"
	QUOTA_CGROUP="/sys/fs/cgroup/${UNIT}-quota"
	QUOTA_UNIT="${UNIT}-quota"
	printf '%s\n' "$QUOTA_UNIT" >>"$ARTIFACT_ROOT/active-units.txt"
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
	quota_status_condition() {
		local expected=$1 output=$2 start_epoch quota_epoch
		bounded_readiness_command 10 "$CHRONICLE" --format json internal recorder-status --state-root "$QUOTA_STATE" >"$output" 2>/dev/null || {
			printf 'quota status unavailable\n'
			return 1
		}
		start_epoch=$(systemd_active_epoch 10 "$QUOTA_UNIT")
		quota_epoch=$(recorder_metadata_epoch "$QUOTA_STATE/recorder.json")
		python3 - "$output" "$expected" "$start_epoch" "$quota_epoch" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
expected, start, metadata = sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
print(f"state={value.get('state')} capture={value.get('capture_readiness')} failure={value.get('failure')} start_epoch={start} metadata_epoch={metadata} counters={value.get('counters', {})}")
if expected == "ready":
    if value.get("state") == "failed" and value.get("failure") != "quota_pressure":
        raise SystemExit(2)
    raise SystemExit(0 if value.get("capture_readiness") == "ready" and (start == 0 or metadata >= start) else 1)
if expected == "pressure":
    raise SystemExit(0 if value.get("capture_readiness") == "not_ready" and value.get("failure") == "quota_pressure" else 1)
if expected == "rejected":
    raise SystemExit(0 if value.get("counters", {}).get("quota_rejected_records", 0) >= 1 else 1)
if expected == "committed":
    raise SystemExit(0 if value.get("counters", {}).get("committed_records", 0) >= 1 else 1)
raise SystemExit(2)
PY
	}
	wait_for_quota_ready() {
		wait_until 30 0.2 'quota recorder fresh capture readiness' quota_status_condition ready "$ARTIFACT_ROOT/quota-status.json"
	}
	quota_mount_absent() {
		if mountpoint -q "$1"; then
			printf 'mounted=%s\n' "$1"
			return 1
		fi
		printf 'unmounted=%s\n' "$1"
	}
	quota_loop_absent() {
		local loops
		loops=$(losetup -j "$1" 2>/dev/null || true)
		printf 'loops=%s\n' "${loops:-none}"
		[[ -z $loops ]]
	}

	systemd-run --quiet --unit="$QUOTA_UNIT" --working-directory="$ROOT" \
		--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=30s \
		--property=Restart=no "$CHRONICLE" --format json internal recorder --config "$ARTIFACT_ROOT/quota-recorder.toml"
	wait_for_unit_active "$QUOTA_UNIT" 30
	wait_for_quota_ready
	printf '%s\n' "$$" >"$QUOTA_CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/quota-workload.json"
	wait_until 30 0.2 'quota workload committed WAL' quota_status_condition committed "$ARTIFACT_ROOT/quota-before-pressure.json"

	# Minimum-free pressure is independent of managed quota usage.
	dd if=/dev/zero of="$QUOTA_STATE/filler.bin" bs=1M count=50 conv=fsync status=none
	wait_until 30 0.2 'minimum-free quota pressure state' quota_status_condition pressure "$ARTIFACT_ROOT/quota-pressure-status.json"
	systemctl is-active --quiet "$QUOTA_UNIT"
	printf '%s\n' "$$" >"$QUOTA_CGROUP/cgroup.procs"
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/quota-pressure-workload.json"
	wait_until 30 0.2 'quota-rejected source observation' quota_status_condition rejected "$ARTIFACT_ROOT/quota-pressure-status.json"
	REJECTED_BEFORE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["counters"]["quota_rejected_records"])' "$ARTIFACT_ROOT/quota-pressure-status.json")
	COMMITTED_AT_PRESSURE=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["counters"]["committed_records"])' "$ARTIFACT_ROOT/quota-pressure-status.json")
	# Stable rejection barrier: another observation must advance rejection while
	# committed count remains fixed.
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" >"$ARTIFACT_ROOT/quota-pressure-workload-confirm.json"
	quota_rejection_advanced() {
		quota_status_condition rejected "$ARTIFACT_ROOT/quota-pressure-status-after.json" || return $?
		local rejected
		rejected=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["counters"]["quota_rejected_records"])' "$ARTIFACT_ROOT/quota-pressure-status-after.json")
		printf 'quota_rejected_records=%s expected>%s\n' "$rejected" "$REJECTED_BEFORE"
		((rejected > REJECTED_BEFORE))
	}
	wait_until 30 0.2 'second quota-rejected source observation' quota_rejection_advanced
	python3 - "$ARTIFACT_ROOT/quota-pressure-status-after.json" "$COMMITTED_AT_PRESSURE" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
if value["counters"]["committed_records"] != int(sys.argv[2]):
    raise SystemExit("quota pressure admitted additional committed WAL records")
PY
	set_check minimum_free_enforcement passed
	rm -f "$QUOTA_STATE/filler.bin"
	wait_for_quota_ready
	set_check minimum_free_recovery passed
	set_check quota_recovery_after_space passed

	# Exhaust configured quota separately from minimum-free pressure.
	dd if=/dev/zero of="$QUOTA_STATE/filler.bin" bs=1M count=62 conv=fsync status=none
	wait_until 30 0.2 'configured quota exhaustion state' quota_status_condition pressure "$ARTIFACT_ROOT/quota-exhaustion-status.json"
	systemctl is-active --quiet "$QUOTA_UNIT"
	set_check quota_exhaustion_admission passed
	rm -f "$QUOTA_STATE/filler.bin"
	wait_for_quota_ready
	systemctl kill --kill-who=main -s SIGKILL "$QUOTA_UNIT" || true
	systemctl restart "$QUOTA_UNIT" 2>/dev/null || true
	wait_for_unit_stable "$QUOTA_UNIT" "$QUOTA_STATE"
	wait_for_quota_ready
	systemctl is-active --quiet "$QUOTA_UNIT"
	cp "$ARTIFACT_ROOT/quota-status.json" "$ARTIFACT_ROOT/quota-restart-status.json"
	python3 - "$ARTIFACT_ROOT/quota-restart-status.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
for quota in value.get("quota", []):
    if quota["free_bytes"] >= quota["quota_bytes"] or quota["reserved_bytes"] > quota["quota_bytes"]:
        raise SystemExit("restart quota accounting reset or exceeded reservation")
PY
	set_check quota_restart_accounting passed

	systemctl stop --no-block "$QUOTA_UNIT"
	wait_for_unit_stopped "$QUOTA_UNIT" 30
	test -f "$QUOTA_STATE/wal/recording.json"
	find "$QUOTA_STATE/wal" -type f -name '*.chwal' -print -quit | grep -q .
	"$CHRONICLE" --format json internal etl --wal-dir "$QUOTA_STATE/wal" --output "$QUOTA_ROOT/recovered-store" >"$ARTIFACT_ROOT/quota-recovery-etl.json"
	python3 - "$ARTIFACT_ROOT/quota-recovery-etl.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
if value["version"] != 1 or value["already_processed"] not in (False, True):
    raise SystemExit("quota WAL recovery proof invalid")
PY
	rm -f "$QUOTA_STATE/filler.bin"
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs" 2>/dev/null || true
	rmdir "$QUOTA_CGROUP"
	umount "$QUOTA_MOUNT"
	wait_until 15 0.2 'quota mount to disappear' quota_mount_absent "$QUOTA_MOUNT"
	QUOTA_LOOP=$(losetup -j "$QUOTA_IMAGE" | cut -d: -f1)
	[[ -z $QUOTA_LOOP ]] || losetup -d "$QUOTA_LOOP"
	wait_until 15 0.2 'quota loop device to detach' quota_loop_absent "$QUOTA_IMAGE"
	rm -rf "$QUOTA_MOUNT" "$QUOTA_IMAGE"
	QUOTA_UNIT=
	QUOTA_CGROUP=
	QUOTA_MOUNT=
	QUOTA_IMAGE=
}
