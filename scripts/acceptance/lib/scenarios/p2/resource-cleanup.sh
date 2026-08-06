#!/usr/bin/env bash
# Central-dispatch scenario: resource-cleanup.
scenario_p2_resource_cleanup() {
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
