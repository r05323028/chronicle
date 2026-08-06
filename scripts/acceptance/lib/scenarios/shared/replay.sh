#!/usr/bin/env bash
# Shared scenario contract; profile extensions provide runtime-specific assertions.
scenario_replay() {
	case "$CHRONICLE_ACCEPTANCE_PROFILE" in
		p1) scenario_p1_replay ;;
		p2) scenario_p2_replay ;;
		*) die "unsupported shared scenario profile: $CHRONICLE_ACCEPTANCE_PROFILE" ;;
	esac
}
