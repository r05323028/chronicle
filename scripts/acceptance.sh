#!/usr/bin/env bash
# Single user-facing Chronicle acceptance command. Runner owns gate deadline so
# it can always normalize timeout evidence and finish report/manifest writing.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 "$ROOT/scripts/acceptance/runner.py" "$@"
