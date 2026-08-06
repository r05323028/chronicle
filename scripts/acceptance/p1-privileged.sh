#!/usr/bin/env bash
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
printf '%s\n' 'DEPRECATED: use ./scripts/acceptance.sh --profile p1 --executor local' >&2
exec "$ROOT/scripts/acceptance.sh" --profile p1 --executor local "$@"
