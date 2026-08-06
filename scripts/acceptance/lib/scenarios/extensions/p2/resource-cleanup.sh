#!/usr/bin/env bash
# Central-dispatch scenario: resource-cleanup.
scenario_p2_resource_cleanup() {
p1_cgroup_status=passed
run_compat_command cgroup-safe cargo test -p chronicle-application --locked deduplicates_threads_and_safe_result_is_count_only || p1_cgroup_status=failed
run_compat_command cgroup-shared cargo test -p chronicle-application --locked shared_posix_group_and_descendants_need_acknowledgement || p1_cgroup_status=failed
run_compat_command cgroup-pid cargo test -p chronicle-application --locked pid_requires_exact_direct_tgid_even_with_acknowledgement || p1_cgroup_status=failed
run_compat_command cgroup-race cargo test -p chronicle-application --locked recorder_descendant_race_namespace_and_forbidden_scope_fail_closed || p1_cgroup_status=failed
set_check cgroup_matrix "$p1_cgroup_status"
if run_compat_command signal cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture; then
	set_check privileged_signal passed
else
	set_check privileged_signal failed
fi
if run_compat_command format cargo fmt --all --check; then
	set_check format_check passed
else
	set_check format_check failed
fi
if run_compat_command workspace cargo check --workspace --all-targets --locked; then
	set_check workspace_check passed
else
	set_check workspace_check failed
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

}
