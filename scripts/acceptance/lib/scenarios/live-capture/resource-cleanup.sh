#!/usr/bin/env bash
# Central-dispatch scenario: resource-cleanup.
scenario_live_capture_resource_cleanup() {
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
		phase 27 "Collect artifacts"
		for artifact in "$ARTIFACT_ROOT/record.log" "$ARTIFACT_ROOT/etl.log" "$ARTIFACT_ROOT/replay.log" "$ARTIFACT_ROOT/doctor.json" "$ARTIFACT_ROOT/record.json" "$ARTIFACT_ROOT/etl.json" "$ARTIFACT_ROOT/inspect.json" "$ARTIFACT_ROOT/replay.json" "$ARTIFACT_ROOT/privileged-kernel.log" "$ARTIFACT_ROOT/signal-tests.log" "$WAL_DIR/recording.json"; do
			assert_file "$artifact"
		done
	else
		skip_phase 27 "Collect artifacts" record.log etl.log replay.log doctor.json record.json etl.json inspect.json replay.json privileged-kernel.log signal-tests.log
	fi

	phase 28 "Cleanup processes, cgroups, and temporary files"
	cleanup_resources
	log "PASS: privileged acceptance artifacts retained at $ARTIFACT_ROOT"

}
