#!/usr/bin/env bash
# Install the repository-managed pre-push hook.
#
# Preferred path: prek (the repository hook manager) installs a push shim
# from .pre-commit-config.yaml. Fallback: a symlink to the tracked
# scripts/pre-push-validation.sh. Either way the validation logic lives in
# tracked files; .git/hooks is never hand-edited.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

if command -v prek >/dev/null 2>&1; then
    prek install --hook-type pre-push
    printf 'pre-push hook installed via prek (see "prek list").\n'
    exit 0
fi

HOOK="$ROOT/.git/hooks/pre-push"
mkdir -p "$ROOT/.git/hooks"
if [[ -e "$HOOK" && ! -L "$HOOK" ]]; then
    if grep -q "pre-push-validation.sh" "$HOOK"; then
        printf 'pre-push hook already installed.\n'
        exit 0
    fi
    printf 'Refusing to overwrite existing %s\n' "$HOOK" >&2
    printf 'Install prek (https://github.com/j178/prek) or remove the hook first.\n' >&2
    exit 1
fi
ln -sfn ../../scripts/pre-push-validation.sh "$HOOK"
printf 'pre-push hook installed (symlink -> scripts/pre-push-validation.sh).\n'
