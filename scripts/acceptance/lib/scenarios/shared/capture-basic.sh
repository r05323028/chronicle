#!/usr/bin/env bash
# Shared scenario contract; profile extensions provide runtime-specific assertions.
scenario_capture_basic() {
	case "$CHRONICLE_ACCEPTANCE_PROFILE" in
		p1) scenario_p1_capture_basic ;;
		p2) scenario_p2_capture_basic ;;
		*) die "unsupported shared scenario profile: $CHRONICLE_ACCEPTANCE_PROFILE" ;;
	esac
}
