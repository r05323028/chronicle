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
VM_ARTIFACTS=/home/ubuntu/p2-artifacts/$SHA/$(date -u +%Y%m%dT%H%M%SZ)-$$
DEST=${CHRONICLE_ACCEPTANCE_DEST:-"$ROOT/target/validation-evidence/privileged/p2/$SHA/ubuntu-24.04"}
COMPACT=${CHRONICLE_ACCEPTANCE_COMPACT:-0}
CARGO_TARGET_DIR_VM=${CARGO_TARGET_DIR:-/home/ubuntu/chronicle-target}
EBPF_TARGET_DIR_VM=${CHRONICLE_EBPF_TARGET_DIR:-/home/ubuntu/chronicle-ebpf-target}

ensure_vm_source() {
	local state
	state=$(multipass info "$VM" 2>/dev/null | awk '/State:/ {print $2; exit}')
	if [[ $state != Running ]]; then
		multipass start "$VM"
	fi
	if ! multipass exec "$VM" -- test -f /mnt/chronicle/scripts/acceptance/p2-privileged.sh; then
		multipass mount "$ROOT" "$VM:/mnt/chronicle"
	fi
	multipass exec "$VM" -- test -f /mnt/chronicle/scripts/acceptance/p2-privileged.sh
}

ensure_vm_source

remote_report_matches() {
	local path=$1 phase=$2 status=$3 exit_code=$4 require_empty=$5
	multipass exec "$VM" -- python3 - "$path" "$phase" "$status" "$exit_code" "$require_empty" <<'PY'
import json
import sys

path, phase, status, exit_code, require_empty = sys.argv[1:]
report = json.load(open(path, encoding="utf-8"))
checks = [
    report.get("phase") == phase,
    report.get("status") == status,
    report.get("exit_code") == int(exit_code),
    isinstance(report.get("not_checked"), list),
]
if require_empty == "true":
    checks.append(not report["not_checked"])
if not all(checks):
    raise SystemExit(f"invalid acceptance report: {path}")
PY
}

PRE_STATUS=0
multipass exec "$VM" -- bash -lc "
  set -euo pipefail
  BOOTSTRAP_MARKER=/home/ubuntu/.chronicle-validation-bootstrap-v1
  if ! command -v clang >/dev/null || ! command -v zstd >/dev/null; then
    sudo apt-get update -qq
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq clang llvm libelf-dev pkg-config zstd
  fi
  if ! rustup toolchain list | grep -q '^nightly'; then
    rustup toolchain install nightly --profile minimal --component rust-src
  fi
  if ! command -v bpf-linker >/dev/null; then
    cargo +nightly install bpf-linker --locked
  fi
  touch \"\$BOOTSTRAP_MARKER\"
  for stale in \$(systemctl list-units --all --plain --no-legend --type=service 'chronicle-p2-*.service' | awk '\$1 ~ /^chronicle-p2-/ {print \$1}'); do
    sudo systemctl stop \"\$stale\" 2>/dev/null || true
  done
  sudo rm -rf '$VM_ROOT' '$VM_ARTIFACTS'
  sudo git clone --quiet --no-local /mnt/chronicle '$VM_ROOT'
  sudo git -C '$VM_ROOT' checkout --quiet --detach '$SHA'
  sudo git config --global --add safe.directory '$VM_ROOT'
  sudo chown -R ubuntu:ubuntu '$VM_ROOT'
  test \"\$(git -C '$VM_ROOT' rev-parse HEAD)\" = '$SHA'
  cd '$VM_ROOT'
  set +e
  sudo -E env \\
    HOME=/home/ubuntu \\
    PATH=\"\$PATH\" \\
    CHRONICLE_ACCEPTANCE_MODE=full \\
    CHRONICLE_ACCEPTANCE_PRE_REBOOT=1 \\
    CHRONICLE_ACCEPTANCE_EXPECTED_SHA='$SHA' \\
    CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$VM_ARTIFACTS' \\
    CARGO_TARGET_DIR='$CARGO_TARGET_DIR_VM' \\
    CHRONICLE_EBPF_TARGET_DIR='$EBPF_TARGET_DIR_VM' \\
    ./scripts/acceptance/p2-privileged.sh
  status=\$?
  set -e
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS' 2>/dev/null || true
  sudo chmod -R u+rwX,go+rX '$VM_ARTIFACTS' 2>/dev/null || true
  exit \$status
" || PRE_STATUS=$?
if [[ $PRE_STATUS -ne 0 ]] && remote_report_matches "$VM_ARTIFACTS/acceptance-report.json" pre_reboot not_checked 0 false; then
	# Multipass can report the detached PRE_REBOOT shell as failed while its report is successful.
	PRE_STATUS=0
fi
if [[ $PRE_STATUS -ne 0 ]]; then
	rm -rf "$DEST"
	mkdir -p "$(dirname "$DEST")"
	multipass transfer --recursive "$VM:$VM_ARTIFACTS" "$DEST" || true
	exit "$PRE_STATUS"
fi

multipass restart "$VM" || true
for _ in $(seq 1 60); do
	if [[ "$(multipass info "$VM" | awk '/State:/ {print $2}')" == Running ]] && multipass exec "$VM" -- true >/dev/null 2>&1; then
		break
	fi
	sleep 2
done
ensure_vm_source

SECOND_STATUS=0
multipass exec "$VM" -- bash -lc "
  set -euo pipefail
  for stale in \$(systemctl list-units --all --plain --no-legend --type=service 'chronicle-p2-*.service' | awk '\$1 ~ /^chronicle-p2-/ {print \$1}'); do
    sudo systemctl kill --kill-who=main -s SIGKILL \"\$stale\" 2>/dev/null || true
    sudo systemctl reset-failed \"\$stale\" 2>/dev/null || true
  done
  test \"\$(git -C '$VM_ROOT' rev-parse HEAD)\" = '$SHA'
  test -f '$VM_ARTIFACTS/acceptance-report.json'
  cd '$VM_ROOT'
  set +e
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
    CARGO_TARGET_DIR='$CARGO_TARGET_DIR_VM' \\
    CHRONICLE_EBPF_TARGET_DIR='$EBPF_TARGET_DIR_VM' \\
    ./scripts/acceptance/p2-privileged.sh
  status=\$?
  set -e
  for stale in \$(systemctl list-units --all --plain --no-legend --type=service 'chronicle-p2-*.service' | awk '\$1 ~ /^chronicle-p2-/ {print \$1}'); do
    sudo systemctl stop \"\$stale\" 2>/dev/null || true
  done
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS' 2>/dev/null || true
  sudo chmod -R u+rwX,go+rX '$VM_ARTIFACTS' 2>/dev/null || true
  if [[ '$COMPACT' == 1 ]] && grep -q '\"status\": \"passed\"' '$VM_ARTIFACTS/reboot-resume/acceptance-report.json'; then
    find '$VM_ARTIFACTS' -mindepth 1 -maxdepth 1 ! -name acceptance-report.json ! -name artifact-manifest.sha256 ! -name reboot-resume -exec rm -rf -- {} +
    find '$VM_ARTIFACTS/reboot-resume' -mindepth 1 -maxdepth 1 ! -name acceptance-report.json ! -name artifact-manifest.sha256 -exec rm -rf -- {} +
    for manifest_root in '$VM_ARTIFACTS' '$VM_ARTIFACTS/reboot-resume'; do
      (cd \"\$manifest_root\" && find . -type f ! -name artifact-manifest.sha256 -print0 | sort -z | xargs -0 sha256sum > artifact-manifest.sha256)
    done
  fi
  exit \$status
" || SECOND_STATUS=$?
if [[ $SECOND_STATUS -ne 0 ]] && remote_report_matches "$VM_ARTIFACTS/reboot-resume/acceptance-report.json" artifacts passed 0 true; then
	SECOND_STATUS=0
fi

rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
multipass transfer --recursive "$VM:$VM_ARTIFACTS" "$DEST"
# Root report summarizes complete two-phase acceptance; pre-reboot report remains nested.
cp "$DEST/reboot-resume/acceptance-report.json" "$DEST/acceptance-report.json"
printf '%s\n' "$SHA" >"$DEST/tested-commit.txt"
(
	cd "$DEST"
	find . -type f ! -name artifact-manifest.sha256 -print0 |
		sort -z |
		xargs -0 sha256sum >artifact-manifest.sha256
)
if [[ $SECOND_STATUS -eq 0 ]]; then
	python3 - "$DEST/reboot-resume/acceptance-report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
if report.get("git_commit_sha") != report.get("expected_git_commit_sha"):
    raise SystemExit("acceptance commit mismatch")
if report.get("status") != "passed" or report.get("not_checked"):
    raise SystemExit(f"acceptance incomplete: {report}")
PY
fi
if [[ -f "$DEST/artifact-manifest.sha256" ]]; then
	(cd "$DEST" && sha256sum -c artifact-manifest.sha256)
fi
printf '%s\n' "Retained evidence: $DEST"
if [[ $SECOND_STATUS -ne 0 ]]; then
	exit "$SECOND_STATUS"
fi
