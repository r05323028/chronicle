#!/usr/bin/env bash
# Single scenario dispatcher. Profile files provide setup/cleanup; scenario files provide assertions.
set -euo pipefail

SCENARIO_ROOT="$ROOT/scripts/acceptance/lib/scenarios"

source_scenarios() {
	local profile=$1 scenario
	shift
	for scenario in "$@"; do
		[[ -f "$SCENARIO_ROOT/$profile/$scenario.sh" ]] || die "scenario $scenario is not implemented for $profile"
		# shellcheck source=/dev/null
		source "$SCENARIO_ROOT/$profile/$scenario.sh"
	done
}

scenario_order_from_toml() {
	local profile=$1
	python3 - "$ROOT/scripts/acceptance/scenarios.toml" "$profile" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as handle:
    value = tomllib.load(handle)
print(",".join(value["profiles"][sys.argv[2]].get("execution_order", value["profiles"][sys.argv[2]]["scenarios"])))
PY
}

run_scenario_plan() {
	local profile=$1 scenario function
	local -a selected
	if [[ -n ${CHRONICLE_ACCEPTANCE_SCENARIOS:-} ]]; then
		IFS=',' read -r -a selected <<<"$CHRONICLE_ACCEPTANCE_SCENARIOS"
	else
		IFS=',' read -r -a selected <<<"$(scenario_order_from_toml "$profile")"
	fi
	[[ ${#selected[@]} -gt 0 ]] || die 'central scenario dispatcher received no scenarios'
	local unique_count
	unique_count=$(printf '%s\n' "${selected[@]}" | sort -u | wc -l | tr -d ' ')
	[[ "$unique_count" == "${#selected[@]}" ]] || die "scenario selection is duplicated for $profile"
	source_scenarios "$profile" "${selected[@]}"
	for scenario in "${selected[@]}"; do
		function="scenario_${profile}_${scenario//-/_}"
		declare -F "$function" >/dev/null || die "scenario function missing: $function"
		if declare -F log >/dev/null; then
			log "[scenario] $profile/$scenario"
		else
			printf '[scenario] %s/%s\n' "$profile" "$scenario"
		fi
		"$function"
	done
}
