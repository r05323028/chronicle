#!/usr/bin/env bash
# Pre-push validation: run the existing portable CI jobs locally with act so
# normal GitHub Actions regressions are caught before pushing.
#
# The source of truth remains .github/workflows; this hook reuses the ci.yml
# "checks" job (fast layered validation + portable smoke/rootless coverage)
# and the website.yml "validate-build" job (localization, committed versions,
# type-check, and static build). It never runs release, privileged, or eBPF
# runtime validation.
#
# Env knobs:
#   CHRONICLE_PRE_PUSH_JOB              ci.yml job to run (default: checks)
#   CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS  bounded deadline per job (default: 900)
#   CHRONICLE_PRE_PUSH_IMAGE            container image for ubuntu-latest
#                                       (default: catthehacker/ubuntu:rust-latest)
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
JOB="${CHRONICLE_PRE_PUSH_JOB:-checks}"
WORKFLOW=".github/workflows/ci.yml"
WEBSITE_JOB="validate-build"
WEBSITE_WORKFLOW=".github/workflows/website.yml"
TIMEOUT_SECONDS="${CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS:-900}"
IMAGE="${CHRONICLE_PRE_PUSH_IMAGE:-catthehacker/ubuntu:rust-latest}"

# The referenced jobs must exist in the workflows that are the source of truth.
for spec in "${JOB}:${WORKFLOW}" "${WEBSITE_JOB}:${WEBSITE_WORKFLOW}"; do
    job="${spec%%:*}"
    workflow="${spec#*:}"
    if ! grep -qE "^[[:space:]]*${job}:" "$ROOT/$workflow"; then
        printf 'Chronicle pre-push validation: job %s not found in %s\n' "$job" "$workflow" >&2
        printf 'Known jobs:' >&2
        grep -oE '^  [a-z0-9_-]+:' "$ROOT/$workflow" | tr -d ' :' | tr '\n' ' ' >&2
        printf '\nPush aborted.\n' >&2
        exit 1
    fi
done

command -v act >/dev/null 2>&1 || {
    printf '%s\n' \
        "Chronicle pre-push validation requires 'act'." \
        "" \
        "act runs the existing GitHub Actions jobs ('${JOB}', '${WEBSITE_JOB}')" \
        "locally so CI regressions are caught before pushing." \
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
            "(act executes the workflow jobs in a container)." \
            "" \
            "Start Docker Desktop / colima / dockerd and retry." \
            "" \
            "Push aborted." >&2
        exit 1
    fi
else
    printf '%s\n' \
        "Chronicle pre-push validation requires 'docker' (act runs the" \
        "'${JOB}' and '${WEBSITE_JOB}' jobs in containers)." \
        "" \
        "Install Docker Desktop (macOS) or docker.io (Linux), start the" \
        "daemon, and retry." \
        "" \
        "Push aborted." >&2
    exit 1
fi

# GitHub-hosted runners preinstall Rust; act containers do not. Map
# ubuntu-latest to a Rust-capable image for local parity.
run_act_job() {
    local job="$1" workflow="$2"
    printf 'Chronicle pre-push: running GitHub Actions job %s (%s) with act (bounded %ss)...\n' \
        "$job" "$workflow" "$TIMEOUT_SECONDS"
    cd "$ROOT"
    # Shared git-cliff installer targets Linux x86_64; force amd64 for Apple Silicon act.
    "$ROOT/scripts/run-with-timeout.sh" "$TIMEOUT_SECONDS" act -j "$job" -W "$ROOT/$workflow" -P "ubuntu-latest=$IMAGE" --container-architecture linux/amd64
}

run_act_job "$JOB" "$WORKFLOW"
run_act_job "$WEBSITE_JOB" "$WEBSITE_WORKFLOW"
printf 'Chronicle pre-push: all portable validation jobs passed.\n'
