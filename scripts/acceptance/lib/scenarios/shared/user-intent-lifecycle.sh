#!/usr/bin/env bash
# shellcheck disable=SC2034 # result globals are consumed by profile summary after scenario returns
# Shared privileged public record/replay lifecycle scenario. Profile extensions
# dispatch with profile-local cgroup/summary wiring; all evidence is
# self-contained under $ARTIFACT_ROOT/user-intent-lifecycle.
user_intent_lifecycle_impl() {
	phase user_intent_lifecycle 'exercise supervised record/replay, retry, selectors, and cleanup'
	local root="$ARTIFACT_ROOT/user-intent-lifecycle"
	local data="$root/data" runtime="$root/runtime"
	local target_uid target_gid before_scope after_scope
	target_uid=$(stat -c '%u' "$ROOT")
	target_gid=$(stat -c '%g' "$ROOT")
	[[ $target_uid -ne 0 ]]
	install -d -m 0700 "$data"
	install -d -o "$target_uid" -g "$target_gid" -m 0700 "$runtime"

	# Self-contained cgroup subtree; removed on exit even on failure. Trap uses
	# globals because EXIT runs outside this function's dynamic scope.
	UI_CGROUP_PARENT=""
	UI_SCOPE=""
	trap 'printf "UI-ERR line %s: %s\n" "$LINENO" "$BASH_COMMAND" >&2' ERR
	cleanup_ui_scope() {
		local pid
		[[ -n ${UI_SCOPE:-} ]] || return 0
		while read -r pid; do
			[[ -n $pid ]] && printf '%s\n' "$pid" >"$UI_CGROUP_PARENT/cgroup.procs" 2>/dev/null || true
		done <"$UI_SCOPE/cgroup.procs"
		rmdir "$UI_SCOPE" 2>/dev/null || true
		UI_SCOPE=""
	}
	# Chain our cleanup onto the profile's existing EXIT trap instead of
	# replacing it, so the summary/report trap still runs on every path.
	UI_PREV_EXIT_TRAP=$(trap -p EXIT)
	cleanup_ui_chain() {
		trap - ERR
		cleanup_ui_scope
		[[ -n ${UI_PREV_EXIT_TRAP:-} ]] && eval "$UI_PREV_EXIT_TRAP"
	}
	trap cleanup_ui_chain EXIT
	UI_CGROUP_PARENT=$(current_cgroup_path) || die "cannot resolve current cgroup v2 path"
	UI_SCOPE="$UI_CGROUP_PARENT/chronicle-ui-$RUN_ID"
	mkdir "$UI_SCOPE"
	local cgroup_parent="$UI_CGROUP_PARENT" scope="$UI_SCOPE"
	before_scope=$(find "$cgroup_parent" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)

	# Command-mode capture starts one target and descendant only after attachment.
	python3 "$ROOT/scripts/acceptance/separated-supervisor.py" "$target_uid" "$target_gid" \
		"$CHRONICLE" --format json --data-dir "$data" record --duration 60 -- \
		bash -c 'if printf "%s\n" "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null; then exit 90; fi; id -u >"$1"; sleep 600 & echo $! >"$2"; exec python3 "$3" serve --port-file "$4" --requests "$5"' \
		bash "$runtime/record.uid" "$runtime/record.descendant" "$DRIVER" "$runtime/record.port" "$runtime/record-requests.jsonl" \
		>"$root/record.json" 2>"$root/record.error.json" &
	RECORDER_PID=$!
	wait_for_file "$runtime/record.port" 15 || die "command-mode record target did not become ready"
	python3 - "$(<"$runtime/record.port")" <<'PY'
import http.client, sys
connection = http.client.HTTPConnection("127.0.0.1", int(sys.argv[1]), timeout=5)
connection.request("POST", "/echo", body=b"write")
response = connection.getresponse()
assert (response.status, response.read()) == (201, b"write")
connection.close()
PY
	# Force publication failure after capture without weakening WAL durability.
	ln -s "$root/missing-publication-root" "$data/sessions"
	kill -INT "$RECORDER_PID"
	set +e
	wait "$RECORDER_PID"
	local record_status=$?
	set -e
	RECORDER_PID=""
	[[ $record_status -eq 3 ]]
	[[ ! -s "$root/record.json" ]]
	assert_json "$root/record.error.json" 'value["version"] == 1 and value["code"] == 3'
	[[ "$(<"$runtime/record.uid")" == "$target_uid" ]]
	local descendant
	descendant=$(<"$runtime/record.descendant")
	! kill -0 "$descendant" 2>/dev/null

	local wal recording_uuid recording_ref before_segments after_segments
	wal=$(find "$data/recordings" -mindepth 1 -maxdepth 1 -type d -print -quit)
	[[ -n $wal && -f $wal/recording.json ]]
	recording_uuid=$(basename "$wal")
	recording_ref="rec_$recording_uuid"
	before_segments=$(find "$wal/segments" -type f -name '*.chwal' -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')
	rm "$data/sessions"
	"$CHRONICLE" --format json --data-dir "$data" record --retry "$recording_ref" >"$root/retry.json" 2>"$root/retry.log"
	assert_json "$root/retry.json" 'value["version"] == 1 and value["recording_id"] == "'"$recording_ref"'" and value["status"] == "published"'
	after_segments=$(find "$wal/segments" -type f -name '*.chwal' -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')
	[[ $after_segments == "$before_segments" ]]

	# Deterministic write-only recording separates policy denial from capture loss.
	local fixture_root="$root/write-fixture" replay_data="$root/replay-data" replay_uuid replay_ref
	"$CHRONICLE" --format json internal record-fixture --input "$ROOT/fixtures/http/binary-body.json" --root "$fixture_root" >"$root/write-fixture.json"
	replay_uuid=$(basename "$(find "$fixture_root/wal" -mindepth 1 -maxdepth 1 -type d -print -quit)")
	replay_ref="rec_$replay_uuid"
	install -d -m 0700 "$replay_data"
	mkdir -p "$replay_data/recordings" "$replay_data/sessions"
	cp -a "$fixture_root/wal/$replay_uuid" "$replay_data/recordings/$replay_uuid"
	cp -a "$fixture_root/sessions/." "$replay_data/sessions/"
	"$CHRONICLE" --format json --data-dir "$replay_data" inspect "$replay_ref" >"$root/write-inspect.json"
	assert_json "$root/write-inspect.json" 'value["replayability"] == "fully_replayable" and value["connections"][0]["operations"][0]["effect"] == "write"'

	local v6_fixture="$root/write-v6-fixture.json" v6_fixture_root="$root/write-v6-fixture" v6_data="$root/replay-v6-data" v6_uuid v6_ref
	python3 - "$ROOT/fixtures/http/binary-body.json" "$v6_fixture" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
value["connections"][0]["local_endpoint"]["host"] = "2001:db8::10"
value["connections"][0]["remote_endpoint"]["host"] = "2001:db8::20"
json.dump(value, open(sys.argv[2], "w", encoding="utf-8"))
PY
	"$CHRONICLE" --format json internal record-fixture --input "$v6_fixture" --root "$v6_fixture_root" >"$root/write-v6-fixture-output.json"
	v6_uuid=$(basename "$(find "$v6_fixture_root/wal" -mindepth 1 -maxdepth 1 -type d -print -quit)")
	v6_ref="rec_$v6_uuid"
	install -d -m 0700 "$v6_data"
	mkdir -p "$v6_data/recordings" "$v6_data/sessions"
	cp -a "$v6_fixture_root/wal/$v6_uuid" "$v6_data/recordings/$v6_uuid"
	cp -a "$v6_fixture_root/sessions/." "$v6_data/sessions/"

	# PID and cgroup selectors attach to existing workloads and never terminate them.
	sleep 600 >"$root/pid-target.log" 2>&1 &
	SELECTOR_TARGET_PID=$!
	printf '%s\n' "$SELECTOR_TARGET_PID" >"$scope/cgroup.procs"
	"$CHRONICLE" --format json --data-dir "$root/pid-data" record --pid "$SELECTOR_TARGET_PID" --duration 1 >"$root/pid-record.json" 2>"$root/pid-record.log"
	assert_json "$root/pid-record.json" 'value["version"] == 1 and value["status"] == "completed"'
	kill -0 "$SELECTOR_TARGET_PID"
	stop_process "$SELECTOR_TARGET_PID"
	SELECTOR_TARGET_PID=""
	# One live process in the cgroup keeps the selector scope unambiguous.
	sleep 600 >"$root/cgroup-target.log" 2>&1 &
	SELECTOR_TARGET_PID=$!
	printf '%s\n' "$SELECTOR_TARGET_PID" >"$scope/cgroup.procs"
	"$CHRONICLE" --format json --data-dir "$root/cgroup-data" record --cgroup "$scope" --duration 1 >"$root/cgroup-record.json" 2>"$root/cgroup-record.log"
	assert_json "$root/cgroup-record.json" 'value["version"] == 1 and value["status"] == "completed"'
	kill -0 "$SELECTOR_TARGET_PID"
	stop_process "$SELECTOR_TARGET_PID"
	SELECTOR_TARGET_PID=""

	# Write denial is target-independent: target marker must never be created.
	set +e
	separated_chronicle "$CHRONICLE" --format json --data-dir "$replay_data" replay "$replay_ref" -- \
		/usr/bin/touch "$runtime/denied-target-started" >"$root/replay-denied.json" 2>"$root/replay-denied.error.json"
	local denied_status=$?
	set -e
	[[ $denied_status -eq 4 && ! -e $runtime/denied-target-started && ! -s $root/replay-denied.error.json ]]
	assert_json "$root/replay-denied.json" 'value["version"] == 1 and value["plan"]["preflight_denied"] is True and value["result"]["counts"]["attempted"] == 0 and value["cleanup"]["status"] == "clean"'

	# Inferred IPv4 and IPv6 listeners both replay; mismatch remains factual exit 6.
	separated_chronicle "$CHRONICLE" --format json --data-dir "$replay_data" replay "$replay_ref" --allow-write -- \
		bash -c 'if printf "%s\n" "$$" >/sys/fs/cgroup/cgroup.procs 2>/dev/null; then exit 90; fi; id -u >"$1"; exec python3 "$2" serve --port 8080 --port-file "$3" --requests "$4"' \
		bash "$runtime/replay-v4.uid" "$DRIVER" "$runtime/replay-v4.port" "$runtime/replay-v4-requests.jsonl" \
		>"$root/replay-v4.json" 2>"$root/replay-v4.log"
	assert_json "$root/replay-v4.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips") and value["cleanup"]["status"] in ("clean", "killed")'
	[[ "$(<"$runtime/replay-v4.uid")" == "$target_uid" ]]

	separated_chronicle "$CHRONICLE" --format json --data-dir "$v6_data" replay "$v6_ref" --allow-write -- \
		python3 "$DRIVER" serve --host ::1 --port 8080 --port-file "$runtime/replay-v6.port" --requests "$runtime/replay-v6-requests.jsonl" \
		>"$root/replay-v6.json" 2>"$root/replay-v6.log"
	assert_json "$root/replay-v6.json" 'value["version"] == 1 and value["result"]["outcome"] in ("completed", "completed_with_skips") and value["cleanup"]["status"] in ("clean", "killed")'

	set +e
	separated_chronicle "$CHRONICLE" --format json --data-dir "$replay_data" replay "$replay_ref" --allow-write -- \
		python3 "$DRIVER" serve --mismatch --port 8080 --port-file "$runtime/replay-fail.port" --requests "$runtime/replay-fail-requests.jsonl" \
		>"$root/replay-fail.json" 2>"$root/replay-fail.log"
	local replay_fail_status=$?
	set -e
	[[ $replay_fail_status -eq 6 ]]
	assert_json "$root/replay-fail.json" 'value["version"] == 1 and value["result"]["outcome"] == "stopped_verification" and value["cleanup"]["status"] in ("clean", "killed")'

	# All owned targets/scopes/programs are gone; production destination log is unchanged.
	# The upstream log exists only when an earlier scenario generated workload (P1).
	if [[ -f "$ARTIFACT_ROOT/upstream-requests.jsonl" ]]; then
		[[ $(wc -l <"$ARTIFACT_ROOT/upstream-requests.jsonl") -eq 3 ]]
	fi
	after_scope=$(find "$cgroup_parent" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort)
	cleanup_ui_scope
	# Restore the profile's original EXIT trap; the scenario-owned chain exits.
	trap - ERR
	if [[ -n ${UI_PREV_EXIT_TRAP:-} ]]; then
		eval "$UI_PREV_EXIT_TRAP"
	else
		trap - EXIT
	fi
	[[ $after_scope == "$before_scope" ]]
	# Absolute end-state check: no chronicle capture program may remain loaded.
	bpftool prog show -j 2>/dev/null | python3 -c 'import json,sys; names=[p.get("name","") for p in json.load(sys.stdin)]; raise SystemExit(0 if not any(n.startswith("chronicle") or n in ("connect4", "connect6") for n in names) else 1)'

	python3 - "$root/acceptance.json" "$recording_ref" "$replay_ref" "$target_uid" "$before_segments" <<'PY'
import json, sys
json.dump({
    "version": 1,
    "status": "passed",
    "recording_id": sys.argv[2],
    "replay_recording_id": sys.argv[3],
    "target_uid": int(sys.argv[4]),
    "publication_retry_without_recapture": True,
    "wal_segments_sha256": sys.argv[5],
    "record_command_and_descendant_cleanup": "passed",
    "pid_attach_non_termination": "passed",
    "cgroup_attach_non_termination": "passed",
    "replay_ipv4": "passed",
    "replay_ipv6": "passed",
    "replay_verification_failure_exit_6": "passed",
    "denied_write_target_not_started": "passed",
    "original_destination_untouched": "passed",
    "credential_and_cgroup_separation": "passed",
    "owned_scope_process_ebpf_cleanup": "passed",
}, open(sys.argv[1], "w", encoding="utf-8"), indent=2, sort_keys=True)
with open(sys.argv[1], "a", encoding="utf-8") as output:
    output.write("\n")
PY
	USER_INTENT_RESULT=passed
	if declare -F set_check >/dev/null; then
		set_check user_intent_lifecycle passed
	fi
}

scenario_user_intent_lifecycle() {
	case "$CHRONICLE_ACCEPTANCE_PROFILE" in
	p1) scenario_p1_user_intent_lifecycle ;;
	p2) scenario_p2_user_intent_lifecycle ;;
	*) die "unsupported shared scenario profile: $CHRONICLE_ACCEPTANCE_PROFILE" ;;
	esac
}
