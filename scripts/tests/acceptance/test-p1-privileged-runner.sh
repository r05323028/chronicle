#!/usr/bin/env bash
# Compatibility test name; acceptance coverage lives in unified test_runner.py.
set -euo pipefail
ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
exec python3 "$ROOT/scripts/tests/acceptance/test_runner.py"
