#!/usr/bin/env bash
# Central-dispatch scenario: retention-interruption.
# Self-contained: generates committed evidence across several one-second
# epoch boundaries under delete-after retention, then SIGKILLs the recorder
# while its cleanup is paused mid-commit and proves recovery convergence.
scenario_p2_retention_interruption() {
	phase cleanup_setup 'restart recorder with delete-after retention and cleanup pause'
	CLEANUP_PAUSE_FILE="$STATE_ROOT/wal/cleanup-pause.control"
	CLEANUP_READY_FILE="${CLEANUP_PAUSE_FILE%.*}.ready"
	rm -f "$CLEANUP_PAUSE_FILE" "$CLEANUP_READY_FILE"
	systemctl stop "$UNIT" 2>/dev/null || true
	wait_for_unit_inactive "$UNIT" 5
	systemctl reset-failed "$UNIT" 2>/dev/null || true
	cat >"$CONFIG" <<EOF
version = 1
state_root = "$STATE_ROOT"
store_root = "$STORE_ROOT"
domain_lock_root = "$DOMAIN_ROOT"
retention = { mode = "delete_after", min_age_seconds = 1, max_retained_bytes = 1 }

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
	systemd-run --quiet --unit="$UNIT" --working-directory="$ROOT" \
		--setenv=CHRONICLE_CLEANUP_PAUSE_FILE="$CLEANUP_PAUSE_FILE" \
		--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=45s \
		--property=Restart=on-failure --property=RestartSec=1s \
		--property=NoNewPrivileges=no \
		"$CHRONICLE" --format json internal recorder --config "$CONFIG"
	wait_for_recorder_ready --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"

	phase cleanup_epochs 'generate evidence across several one-second epoch boundaries'
	# The cleanup only acts when retained evidence exceeds the ceiling, so the
	# recorder must first commit segments across multiple finalized epochs.
	# The workload must run inside the recorder's capture cgroup.
	printf '%s\n' "$$" >"$CGROUP/cgroup.procs" 2>/dev/null || true
	python3 "$DRIVER" workload --origin "http://127.0.0.1:$PORT" \
		>"$ARTIFACT_ROOT/retention-workload.json" 2>/dev/null || true
	wait_for_elapsed_time 2 'configured one-second retention and epoch age thresholds'
	# Leave the recorder's capture cgroup: the interrupted stop's cgroup sweep
	# would otherwise SIGTERM this scenario shell mid-recovery.
	printf '%s\n' "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null || true
	retention_epochs_ready() {
		count=$(find "$STATE_ROOT/wal/epochs" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
		printf 'finalized_epoch_dirs=%s expected>=3\n' "$count"
		((count >= 3))
	}
	wait_until 30 0.5 'at least three retention epoch directories' retention_epochs_ready
	count=$(find "$STATE_ROOT/wal/epochs" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
	if ((count < 3)); then
		echo "cleanup_setup: expected >= 3 finalized epoch dirs, saw $count" >&2
		exit 1
	fi
	ACTIVE_EPOCH_DIR=$(
		python3 - "$STATE_ROOT/wal/epochs.json" "$STATE_ROOT/wal" <<'PY'
import json, sys
from pathlib import Path
catalog = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
active = next(entry for entry in catalog["epochs"] if entry["state"] == "Active")
print(Path(sys.argv[2], active["path"]))
PY
	)
	ELIGIBLE=$(find "$STATE_ROOT/wal/epochs" -mindepth 3 -maxdepth 3 -name '*.chwal' \
		! -path "$ACTIVE_EPOCH_DIR/*" -print -quit)
	if [[ -z "$ELIGIBLE" ]]; then
		echo 'cleanup_setup: no eligible finalized segment for cleanup' >&2
		exit 1
	fi
	wait_for_elapsed_time 2 'configured retention minimum age before cleanup trigger'
	# Protected lineage marker inside the active epoch survives cleanup
	# (cleanup only ever targets .chwal proof-eligible segments in the
	# segments subdirectory).
	PROTECTED_MARKER="$ACTIVE_EPOCH_DIR/protected-lineage.marker"
	mkdir -p "$ACTIVE_EPOCH_DIR"
	printf '%s\n' 'protected-lineage-v1' >"$PROTECTED_MARKER"

	phase cleanup_interrupt 'SIGKILL recorder while cleanup is paused mid-commit'
	: >"$CLEANUP_PAUSE_FILE"
	# Graceful systemd stop runs the production shutdown path; cleanup pauses
	# at the first proof-eligible segment and signals readiness.
	systemctl stop --no-block "$UNIT"
	wait_for_path "$CLEANUP_READY_FILE" 50 || true
	CLEANUP_PID=$(systemctl show "$UNIT" -p ExecMainPID --value)
	{
		echo "cleanup_interrupt state: ready=$([[ -f "$CLEANUP_READY_FILE" ]] && echo yes || echo no)"
		echo "cleanup_interrupt state: pid=$CLEANUP_PID unit=$(systemctl is-active "$UNIT" 2>/dev/null || true)"
		[[ -f "$CLEANUP_READY_FILE" ]] && stat -c 'cleanup_interrupt state: ready_mtime=%y' "$CLEANUP_READY_FILE"
	} >&2
	if [[ ! -f "$CLEANUP_READY_FILE" ]]; then
		echo 'cleanup_interrupt: cleanup pause never signalled readiness' >&2
		exit 1
	fi
	# The paused cleanup has written its intent and renamed the segment into
	# trash; the active epoch remains protected lineage.
	INTENT=$(find "$STATE_ROOT/wal" -name 'cleanup-intent-*.json' -print -quit)
	if [[ -z "$INTENT" ]]; then
		echo 'cleanup_interrupt: no cleanup intent while paused' >&2
		exit 1
	fi
	SOURCE=$(
		python3 - "$INTENT" <<'PY'
import json, sys
from pathlib import Path
value = json.load(open(sys.argv[1], encoding="utf-8"))
source = value["source"]
# Normalize a relative intent path against the WAL root.
if not Path(source).is_absolute():
    source = str(Path(sys.argv[1]).parent.parent / source)
print(source)
PY
	)
	if [[ -e "$SOURCE" ]]; then
		echo "cleanup_interrupt: intent source still present: $SOURCE" >&2
		exit 1
	fi
	TRASHED=$(find "$STATE_ROOT/wal/retention-trash" -type f ! -name '*.deleted' -print -quit)
	if [[ -z "$TRASHED" ]]; then
		echo 'cleanup_interrupt: no segment moved to retention-trash while paused' >&2
		exit 1
	fi
	if [[ ! -f "$PROTECTED_MARKER" ]]; then
		echo "cleanup_interrupt: protected lineage marker missing: $PROTECTED_MARKER" >&2
		exit 1
	fi
	{
		echo "cleanup_interrupt: intent=$INTENT source=$SOURCE trash=$TRASHED marker=$PROTECTED_MARKER"
		echo "cleanup_interrupt: eligible=$ELIGIBLE"
	} >&2
	# Protected evidence digest (empty-tolerant: finalized epochs are ETL'd
	# during the shutdown, so their segment directories may be empty).
	ACTIVE_DIGEST_PAUSED=$(find "$ACTIVE_EPOCH_DIR/segments" -name '*.chwal' -type f -print0 2>/dev/null | sort -z | xargs -0 sha256sum 2>/dev/null | sha256sum | awk '{print $1}')
	kill -KILL "$CLEANUP_PID" 2>/dev/null || true
	rm -f "$CLEANUP_PAUSE_FILE" "$CLEANUP_READY_FILE"
	# The administratively stopped transient unit is removed on stop
	# completion; recreate it and let startup recovery replay the durable
	# cleanup intent.
	systemctl reset-failed "$UNIT" 2>/dev/null || true
	if ! systemctl start "$UNIT" 2>/dev/null; then
		systemd-run --quiet --unit="$UNIT" --working-directory="$ROOT" \
			--setenv=CHRONICLE_CLEANUP_PAUSE_FILE="$CLEANUP_PAUSE_FILE" \
			--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=45s \
			--property=Restart=on-failure --property=RestartSec=1s \
			--property=NoNewPrivileges=no \
			"$CHRONICLE" --format json internal recorder --config "$CONFIG"
	fi

	phase cleanup_recovery 'restart and prove cleanup recovery convergence'
	wait_for_unit_stable "$UNIT"
	wait_for_recorder_ready --allow-stale-owner --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	wait_for_path_absent "$CLEANUP_PAUSE_FILE" 5
	wait_for_path_absent "$CLEANUP_READY_FILE" 5
	# Recovery replayed the durable cleanup intent: no intent or trash remnant
	# survives and the interrupted segment stays deleted. Protected lineage
	# (the marker, and any active-epoch segments) remains untouched.
	if [[ -n "$(find "$STATE_ROOT/wal" -name 'cleanup-intent-*.json' -print -quit)" ]]; then
		echo 'cleanup_recovery: intent survived restart recovery' >&2
		exit 1
	fi
	if [[ -n "$(find "$STATE_ROOT/wal/retention-trash" -type f ! -name '*.deleted' -print -quit)" ]]; then
		echo 'cleanup_recovery: trash retained files after restart recovery' >&2
		exit 1
	fi
	if [[ -e "$SOURCE" ]]; then
		echo "cleanup_recovery: interrupted segment reappeared: $SOURCE" >&2
		exit 1
	fi
	if [[ ! -f "$PROTECTED_MARKER" ]]; then
		echo 'cleanup_recovery: protected lineage marker lost' >&2
		exit 1
	fi
	if [[ -n "$ACTIVE_DIGEST_PAUSED" ]]; then
		NOW_DIGEST=$(find "$ACTIVE_EPOCH_DIR/segments" -name '*.chwal' -type f -print0 2>/dev/null | sort -z | xargs -0 sha256sum 2>/dev/null | sha256sum | awk '{print $1}')
		if [[ "$NOW_DIGEST" != "$ACTIVE_DIGEST_PAUSED" ]]; then
			echo 'cleanup_recovery: active epoch segments changed across restart' >&2
			exit 1
		fi
	fi

	phase cleanup_restart 'stop cleanly and prove restart consistency'
	systemctl stop "$UNIT" 2>/dev/null || true
	if ! wait_for_unit_stopped "$UNIT" 12; then
		echo 'recorder did not stop for clean cleanup' >&2
		exit 1
	fi
	# Full cleanup drained all eligible finalized evidence without touching
	# protected lineage.
	if [[ -n "$(find "$STATE_ROOT/wal" -name 'cleanup-intent-*.json' -print -quit)" ]]; then
		echo 'cleanup_restart: intent survived clean shutdown' >&2
		exit 1
	fi
	if [[ -n "$(find "$STATE_ROOT/wal/retention-trash" -type f ! -name '*.deleted' -print -quit)" ]]; then
		echo 'cleanup_restart: trash retained files after clean shutdown' >&2
		exit 1
	fi
	if [[ ! -f "$PROTECTED_MARKER" ]]; then
		echo 'cleanup_restart: protected lineage marker lost' >&2
		exit 1
	fi
	wait_for_path_absent "$CLEANUP_PAUSE_FILE" 5
	wait_for_path_absent "$CLEANUP_READY_FILE" 5
	systemctl reset-failed "$UNIT" 2>/dev/null || true
	set_check cleanup_interrupted_recovery passed
	set_check cleanup_protected_lineage passed
	set_check cleanup_restart_consistency passed
}
