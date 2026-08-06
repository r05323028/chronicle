#!/usr/bin/env bash
# Shared scenario contract; profile extensions provide runtime-specific assertions.
scenario_wal_recovery() {
	case "$CHRONICLE_ACCEPTANCE_PROFILE" in
		p1) scenario_p1_wal_recovery ;;
		p2) scenario_p2_wal_recovery ;;
		*) die "unsupported shared scenario profile: $CHRONICLE_ACCEPTANCE_PROFILE" ;;
	esac
}
