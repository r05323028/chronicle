#!/usr/bin/env bash
# Rootless checks for P2 privileged runner guards and retained-report schema.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)
SCRIPT="$ROOT/scripts/acceptance/p2-privileged.sh"
MULTIPASS="$ROOT/scripts/acceptance/p2-multipass.sh"
TMP_DIR=$(mktemp -d)
trap 'rm -rf -- "$TMP_DIR"' EXIT
mkdir -p "$TMP_DIR/bin" "$TMP_DIR/home"
cat >"$TMP_DIR/bin/uname" <<'EOF'
#!/usr/bin/env bash
case ${1:-} in
  -s) printf '%s\n' Linux ;;
  -r) printf '%s\n' test-kernel ;;
  -m) printf '%s\n' test-arch ;;
  *) printf '%s\n' Linux ;;
esac
EOF
chmod +x "$TMP_DIR/bin/uname"

artifact_root="$TMP_DIR/artifacts"
set +e
PATH="$TMP_DIR/bin:$PATH" HOME="$TMP_DIR/home" \
	CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT="$artifact_root" \
	CHRONICLE_ACCEPTANCE_MODE=full \
	"$SCRIPT" >/dev/null 2>&1
status=$?
set -e
[[ $status -eq 77 ]]
python3 - "$artifact_root/acceptance-report.json" <<'PY'
import json
import sys
report = json.load(open(sys.argv[1], encoding="utf-8"))
assert report["version"] == 1
assert report["task"] == "8.4"
assert report["status"] == "not_checked"
assert report["checks"]["host_reboot_recovery"] == "not_checked"
assert report["checks"]["systemd_type_simple"] == "not_checked"
PY

for needle in \
	'CHRONICLE_ACCEPTANCE_EXPECTED_SHA' \
	'working tree must be clean' \
	'systemd-run' \
	'--property=Type=simple' \
	'cgroup v2' \
	'capture_readiness' \
	'artifact-manifest.sha256'; do
	grep -Fq -- "$needle" "$SCRIPT"
done
for needle in \
	'ensure_vm_source' \
	'multipass mount' \
	'git clone --quiet --no-local' \
	'checkout --quiet --detach' \
	'multipass restart' \
	'rev-parse HEAD'; do
	grep -Fq -- "$needle" "$MULTIPASS"
done
! grep -Fq 'openspec validate' "$SCRIPT"
! grep -Fq 'openspec validate' "$MULTIPASS"
printf '%s\n' 'p2 privileged runner rootless checks passed'
