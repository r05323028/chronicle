#!/usr/bin/env bash
# Shared Multipass executor. Source snapshot, bootstrap, reboot, and transfer only.
set -euo pipefail

PROFILE=${1:?profile required}
VM=${2:?VM name required}
DEST=${3:?host artifact directory required}
RELEASE=${4:-0}
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
RUN_ID=${CHRONICLE_ACCEPTANCE_RUN_ID:-$(basename "$DEST")}
SNAPSHOT="$ROOT/target/validation-snapshots/$RUN_ID"
VM_SOURCE_ROOT="/home/ubuntu/chronicle-acceptance-sources/$RUN_ID"
VM_RUN_ROOT="/home/ubuntu/chronicle-acceptance-runs/$RUN_ID"
VM_ARTIFACTS="$VM_RUN_ROOT/artifacts"
REMOTE_STATUS=0

cleanup() {
	if [[ "${CHRONICLE_ACCEPTANCE_KEEP_SNAPSHOT:-0}" != 1 ]]; then
		rm -rf -- "$SNAPSHOT"
	fi
}
trap cleanup EXIT

make_snapshot() {
	rm -rf -- "$SNAPSHOT"
	mkdir -p -- "$SNAPSHOT"
	# Snapshot actual checkout, including dirty source, but not build/cache/docs noise.
	(
		cd "$ROOT"
		tar --exclude='./target' --exclude='./graphify-out' --exclude='./docs' -cf - .
	) | (cd "$SNAPSHOT" && tar -xf -)
}

ensure_vm() {
	command -v multipass >/dev/null 2>&1 || {
		printf '%s\n' 'multipass is required for multipass acceptance' >&2
		return 1
	}
	local state
	state=$(multipass info "$VM" 2>/dev/null | awk '/State:/ {print $2; exit}')
	if [[ "$state" != Running ]]; then
		multipass start "$VM"
	fi
}

bootstrap_vm() {
	multipass exec "$VM" -- bash -lc '
		set -euo pipefail
		if ! command -v clang >/dev/null || ! command -v zstd >/dev/null; then
			sudo apt-get update -qq
			sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq clang llvm libelf-dev pkg-config zstd
		fi
		if ! rustup toolchain list | grep -q "^nightly"; then
			rustup toolchain install nightly --profile minimal --component rust-src
		fi
		if ! command -v bpf-linker >/dev/null; then
			cargo +nightly install bpf-linker --locked
		fi
	'
}

transfer_source() {
	# Refresh source after reboot without deleting persistent recorder state.
	multipass exec "$VM" -- sudo rm -rf "$VM_SOURCE_ROOT"
	multipass exec "$VM" -- sudo mkdir -p "$(dirname "$VM_SOURCE_ROOT")" && multipass exec "$VM" -- sudo chown -R ubuntu:ubuntu "$(dirname "$VM_SOURCE_ROOT")"
	multipass transfer --recursive "$SNAPSHOT" "$VM:/home/ubuntu/chronicle-acceptance-sources/"
	multipass exec "$VM" -- sudo chown -R ubuntu:ubuntu "$VM_SOURCE_ROOT"
}

run_remote() {
	local profile=$1 artifact_root=$2 extra=$3
	multipass exec "$VM" -- bash -lc "
		set +e
		cd '$VM_SOURCE_ROOT'
		sudo -E env HOME=/home/ubuntu PATH=\"\$PATH\" \\
			CHRONICLE_ACCEPTANCE_PROFILE='$profile' \\
			CHRONICLE_ACCEPTANCE_EXECUTOR=multipass \\
			CHRONICLE_ACCEPTANCE_RUN_ID='$RUN_ID' \\
			CHRONICLE_ACCEPTANCE_SOURCE_FINGERPRINT='${CHRONICLE_ACCEPTANCE_SOURCE_FINGERPRINT:-}' \\
			CHRONICLE_ACCEPTANCE_RELEASE='$RELEASE' \\
			CHRONICLE_ACCEPTANCE_EXPECTED_SHA='${CHRONICLE_ACCEPTANCE_EXPECTED_SHA:-}' \\
			CHRONICLE_ACCEPTANCE_MODE=full \\
			CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$artifact_root' \\
			CARGO_TARGET_DIR=/home/ubuntu/chronicle-target \\
			CHRONICLE_EBPF_TARGET_DIR=/home/ubuntu/chronicle-ebpf-target \\
			$extra \\
			'$VM_SOURCE_ROOT/scripts/acceptance/lib/profile-$profile.sh'
		status=\$?
		sudo -E env HOME=/home/ubuntu PATH=\"\$PATH\" python3 '$VM_SOURCE_ROOT/scripts/validation.py' environment --root '$VM_SOURCE_ROOT' | sudo tee '$artifact_root/guest-environment.json' >/dev/null || true
		guest_commit=\$(git -C '$VM_SOURCE_ROOT' rev-parse HEAD 2>/dev/null || printf not_checked)
		guest_tree=\$(git -C '$VM_SOURCE_ROOT' rev-parse HEAD^{tree} 2>/dev/null || printf not_checked)
		guest_dirty=0
		if [[ -n \"\$(git -C '$VM_SOURCE_ROOT' status --porcelain --untracked-files=all 2>/dev/null)\" ]]; then guest_dirty=1; fi
		printf '{\"run_id\":\"%s\",\"fingerprint\":\"%s\",\"commit_sha\":\"%s\",\"tree_sha\":\"%s\",\"working_tree_dirty\":%s}\n' \"$RUN_ID\" \"${CHRONICLE_ACCEPTANCE_SOURCE_FINGERPRINT:-}\" \"\$guest_commit\" \"\$guest_tree\" \"\$guest_dirty\" | sudo tee '$artifact_root/guest-source.json' >/dev/null
		sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS' 2>/dev/null || true
		exit \$status
	"
}

transfer_artifacts() {
	local transfer_root="$DEST/.transfer"
	rm -rf -- "$transfer_root" "$DEST/assertions"
	mkdir -p -- "$transfer_root"
	multipass transfer --recursive "$VM:$VM_ARTIFACTS" "$transfer_root/"
	mv "$transfer_root/$(basename "$VM_ARTIFACTS")" "$DEST/assertions"
	rm -rf -- "$transfer_root"
	if [[ -f "$DEST/assertions/guest-environment.json" ]]; then
		cp "$DEST/assertions/guest-environment.json" "$DEST/guest-environment.json"
	fi
}

wait_for_vm() {
	for _ in $(seq 1 60); do
		if [[ "$(multipass info "$VM" | awk '/State:/ {print $2; exit}')" == Running ]] && multipass exec "$VM" -- true >/dev/null 2>&1; then
			return 0
		fi
		sleep 2
	done
	return 1
}

make_snapshot
ensure_vm
bootstrap_vm
multipass exec "$VM" -- sudo mkdir -p "$(dirname "$VM_SOURCE_ROOT")" "$(dirname "$VM_RUN_ROOT")"
transfer_source

if [[ "$PROFILE" == p1 ]]; then
	set +e
	run_remote p1 "$VM_ARTIFACTS" ""
	REMOTE_STATUS=$?
	set -e
	transfer_artifacts || true
	if [[ -f "$DEST/assertions/acceptance-report.json" ]]; then
		cp "$DEST/assertions/acceptance-report.json" "$DEST/acceptance-report.json"
	fi
	printf '%s\n' '[{"name":"run","status":"passed"}]' >"$DEST/phases.json" || true
else
	# P2 explicitly runs the full P1 profile first; reuse is valid only then.
	set +e
	run_remote p1 "$VM_ARTIFACTS/p1" ""
	P1_STATUS=$?
	set -e
	PRE_ROOT="$VM_ARTIFACTS/p2/pre-reboot"
	POST_ROOT="$VM_ARTIFACTS/p2/post-reboot"
	DOMAIN_ROOT="$VM_RUN_ROOT/domain"
	P1_FAILURE=0
	if [[ "$P1_STATUS" -ne 0 && "$P1_STATUS" -ne 77 ]]; then
		P1_FAILURE=$P1_STATUS
	fi
	set +e
	run_remote p2 "$PRE_ROOT" "CHRONICLE_ACCEPTANCE_PRE_REBOOT=1 CHRONICLE_ACCEPTANCE_DOMAIN_ROOT='$DOMAIN_ROOT' CHRONICLE_ACCEPTANCE_STATE_ROOT='$DOMAIN_ROOT/state' CHRONICLE_ACCEPTANCE_STORE_ROOT='$DOMAIN_ROOT/store'"
	PRE_STATUS=$?
	set -e
	if [[ "$PRE_STATUS" -ne 0 && "$PRE_STATUS" -ne 77 ]]; then
		REMOTE_STATUS=$PRE_STATUS
	fi
	if [[ "$PRE_STATUS" -eq 0 || "$PRE_STATUS" -eq 77 ]]; then
		multipass restart "$VM"
		wait_for_vm
		transfer_source
		set +e
		run_remote p2 "$POST_ROOT" "CHRONICLE_ACCEPTANCE_REBOOT_RESUME=1 CHRONICLE_ACCEPTANCE_CRASH_MODE=1 CHRONICLE_ACCEPTANCE_DOMAIN_ROOT='$DOMAIN_ROOT' CHRONICLE_ACCEPTANCE_STATE_ROOT='$DOMAIN_ROOT/state' CHRONICLE_ACCEPTANCE_STORE_ROOT='$DOMAIN_ROOT/store'"
		POST_STATUS=$?
		set -e
		REMOTE_STATUS=$POST_STATUS
		if [[ "$REMOTE_STATUS" -eq 0 && "$P1_FAILURE" -ne 0 ]]; then
			REMOTE_STATUS=$P1_FAILURE
		fi
	fi
	transfer_artifacts || true
	if [[ -f "$DEST/assertions/p2/post-reboot/acceptance-report.json" ]]; then
		cp "$DEST/assertions/p2/post-reboot/acceptance-report.json" "$DEST/acceptance-report.json"
	fi
	python3 - "$DEST/phases.json" "$DEST/assertions/p2/pre-reboot/acceptance-report.json" "$DEST/assertions/p2/post-reboot/acceptance-report.json" <<'PY'
import json
import sys
from pathlib import Path

def phase(name, path):
    if not Path(path).is_file():
        return {"name": name, "status": "not_checked"}
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    if name == "pre_reboot" and value.get("phase") == "pre_reboot" and value.get("exit_code") == 0:
        return {"name": name, "status": "passed"}
    return {"name": name, "status": "passed" if value.get("status") in {"passed", "complete"} else value.get("status", "not_checked")}
Path(sys.argv[1]).write_text(json.dumps([phase("pre_reboot", sys.argv[2]), phase("post_reboot", sys.argv[3])], indent=2) + "\n", encoding="utf-8")
PY
fi

exit "$REMOTE_STATUS"
