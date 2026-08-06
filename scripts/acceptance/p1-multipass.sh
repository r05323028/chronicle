#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
printf '%s\n' 'DEPRECATED: use ./scripts/acceptance.sh --profile p1 --executor multipass' >&2
ARGS=(--profile p1 --executor multipass)
if [[ $# -gt 0 ]]; then ARGS+=(--vm "$1"); shift; fi
exec "$ROOT/scripts/acceptance.sh" "${ARGS[@]}" "$@"
