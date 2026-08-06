#!/usr/bin/env bash
# Shared scenario contract; profile extensions provide runtime-specific assertions.
scenario_resource_cleanup() {
	case "$CHRONICLE_ACCEPTANCE_PROFILE" in
	p1) scenario_p1_resource_cleanup ;;
	p2) scenario_p2_resource_cleanup ;;
	*) die "unsupported shared scenario profile: $CHRONICLE_ACCEPTANCE_PROFILE" ;;
	esac
}
