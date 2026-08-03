#!/usr/bin/env bash
# Runs exact-commit privileged acceptance in a VM-local clone and retains safe evidence.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
VM=${1:-chronicle-ubuntu}
SHA=$(git -C "$ROOT" rev-parse HEAD)
[[ -z $(git -C "$ROOT" status --porcelain --untracked-files=all) ]] || {
	printf '%s\n' 'host working tree must be clean' >&2
	exit 1
}

VM_ROOT=/home/ubuntu/chronicle-acceptance
VM_ARTIFACTS=/home/ubuntu/p1-artifacts/$SHA/$(date -u +%Y%m%dT%H%M%SZ)-$$
DEST=${CHRONICLE_ACCEPTANCE_DEST:-"$ROOT/target/validation-evidence/privileged/$SHA/ubuntu-24.04"}
ACCEPTANCE_MODE=${CHRONICLE_ACCEPTANCE_MODE:-full}
COMPACT=${CHRONICLE_ACCEPTANCE_COMPACT:-0}
CARGO_TARGET_DIR_VM=${CARGO_TARGET_DIR:-/home/ubuntu/chronicle-target}
EBPF_TARGET_DIR_VM=${CHRONICLE_EBPF_TARGET_DIR:-/home/ubuntu/chronicle-ebpf-target}

ensure_vm_source() {
	local state
	state=$(multipass info "$VM" 2>/dev/null | awk '/State:/ {print $2; exit}')
	if [[ $state != Running ]]; then
		multipass start "$VM"
	fi
	if ! multipass exec "$VM" -- test -f /mnt/chronicle/scripts/acceptance/p1-privileged.sh; then
		multipass mount "$ROOT" "$VM:/mnt/chronicle"
	fi
	multipass exec "$VM" -- test -f /mnt/chronicle/scripts/acceptance/p1-privileged.sh
}

ensure_vm_source
REMOTE_STATUS=0
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
  sudo rm -rf '$VM_ROOT' '$VM_ARTIFACTS'
  sudo git clone --quiet --no-local /mnt/chronicle '$VM_ROOT'
  sudo git -C '$VM_ROOT' checkout --quiet --detach '$SHA'
  cd '$VM_ROOT'
  set +e
  sudo -E env \\
    HOME=/home/ubuntu \\
    PATH=\"\$PATH\" \\
    CHRONICLE_ACCEPTANCE_MODE='$ACCEPTANCE_MODE' \\
    CHRONICLE_ACCEPTANCE_EXPECTED_SHA='$SHA' \\
    CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$VM_ARTIFACTS' \\
    CHRONICLE_ACCEPTANCE_COMPACT='$COMPACT' \\
    CARGO_TARGET_DIR='$CARGO_TARGET_DIR_VM' \\
    CHRONICLE_EBPF_TARGET_DIR='$EBPF_TARGET_DIR_VM' \\
    ./scripts/acceptance/p1-privileged.sh
  status=\$?
  set -e
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS'
  sudo chmod -R u+rwX,go+rX '$VM_ARTIFACTS'
  if [[ '$COMPACT' == 1 ]] && grep -q '\"status\": \"passed\"' '$VM_ARTIFACTS/acceptance-report.json'; then
    find '$VM_ARTIFACTS' -mindepth 1 -maxdepth 1 ! -name acceptance-report.json -exec rm -rf -- {} +
  fi
  cd '$VM_ARTIFACTS'
  find . -type f ! -name artifact-manifest.sha256 -print0 \\
    | sort -z \\
    | xargs -0 sha256sum > artifact-manifest.sha256
  exit \$status
" || REMOTE_STATUS=$?

rm -rf "$DEST"
mkdir -p "$(dirname "$DEST")"
multipass transfer --recursive "$VM:$VM_ARTIFACTS" "$DEST"
printf '%s\n' "$SHA" >"$DEST/tested-commit.txt"
(
	cd "$DEST"
	sha256sum -c artifact-manifest.sha256
)
printf '%s\n' "Retained evidence: $DEST"
if [[ $REMOTE_STATUS -ne 0 ]]; then
	if [[ -f "$DEST/acceptance-report.json" ]] && grep -q '\"status\": \"passed\"' "$DEST/acceptance-report.json"; then
		REMOTE_STATUS=0
	else
		exit "$REMOTE_STATUS"
	fi
fi
