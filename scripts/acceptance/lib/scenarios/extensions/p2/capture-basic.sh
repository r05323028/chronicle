#!/usr/bin/env bash
# Central-dispatch scenario: capture-basic.
scenario_p2_capture_basic() {
	phase environment 'validate supported host and collect source provenance'
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
	wait_for_path "$ARTIFACT_ROOT/upstream.port" 10
	[[ -s "$ARTIFACT_ROOT/upstream.port" ]]
	PORT=$(cat "$ARTIFACT_ROOT/upstream.port")

	phase start 'start foreground recorder under systemd Type=simple'
	systemd-run --quiet --unit="$UNIT" --working-directory="$ROOT" \
		--setenv=CHRONICLE_CHECKPOINT_PAUSE_FILE="$CHECKPOINT_PAUSE_FILE" \
		--property=Type=simple --property=KillSignal=SIGTERM --property=TimeoutStopSec=45s \
		--property=Restart=on-failure --property=RestartSec=1s \
		--property=NoNewPrivileges=no \
		"$CHRONICLE" --format json recorder --config "$CONFIG"
	if ! wait_for_unit_active "$UNIT" 30; then
		collect_recorder_readiness_diagnostics
		exit 1
	fi
	[[ "$(systemctl show "$UNIT" -p Type --value)" == simple ]]
	set_check systemd_type_simple passed

	phase readiness 'poll recorder status before workload admission'
	# Contract requires capture_readiness=ready and state=ready before admission.
	wait_for_recorder_ready --timeout "$READINESS_TIMEOUT" --interval "$READINESS_INTERVAL"
	set_check privileged_acceptance passed

}
