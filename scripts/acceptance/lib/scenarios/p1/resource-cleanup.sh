#!/usr/bin/env bash
# Central-dispatch scenario: resource-cleanup.
scenario_p1_resource_cleanup() {
phase 25 "Verify doctor has no persistent side effects"
[[ ! -e "$ARTIFACT_ROOT/doctor-wal/recording.json" ]] || die "doctor created recording metadata"
[[ ! -e "$ARTIFACT_ROOT/doctor-output/sessions" ]] || die "doctor published session"
assert_json "$ARTIFACT_ROOT/doctor.json" 'value["version"] == 1 and value["status"] in ("supported", "supported_with_warnings")'

phase 26 "Verify safe command JSON artifacts"
python3 - "$ARTIFACT_ROOT" <<'PY'
import json
import sys
from pathlib import Path
root = Path(sys.argv[1])
for path in [root / "record.json", root / "etl.json", root / "etl-rerun.json", root / "etl-second-root.json", root / "inspect.json", root / "replay.json", root / "doctor.json"]:
    value = json.loads(path.read_text(encoding="utf-8"))
    assert value.get("version") == 1, path
    assert "command line" not in path.read_text(encoding="utf-8").lower()
PY

if full_mode; then
	phase 27 "Run final workspace checks"
	cargo fmt --all --check >"$ARTIFACT_ROOT/fmt.log" 2>&1
	FMT_RESULT="passed"
	cargo check --workspace --all-targets --locked >"$ARTIFACT_ROOT/check.log" 2>&1
	WORKSPACE_CHECK_RESULT="passed"
else
	skip_phase 27 "Run final workspace checks" fmt.log check.log
fi

phase 28 "Collect artifacts"
for artifact in "$ARTIFACT_ROOT/record.log" "$ARTIFACT_ROOT/etl.log" "$ARTIFACT_ROOT/replay.log" "$ARTIFACT_ROOT/doctor.json" "$ARTIFACT_ROOT/record.json" "$ARTIFACT_ROOT/etl.json" "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/replay.json" "$ARTIFACT_ROOT/wal-tests.log" "$ARTIFACT_ROOT/privileged-feasibility.log" "$ARTIFACT_ROOT/signal-tests.log" "$WAL_DIR/recording.json"; do
	assert_file "$artifact"
done

phase 29 "Cleanup processes, cgroups, and temporary files"
cleanup_resources
log "PASS: privileged acceptance artifacts retained at $ARTIFACT_ROOT"

}
