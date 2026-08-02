#!/usr/bin/env bash
# Rootless contract tests for machine-readable P2 readiness polling.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
LIB="$ROOT/scripts/acceptance/recorder-readiness.sh"
TMP_DIR=$(mktemp -d)
trap 'rm -rf -- "$TMP_DIR"' EXIT

make_fake() {
	local path=$1
	cat >"$path" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
count=$(cat "$FAKE_COUNT" 2>/dev/null || printf 0)
count=$((count + 1))
printf '%s\n' "$count" >"$FAKE_COUNT"
case "$FAKE_MODE:$count" in
  unavailable:1|timeout:*) exit 1 ;;
  failed:1|stale:1|restart:1) printf '%s\n' '{"state":"failed","lifecycle":"failed","capture_readiness":"not_ready","processing_readiness":"not_ready","health":"failed","stale_owner":true}' ;;
  legacy:1) printf '%s\n' '{"lifecycle":"running","capture_readiness":"ready","processing_readiness":"ready","health":"healthy","stale_owner":false}' ;;
  transition:1|recovery:1) printf '%s\n' '{"state":"starting","lifecycle":"starting","capture_readiness":"not_ready","processing_readiness":"unknown","health":"degraded"}' ;;
  transition:2|recovery:2) printf '%s\n' '{"state":"recovering","lifecycle":"recovering","capture_readiness":"not_ready","processing_readiness":"unknown","health":"degraded"}' ;;
  transition:3|recovery:3) printf '%s\n' '{"state":"loading_ebpf","lifecycle":"recovering","capture_readiness":"not_ready","processing_readiness":"unknown","health":"degraded"}' ;;
  *) printf '%s\n' '{"state":"ready","lifecycle":"running","capture_readiness":"ready","processing_readiness":"not_ready","health":"degraded","stale_owner":false}' ;;
esac
EOF
	chmod +x "$path"
}

new_case() {
	CASE_DIR=$(mktemp -d "$TMP_DIR/case.XXXXXX")
	mkdir -p "$CASE_DIR/state/wal" "$CASE_DIR/cgroup"
	: >"$CASE_DIR/count"
	FAKE="$CASE_DIR/chronicle"
	make_fake "$FAKE"
	export ARTIFACT_ROOT="$CASE_DIR" STATE_ROOT="$CASE_DIR/state" CGROUP="$CASE_DIR/cgroup"
	export UNIT=chronicle-test RECORDER_STATUS="$CASE_DIR/recorder-status.json"
	export CHRONICLE="$FAKE" FAKE_COUNT="$CASE_DIR/count"
}

# 1, 2, 9: unavailable startup, explicit transitions, visible transition output.
new_case; export FAKE_MODE=transition
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0
[[ $(grep -c 'recorder-state=' "$CASE_DIR/readiness-transitions.log") -ge 4 ]]
grep -q 'state=starting' "$CASE_DIR/readiness-transitions.log"
grep -q 'state=recovering' "$CASE_DIR/readiness-transitions.log"
grep -q 'state=loading_ebpf' "$CASE_DIR/readiness-transitions.log"

# Legacy status without explicit state still derives ready state.
new_case; export FAKE_MODE=legacy
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0
grep -q 'state=ready' "$CASE_DIR/readiness-transitions.log"

# 3: caller admits workload only after helper returns ready.
new_case; export FAKE_MODE=transition
source "$LIB"
admitted=false
wait_for_recorder_ready --timeout 5 --interval 0
admitted=true
[[ $admitted == true ]]

# 4, 8: terminal failed and stale-owner states stop immediately.
for mode in failed stale; do
	new_case; export FAKE_MODE=$mode
	source "$LIB"
	if wait_for_recorder_ready --timeout 5 --interval 0; then
		echo "${mode} state unexpectedly reached ready" >&2
		exit 1
	fi
	[[ $(cat "$CASE_DIR/count") == 1 ]]
done

# 5: timeout produces bounded diagnostics, not build/cache data.
new_case; export FAKE_MODE=timeout
source "$LIB"
if wait_for_recorder_ready --timeout 1 --interval 1; then exit 1; fi
for file in recorder-status.json recorder-service-status.txt recorder-journal.log kernel-version.txt kernel-capabilities.json cgroup-information.txt btf-information.txt bpftool-programs.txt bpftool-links.txt wal-directory-listing.txt checkpoint-metadata.json process-list.txt disk-space.txt readiness-transitions.log; do
	[[ -f "$CASE_DIR/$file" ]]
done
[[ ! -e "$CASE_DIR/target" ]]

# 6: recorder restart follows same contract.
new_case; export FAKE_MODE=transition
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0
: >"$CASE_DIR/count"
wait_for_recorder_ready --timeout 5 --interval 0

# Restart tolerates transient stale ownership, but still requires fresh ready state.
new_case; export FAKE_MODE=restart
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0 --allow-stale-owner

# 7: recovery sequence with checkpoint metadata reaches ready.
new_case; export FAKE_MODE=recovery
printf '%s\n' '{"version":1,"owner":"recorder","lifecycle":"active","decoder":{"state":[]},"segment_lineage":[]}' >"$CASE_DIR/state/wal/incremental-etl-checkpoint.json"
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0

# 10: success emits only small polling metadata, not failure diagnostics.
new_case; export FAKE_MODE=transition
source "$LIB"
wait_for_recorder_ready --timeout 5 --interval 0
[[ ! -e "$CASE_DIR/recorder-journal.log" ]]
[[ $(du -sk "$CASE_DIR" | awk '{print $1}') -lt 128 ]]
printf '%s\n' 'p2 readiness contract tests passed'
