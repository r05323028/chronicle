#!/usr/bin/env bash
# Central-dispatch scenario: reboot-recovery.
scenario_p2_reboot_recovery() {
if [[ "$PRE_REBOOT" == 1 ]]; then
	phase pre_reboot 'retain live recorder state for VM reboot'
	"$CHRONICLE" --format json recorder-status --state-root "$STATE_ROOT" >"$ARTIFACT_ROOT/pre-reboot-status.json"

	test -f "$STATE_ROOT/wal/recording.json"
	set_check pre_reboot_persistence passed
	exit 0
fi


}
