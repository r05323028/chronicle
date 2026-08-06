#!/usr/bin/env bash
# Single user-facing Chronicle acceptance command.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
exec python3 "$ROOT/scripts/acceptance/runner.py" "$@"
