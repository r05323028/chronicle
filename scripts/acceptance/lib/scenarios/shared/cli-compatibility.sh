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

	# Legacy and internal ETL forms remain available; the removed pre-0.1
	# record --source ebpf one-shot entrypoint is intentionally gone and is
	# covered by its rejection test in the CLI contract suite.

	python3 - "$compat/compatibility-report.json" <<'PY'
import json, sys
json.dump({
    "version": 1,
    "status": "passed",
    "legacy_forms": ["record_source_fixture", "replay_root", "inspect_root"],
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
