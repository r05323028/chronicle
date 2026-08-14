#!/usr/bin/env bash
# Central-dispatch scenario: resource-cleanup.
scenario_p2_resource_cleanup() {
	# Portable cgroup decision matrix, fmt, and workspace checks are
	# separately selected gate prerequisites (task 8.1); the privileged
	# scenario keeps only the real signal/cleanup invariants.
	if run_compat_command signal cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture; then
		set_check privileged_signal passed
	else
		set_check privileged_signal failed
	fi
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
	stop_process "$UPSTREAM_PID"
	UPSTREAM_PID=""
	systemctl stop --no-block "$UNIT" 2>/dev/null || true
	wait_for_unit_inactive "$UNIT" 45
	printf '%s\n' "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null || true
	rmdir "$CGROUP"
	[[ ! -d $CGROUP ]]
	[[ $(systemctl is-active "$UNIT" 2>/dev/null || true) == inactive ]]
	set_check resource_cleanup passed
	set +e
	python3 - "$CHECKS_JSON" <<'PY'
import json, sys
checks = json.load(open(sys.argv[1], encoding="utf-8"))
if any(value not in {"passed", "complete", "not_checked"} for value in checks.values()):
    raise SystemExit(1)
raise SystemExit(77 if any(value == "not_checked" for value in checks.values()) else 0)
PY
	check_status=$?
	set -e
	return "$check_status"

}
