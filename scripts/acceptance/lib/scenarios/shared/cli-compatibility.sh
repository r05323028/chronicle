#!/usr/bin/env bash
# shellcheck disable=SC2034 # result globals are consumed by profile summary after scenario returns
# Dedicated 0.1.x CLI compatibility scenario. Legacy syntax belongs only here.
# Fully self-contained: own cgroup subtree and WAL copies, so it runs under
# both live-capture and recorder profiles without depending on other scenarios' state.
cli_compatibility_impl() {
	phase cli_compatibility 'prove legacy forms preserve behavior and emit one safe warning'
	local compat="$ARTIFACT_ROOT/cli-compatibility"
	local fixture="$ROOT/fixtures/http/basic-session.json"
	mkdir -p "$compat"

	assert_compat_warning() {
		python3 - "$1" "$2" <<'PY'
import json, sys
from pathlib import Path
lines = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
assert len(lines) == 1, lines
value = json.loads(lines[0])
assert value == {
    "version": 1,
    "level": "warning",
    "code": "deprecated_cli",
    "invocation": sys.argv[2],
    "replacement": value["replacement"],
}, value
assert value["replacement"].startswith("chronicle "), value
PY
	}

	"$CHRONICLE" --format json record --source fixture --input "$fixture" --root "$compat/fixture-store" \
		>"$compat/fixture.json" 2>"$compat/fixture.warning.json"
	assert_compat_warning "$compat/fixture.warning.json" record_source_fixture
	assert_json "$compat/fixture.json" 'value["version"] == 1 and value["session_id"]'
	local fixture_session
	fixture_session=$(json_value "$compat/fixture.json" 'value["session_id"]')

	"$CHRONICLE" --format json internal record-fixture --input "$fixture" --root "$compat/internal-fixture-store" \
		>"$compat/internal-fixture.json" 2>"$compat/internal-fixture.stderr"
	assert_json "$compat/internal-fixture.json" 'value["version"] == 1 and value["session_id"]'
	[[ ! -s "$compat/internal-fixture.stderr" ]]

	"$CHRONICLE" --format json inspect "$fixture_session" --root "$compat/fixture-store" \
		>"$compat/inspect.json" 2>"$compat/inspect.warning.json"
	assert_compat_warning "$compat/inspect.warning.json" inspect_root
	assert_json "$compat/inspect.json" 'value["version"] == 1 and value["session_id"] == "'"$fixture_session"'" and sum(len(connection["operations"]) for connection in value["connections"]) >= 1'

	python3 "$DRIVER" serve --port-file "$TMP_DIR/compat-replay.port" --requests "$compat/replay-requests.jsonl" \
		>"$compat/replay-target.log" 2>&1 &
	REPLAY_PID=$!
	wait_for_file "$TMP_DIR/compat-replay.port" 5 || die "compatibility replay target did not become ready"
	local replay_origin
	replay_origin="http://127.0.0.1:$(<"$TMP_DIR/compat-replay.port")"
	"$CHRONICLE" --format json replay "$fixture_session" --root "$compat/fixture-store" \
		--target "$replay_origin" --allow-host 127.0.0.1 --allow-read --allow-write --execute \
		>"$compat/replay.json" 2>"$compat/replay.warning.json"
	assert_compat_warning "$compat/replay.warning.json" replay_root
	assert_json "$compat/replay.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips")'
	stop_process "$REPLAY_PID"
	REPLAY_PID=""

	# Legacy eBPF record targets a dedicated cgroup holding one live process.
	local cgroup_parent scope upstream_origin
	cgroup_parent=$(current_cgroup_path) || die "cannot resolve current cgroup v2 path"
	scope="$cgroup_parent/chronicle-compat-$RUN_ID"
	mkdir "$scope"
	sleep 600 >"$compat/compat-target.log" 2>&1 &
	SELECTOR_TARGET_PID=$!
	printf '%s\n' "$SELECTOR_TARGET_PID" >"$scope/cgroup.procs"
	python3 "$DRIVER" serve --port-file "$compat/upstream.port" --requests "$compat/upstream-requests.jsonl" \
		>"$compat/upstream.log" 2>&1 &
	UPSTREAM_PID=$!
	wait_for_file "$compat/upstream.port" 5 || die "compatibility upstream did not become ready"
	upstream_origin="http://127.0.0.1:$(<"$compat/upstream.port")"
	"$CHRONICLE" --format json record --source ebpf --cgroup "$scope" \
		--wal-dir "$compat/ebpf-wal" --duration-seconds 20 --segment-bytes 16777216 --max-wal-bytes 16777216 \
		>"$compat/ebpf.json" 2>"$compat/ebpf.warning.json" &
	COMPAT_RECORD_PID=$!
	wait_for_path "$compat/ebpf-wal/recording.json" 15
	# recording.json appears before the eBPF ring is attached; a workload
	# racing that window can finish before capture is live. The legacy writer
	# also defers commits to record end (300s segment age, small events), so
	# mid-record commit is not observable. Drive bounded probes across the
	# record window so at least one lands after attach; the final result then
	# proves capture committed evidence.
	_legacy_ebpf_probe() {
		bash -c 'printf "%s\n" "$$" >"$1/cgroup.procs"; exec python3 "$2" workload --origin "$3"' \
			bash "$scope" "$DRIVER" "$upstream_origin" >"$compat/ebpf-workload.json" 2>/dev/null
	}
	for _legacy_probe_attempt in 1 2 3 4 5 6 7 8; do
		_legacy_ebpf_probe
		wait_for_elapsed_time 2 "legacy eBPF probe cadence"
	done
	wait "$COMPAT_RECORD_PID"
	assert_compat_warning "$compat/ebpf.warning.json" record_source_ebpf
	assert_json "$compat/ebpf.json" 'value["version"] == 1 and value["status"] == "completed" and value["shutdown_reason"] == "duration_limit" and value["physical_wal_bytes"] > 0'
	stop_process "$UPSTREAM_PID"
	UPSTREAM_PID=""
	stop_process "$SELECTOR_TARGET_PID"
	SELECTOR_TARGET_PID=""
	rmdir "$scope" 2>/dev/null || true

	# Legacy and internal ETL consume byte-identical ebpf WAL copies.
	cp -a "$compat/ebpf-wal" "$compat/legacy-etl-wal"
	cp -a "$compat/ebpf-wal" "$compat/internal-etl-wal"
	"$CHRONICLE" --format json etl --wal-dir "$compat/legacy-etl-wal" --output "$compat/legacy-etl-store" \
		>"$compat/etl.json" 2>"$compat/etl.warning.json"
	assert_compat_warning "$compat/etl.warning.json" etl
	"$CHRONICLE" --format json internal etl --wal-dir "$compat/internal-etl-wal" --output "$compat/internal-etl-store" \
		>"$compat/internal-etl.json" 2>"$compat/internal-etl.stderr"
	assert_json "$compat/etl.json" 'value["version"] == 1 and value["session_id"]'
	assert_json "$compat/internal-etl.json" 'value["version"] == 1 and value["session_id"]'
	[[ "$(json_value "$compat/etl.json" 'value["session_id"]')" == "$(json_value "$compat/internal-etl.json" 'value["session_id"]')" ]]
	[[ ! -s "$compat/internal-etl.stderr" ]]

	python3 - "$compat/compatibility-report.json" <<'PY'
import json, sys
json.dump({
    "version": 1,
    "status": "passed",
    "legacy_forms": ["etl", "record_source_fixture", "record_source_ebpf", "replay_root", "inspect_root"],
    "warning": "deprecated_cli",
}, open(sys.argv[1], "w", encoding="utf-8"), indent=2, sort_keys=True)
with open(sys.argv[1], "a", encoding="utf-8") as output:
    output.write("\n")
PY
	CLI_COMPATIBILITY_RESULT=passed
	if declare -F set_check >/dev/null; then
		set_check cli_compatibility passed
	fi
}

scenario_cli_compatibility() {
	cli_compatibility_impl
}
