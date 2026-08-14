#!/usr/bin/env bash
# Central-dispatch scenario: wal-recovery.
scenario_live_capture_wal_recovery() {
	phase 17 "Verify recovery report and categorized counters"
	for key in received accepted_into_queue written_not_committed committed discarded_from_queue_due_to_wal_limit kernel_or_backend_dropped rejected_after_stop etl_checkpointed; do
		grep -q "\"$key\"" "$ARTIFACT_ROOT/record.json"
		grep -q "\"$key\"" "$WAL_DIR/recording.json"
	done

	STALE_WAL="$ARTIFACT_ROOT/stale-recovery-wal"
	STALE_OUTPUT="$ARTIFACT_ROOT/stale-recovery-output"
	cp -a "$WAL_DIR" "$STALE_WAL"
	python3 - "$STALE_WAL/recording.json" <<'PY'
import json
import sys
from pathlib import Path
path = Path(sys.argv[1])
metadata = json.loads(path.read_text(encoding="utf-8"))
metadata["status"] = "recording"
metadata["shutdown_reason"] = None
path.write_text(json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY
	"$CHRONICLE" --format json internal etl --wal-dir "$STALE_WAL" --output "$STALE_OUTPUT" >"$ARTIFACT_ROOT/stale-recovery-etl.json" 2>"$ARTIFACT_ROOT/stale-recovery-etl.log"
	assert_json "$ARTIFACT_ROOT/stale-recovery-etl.json" 'value["status"] == "aborted" and value["shutdown_reason"] == "process_crash_recovered" and value["checkpoint"]["status"] == "aborted"'
	assert_json "$STALE_WAL/etl/recovery-report.json" 'value["status"] == "aborted" and value["shutdown_reason"] == "process_crash_recovered"'
	if full_mode; then
		F2_8_RESULT=passed
	fi

	phase 18 "Rerun ETL in same output root"
	"$CHRONICLE" --format json internal etl --wal-dir "$WAL_DIR" --output "$SESSION_ROOT" >"$ARTIFACT_ROOT/etl-rerun.json" 2>"$ARTIFACT_ROOT/etl-rerun.log"
	assert_json "$ARTIFACT_ROOT/etl-rerun.json" 'value["already_processed"] is True and value["session_id"] == "'"$SESSION_ID"'"'

	phase 19 "Publish ETL into second output root"
	SECOND_ROOT="$ARTIFACT_ROOT/second-sessions"
	"$CHRONICLE" --format json internal etl --wal-dir "$WAL_DIR" --output "$SECOND_ROOT" >"$ARTIFACT_ROOT/etl-second-root.json" 2>"$ARTIFACT_ROOT/etl-second-root.log"
	assert_json "$ARTIFACT_ROOT/etl-second-root.json" 'value["already_processed"] is False and value["session_id"] == "'"$SESSION_ID"'"'
	assert_dir "$SECOND_ROOT/sessions/$SESSION_ID"

}
