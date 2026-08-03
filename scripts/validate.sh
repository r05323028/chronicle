#!/usr/bin/env bash
# Layered Chronicle validation. Existing acceptance scripts remain gate authority.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HELPER="$ROOT/scripts/validation.py"
CONFIG="$ROOT/validation/groups.toml"
MODE=${1:-}
shift || true
VM=${CHRONICLE_MULTIPASS_VM:-chronicle-ubuntu}
CHANGED_SINCE=
REUSE_EVIDENCE=false
KEEP_WORKDIR=false
NO_ARTIFACT=false
DRY_RUN=${CHRONICLE_VALIDATE_DRY_RUN:-false}
RUN_ID=$(date -u +%Y%m%dT%H%M%SZ)-$$
WORKDIR="$ROOT/target/validation-work/$RUN_ID"
EVIDENCE_ROOT=${CHRONICLE_VALIDATION_EVIDENCE_ROOT:-"$ROOT/target/validation-evidence"}
ARTIFACT_MODE=artifact-on-failure

usage() {
	cat <<'EOF'
Usage:
  ./scripts/validate.sh fast [--no-artifact] [--keep-workdir]
  ./scripts/validate.sh targeted --changed-since <git-ref> [options]
  ./scripts/validate.sh gate p1|p2 [--reuse-evidence] [options]
  ./scripts/validate.sh release [--reuse-evidence] [options]

Options: --changed-since REF --reuse-evidence --force --no-artifact --artifact-on-failure --keep-workdir
Set CHRONICLE_VALIDATE_DRY_RUN=1 to inspect selection without running commands.
EOF
}

[[ -n $MODE ]] || {
	usage >&2
	exit 2
}
GATE=
if [[ $MODE == gate ]]; then
	GATE=${1:-}
	shift || true
	[[ $GATE == p1 || $GATE == p2 ]] || {
		usage >&2
		exit 2
	}
fi

while (($#)); do
	case $1 in
	--changed-since)
		CHANGED_SINCE=${2:?missing git ref}
		shift 2
		;;
	--reuse-evidence)
		REUSE_EVIDENCE=true
		shift
		;;
	--force)
		REUSE_EVIDENCE=false
		shift
		;;
	--no-artifact)
		NO_ARTIFACT=true
		ARTIFACT_MODE=no-artifact
		shift
		;;
	--artifact-on-failure)
		ARTIFACT_MODE=artifact-on-failure
		shift
		;;
	--keep-workdir)
		KEEP_WORKDIR=true
		shift
		;;
	--vm)
		VM=${2:?missing VM name}
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		printf 'unknown option: %s\n' "$1" >&2
		usage >&2
		exit 2
		;;
	esac
done

if [[ $MODE == release ]]; then
	if [[ $NO_ARTIFACT == true ]]; then
		ARTIFACT_MODE=no-artifact
	else
		ARTIFACT_MODE=release
	fi
fi
mkdir -p "$WORKDIR"

cleanup() {
	local status=$?
	set +e
	if [[ $status -ne 0 && $NO_ARTIFACT == false ]]; then
		dmesg 2>/dev/null >"$WORKDIR/kernel-log.txt" || true
		python3 "$HELPER" compact --source "$WORKDIR" --dest "$EVIDENCE_ROOT/$MODE/$RUN_ID" \
			--gate "${GATE:-local}" --status failed --fingerprint not_applicable \
			--commit "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)" \
			--checks "$MODE" --artifact-mode "$ARTIFACT_MODE" >/dev/null 2>&1 || true
	fi
	if [[ $status -eq 0 && $KEEP_WORKDIR == false ]]; then
		rm -rf "$WORKDIR"
	fi
	exit "$status"
}
trap cleanup EXIT

ensure_vm_mount() {
	local state
	command -v multipass >/dev/null 2>&1 || {
		printf '%s\n' 'multipass is required for privileged validation' >&2
		return 1
	}
	state=$(multipass info "$VM" 2>/dev/null | awk '/State:/ {print $2; exit}')
	if [[ $state != Running ]]; then
		multipass start "$VM"
	fi
	if ! multipass exec "$VM" -- test -f /mnt/chronicle/scripts/validation.py; then
		multipass mount "$ROOT" "$VM:/mnt/chronicle"
	fi
	multipass exec "$VM" -- test -f /mnt/chronicle/scripts/validation.py
}

vm_environment() {
	ensure_vm_mount
	local value
	value=$(multipass exec "$VM" -- bash -lc \
		'cd /mnt/chronicle && sudo -E env HOME=/home/ubuntu PATH="$PATH" python3 scripts/validation.py environment --root /mnt/chronicle')
	printf '%s' "$value" | python3 -c '
import json
import sys
value = json.load(sys.stdin)
capabilities = value.get("capabilities", {})
if not any(int(raw, 16) for raw in capabilities.values() if raw):
    raise SystemExit("privileged VM environment has no effective capabilities")
print(json.dumps(value, indent=2, sort_keys=True))
'
}

checks_for() {
	python3 - "$CONFIG" "$1" <<'PY'
import sys
import tomllib
from pathlib import Path
config = tomllib.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
print(",".join(config["gates"][sys.argv[2]].get("checks", [])))
PY
}

run_step() {
	local name=$1
	shift
	local log="$WORKDIR/$name.log"
	printf '\n== %s ==\n' "$name"
	printf '+ %q' "$@"
	printf '\n'
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'DRY RUN\n'
		return 0
	fi
	if "$@" >"$log" 2>&1; then
		cat "$log"
	else
		cat "$log"
		cp "$log" "$WORKDIR/failed-test.log"
		printf 'FAILED: %s\n' "$name" >&2
		return 1
	fi
}

run_shell() {
	local name=$1 command=$2
	local log="$WORKDIR/$name.log"
	printf '\n== %s ==\n+ %s\n' "$name" "$command"
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'DRY RUN\n'
		return 0
	fi
	if bash -lc "cd '$ROOT' && $command" >"$log" 2>&1; then
		cat "$log"
	else
		cat "$log"
		cp "$log" "$WORKDIR/failed-test.log"
		printf 'FAILED: %s\n' "$name" >&2
		return 1
	fi
}

fingerprint_for() {
	local gate=$1
	local environment_file=$2
	local out="$WORKDIR/$gate-fingerprint.json"
	python3 "$HELPER" fingerprint --root "$ROOT" --gate "$gate" --config "$CONFIG" \
		--environment "$environment_file" >"$out"
	python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["fingerprint"])' "$out"
}

run_gate() {
	local gate=$1 fp source dest status environment_file checks
	status=0
	environment_file="$WORKDIR/$gate-environment.json"
	checks=$(checks_for "$gate")
	vm_environment >"$environment_file"
	fp=$(fingerprint_for "$gate" "$environment_file")
	printf '\nGate %s fingerprint: %s\n' "$gate" "$fp"
	if [[ $REUSE_EVIDENCE == true && $DRY_RUN != true && $DRY_RUN != 1 ]]; then
		if reused=$(python3 "$HELPER" reuse --evidence-root "$EVIDENCE_ROOT" --gate "$gate" --fingerprint "$fp" --checks "$checks" --commit "$(git -C "$ROOT" rev-parse HEAD)" 2>/dev/null); then
			printf 'REUSED evidence: %s\n' "$reused"
			printf '%s reused\n' "$gate" >>"$WORKDIR/reused.txt"
			return 0
		fi
		printf 'Evidence not reusable: fingerprint or manifest mismatch.\n'
	fi
	source="$WORKDIR/$gate-source"
	dest="$EVIDENCE_ROOT/$gate"
	mkdir -p "$source"
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'WOULD RUN complete %s gate via existing Multipass acceptance.\n' "$gate"
		return 0
	fi
	local compact=1
	if [[ $gate == p1 ]]; then
		CHRONICLE_ACCEPTANCE_DEST="$source" CHRONICLE_ACCEPTANCE_COMPACT="$compact" \
			CHRONICLE_ACCEPTANCE_MODE=full CARGO_TARGET_DIR=/home/ubuntu/chronicle-target \
			CHRONICLE_EBPF_TARGET_DIR=/home/ubuntu/chronicle-ebpf-target \
			"$ROOT/scripts/acceptance/p1-multipass.sh" "$VM" || status=$?
	else
		CHRONICLE_ACCEPTANCE_DEST="$source" CHRONICLE_ACCEPTANCE_COMPACT="$compact" \
			CARGO_TARGET_DIR=/home/ubuntu/chronicle-target \
			CHRONICLE_EBPF_TARGET_DIR=/home/ubuntu/chronicle-ebpf-target \
			"$ROOT/scripts/acceptance/p2-multipass.sh" "$VM" || status=$?
	fi
	if [[ $status -eq 0 ]]; then
		python3 "$HELPER" compact --source "$source" --dest "$dest" --gate "$gate" --status passed \
			--fingerprint "$fp" --commit "$(git -C "$ROOT" rev-parse HEAD)" \
			--checks "$checks" --environment "$environment_file" --artifact-mode "$ARTIFACT_MODE"
	else
		python3 "$HELPER" compact --source "$source" --dest "$dest" --gate "$gate" --status failed \
			--fingerprint "$fp" --commit "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || printf unknown)" \
			--checks "$checks" --environment "$environment_file" --artifact-mode "$ARTIFACT_MODE" || true
		return "$status"
	fi
}

run_fast() {
	run_shell fmt 'cargo fmt --all --check'
	run_shell clippy "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
	run_shell workspace-tests "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo test --workspace --all-features --locked"
	run_shell openspec 'openspec validate --all --strict --no-interactive'
	printf '\nChanged-crate tests: covered by workspace unit-test invocation.\n'
}

print_selection() {
	local selection=$1
	python3 - "$selection" <<'PY'
import json, sys
value = json.load(open(sys.argv[1], encoding="utf-8"))
print("Changed paths")
for path in value["changed_paths"]:
    print(f"  {path}")
print("Selected validation groups")
for name in value["selected"]:
    print(f"  {name}: {value['decisions'][name]['reason']}")
print("Skipped validation groups")
for name, decision in value["decisions"].items():
    if not decision["selected"]:
        print(f"  {name}: {decision['reason']}")
PY
}

run_targeted() {
	local selection="$WORKDIR/selection.json"
	if [[ -n $CHANGED_SINCE ]]; then
		python3 "$HELPER" select --root "$ROOT" --changed-since "$CHANGED_SINCE" --config "$CONFIG" >"$selection"
	else
		python3 "$HELPER" select --root "$ROOT" --config "$CONFIG" >"$selection"
	fi
	print_selection "$selection"
	local groups group package
	local ran_p1=false ran_p2=false
	groups=$(python3 -c 'import json,sys; print("\n".join(json.load(open(sys.argv[1]))["selected"]))' "$selection")
	while IFS= read -r group; do
		[[ -n $group ]] || continue
		case $group in
		portable)
			packages=$(
				python3 - "$selection" <<'PY'
import json, pathlib, sys
paths=json.load(open(sys.argv[1]))["changed_paths"]
seen=[]
for path in paths:
    parts=path.split("/")
    if len(parts)>1 and parts[0]=="crates":
        manifest=pathlib.Path("crates")/parts[1]/"Cargo.toml"
        if manifest.is_file() and parts[1] not in seen: seen.append(parts[1])
print(" ".join(seen))
PY
			)
			if [[ -n $packages ]]; then
				for package in $packages; do run_shell "test-$package" "cargo test -p '$package' --locked"; done
			else run_shell portable-check 'cargo check --workspace --all-targets --locked'; fi
			;;
		ebpf)
			run_shell ebpf-build 'cargo +nightly build -Z build-std=core --manifest-path ebpf/Cargo.toml --target bpfel-unknown-none --release --locked'
			printf 'eBPF source changed: minimal privileged smoke selects P1 smoke path.\n'
			if [[ ${CHRONICLE_SKIP_PRIVILEGED_SMOKE:-false} != true ]]; then
				CHRONICLE_ACCEPTANCE_MODE=smoke CHRONICLE_ACCEPTANCE_DEST="$WORKDIR/p1-smoke-source" \
					CHRONICLE_ACCEPTANCE_COMPACT=1 "$ROOT/scripts/acceptance/p1-multipass.sh" "$VM"
			fi
			;;
		wal)
			run_shell wal-focused 'cargo test -p chronicle-wal --locked'
			run_shell quota-focused 'cargo test -p chronicle-application --locked recorder_quota'
			;;
		etl)
			run_shell etl-focused 'cargo test -p chronicle-etl --locked'
			run_shell checkpoint-focused 'cargo test -p chronicle-application --locked checkpoint'
			;;
		replay) run_shell replay-focused 'cargo test -p chronicle-replay --locked' ;;
		cli_docs) run_shell openspec-targeted 'openspec validate --all --strict --no-interactive' ;;
		acceptance | build_tooling)
			if [[ $ran_p1 == false ]]; then
				run_gate p1
				ran_p1=true
			fi
			if [[ $ran_p2 == false ]]; then
				run_gate p2
				ran_p2=true
			fi
			;;
		esac
	done <<<"$groups"
}

case $MODE in
fast) run_fast ;;
targeted) run_targeted ;;
gate) run_gate "$GATE" ;;
release)
	# Release must prove portable quality before privileged gates.
	run_shell release-fmt 'cargo fmt --all --check'
	run_shell release-clippy "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
	run_shell release-tests "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo test --workspace --all-features --locked"
	run_shell release-openspec 'openspec validate --all --strict --no-interactive'
	run_gate p1
	run_gate p2
	;;
help | -h | --help) usage ;;
*)
	usage >&2
	exit 2
	;;
esac
printf '\nValidation mode %s passed.\n' "$MODE"
