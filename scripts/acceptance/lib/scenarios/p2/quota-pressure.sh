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
	for _ in $(seq 1 150); do
		if "$CHRONICLE" --format json recorder-status --state-root "$QUOTA_STATE" >"$ARTIFACT_ROOT/quota-status.json" 2>/dev/null &&
			grep -q '"capture_readiness"[[:space:]]*:[[:space:]]*"ready"' "$ARTIFACT_ROOT/quota-status.json"; then
			local started quota_mtime=0 start_epoch=0
			started=$(systemctl show "$QUOTA_UNIT" -p ActiveEnterTimestamp --value 2>/dev/null || printf '')
			if [[ -n "$started" ]] && command -v date >/dev/null 2>&1; then
start_epoch=$(date -d "$started" +%s 2>/dev/null || printf '0')
			fi
			quota_mtime=$(stat -c %Y "$QUOTA_STATE/recorder.json" 2>/dev/null || printf '0')
			if ((start_epoch == 0)) || ((quota_mtime >= start_epoch)); then
return 0
			fi
		fi
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
systemctl restart "$QUOTA_UNIT" 2>/dev/null || true
wait_for_unit_stable "$QUOTA_UNIT" "$QUOTA_STATE" || true
wait_for_quota_ready
systemctl is-active --quiet "$QUOTA_UNIT"
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


}
