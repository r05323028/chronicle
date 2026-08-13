#!/usr/bin/env bash
# Rootless tests for bounded acceptance convergence primitives.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
TMP=$(mktemp -d)
trap 'rm -rf -- "$TMP"' EXIT
export ARTIFACT_ROOT="$TMP"
export CURRENT_PHASE=test-wait
export SCENARIO_TIMEOUT_NAME=wait-contract
# shellcheck source=scripts/acceptance/lib/wait.sh
source "$ROOT/scripts/acceptance/lib/wait.sh"

immediate() { printf 'state=ready\n'; }
wait_until 2 0 'immediate condition' immediate
[[ ! -e "$ARTIFACT_ROOT/current-wait.json" ]]

COUNT=0
eventual() {
	COUNT=$((COUNT + 1))
	printf 'count=%s\n' "$COUNT"
	((COUNT >= 3))
}
wait_until 2 0 'eventual count' eventual
[[ $COUNT == 3 ]]

never() {
	printf 'state=still-waiting\n'
	return 1
}
started=$SECONDS
if wait_until 1 0.05 'condition that never converges' never; then exit 1; fi
((SECONDS - started < 3))
python3 - "$ARTIFACT_ROOT/wait-failure-wait-contract.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
assert value["status"] == "timed_out", value
assert value["condition"] == "condition that never converges", value
assert value["last_observation"] == "state=still-waiting", value
assert value["scenario"] == "wait-contract", value
PY

terminal() {
	printf 'state=failed reason=corruption\n'
	return 2
}
started=$SECONDS
if wait_until 5 0 'terminal condition' terminal; then exit 1; fi
((SECONDS - started < 2))
grep -q 'state=failed reason=corruption' "$ARTIFACT_ROOT/wait-failure-wait-contract.json"

present="$TMP/present"
: >"$present"
wait_for_path "$present" 1
rm "$present"
wait_for_path_absent "$present" 1

started=$SECONDS
wait_for_elapsed_time 1 'configured age threshold'
((SECONDS - started >= 1))
[[ ! -e "$ARTIFACT_ROOT/current-wait.json" ]]
printf '%s\n' 'acceptance wait contract tests passed'
