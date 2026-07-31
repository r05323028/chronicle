#!/usr/bin/env bash
# Rootless checks for acceptance configuration and retained report metadata.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
SCRIPT="$ROOT/scripts/p1-privileged-acceptance.sh"
TMP_DIR=$(mktemp -d)
trap 'rm -rf -- "$TMP_DIR"' EXIT
mkdir -p "$TMP_DIR/bin" "$TMP_DIR/home"
cat >"$TMP_DIR/bin/uname" <<'EOF'
#!/usr/bin/env bash
case ${1:-} in
  -s) printf '%s\n' Darwin ;;
  -r) printf '%s\n' test-kernel ;;
  -m) printf '%s\n' test-arch ;;
  *) printf '%s\n' Darwin ;;
esac
EOF
chmod +x "$TMP_DIR/bin/uname"

run_environment_skip() {
  local mode=$1 artifact_root=$2 ebpf_target=$3 status
  set +e
  if [[ $mode == default ]]; then
    env -u CHRONICLE_ACCEPTANCE_MODE \
      PATH="$TMP_DIR/bin:$PATH" \
      HOME="$TMP_DIR/home" \
      CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$artifact_root" \
      CHRONICLE_EBPF_TARGET_DIR="$ebpf_target" \
      CARGO_TARGET_DIR="$TMP_DIR/workspace-target" \
      "$SCRIPT" >/dev/null 2>&1
  else
    PATH="$TMP_DIR/bin:$PATH" \
      HOME="$TMP_DIR/home" \
      CHRONICLE_ACCEPTANCE_MODE="$mode" \
      CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$artifact_root" \
      CHRONICLE_EBPF_TARGET_DIR="$ebpf_target" \
      CARGO_TARGET_DIR="$TMP_DIR/workspace-target" \
      "$SCRIPT" >/dev/null 2>&1
  fi
  status=$?
  set -e
  [[ $status -eq 77 ]]
}

# Default mode is full, and a stable artifact root can be safely reused.
full_root="$TMP_DIR/full"
run_environment_skip default "$full_root" "$TMP_DIR/ebpf-target"
python3 - "$full_root/acceptance-report.json" <<'PY'
import json
import sys
assert json.load(open(sys.argv[1], encoding="utf-8"))["acceptance_mode"] == "full"
PY
run_environment_skip full "$full_root" "$TMP_DIR/ebpf-target"

# Fast mode is recorded, and SHA calculation follows configured eBPF target.
ebpf_object="$TMP_DIR/ebpf-target/bpfel-unknown-none/release/chronicle-ebpf-capture"
mkdir -p "$(dirname "$ebpf_object")"
printf '%s\n' configured-ebpf-object >"$ebpf_object"
run_environment_skip fast "$TMP_DIR/fast" "$TMP_DIR/ebpf-target"
python3 - "$TMP_DIR/fast/acceptance-report.json" "$ebpf_object" <<'PY'
import hashlib
import json
import sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
object_path = sys.argv[2]
assert report["acceptance_mode"] == "fast"
assert report["ebpf_object_sha256"] == hashlib.sha256(open(object_path, "rb").read()).hexdigest()
assert report["checks"]["p1_retained_acceptance"] == "not_checked"
assert "openspec_validation" not in report["checks"]
PY

# Unsupported modes and dangerous roots fail before any privileged work.
set +e
PATH="$TMP_DIR/bin:$PATH" HOME="$TMP_DIR/home" CHRONICLE_ACCEPTANCE_MODE=debug \
  CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$TMP_DIR/invalid-mode" "$SCRIPT" >/dev/null 2>&1
status=$?
set -e
[[ $status -ne 0 ]]

set +e
PATH="$TMP_DIR/bin:$PATH" HOME="$TMP_DIR/home" CHRONICLE_ACCEPTANCE_MODE=fast \
  CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$ROOT" "$SCRIPT" >/dev/null 2>&1
status=$?
set -e
[[ $status -ne 0 ]]

set +e
PATH="$TMP_DIR/bin:$PATH" HOME="$TMP_DIR/home" CHRONICLE_ACCEPTANCE_MODE=fast \
  CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$TMP_DIR/home" "$SCRIPT" >/dev/null 2>&1
status=$?
set -e
[[ $status -ne 0 ]]

set +e
PATH="$TMP_DIR/bin:$PATH" HOME="$TMP_DIR/home" CHRONICLE_ACCEPTANCE_MODE=fast \
  CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT= "$SCRIPT" >/dev/null 2>&1
status=$?
set -e
[[ $status -ne 0 ]]

# Keep fast skips explicit and tied to retained log names.
grep -Fq 'Skipped in fast mode; covered by full privileged acceptance.' "$SCRIPT"
for log in wal-tests.log ingest-limit-tests.log replay-tests.log cgroup-tests.log signal-tests.log fmt.log check.log; do
  grep -Fq "$log" "$SCRIPT"
done
! grep -Fq 'openspec-validation.log' "$SCRIPT"
! grep -Fq 'openspec validate' "$SCRIPT"
! grep -Fq 'stale-language' "$SCRIPT"
grep -Fq 'TOTAL_PHASES=29' "$SCRIPT"
grep -Fq 'phase 29 "Cleanup processes, cgroups, and temporary files"' "$SCRIPT"
! grep -Fq 'phase 30' "$SCRIPT"
printf '%s\n' 'p1 acceptance rootless checks passed'
