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
VM_ARTIFACTS=/home/ubuntu/p1-artifacts/$SHA
DEST="$ROOT/evidence/privileged/$SHA/ubuntu-24.04"

multipass exec "$VM" -- bash -lc "
  set -euo pipefail
  sudo rm -rf '$VM_ROOT' '$VM_ARTIFACTS'
  git clone --quiet --no-local /mnt/chronicle '$VM_ROOT'
  git -C '$VM_ROOT' checkout --quiet --detach '$SHA'
  cd '$VM_ROOT'
  sudo -E env \
    HOME=/home/ubuntu \
    PATH=\"\$PATH\" \
    CHRONICLE_ACCEPTANCE_MODE=full \
    CHRONICLE_ACCEPTANCE_EXPECTED_SHA='$SHA' \
    CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT='$VM_ARTIFACTS' \
    ./scripts/acceptance/p1-privileged.sh
  sudo chown -R ubuntu:ubuntu '$VM_ARTIFACTS'
  cd '$VM_ARTIFACTS'
  find . -type f ! -name artifact-manifest.sha256 -print0 \
    | sort -z \
    | xargs -0 sha256sum > artifact-manifest.sha256
"

mkdir -p "$DEST"
multipass transfer --recursive "$VM:$VM_ARTIFACTS" "$DEST/"
printf '%s\n' "$SHA" >"$DEST/tested-commit.txt"
(
  cd "$DEST"
  sha256sum -c artifact-manifest.sha256
)
printf '%s\n' "Retained evidence: $DEST"
