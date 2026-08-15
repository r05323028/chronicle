#!/usr/bin/env bash
# Pre-push validation: run the existing portable CI checks job locally with
# act so normal GitHub Actions regressions are caught before pushing.
#
# The source of truth remains .github/workflows/ci.yml; this hook reuses the
# existing "checks" job (fast layered validation + portable smoke/rootless
# coverage). It never runs release, privileged, or eBPF runtime validation.
#
# Env knobs:
#   CHRONICLE_PRE_PUSH_JOB              job to run (default: checks)
#   CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS  bounded deadline (default: 900)
#   CHRONICLE_PRE_PUSH_IMAGE            container image for ubuntu-latest
#                                       (default: catthehacker/ubuntu:rust-latest)
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
JOB="${CHRONICLE_PRE_PUSH_JOB:-checks}"
WORKFLOW=".github/workflows/ci.yml"
TIMEOUT_SECONDS="${CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS:-900}"
IMAGE="${CHRONICLE_PRE_PUSH_IMAGE:-catthehacker/ubuntu:rust-latest}"

# The referenced job must exist in the workflow that is the source of truth.
if ! grep -qE "^[[:space:]]*${JOB}:" "$ROOT/$WORKFLOW"; then
    printf 'Chronicle pre-push validation: job %s not found in %s\n' "$JOB" "$WORKFLOW" >&2
    printf 'Known jobs:' >&2
    grep -oE '^  [a-z0-9_-]+:' "$ROOT/$WORKFLOW" | tr -d ' :' | tr '\n' ' ' >&2
    printf '\nPush aborted.\n' >&2
    exit 1
fi

command -v act >/dev/null 2>&1 || {
    printf '%s\n' \
        "Chronicle pre-push validation requires 'act'." \
        "" \
        "act runs the existing GitHub Actions '${JOB}' job locally so CI" \
        "regressions are caught before pushing." \
        "" \
        "Install: https://github.com/nektos/act" \
        "  brew install act   (macOS)" \
        "  see https://github.com/nektos/act#installation (Linux)" \
        "" \
        "Then install the hook once:" \
        "  ./scripts/install-pre-push-hook.sh" \
        "" \
        "Push aborted." >&2
    exit 1
}

if command -v docker >/dev/null 2>&1; then
    if ! docker info >/dev/null 2>&1; then
        printf '%s\n' \
            "Chronicle pre-push validation requires a running Docker daemon" \
            "(act executes the workflow job in a container)." \
            "" \
            "Start Docker Desktop / colima / dockerd and retry." \
            "" \
            "Push aborted." >&2
        exit 1
    fi
else
    printf '%s\n' \
        "Chronicle pre-push validation requires 'docker' (act runs the" \
        "'${JOB}' job in a container)." \
        "" \
        "Install Docker Desktop (macOS) or docker.io (Linux), start the" \
        "daemon, and retry." \
        "" \
        "Push aborted." >&2
    exit 1
fi

printf 'Chronicle pre-push: running GitHub Actions job %s with act (bounded %ss)...\n' \
    "$JOB" "$TIMEOUT_SECONDS"
cd "$ROOT"
# GitHub-hosted runners preinstall Rust; act containers do not. Map
# ubuntu-latest to a Rust-capable image for local parity.
exec "$ROOT/scripts/run-with-timeout.sh" "$TIMEOUT_SECONDS" act -j "$JOB" -W "$ROOT/$WORKFLOW" -P "ubuntu-latest=$IMAGE"
