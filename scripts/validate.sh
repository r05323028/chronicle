#!/usr/bin/env bash
# Layered Chronicle validation. Existing acceptance scripts remain gate authority.
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
HELPER="$ROOT/scripts/validation.py"
CONFIG="$ROOT/validation/groups.toml"
MODE=${1:-}
COMMAND_TIMEOUT=${CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS:-900}
GATE_TIMEOUT=${CHRONICLE_VALIDATION_GATE_TIMEOUT_SECONDS:-3600}
ACCEPTANCE_PROFILE_TIMEOUT=${CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS:-3300}
[[ $COMMAND_TIMEOUT =~ ^[1-9][0-9]*$ ]] || {
	printf '%s\n' 'CHRONICLE_VALIDATION_COMMAND_TIMEOUT_SECONDS must be a positive integer' >&2
	exit 2
}
[[ $GATE_TIMEOUT =~ ^[1-9][0-9]*$ ]] || {
	printf '%s\n' 'CHRONICLE_VALIDATION_GATE_TIMEOUT_SECONDS must be a positive integer' >&2
	exit 2
}
[[ $ACCEPTANCE_PROFILE_TIMEOUT =~ ^[1-9][0-9]*$ && $ACCEPTANCE_PROFILE_TIMEOUT -lt $GATE_TIMEOUT ]] || {
	printf '%s\n' 'CHRONICLE_ACCEPTANCE_PROFILE_TIMEOUT_SECONDS must be positive and shorter than validation gate timeout' >&2
	exit 2
}
if [[ $MODE =~ ^(fast|targeted|live-capture|recorder|release)$ && ${CHRONICLE_VALIDATION_GATE_WRAPPED:-0} != 1 ]]; then
	mkdir -p "$ROOT/target/validation-work"
	exec env CHRONICLE_VALIDATION_GATE_WRAPPED=1 CHRONICLE_TIMEOUT_LAYER=validation_gate CHRONICLE_TIMEOUT_NAME="$MODE" CHRONICLE_TIMEOUT_PHASE="$MODE" \
		CHRONICLE_TIMEOUT_EVIDENCE_FILE="$ROOT/target/validation-work/$MODE-gate-timeout-$$.json" \
		"$ROOT/scripts/run-with-timeout.sh" "$GATE_TIMEOUT" "$0" "$@"
fi
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
  ./scripts/validate.sh live-capture|recorder [--reuse-evidence] [options]
  ./scripts/validate.sh release [--reuse-evidence] [options]

Options: --changed-since REF --reuse-evidence --force --no-artifact --artifact-on-failure --keep-workdir
Set CHRONICLE_VALIDATE_DRY_RUN=1 to inspect selection without running commands.
Timeouts: command=900s, acceptance profile=3300s, validation gate=3600s.
EOF
}

[[ -n $MODE ]] || {
	usage >&2
	exit 2
}
GATE=
if [[ $MODE == live-capture || $MODE == recorder ]]; then
	GATE=$MODE
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

run_step() {
	local name=$1
	shift
	local timeout=$COMMAND_TIMEOUT layer=command status
	if [[ $name == acceptance-* ]]; then
		timeout=$GATE_TIMEOUT
		layer=acceptance_gate
	fi
	local log="$WORKDIR/$name.log"
	printf '\n== %s ==\n' "$name"
	printf '+ %q' "$@"
	printf '\n'
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'DRY RUN\n'
		return 0
	fi
	if CHRONICLE_TIMEOUT_EVIDENCE_FILE="$WORKDIR/$name-timeout.json" CHRONICLE_TIMEOUT_LAYER="$layer" CHRONICLE_TIMEOUT_NAME="$name" CHRONICLE_TIMEOUT_PHASE="$name" \
		"$ROOT/scripts/run-with-timeout.sh" "$timeout" "$@" >"$log" 2>&1; then
		cat "$log"
	else
		status=$?
		cat "$log"
		cp "$log" "$WORKDIR/failed-test.log"
		printf 'FAILED: %s (exit %s)\n' "$name" "$status" >&2
		return "$status"
	fi
}

run_shell() {
	local name=$1 command=$2 status
	local log="$WORKDIR/$name.log"
	printf '\n== %s ==\n+ %s\n' "$name" "$command"
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'DRY RUN\n'
		return 0
	fi
	if CHRONICLE_TIMEOUT_EVIDENCE_FILE="$WORKDIR/$name-timeout.json" CHRONICLE_TIMEOUT_LAYER=command CHRONICLE_TIMEOUT_NAME="$name" CHRONICLE_TIMEOUT_PHASE="$name" \
		"$ROOT/scripts/run-with-timeout.sh" "$COMMAND_TIMEOUT" bash -c "cd '$ROOT' && $command" >"$log" 2>&1; then
		cat "$log"
	else
		status=$?
		cat "$log"
		cp "$log" "$WORKDIR/failed-test.log"
		printf 'FAILED: %s (exit %s)\n' "$name" "$status" >&2
		return "$status"
	fi
}

run_gate_portable_prerequisites() {
	# Gate selects required lower-layer prerequisites as separate steps (task 8.1):
	# portable correctness runs rootlessly here and its evidence is retained
	# separately; privileged scenario bodies never rerun it.
	local status=0
	run_shell gate-fmt 'cargo fmt --all --check' || status=$?
	run_shell gate-clippy "CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}" cargo clippy --workspace --all-targets --all-features --locked -- -D warnings" || status=$?
	run_shell gate-workspace-tests "CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-target}" cargo test --workspace --all-features --locked" || status=$?
	run_shell gate-openspec 'openspec validate --all --strict --no-interactive' || status=$?
	run_shell gate-source-ownership "python3 '$HELPER' ownership --root '$ROOT' --config '$CONFIG'" || status=$?
	run_shell gate-architecture "python3 '$HELPER' architecture --root '$ROOT' --config '$ROOT/validation/architecture.toml'" || status=$?
	run_shell gate-catalog "python3 '$HELPER' catalog --root '$ROOT'" || status=$?
	run_shell gate-tooling-tests 'python3 scripts/tests/validation/test_validation_architecture.py && python3 scripts/tests/validation/test_sleep_policy.py && python3 scripts/tests/validation/test_preflight.py && python3 scripts/tests/validation/test_lifecycle_cleanup.py && python3 scripts/tests/validation/test_select_fixtures.py && python3 scripts/tests/validation/test_evidence_reuse.py && python3 scripts/tests/validation/test_layered_validation.py && python3 scripts/tests/validation/test_architecture_boundaries.py && python3 scripts/tests/validation/test_release_workflow.py' || status=$?
	return "$status"
}

run_gate() {
	local gate=$1 status=0
	# Portable prerequisites first (separately selected + evidenced), then the
	# privileged acceptance profile proves only privileged invariants.
	run_gate_portable_prerequisites || return $?
	local -a command=("$ROOT/scripts/acceptance.sh" --profile "$gate" --executor multipass --vm "$VM" --evidence-root "$EVIDENCE_ROOT/acceptance" --gate-timeout-seconds "$ACCEPTANCE_PROFILE_TIMEOUT")
	if [[ $MODE == release ]]; then
		command+=(--release)
	fi
	if [[ $REUSE_EVIDENCE != true ]]; then
		command+=(--no-reuse)
	fi
	if [[ $ARTIFACT_MODE != release ]]; then
		command+=(--compact)
	fi
	if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
		printf 'WOULD RUN unified acceptance: %q ' "${command[@]}"
		printf '\n'
		return 0
	fi
	run_step "acceptance-$gate" "${command[@]}" || status=$?
	return "$status"
}

run_fast() {
	run_shell fmt 'cargo fmt --all --check'
	run_shell clippy "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
	run_shell workspace-tests "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo test --workspace --all-features --locked"
	run_shell openspec 'openspec validate --all --strict --no-interactive'
	run_shell source-ownership "python3 '$HELPER' ownership --root '$ROOT' --config '$CONFIG'"
	run_shell architecture "python3 '$HELPER' architecture --root '$ROOT' --config '$ROOT/validation/architecture.toml'"
	run_shell catalog "python3 '$HELPER' catalog --root '$ROOT'"
	run_shell tooling-tests 'python3 scripts/tests/validation/test_validation_architecture.py && python3 scripts/tests/validation/test_sleep_policy.py && python3 scripts/tests/validation/test_preflight.py && python3 scripts/tests/validation/test_lifecycle_cleanup.py && python3 scripts/tests/validation/test_select_fixtures.py && python3 scripts/tests/validation/test_evidence_reuse.py && python3 scripts/tests/validation/test_layered_validation.py && python3 scripts/tests/validation/test_architecture_boundaries.py && python3 scripts/tests/validation/test_release_workflow.py'
	printf '\nUnit + integration: workspace test invocation. Cheap integration: catalog + tooling meta-tests above.\n'
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
	local ran_live_capture=false ran_recorder=false
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
			printf 'eBPF source changed: minimal privileged smoke selects the live-capture smoke path.\n'
			if [[ ${CHRONICLE_SKIP_PRIVILEGED_SMOKE:-false} != true ]]; then
				if [[ $DRY_RUN == true || $DRY_RUN == 1 ]]; then
					printf 'WOULD RUN privileged live-capture smoke: %q ' "$ROOT/scripts/acceptance.sh" --profile live-capture --executor multipass --vm "$VM" --evidence-root "$WORKDIR/live-capture-smoke-source" --gate-timeout-seconds "$ACCEPTANCE_PROFILE_TIMEOUT" --no-reuse --compact
					printf '\n'
				else
					"$ROOT/scripts/acceptance.sh" --profile live-capture --executor multipass --vm "$VM" \
						--evidence-root "$WORKDIR/live-capture-smoke-source" --gate-timeout-seconds "$ACCEPTANCE_PROFILE_TIMEOUT" --no-reuse --compact
				fi
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
		cli_docs)
			run_shell openspec-targeted 'openspec validate --all --strict --no-interactive'
			run_shell doc-commands 'python3 tests/smoke/test_documented_commands.py'
			run_shell installer-test 'python3 tests/smoke/test_installer.py'
			;;
		acceptance | build_tooling)
			if [[ $ran_live_capture == false ]]; then
				run_gate live-capture
				ran_live_capture=true
			fi
			if [[ $ran_recorder == false ]]; then
				run_gate recorder
				ran_recorder=true
			fi
			;;
		esac
	done <<<"$groups"
}

case $MODE in
fast) run_fast ;;
targeted) run_targeted ;;
live-capture) run_gate live-capture ;;
recorder) run_gate recorder ;;
release)
	# Release must prove portable quality before privileged gates.
	run_shell release-fmt 'cargo fmt --all --check'
	run_shell release-clippy "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo clippy --workspace --all-targets --all-features --locked -- -D warnings"
	run_shell release-tests "CARGO_TARGET_DIR=\"${CARGO_TARGET_DIR:-target}\" cargo test --workspace --all-features --locked"
	run_shell release-openspec 'openspec validate --all --strict --no-interactive'
	run_shell release-source-ownership "python3 '$HELPER' ownership --root '$ROOT' --config '$CONFIG'"
	run_shell release-architecture "python3 '$HELPER' architecture --root '$ROOT' --config '$ROOT/validation/architecture.toml'"
	release_command=("$ROOT/scripts/acceptance.sh" --profile all --executor multipass --vm "$VM" --evidence-root "$EVIDENCE_ROOT/acceptance" --gate-timeout-seconds "$ACCEPTANCE_PROFILE_TIMEOUT" --release)
	if [[ $REUSE_EVIDENCE != true ]]; then release_command+=(--no-reuse); fi
	run_step acceptance-release "${release_command[@]}"
	;;
help | -h | --help) usage ;;
*)
	usage >&2
	exit 2
	;;
esac
printf '\nValidation mode %s passed.\n' "$MODE"
