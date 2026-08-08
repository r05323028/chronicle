#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
TIMEOUT="$ROOT/scripts/run-with-timeout.sh"
TMP=$(mktemp -d)
trap 'rm -rf -- "$TMP"' EXIT

fail() {
	printf 'timeout wrapper test failed: %s\n' "$*" >&2
	exit 1
}

assert_gone() {
	local pid=$1 state
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		if ! kill -0 "$pid" 2>/dev/null; then return 0; fi
		state=$(ps -o stat= -p "$pid" 2>/dev/null || true)
		[[ $state == Z* ]] && return 0
		sleep 0.1
	done
	fail "process $pid survived timeout"
}

[[ $("$TIMEOUT" 5 sh -c 'printf success') == success ]]
set +e
"$TIMEOUT" 5 sh -c 'exit 7'
status=$?
set -e
[[ $status == 7 ]] || fail "expected original exit 7, got $status"

"$TIMEOUT" 5 sh -c 'printf stdout; printf stderr >&2' >"$TMP/stdout" 2>"$TMP/stderr"
[[ $(<"$TMP/stdout") == stdout ]]
[[ $(<"$TMP/stderr") == stderr ]]

set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 1 sh -c 'sleep 30 & echo $! >"$1"; wait' sh "$TMP/child.pid" >/dev/null 2>"$TMP/child.err"
status=$?
set -e
[[ $status == 124 ]] || fail "expected child timeout 124, got $status"
grep -q 'timed out after 1s' "$TMP/child.err"
assert_gone "$(<"$TMP/child.pid")"

set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 1 sh -c 'trap "" TERM; echo $$ >"$1"; while :; do sleep 1; done' sh "$TMP/ignore.pid" >/dev/null 2>"$TMP/ignore.err"
status=$?
set -e
[[ $status == 124 ]] || fail "expected TERM-ignoring timeout 124, got $status"
assert_gone "$(<"$TMP/ignore.pid")"

set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 1 sh -c '
python3 - "$1" <<"PY" &
import os, signal, sys, time
os.setsid()
signal.signal(signal.SIGTERM, signal.SIG_IGN)
open(sys.argv[1], "w", encoding="utf-8").write(str(os.getpid()))
time.sleep(30)
PY
wait
' sh "$TMP/detached.pid" >/dev/null 2>"$TMP/detached.err"
status=$?
set -e
[[ $status == 124 ]] || fail "expected detached-child timeout 124, got $status"
assert_gone "$(<"$TMP/detached.pid")"

set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 1 python3 - "$TMP/double-fork.pid" <<'PY' >/dev/null 2>"$TMP/double-fork.err"
import os, signal, sys, time
if os.fork() == 0:
    if os.fork() == 0:
        os.setsid()
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        with open(sys.argv[1], "w", encoding="utf-8") as target:
            target.write(str(os.getpid()))
        time.sleep(30)
    os._exit(0)
time.sleep(30)
PY
status=$?
set -e
[[ $status == 124 ]] || fail "expected double-fork timeout 124, got $status"
if [[ $(uname -s) == Linux ]]; then
	assert_gone "$(<"$TMP/double-fork.pid")"
else
	# Linux subreaper is authoritative; macOS has no stdlib subreaper API.
	kill -KILL "$(<"$TMP/double-fork.pid")" 2>/dev/null || true
fi

# Normal direct-child completion also cleans adopted descendants while
# preserving the direct command status.
set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 5 python3 - "$TMP/normal-double-fork.pid" <<'PY' >/dev/null 2>"$TMP/normal-double-fork.err"
import os, signal, sys, time
if os.fork() == 0:
    if os.fork() == 0:
        os.setsid()
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        with open(sys.argv[1], "w", encoding="utf-8") as target:
            target.write(str(os.getpid()))
        time.sleep(30)
    os._exit(0)
time.sleep(0.2)
PY
status=$?
set -e
[[ $status == 0 ]] || fail "expected normal double-fork command exit 0, got $status"
if [[ $(uname -s) == Linux ]]; then
	assert_gone "$(<"$TMP/normal-double-fork.pid")"
else
	kill -KILL "$(<"$TMP/normal-double-fork.pid")" 2>/dev/null || true
fi

set +e
CHRONICLE_TIMEOUT_GRACE_SECONDS=1 "$TIMEOUT" 1 "$TIMEOUT" 30 sh -c 'trap "" TERM; echo $$ >"$1"; while :; do sleep 1; done' sh "$TMP/nested.pid" >/dev/null 2>"$TMP/nested.err"
status=$?
set -e
[[ $status == 124 ]] || fail "expected nested-wrapper timeout 124, got $status"
assert_gone "$(<"$TMP/nested.pid")"

set +e
"$TIMEOUT" nope true >/dev/null 2>"$TMP/invalid"
status=$?
set -e
[[ $status == 2 ]] || fail "expected invalid timeout exit 2, got $status"
grep -q 'positive integer' "$TMP/invalid"

printf 'timeout wrapper tests passed\n'
