#!/usr/bin/env bash
# shellcheck disable=SC2034 # result globals are consumed by profile summary
# Privileged quick-start scenario: executes the exact command forms documented
# in the README Quick Start against a release-built Linux binary:
#   chronicle doctor
#   chronicle record --name checkout -- ./my-app
#   chronicle list
#   chronicle inspect checkout
#   chronicle replay checkout -- ./my-app
# Self-contained (own data dir, cgroup subtree owned by the supervisor), so a
# single shared owner runs it under both live-capture and recorder.
documented_quickstart_impl() {
	phase documented_quickstart 'execute documented quick start forms: doctor, record, list, inspect, replay'
	local root="$ARTIFACT_ROOT/documented-quickstart"
	local data="$root/data" runtime="$root/runtime"
	local target_uid target_gid
	target_uid=$(stat -c '%u' "$ROOT")
	target_gid=$(stat -c '%g' "$ROOT")
	[[ $target_uid -ne 0 ]]
	install -d -m 0700 "$data"
	install -d -o "$target_uid" -g "$target_gid" -m 0700 "$runtime"

	# 1. chronicle doctor: the documented readiness check. On the privileged
	#    profile the capture probes must be supported, not merely compiled.
	"$CHRONICLE" --format json --data-dir "$data" doctor >"$root/doctor.json"
	assert_json "$root/doctor.json" 'value["version"] == 1'
	python3 - "$root/doctor.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
probes = {probe["code"]: probe["status"] for probe in report["probes"]}
for code in (
    "capture.platform", "capture.cgroup_v2", "capture.btf",
    "capture.object", "capture.programs",
):
    assert probes.get(code) == "supported", (code, probes.get(code))
print("doctor: capture readiness confirmed")
PY

	# 2. chronicle record --name checkout -- ./my-app (command mode). The
	#    recorded application serves HTTP; the scenario sends one representative
	#    request from outside the supervised scope (matching the passing live
	#    capture pattern), then stops the recording with Ctrl+C. The separated
	#    supervisor keeps the real IDs at the target uid while retaining
	#    effective root, so the supervised cgroup scope gets the required
	#    root-only membership-control separation.
	local port_file="$runtime/qs.port" requests="$runtime/qs-requests.jsonl"
	# The recorded application must be reachable on a NON-loopback address:
	# replay refuses to target the exact recorded destination, so a loopback-only
	# recording cannot be command-mode replayed. Production-style traffic sees
	# the app on this host's primary address; the replay copy binds loopback.
	local vm_ip
	vm_ip=$(hostname -I 2>/dev/null | awk '{print $1}')
	[[ -n $vm_ip && $vm_ip != 127.0.0.1 ]] || die "no non-loopback host address available"
	python3 "$ROOT/scripts/acceptance/separated-supervisor.py" "$target_uid" "$target_gid" \
		"$CHRONICLE" --format json --data-dir "$data" record --name checkout -- \
		python3 "$DRIVER" serve --host 0.0.0.0 --port 18080 \
		--port-file "$port_file" --requests "$requests" \
		>"$root/record.json" 2>"$root/record.error.json" &
	local recorder=$!
	wait_for_file "$port_file" 15 || die "quick-start record target did not become ready"
	local port
	port=$(<"$port_file")
	# Representative read-only traffic via the non-loopback address (GET
	# /content-length is a read effect and the exact-minimal plaintext shape
	# proven complete by live capture; the documented replay form therefore
	# needs no --allow-write). The recorded port is fixed (18080) so
	# command-mode replay listener discovery matches while the replay copy's
	# loopback host differs from the recorded non-loopback destination.
	python3 - "$port" "$vm_ip" <<'PY'
import socket, sys
connection = socket.create_connection((sys.argv[2], int(sys.argv[1])), timeout=5)
connection.sendall(b"GET /content-length HTTP/1.1\r\nhost: x\r\n\r\n")
buffered = b""
while b"\r\n\r\n" not in buffered:
    buffered += connection.recv(4096)
head, rest = buffered.split(b"\r\n\r\n", 1)
assert head.split(b"\r\n", 1)[0] == b"HTTP/1.1 200 OK"
connection.close()
PY
	kill -INT "$recorder"
	set +e
	wait "$recorder"
	local record_status=$?
	set -e
	[[ $record_status -eq 0 ]] || die "documented record form failed (exit $record_status)"
	# 3. chronicle list: the catalog shows the named recording.
	"$CHRONICLE" --format json --data-dir "$data" list >"$root/list.json"
	assert_json "$root/list.json" 'any(item["name"] == "checkout" and item["recording_id"].startswith("rec_") for item in value["recordings"])'

	# 4. chronicle inspect checkout: the documented recording reference resolves.
	"$CHRONICLE" --format json --data-dir "$data" inspect checkout >"$root/inspect.json"
	# Live captures carry no per-event timestamps and can show loss-adjacent
	# warnings, so live sessions are at least partially replayable rather than
	# always fully replayable (fixtures are fully replayable deterministically).
	assert_json "$root/inspect.json" 'value["version"] == 1 and value["name"] == "checkout" and value["replayability"] in ("partially_replayable", "fully_replayable")'

	# 5. chronicle replay checkout -- ./my-app (command mode): the supervised
	#    copy is started, its loopback listener inferred, and the read-only
	#    recording replayed and verified. Never contacts a recorded destination.
	python3 "$ROOT/scripts/acceptance/separated-supervisor.py" "$target_uid" "$target_gid" \
		"$CHRONICLE" --format json --data-dir "$data" replay checkout -- \
		python3 "$DRIVER" serve --host 127.0.0.1 --port 18080 \
		--port-file "$runtime/replay.port" --requests "$runtime/replay-requests.jsonl" \
		>"$root/replay.json" 2>"$root/replay.error.json"
	assert_json "$root/replay.json" 'value["result"]["outcome"] in ("completed", "completed_with_skips") and value["result"]["dry_run"] == False and value["result"]["preflight_denied"] == False'
}

scenario_documented_quickstart() {
	documented_quickstart_impl
}
