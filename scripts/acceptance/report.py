"""Unified acceptance evidence, provenance, and compatibility helpers."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import subprocess
import tomllib
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

SCHEMA_VERSION = 3
FINGERPRINT_VERSION = 2
COMPATIBILITY_VERSION = 2


def _known_identity(value: Any) -> bool:
    return isinstance(value, str) and value not in {
        "",
        "not_checked",
        "unknown",
        "unavailable",
    }


def load_scenarios(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        value = tomllib.load(handle)
    profiles = value.get("profiles", {})
    p1 = set(profiles.get("p1", {}).get("scenarios", []))
    p2 = set(profiles.get("p2", {}).get("scenarios", []))
    if not p1.issubset(p2):
        raise ValueError("p2 profile must include every p1 scenario")
    scenario_definitions = value.get("scenarios", {})
    if any(
        not isinstance(item.get("timeout_seconds"), int) or item["timeout_seconds"] <= 0
        for item in scenario_definitions.values()
    ):
        raise ValueError("every scenario timeout_seconds must be a positive integer")
    for profile in ("p1", "p2"):
        selected = list(profiles.get(profile, {}).get("scenarios", []))
        if any(scenario not in scenario_definitions for scenario in selected):
            raise ValueError(f"{profile} selects undefined scenario")
        ordered = list(profiles.get(profile, {}).get("execution_order", selected))
        if set(ordered) != set(selected) or len(ordered) != len(selected):
            raise ValueError(
                f"{profile} execution_order must contain each selected scenario exactly once"
            )
        for scenario in selected:
            scenario_root = path.parent / "lib" / "scenarios"
            shared = scenario_root / "shared" / f"{scenario}.sh"
            profile_impl = scenario_root / "extensions" / profile / f"{scenario}.sh"
            profile_only = scenario_root / profile / f"{scenario}.sh"
            if shared.is_file():
                implementation = profile_impl
            else:
                implementation = profile_only
            if not implementation.is_file():
                raise ValueError(f"missing central implementation: {implementation}")
    return value


def profile_scenarios(definition: dict[str, Any], profile: str) -> list[str]:
    if profile not in {"p1", "p2"}:
        raise ValueError(f"unknown evidence profile: {profile}")
    return list(definition["profiles"][profile]["scenarios"])


def dispatch_scenarios(definition: dict[str, Any], profile: str) -> list[str]:
    """Return centrally ordered scenario IDs, validating configured coverage."""
    selected = profile_scenarios(definition, profile)
    ordered = list(definition["profiles"][profile].get("execution_order", selected))
    if set(ordered) != set(selected) or len(ordered) != len(selected):
        raise ValueError(
            f"{profile} execution_order must contain each selected scenario exactly once"
        )
    return ordered


def _git_value(root: Path, *args: str) -> tuple[str | None, bool]:
    try:
        value = subprocess.check_output(
            ["git", "-C", str(root), *args],
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=30,
        ).strip()
    except (OSError, subprocess.SubprocessError):
        return None, True
    return value or None, False


def source_provenance(root: Path, fingerprint: str) -> dict[str, Any]:
    commit, commit_error = _git_value(root, "rev-parse", "HEAD")
    tree, tree_error = _git_value(root, "rev-parse", "HEAD^{tree}")
    dirty, status_error = _git_value(
        root, "status", "--porcelain", "--untracked-files=all"
    )
    identity_errors = [
        name
        for name, failed in (
            ("commit", commit_error),
            ("tree", tree_error),
            ("status", status_error),
        )
        if failed
    ]
    return {
        "commit_sha": commit,
        "tree_sha": tree,
        "fingerprint": fingerprint,
        "working_tree_dirty": bool(dirty) if not status_error else None,
        "identity_error": ",".join(identity_errors) or None,
    }


def environment(root: Path) -> dict[str, Any]:
    """Collect stable compatibility identity without making it a source key."""
    os_release: dict[str, str] = {}
    release = Path("/etc/os-release")
    if release.is_file():
        for line in release.read_text(encoding="utf-8", errors="replace").splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                os_release[key] = value.strip('"')
    capabilities: dict[str, str] = {}
    status = Path("/proc/self/status")
    if status.is_file():
        for line in status.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith(("CapPrm:", "CapEff:", "CapBnd:")):
                key, value = line.split(":", 1)
                capabilities[key] = value.strip()
    try:
        rustc = subprocess.check_output(
            ["rustc", "-Vv"],
            cwd=root,
            text=True,
            stderr=subprocess.DEVNULL,
            timeout=30,
        ).strip()
    except (OSError, subprocess.SubprocessError):
        rustc = "not_checked"
    return {
        "os": platform.system(),
        "ubuntu": os_release.get("VERSION_ID", "not_checked"),
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "rustc": rustc,
        "target": os.environ.get("CARGO_BUILD_TARGET", "host"),
        "btf": Path("/sys/kernel/btf/vmlinux").is_file(),
        "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(),
        "capabilities": capabilities,
    }


def environment_projection(value: dict[str, Any]) -> dict[str, Any]:
    return {
        key: value.get(key)
        for key in (
            "executor",
            "os",
            "ubuntu",
            "architecture",
            "kernel",
            "rustc",
            "target",
            "btf",
            "cgroup_v2",
            "capabilities",
        )
    }


def environments_compatible(left: dict[str, Any], right: dict[str, Any]) -> bool:
    return (
        bool(left)
        and bool(right)
        and environment_projection(left) == environment_projection(right)
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def write_manifest(root: Path) -> Path:
    manifest = root / "artifact-manifest.sha256"
    entries: list[str] = []
    for path in sorted(
        path
        for path in root.rglob("*")
        if path.is_file() and path.name != "artifact-manifest.sha256"
    ):
        entries.append(f"{sha256(path)}  {path.relative_to(root)}")
    manifest.write_text(
        "\n".join(entries) + ("\n" if entries else ""), encoding="utf-8"
    )
    return manifest


def verify_manifest(root: Path) -> bool:
    manifest = root / "artifact-manifest.sha256"
    if not manifest.is_file():
        return False
    try:
        lines = manifest.read_text(encoding="utf-8").splitlines()
        listed: set[Path] = set()
        for line in lines:
            expected, relative = line.split("  ", 1)
            path = Path(relative)
            if path.is_absolute() or ".." in path.parts or path in listed:
                return False
            candidate = (root / path).resolve()
            candidate.relative_to(root.resolve())
            if not candidate.is_file() or sha256(candidate) != expected:
                return False
            listed.add(path)
        actual = {
            path.relative_to(root)
            for path in root.rglob("*")
            if path.is_file() and path.name != "artifact-manifest.sha256"
        }
        return listed == actual
    except (OSError, ValueError):
        return False


def _legacy_checks(profile: str, legacy: dict[str, Any]) -> dict[str, str]:
    checks = legacy.get("checks", {})
    if profile == "p1":
        return {key: value for key, value in checks.items() if isinstance(value, str)}
    return {key: value for key, value in checks.items() if isinstance(value, str)}


def normalize_legacy_report(
    definition: dict[str, Any],
    profile: str,
    legacy: dict[str, Any] | None,
    selected: list[str],
    return_code: int,
) -> tuple[str, list[str], list[str], list[str]]:
    """Map existing assertion reports into central scenario status."""
    if not legacy:
        status = "not_checked" if return_code == 77 else "failed"
        return status, [], [], selected
    checks = _legacy_checks(profile, legacy)
    status = str(legacy.get("status", "not_checked"))
    completed: list[str] = []
    failed: list[str] = []
    not_checked: list[str] = []
    for scenario in selected:
        item = definition["scenarios"].get(scenario, {})
        names = item.get(f"legacy_{profile}_checks", [])
        values = [checks.get(name, "not_checked") for name in names]
        if values and all(value in {"passed", "complete"} for value in values):
            completed.append(scenario)
        elif any(value == "failed" for value in values) or status == "failed":
            failed.append(scenario)
        else:
            not_checked.append(scenario)
    if status == "passed" and not failed and not not_checked:
        result = "passed"
    elif status == "failed" or failed:
        result = "failed"
    else:
        result = "not_checked"
    return result, completed, failed, not_checked


def timeout_records(root: Path) -> list[dict[str, Any]]:
    """Load bounded timeout evidence, failing closed on malformed artifacts."""
    records: list[dict[str, Any]] = []
    for path in sorted(root.rglob("*timeout*.json")):
        evidence_file = str(path.relative_to(root))
        try:
            value = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            records.append({"timeout_layer": "invalid", "evidence_file": evidence_file})
            continue
        if isinstance(value, dict) and value.get("timeout_layer"):
            records.append({**value, "evidence_file": evidence_file})
        else:
            records.append({"timeout_layer": "invalid", "evidence_file": evidence_file})
    return records


def make_report(
    *,
    run_id: str,
    profile: str,
    executor: str,
    selected: list[str],
    completed: list[str],
    failed: list[str],
    skipped: list[str],
    not_checked: list[str],
    phases: list[dict[str, Any]],
    source: dict[str, Any],
    environment_value: dict[str, Any],
    return_code: int,
    reused: bool = False,
    reused_from: str | None = None,
    legacy: dict[str, Any] | None = None,
    release_requested: bool = False,
    source_stable: bool = True,
    reuse_requested: bool = True,
    guest_source: dict[str, Any] | None = None,
    guest_source_ok: bool = True,
    timeouts: list[dict[str, Any]] | None = None,
) -> dict[str, Any]:
    timeouts = [] if timeouts is None else timeouts
    status = (
        "passed"
        if not failed and not not_checked and set(selected) <= set(completed)
        else "not_checked"
    )
    if failed or return_code not in {0, 77} or timeouts:
        status = "failed"
    release_eligible = (
        status == "passed"
        and not skipped
        and not not_checked
        and isinstance(source.get("working_tree_dirty"), bool)
        and not source.get("working_tree_dirty")
        and not source.get("identity_error")
        and _known_identity(source.get("commit_sha"))
        and _known_identity(source.get("tree_sha"))
        and source_stable
        and guest_source_ok
        and bool(phases)
        and all(
            isinstance(phase, dict)
            and phase.get("name")
            and phase.get("status") == "passed"
            for phase in phases
        )
    )
    return {
        "schema_version": SCHEMA_VERSION,
        "compatibility_version": COMPATIBILITY_VERSION,
        "run_id": run_id,
        "created_at": datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
        "profile": profile,
        "executor": executor,
        "selected_scenarios": selected,
        "completed_scenarios": completed,
        "failed_scenarios": failed,
        "skipped_scenarios": skipped,
        "not_checked_scenarios": not_checked,
        "phases": phases,
        "source": source,
        "environment": environment_value,
        "guest_source": guest_source,
        "reuse": {"requested": reuse_requested, "reused": reused, "from": reused_from},
        "release_requested": release_requested,
        "release_eligible": release_eligible,
        "status": status,
        "exit_code": return_code,
        "timeouts": timeouts,
        "legacy_report": legacy,
    }


def write_report(path: Path, report: dict[str, Any]) -> None:
    path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def report_is_complete(value: dict[str, Any], required: Iterable[str]) -> bool:
    selected = value.get("selected_scenarios")
    completed = value.get("completed_scenarios")
    phases = value.get("phases")
    required_set = set(required)
    return (
        value.get("schema_version") == SCHEMA_VERSION
        and value.get("compatibility_version") == COMPATIBILITY_VERSION
        and isinstance(value.get("run_id"), str)
        and bool(value.get("run_id"))
        and isinstance(value.get("created_at"), str)
        and bool(value.get("created_at"))
        and value.get("profile") in {"p1", "p2"}
        and value.get("executor") in {"local", "multipass"}
        and isinstance(value.get("environment"), dict)
        and value["environment"].get("executor") == value.get("executor")
        and value.get("status") == "passed"
        and value.get("exit_code") == 0
        and isinstance(value.get("release_requested"), bool)
        and isinstance(value.get("release_eligible"), bool)
        and isinstance(value.get("source"), dict)
        and isinstance(value.get("reuse"), dict)
        and isinstance(value.get("timeouts"), list)
        and not value.get("timeouts")
        and isinstance(selected, list)
        and all(isinstance(item, str) for item in selected)
        and isinstance(completed, list)
        and all(isinstance(item, str) for item in completed)
        and len(selected) == len(set(selected))
        and len(completed) == len(set(completed))
        and set(selected) == set(completed)
        and required_set.issubset(set(selected))
        and isinstance(value.get("failed_scenarios"), list)
        and not value.get("failed_scenarios")
        and isinstance(value.get("skipped_scenarios"), list)
        and not value.get("skipped_scenarios")
        and isinstance(value.get("not_checked_scenarios"), list)
        and not value.get("not_checked_scenarios")
        and isinstance(phases, list)
        and bool(phases)
        and all(
            isinstance(phase, dict)
            and phase.get("name")
            and phase.get("status") == "passed"
            for phase in phases
        )
        and value.get("artifact_manifest")
        == {"file": "artifact-manifest.sha256", "complete": True}
    )


def guest_source_is_valid(value: dict[str, Any], fingerprint: str) -> bool:
    if value.get("executor") != "multipass":
        return True
    guest = value.get("guest_source", {})
    records = guest.get("records") if isinstance(guest, dict) else None
    return (
        isinstance(records, list)
        and bool(records)
        and all(
            isinstance(record, dict)
            and record.get("fingerprint") == fingerprint
            and _known_identity(record.get("commit_sha"))
            and _known_identity(record.get("tree_sha"))
            and isinstance(record.get("working_tree_dirty"), (bool, int))
            and (
                value.get("reuse", {}).get("reused")
                or (
                    record.get("run_id") == value.get("run_id")
                    and record.get("commit_sha")
                    == value.get("source", {}).get("commit_sha")
                    and record.get("tree_sha")
                    == value.get("source", {}).get("tree_sha")
                    and bool(record.get("working_tree_dirty"))
                    == value.get("source", {}).get("working_tree_dirty")
                )
            )
            for record in records
        )
    )


def verify_reuse_chain(
    value: dict[str, Any], *, max_depth: int = 8, seen: set[str] | None = None
) -> bool:
    reuse = value.get("reuse", {})
    if not reuse.get("reused"):
        return True
    origin = reuse.get("origin", {})
    origin_root = Path(origin.get("root", ""))
    manifest_path = origin_root / str(
        origin.get("manifest", "artifact-manifest.sha256")
    )
    if not origin_root.is_dir() or not manifest_path.is_file() or max_depth <= 0:
        return False
    if sha256(manifest_path) != origin.get("manifest_sha256") or not verify_manifest(
        origin_root
    ):
        return False
    seen = set() if seen is None else seen
    key = str(origin_root.resolve())
    if key in seen:
        return False
    seen.add(key)
    try:
        origin_report = json.loads(
            (origin_root / "acceptance-report.json").read_text(encoding="utf-8")
        )
    except (OSError, json.JSONDecodeError):
        return False
    fingerprint = value.get("source", {}).get("fingerprint")
    required = value.get("selected_scenarios", [])
    return (
        isinstance(origin_report, dict)
        and report_is_complete(origin_report, required)
        and origin_report.get("source", {}).get("fingerprint") == fingerprint
        and origin_report.get("executor") == value.get("executor")
        and environments_compatible(
            origin_report.get("environment", {}), value.get("environment", {})
        )
        and guest_source_is_valid(origin_report, fingerprint)
        and not timeout_records(origin_root)
        and verify_reuse_chain(origin_report, max_depth=max_depth - 1, seen=seen)
    )


def report_is_compatible(
    report: dict[str, Any],
    *,
    fingerprint: str,
    required: Iterable[str],
    environment_value: dict[str, Any],
    release: bool,
    source: dict[str, Any],
    root: Path,
) -> bool:
    if not report_is_complete(report, required):
        return False
    if report.get("source", {}).get("fingerprint") != fingerprint:
        return False
    if not verify_reuse_chain(report):
        return False
    if not environments_compatible(report.get("environment", {}), environment_value):
        return False
    if not guest_source_is_valid(report, fingerprint):
        return False
    if timeout_records(root):
        return False
    return verify_manifest(root)


def candidate_reports(evidence_root: Path) -> Iterable[tuple[Path, dict[str, Any]]]:
    if not evidence_root.is_dir():
        return
    for path in evidence_root.glob("*/**/acceptance-report.json"):
        try:
            yield path.parent, json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
