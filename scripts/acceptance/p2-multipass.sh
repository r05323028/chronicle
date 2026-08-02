#!/usr/bin/env bash
# Runs P2 privileged acceptance in a clean exact-SHA Multipass VM.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VM=${1:-chronicle-ubuntu}
SHA=${CHRONICLE_ACCEPTANCE_SOURCE_SHA:-$(git -C "$ROOT" rev-parse HEAD)}
if [[ -z "${CHRONICLE_ACCEPTANCE_SOURCE_SHA:-}" && -n "$(git -C "$ROOT" status --porcelain --untracked-files=all)" ]]; then
	printf '%s\n' 'host working tree must be clean' >&2
	exit 1
fi

VM_ROOT=/home/ubuntu/chronicle-p2-acceptance
VM_ARTIFACTS=/home/ubuntu/p2-artifacts/$SHA
DEST="$ROOT/evidence/privileged/$SHA/ubuntu-24.04"

multipass exec "$VM" -- bash -lc "
  set -euo pipefail
  sudo rm -rf '$VM_ROOT' '$VM_ARTIFACTS'
  git clone --quiet --no-local /mnt/chronicle '$VM_ROOT'
  git -C '$VM_ROOT' checkout --quiet --detach '$SHA'
  test \"\$(git -C '$VM_ROOT' rev-parse HEAD)\" = '$SHA'
  cd '$VM_ROOT'
  sudo -E env \\
    HOME=/home/ubuntu \\
    PATH=\"\$PATH\" \\
    CHRONICLE_ACCEPTANCE_MODE=full \\
    CHRONICLE_ACCEPTANCE_PRE_REBOOT=1 \\
    CHRONICLE_ACCEPTANCE_EXPECTED_SHA='$SHA' \\
    CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$VM_ARTIFACTS' \\
    ./scripts/acceptance/p2-privileged.sh
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS'
"

multipass restart "$VM" || true
for _ in $(seq 1 60); do
	if [[ "$(multipass info "$VM" | awk '/State:/ {print $2}')" == Running ]] && multipass exec "$VM" -- true >/dev/null 2>&1; then
		break
	fi
	sleep 2
done
multipass mount "$ROOT" "$VM:/mnt/chronicle" 2>/dev/null || true
multipass exec "$VM" -- bash -lc "
  set -euo pipefail
  test \"\$(git -C '$VM_ROOT' rev-parse HEAD)\" = '$SHA'
  test -f '$VM_ARTIFACTS/acceptance-report.json'
  cd '$VM_ROOT'
  sudo -E env \\
    HOME=/home/ubuntu \\
    PATH=\"\$PATH\" \\
    CHRONICLE_ACCEPTANCE_MODE=full \\
    CHRONICLE_ACCEPTANCE_REBOOT_RESUME=1 \\
    CHRONICLE_ACCEPTANCE_CRASH_MODE=1 \\
    CHRONICLE_ACCEPTANCE_EXPECTED_SHA='$SHA' \\
    CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$VM_ARTIFACTS/reboot-resume' \\
    CHRONICLE_ACCEPTANCE_STATE_ROOT='$VM_ARTIFACTS/state' \\
    CHRONICLE_ACCEPTANCE_STORE_ROOT='$VM_ARTIFACTS/store' \\
    CHRONICLE_ACCEPTANCE_DOMAIN_ROOT='$VM_ARTIFACTS' \\
    ./scripts/acceptance/p2-privileged.sh
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS'
"

mkdir -p "$DEST"
multipass transfer --recursive "$VM:$VM_ARTIFACTS/." "$DEST/"
printf '%s\n' "$SHA" >"$DEST/tested-commit.txt"
python3 - "$DEST/reboot-resume/acceptance-report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["git_commit_sha"] == report["expected_git_commit_sha"]
assert report["status"] == "passed", report
assert not report["not_checked"], report
PY
if [[ -f "$DEST/artifact-manifest.sha256" ]]; then
	(cd "$DEST" && sha256sum -c artifact-manifest.sha256)
fi
printf '%s\n' "Retained evidence: $DEST"
