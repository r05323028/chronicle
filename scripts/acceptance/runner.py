#!/usr/bin/env python3
"""One acceptance command: profiles select scenarios, executors run them."""

from __future__ import annotations

import argparse
import base64
import json
import os
import shutil
import shlex
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


def parser() -> argparse.ArgumentParser:
    value = argparse.ArgumentParser(description="Run Chronicle acceptance scenarios")
    value.add_argument("--profile", choices=("p1", "p2", "all"), required=True)
    value.add_argument("--executor", choices=("local", "multipass"), default="local")
    value.add_argument("--release", action="store_true", help="require release-grade source identity")
    value.add_argument("--no-reuse", action="store_true", help="force a fresh run")
    value.add_argument("--compact", action="store_true", help="retain compact successful evidence")
    value.add_argument("--vm", default=os.environ.get("CHRONICLE_MULTIPASS_VM", "chronicle-ubuntu"))
    value.add_argument("--evidence-root", type=Path, default=ROOT / "target/validation-evidence/acceptance")
    return value


def multipass_environment(vm: str) -> dict[str, Any] | None:
    script = r'''
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
'''
    encoded = base64.b64encode(script.encode()).decode()
    command = "sudo -E env HOME=/home/ubuntu PATH=\"$PATH\" python3 -c " + shlex.quote(f"import base64; exec(base64.b64decode('{encoded}'))")
    try:
        state = subprocess.check_output(
            ["multipass", "info", vm], text=True, stderr=subprocess.DEVNULL, timeout=30
        )
        if "State: Running" not in state:
            subprocess.run(["multipass", "start", vm], check=True, timeout=60, stdout=subprocess.DEVNULL)
        value = subprocess.check_output(
            ["multipass", "exec", vm, "--", "bash", "-lc", command],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=60,
        )
        parsed = json.loads(value)
        if isinstance(parsed, dict):
            return parsed
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired, json.JSONDecodeError):
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


def release_source_errors(source: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if source.get("working_tree_dirty"):
        errors.append("working tree is dirty")
    if not source.get("commit_sha"):
        errors.append("commit SHA is unknown")
    if not source.get("tree_sha"):
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


def run_profile(args: argparse.Namespace, profile: str, definition: dict[str, Any]) -> int:
    selected = report.profile_scenarios(definition, profile)
    fingerprint, fingerprint_payload = source_fingerprint(profile, args.executor)
    source_start = report.source_provenance(ROOT, fingerprint)
    if args.release:
        errors = release_source_errors(source_start)
        if errors:
            print(f"release source validation failed: {', '.join(errors)}", file=sys.stderr)
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
            print(f"REUSED evidence: {path}")
            print(json.dumps({"reused": True, "run_id": candidate.get("run_id"), "profile": candidate.get("profile"), "fingerprint": fingerprint}, sort_keys=True))
            return 0

    run_id = now_run_id()
    run_root = evidence_root / profile / fingerprint / run_id
    runtime_root = run_root / "runtime"
    runtime_root.mkdir(parents=True, exist_ok=False)
    assertions_root = runtime_root / "assertions"
    assertions_root.mkdir()
    (assertions_root / ".chronicle-acceptance-root").write_text("chronicle-unified-acceptance-root-v1\n", encoding="utf-8")
    (runtime_root / "fingerprint.json").write_text(
        json.dumps(fingerprint_payload, indent=2, sort_keys=True) + "\n", encoding="utf-8"
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
        }
    )
    print(f"Running profile={profile} executor={args.executor} fingerprint={fingerprint}")
    p1_return_code: int | None = None
    if args.executor == "local" and profile == "p2":
        p1_artifact_root = assertions_root / "p1"
        p2_artifact_root = assertions_root / "p2"
        p1_artifact_root.mkdir()
        p2_artifact_root.mkdir()
        (p1_artifact_root / ".chronicle-acceptance-root").write_text("chronicle-p1-acceptance-root-v1\n", encoding="utf-8")
        p1_env = env.copy()
        p1_env["CHRONICLE_ACCEPTANCE_PROFILE"] = "p1"
        p1_env["CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT"] = str(p1_artifact_root)
        p1_process = subprocess.run(
            ["bash", str(ROOT / "scripts/acceptance/lib/profile-p1.sh")],
            cwd=ROOT,
            env=p1_env,
        )
        p1_return_code = p1_process.returncode
        p2_env = env.copy()
        p2_env["CHRONICLE_ACCEPTANCE_ARTIFACT_ROOT"] = str(p2_artifact_root)
        p2_process = subprocess.run(
            ["bash", str(ROOT / "scripts/acceptance/lib/profile-p2.sh")],
            cwd=ROOT,
            env=p2_env,
        )
        if (p2_artifact_root / "acceptance-report.json").is_file():
            shutil.copy2(p2_artifact_root / "acceptance-report.json", assertions_root / "acceptance-report.json")
        return_code = p2_process.returncode if p2_process.returncode else (p1_return_code or 0)
    else:
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
        completed_process = subprocess.run(command, cwd=ROOT, env=env)
        return_code = completed_process.returncode
        if args.executor == "local" and (assertions_root / "acceptance-report.json").is_file():
            shutil.copy2(assertions_root / "acceptance-report.json", runtime_root / "acceptance-report.json")

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
    if profile == "p2":
        p1_path = assertions_root / "p1/acceptance-report.json"
        if p1_path.is_file():
            try:
                value = json.loads(p1_path.read_text(encoding="utf-8"))
                if isinstance(value, dict):
                    p1_legacy = value
            except (OSError, json.JSONDecodeError):
                p1_legacy = None
    status, completed, failed, not_checked = report.normalize_legacy_report(
        definition, profile, legacy, selected, return_code
    )
    if profile == "p2":
        p1_selected = report.profile_scenarios(definition, "p1")
        _, p1_completed, p1_failed, p1_not_checked = report.normalize_legacy_report(
            definition, "p1", p1_legacy, p1_selected, p1_return_code or 77
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
    if return_code == 77 and not failed:
        status = "not_checked"
    if return_code not in {0, 77}:
        status = "failed"
        if not failed:
            failed = list(selected)
            completed = []
            not_checked = []
    phases_path = runtime_root / "phases.json"
    if phases_path.is_file():
        try:
            phases = json.loads(phases_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            phases = []
    else:
        phases = [{"name": "run", "status": "passed" if status == "passed" else status}]
    if not isinstance(phases, list):
        phases = []

    source_end = report.source_provenance(ROOT, fingerprint)
    source = {
        **source_start,
        "start": source_start,
        "end": source_end,
        "stable": source_start == source_end,
    }
    final_environment = environment_value
    guest_environment_path = runtime_root / "guest-environment.json"
    if guest_environment_path.is_file():
        try:
            guest = json.loads(guest_environment_path.read_text(encoding="utf-8"))
            if isinstance(guest, dict):
                final_environment = {**guest, "executor": args.executor}
        except (OSError, json.JSONDecodeError):
            pass
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
        legacy=(
            {"p1": p1_legacy, "p2": legacy} if profile == "p2" else legacy
        ),
        release_requested=args.release,
        source_stable=bool(source["stable"]),
    )
    # Normalize status from legacy report into the central result.
    final_report["status"] = status
    final_report["release_eligible"] = bool(final_report["release_eligible"] and status == "passed")
    if args.compact and status == "passed":
        final_report["legacy_report"] = None
        for child in runtime_root.iterdir():
            if child.name not in {"fingerprint.json", "environment.json", "guest-environment.json", "phases.json"}:
                try:
                    if child.is_dir():
                        shutil.rmtree(child, ignore_errors=True)
                    else:
                        child.unlink(missing_ok=True)
                except OSError:
                    pass
    final_report["artifact_manifest"] = {"file": "artifact-manifest.sha256", "complete": False}
    final_report_path = run_root / "acceptance-report.json"
    report.write_report(final_report_path, final_report)
    manifest = report.write_manifest(run_root)
    final_report["artifact_manifest"] = {"file": manifest.name, "complete": True}
    report.write_report(final_report_path, final_report)
    # The report changed after its first manifest entry; refresh manifest once.
    report.write_manifest(run_root)
    print(f"Evidence: {run_root}")
    if status == "passed":
        return 0
    return 77 if status == "not_checked" else 1


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    definition = report.load_scenarios(ROOT / "scripts/acceptance/scenarios.toml")
    profiles = ("p1", "p2") if args.profile == "all" else (args.profile,)
    statuses = [run_profile(args, profile, definition) for profile in profiles]
    if any(status not in {0} for status in statuses):
        return next(status for status in statuses if status != 0)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
