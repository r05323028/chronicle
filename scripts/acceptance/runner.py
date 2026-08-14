#!/usr/bin/env python3
"""One acceptance command: profiles select scenarios, executors run them."""

from __future__ import annotations

import argparse
import base64
import json
import os
import shlex
import shutil
import subprocess
import sys
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = ROOT / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))

import validation  # noqa: E402
from acceptance import report  # noqa: E402

validation_api: Any = validation


def now_run_id() -> str:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    return f"{stamp}-{os.getpid()}-{uuid.uuid4().hex[:8]}"


def positive_seconds(value: str) -> int:
    try:
        seconds = int(value)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("timeout must be a positive integer") from exc
    if seconds <= 0:
        raise argparse.ArgumentTypeError("timeout must be a positive integer")
    return seconds


def bounded_profile_command(command: list[str], timeout: int) -> list[str]:
    cleanup_grace = os.environ.get("CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS", "180")
    command_grace = os.environ.get("CHRONICLE_TIMEOUT_GRACE_SECONDS", "5")
    return [
        "env",
        f"CHRONICLE_TIMEOUT_GRACE_SECONDS={cleanup_grace}",
        str(ROOT / "scripts/run-with-timeout.sh"),
        str(timeout),
        "env",
        f"CHRONICLE_TIMEOUT_GRACE_SECONDS={command_grace}",
        *command,
    ]


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description="Run Chronicle acceptance scenarios")
    value.add_argument("--profile", choices=("p1", "p2", "all"), required=True)
    value.add_argument("--executor", choices=("local", "multipass"), default="local")
    value.add_argument(
        "--release", action="store_true", help="require release-grade complete evidence"
    )
    value.add_argument("--no-reuse", action="store_true", help="force a fresh run")
    value.add_argument(
        "--compact", action="store_true", help="retain compact successful evidence"
    )
    value.add_argument(
        "--vm", default=os.environ.get("CHRONICLE_MULTIPASS_VM", "chronicle-ubuntu")
    )
    value.add_argument(
        "--evidence-root",
        type=Path,
        default=ROOT / "target/validation-evidence/acceptance",
    )
    value.add_argument(
        "--gate-timeout-seconds",
        type=positive_seconds,
        default=positive_seconds(
            os.environ.get("CHRONICLE_ACCEPTANCE_GATE_TIMEOUT_SECONDS", "3600")
        ),
        help="hard deadline for one complete acceptance profile",
    )
    return value


def multipass_environment(vm: str) -> dict[str, Any] | None:
    script = r"""
import json, os, platform, subprocess
from pathlib import Path
release = {}
path = Path("/etc/os-release")
if path.is_file():
    for line in path.read_text(errors="replace").splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            release[key] = value.strip('"')
caps = {}
path = Path("/proc/self/status")
if path.is_file():
    for line in path.read_text(errors="replace").splitlines():
        if line.startswith(("CapPrm:", "CapEff:", "CapBnd:")):
            key, value = line.split(":", 1)
            caps[key] = value.strip()
try:
    rustc = subprocess.check_output(["rustc", "-Vv"], text=True, stderr=subprocess.DEVNULL).strip()
except (OSError, subprocess.CalledProcessError):
    rustc = "not_checked"
print(json.dumps({"os": platform.system(), "ubuntu": release.get("VERSION_ID", "not_checked"), "architecture": platform.machine(), "kernel": platform.release(), "rustc": rustc, "target": os.environ.get("CARGO_BUILD_TARGET", "host"), "btf": Path("/sys/kernel/btf/vmlinux").is_file(), "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(), "capabilities": caps}))
"""
    encoded = base64.b64encode(script.encode()).decode()
    command = 'sudo -E env HOME=/home/ubuntu PATH="$PATH" python3 -c ' + shlex.quote(
        f"import base64; exec(base64.b64decode('{encoded}'))"
    )
    try:
        state = subprocess.check_output(
            ["multipass", "info", vm], text=True, stderr=subprocess.DEVNULL, timeout=30
        )
        if "State: Running" not in state:
            subprocess.run(
                ["multipass", "start", vm],
                check=True,
                timeout=60,
                stdout=subprocess.DEVNULL,
            )
        value = subprocess.check_output(
            ["multipass", "exec", vm, "--", "bash", "-lc", command],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=60,
        )
        parsed = json.loads(value)
        if isinstance(parsed, dict):
            return parsed
    except (
        OSError,
        subprocess.CalledProcessError,
        subprocess.TimeoutExpired,
        json.JSONDecodeError,
    ):
        return None
    return None


def collect_environment(executor: str, vm: str) -> dict[str, Any]:
    if executor == "multipass":
        value = multipass_environment(vm)
        if value is None:
            raise RuntimeError("could not collect Multipass guest environment identity")
    else:
        value = report.environment(ROOT)
    return {**value, "executor": executor}


def known_identity(value: Any) -> bool:
    return isinstance(value, str) and value not in {
        "",
        "not_checked",
        "unknown",
        "unavailable",
    }


def release_source_errors(source: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if source.get("identity_error"):
        errors.append(f"source identity unavailable: {source['identity_error']}")
    if source.get("working_tree_dirty"):
        errors.append("working tree is dirty")
    if not known_identity(source.get("commit_sha")):
        errors.append("commit SHA is unknown")
    if not known_identity(source.get("tree_sha")):
        errors.append("tree SHA is unknown")
    return errors


def source_fingerprint(profile: str, executor: str) -> tuple[str, dict[str, Any]]:
    del profile, executor
    cfg = validation_api.config(ROOT)
    value = validation_api.acceptance_fingerprint(ROOT, cfg)
    return str(value["fingerprint"]), value


def compatible_evidence(
    evidence_root: Path,
    *,
    fingerprint: str,
    required: list[str],
    environment_value: dict[str, Any],
    release: bool,
    source: dict[str, Any],
) -> tuple[Path, dict[str, Any]] | None:
    for path, candidate in report.candidate_reports(evidence_root):
        if report.report_is_compatible(
            candidate,
            fingerprint=fingerprint,
            required=required,
            environment_value=environment_value,
            release=release,
            source=source,
            root=path,
        ):
            return path, candidate
    return None


def write_evidence(root: Path, value: dict[str, Any]) -> None:
    value["artifact_manifest"] = {"file": "artifact-manifest.sha256", "complete": False}
    report_path = root / "acceptance-report.json"
    report.write_report(report_path, value)
    manifest = report.write_manifest(root)
    value["artifact_manifest"] = {"file": manifest.name, "complete": True}
    report.write_report(report_path, value)
    report.write_manifest(root)


def guest_identity(
    runtime_root: Path,
    run_id: str,
    fingerprint: str,
    release: bool,
    source: dict[str, Any] | None = None,
) -> tuple[dict[str, Any] | None, bool]:
    records: list[dict[str, Any]] = []
    for path in sorted(runtime_root.rglob("guest-source.json")):
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(value, dict):
            records.append(value)
    if not records:
        return None, False
    valid = all(
        value.get("run_id") == run_id
        and value.get("fingerprint") == fingerprint
        and known_identity(value.get("commit_sha"))
        and known_identity(value.get("tree_sha"))
        # multipass.sh serializes dirty as a bash integer (0/1).
        and isinstance(value.get("working_tree_dirty"), (bool, int))
        and (not release or not value.get("working_tree_dirty"))
        and (
            source is None
            or (
                value.get("commit_sha") == source.get("commit_sha")
                and value.get("tree_sha") == source.get("tree_sha")
                and bool(value.get("working_tree_dirty"))
                == source.get("working_tree_dirty")
            )
        )
        for value in records
    )
    return {"records": records, "final": records[-1]}, valid


def write_reuse_receipt(
    args: argparse.Namespace,
    profile: str,
    selected: list[str],
    fingerprint: str,
    source: dict[str, Any],
    environment_value: dict[str, Any],
    candidate_root: Path,
    candidate_report: dict[str, Any],
    source_stable: bool = True,
) -> int:
    receipt_id = now_run_id()
    receipt_root = args.evidence_root.resolve() / profile / fingerprint / receipt_id
    receipt_root.mkdir(parents=True, exist_ok=False)
    origin_manifest = candidate_root / "artifact-manifest.sha256"
    if not origin_manifest.is_file():
        raise RuntimeError(f"reusable evidence manifest missing: {origin_manifest}")
    origin_manifest_sha256 = report.sha256(origin_manifest)
    value = report.make_report(
        run_id=receipt_id,
        profile=profile,
        executor=args.executor,
        selected=selected,
        completed=selected,
        failed=[],
        skipped=[],
        not_checked=[],
        phases=[{"name": "reuse", "status": "passed"}],
        source={**source, "reused_from": str(candidate_root)},
        environment_value=environment_value,
        return_code=0,
        reused=True,
        reused_from=str(candidate_root),
        legacy={"source_evidence": str(candidate_root)},
        release_requested=args.release,
        source_stable=source_stable,
        reuse_requested=True,
        guest_source=candidate_report.get("guest_source"),
        guest_source_ok=(
            args.executor != "multipass" or bool(candidate_report.get("guest_source"))
        ),
    )
    value["status"] = "passed"
    value["release_eligible"] = bool(args.release and source_stable)
    value["reuse"]["origin"] = {
        "root": str(candidate_root),
        "manifest": origin_manifest.name,
        "manifest_sha256": origin_manifest_sha256,
    }
    write_evidence(receipt_root, value)
    print(f"REUSED evidence: {candidate_root}")
    print(f"Reuse receipt: {receipt_root}")
    return 0


def run_profile(
    args: argparse.Namespace, profile: str, definition: dict[str, Any]
) -> int:
    selected = report.profile_scenarios(definition, profile)
    fingerprint, fingerprint_payload = source_fingerprint(profile, args.executor)
    source_start = report.source_provenance(ROOT, fingerprint)
    if args.release:
        errors = release_source_errors(source_start)
        if errors:
            print(
                f"release source validation failed: {', '.join(errors)}",
                file=sys.stderr,
            )
            return 2
    try:
        environment_value = collect_environment(args.executor, args.vm)
    except RuntimeError as exc:
        print(str(exc), file=sys.stderr)
        return 2
    evidence_root = args.evidence_root.resolve()
    if not args.no_reuse:
        reused = compatible_evidence(
            evidence_root,
            fingerprint=fingerprint,
            required=selected,
            environment_value=environment_value,
            release=args.release,
            source=source_start,
        )
        if reused:
            path, candidate = reused
            end_fingerprint, _ = source_fingerprint(profile, args.executor)
            source_end = report.source_provenance(ROOT, end_fingerprint)
            source_stable = (
                fingerprint == end_fingerprint and source_start == source_end
            )
            if not source_stable:
                print(
                    "source changed while validating reusable evidence", file=sys.stderr
                )
                return 2
            return write_reuse_receipt(
                args,
                profile,
                selected,
                fingerprint,
                {
                    **source_start,
                    "start": source_start,
                    "end": source_end,
                    "stable": source_stable,
                },
                environment_value,
                path,
                candidate,
                source_stable,
            )

    run_id = now_run_id()
    run_root = evidence_root / profile / fingerprint / run_id
    runtime_root = run_root / "runtime"
    runtime_root.mkdir(parents=True, exist_ok=False)
    assertions_root = runtime_root / "assertions"
    assertions_root.mkdir()
    (assertions_root / ".chronicle-acceptance-root").write_text(
        "chronicle-unified-acceptance-root-v1\n", encoding="utf-8"
    )
    (runtime_root / "fingerprint.json").write_text(
        json.dumps(fingerprint_payload, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (runtime_root / "environment.json").write_text(
        json.dumps(environment_value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    env = os.environ.copy()
    env.update(
        {
            "CHRONICLE_ACCEPTANCE_PROFILE": profile,
            "CHRONICLE_ACCEPTANCE_EXECUTOR": args.executor,
            "CHRONICLE_ACCEPTANCE_RUN_ID": run_id,
            "CHRONICLE_ACCEPTANCE_SOURCE_FINGERPRINT": fingerprint,
            "CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT": str(assertions_root),
            "CHRONICLE_ACCEPTANCE_RELEASE": "1" if args.release else "0",
            "CHRONICLE_ACCEPTANCE_EXPECTED_SHA": source_start.get("commit_sha") or "",
            "CHRONICLE_ACCEPTANCE_MODE": "full",
            "CHRONICLE_ACCEPTANCE_SCENARIOS": ",".join(
                report.dispatch_scenarios(definition, profile)
            ),
            "CHRONICLE_ACCEPTANCE_P1_SCENARIOS": ",".join(
                report.dispatch_scenarios(definition, "p1")
            ),
            "CHRONICLE_TIMEOUT_EVIDENCE_FILE": str(runtime_root / "gate-timeout.json"),
            "CHRONICLE_TIMEOUT_PHASE_FILE": str(assertions_root / "current-phase.txt"),
            "CHRONICLE_TIMEOUT_DIAGNOSTICS": ":".join(
                str(Path("assertions") / name)
                for name in (
                    "recorder-service-status.txt",
                    "recorder-journal.log",
                    "process-list.txt",
                    "disk-space.txt",
                    "readiness-transitions.log",
                )
            ),
            "CHRONICLE_TIMEOUT_LAYER": "acceptance_gate",
            "CHRONICLE_ACCEPTANCE_GATE_TIMEOUT_SECONDS": str(args.gate_timeout_seconds),
            "CHRONICLE_TIMEOUT_NAME": profile,
            "CHRONICLE_ACCEPTANCE_GATE_WRAPPED": "1",
        }
    )
    print(
        f"Running profile={profile} executor={args.executor} fingerprint={fingerprint}"
    )
    if args.executor == "local":
        command = ["bash", str(ROOT / f"scripts/acceptance/lib/profile-{profile}.sh")]
    else:
        command = [
            "bash",
            str(ROOT / "scripts/acceptance/lib/multipass.sh"),
            profile,
            args.vm,
            str(runtime_root),
            "1" if args.release else "0",
        ]
    completed_process = subprocess.run(
        bounded_profile_command(command, args.gate_timeout_seconds), cwd=ROOT, env=env
    )
    return_code = completed_process.returncode
    if (
        args.executor == "local"
        and (assertions_root / "acceptance-report.json").is_file()
    ):
        shutil.copy2(
            assertions_root / "acceptance-report.json",
            runtime_root / "acceptance-report.json",
        )

    legacy_path = runtime_root / "acceptance-report.json"
    legacy: dict[str, Any] | None = None
    if legacy_path.is_file():
        try:
            value = json.loads(legacy_path.read_text(encoding="utf-8"))
            if isinstance(value, dict):
                legacy = value
        except (OSError, json.JSONDecodeError):
            legacy = None
    p1_legacy: dict[str, Any] | None = None
    if profile == "p2" and legacy:
        p1_legacy = {"status": legacy.get("status"), "checks": legacy.get("checks", {})}
    status, completed, failed, not_checked = report.normalize_legacy_report(
        definition, profile, legacy, selected, return_code
    )
    if profile == "p2":
        p1_selected = report.profile_scenarios(definition, "p1")
        _, p1_completed, p1_failed, p1_not_checked = report.normalize_legacy_report(
            definition, "p1", p1_legacy, p1_selected, return_code
        )
        for scenario in p1_selected:
            if scenario in p1_completed:
                continue
            if scenario in completed:
                completed.remove(scenario)
            if scenario in p1_failed:
                if scenario not in failed:
                    failed.append(scenario)
            elif scenario not in not_checked:
                not_checked.append(scenario)
    legacy_status = status
    scenario_state_error = False
    try:
        scenario_state = report.load_scenario_state(runtime_root, selected, profile)
    except ValueError:
        scenario_state = None
        scenario_state_error = True
    if scenario_state is not None:
        completed, failed, not_checked = scenario_state
        status = (
            "passed"
            if len(completed) == len(selected) and not failed and not not_checked
            else "failed"
            if failed
            else "not_checked"
        )
        if legacy_status == "failed":
            status = "failed"
    timeouts = report.timeout_records(runtime_root)
    preflight_result = None
    preflight_paths = sorted(assertions_root.rglob("preflight.json"))
    if preflight_paths:
        # P2 writes one preflight.json per phase (pre-reboot, post-reboot);
        # the last phase is the binding environment for the completed run.
        preflight_path = preflight_paths[-1]
        try:
            preflight_value = json.loads(preflight_path.read_text(encoding="utf-8"))
            if isinstance(preflight_value, dict):
                preflight_result = preflight_value
        except (OSError, json.JSONDecodeError):
            preflight_result = {"error": "invalid preflight.json"}
    if return_code == 77 and not failed:
        status = "not_checked"
    if return_code not in {0, 77} or timeouts or scenario_state_error:
        status = "failed"
    phases_path = runtime_root / "phases.json"
    phase_evidence_error = False
    if phases_path.is_file():
        try:
            phases = json.loads(phases_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            phases = []
            phase_evidence_error = True
    else:
        phases = [{"name": "run", "status": "passed" if status == "passed" else status}]
    if (
        not isinstance(phases, list)
        or not phases
        or any(
            not isinstance(phase, dict)
            or not phase.get("name")
            or phase.get("status") not in {"passed", "failed", "not_checked"}
            for phase in phases
        )
    ):
        phase_evidence_error = True
        phases = []
    if phase_evidence_error:
        status = "failed"

    end_fingerprint, _ = source_fingerprint(profile, args.executor)
    source_end = report.source_provenance(ROOT, end_fingerprint)
    source_stable = fingerprint == end_fingerprint and source_start == source_end
    source = {
        **source_start,
        "start": source_start,
        "end": source_end,
        "stable": source_stable,
    }
    if args.release and not source_stable:
        status = "failed"
    final_environment = environment_value
    guest_environment_paths = sorted(runtime_root.rglob("guest-environment.json"))
    if guest_environment_paths:
        try:
            guest = json.loads(guest_environment_paths[-1].read_text(encoding="utf-8"))
            if isinstance(guest, dict):
                final_environment = {**guest, "executor": args.executor}
        except (OSError, json.JSONDecodeError):
            pass
    guest_source, guest_source_ok = guest_identity(
        runtime_root, run_id, fingerprint, args.release, source_start
    )
    if args.executor == "multipass" and not guest_source_ok and status == "passed":
        status = "failed"
    final_report = report.make_report(
        run_id=run_id,
        profile=profile,
        executor=args.executor,
        selected=selected,
        completed=completed,
        failed=failed,
        skipped=[],
        not_checked=not_checked,
        phases=phases,
        source=source,
        environment_value=final_environment,
        return_code=return_code if status != "passed" else 0,
        legacy=({"p1": p1_legacy, "p2": legacy} if profile == "p2" else legacy),
        release_requested=args.release,
        source_stable=source_stable,
        reuse_requested=not args.no_reuse,
        guest_source=guest_source,
        guest_source_ok=guest_source_ok if args.executor == "multipass" else True,
        timeouts=timeouts,
        preflight=preflight_result,
    )
    # Normalize status from legacy report into the central result.
    final_report["status"] = status
    final_report["release_eligible"] = bool(
        final_report["release_eligible"] and status == "passed"
    )
    if args.compact and status == "passed":
        final_report["legacy_report"] = None
        for child in runtime_root.iterdir():
            if child.name not in {
                "fingerprint.json",
                "environment.json",
                "guest-environment.json",
                "phases.json",
                "reboot-boot-ids.json",
            }:
                try:
                    if child.is_dir():
                        shutil.rmtree(child, ignore_errors=True)
                    else:
                        child.unlink(missing_ok=True)
                except OSError:
                    pass
    write_evidence(run_root, final_report)
    print(f"Evidence: {run_root}")
    if status == "passed":
        return 0
    if timeouts:
        return 124
    return 77 if status == "not_checked" else 1


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    definition = report.load_scenarios(ROOT / "scripts/acceptance/scenarios.toml")
    profiles = ("p2",) if args.profile == "all" else (args.profile,)
    if args.profile == "all":
        print("Profile all: executing P2 superset once; P1 scenarios are included.")
    statuses = [run_profile(args, profile, definition) for profile in profiles]
    if any(status not in {0} for status in statuses):
        return next(status for status in statuses if status != 0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
