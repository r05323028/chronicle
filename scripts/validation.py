#!/usr/bin/env python3
"""Small, dependency-free helpers for layered Chronicle validation."""

from __future__ import annotations

import argparse
import datetime as dt
import fnmatch
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path
from typing import Any

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python 3.10 fallback is documented.
    tomllib = None


def run(root: Path, *args: str) -> str:
    try:
        return subprocess.check_output(
            args, cwd=root, text=True, stderr=subprocess.DEVNULL
        ).strip()
    except (OSError, subprocess.CalledProcessError):
        return "not_checked"


def config(root: Path, path: Path | None = None) -> dict[str, Any]:
    if tomllib is None:
        raise RuntimeError("Python 3.11+ is required (tomllib missing)")
    with (path or root / "validation/groups.toml").open("rb") as handle:
        return tomllib.load(handle)


def changed_paths(root: Path, changed_since: str | None) -> list[str]:
    if changed_since:
        args = ("git", "diff", "--name-only", changed_since)
        paths = (
            run(root, *args).splitlines() if run(root, *args) != "not_checked" else []
        )
    else:
        paths = []
        for args in (
            ("git", "diff", "--name-only", "HEAD"),
            ("git", "diff", "--cached", "--name-only"),
        ):
            value = run(root, *args)
            if value != "not_checked":
                paths.extend(value.splitlines())
    untracked = run(root, "git", "ls-files", "--others", "--exclude-standard")
    if untracked != "not_checked":
        paths.extend(untracked.splitlines())
    return sorted(set(path for path in paths if path))


def matches(path: str, pattern: str) -> bool:
    path = path.lstrip("./")
    return fnmatch.fnmatchcase(path, pattern) or Path(path).match(pattern)


def select(root: Path, paths: list[str], cfg: dict[str, Any]) -> dict[str, Any]:
    groups = cfg.get("groups", {})
    selected: list[str] = []
    decisions: dict[str, dict[str, Any]] = {}
    matched_paths: set[str] = set()
    for name in sorted(groups):
        group = groups[name]
        hits = [
            path
            for path in paths
            if any(matches(path, pattern) for pattern in group.get("paths", []))
        ]
        matched_paths.update(hits)
        if hits:
            selected.append(name)
            decisions[name] = {
                "selected": True,
                "paths": hits,
                "reason": f"matched {', '.join(hits)}",
            }
        else:
            decisions[name] = {
                "selected": False,
                "paths": [],
                "reason": "no changed path matches group ownership",
            }
    unknown_paths = sorted(set(paths) - matched_paths)
    if unknown_paths:
        selected = sorted(groups)
        for name in selected:
            decisions[name] = {
                "selected": True,
                "paths": decisions[name]["paths"],
                "reason": f"unknown path requires conservative full validation: {', '.join(unknown_paths)}",
            }
    return {
        "changed_paths": paths,
        "unknown_paths": unknown_paths,
        "selected": selected,
        "decisions": decisions,
    }


def source_ownership(root: Path, cfg: dict[str, Any]) -> dict[str, Any]:
    """Ensure every production application source has a validation owner."""
    tracked_value = run(root, "git", "ls-files", "crates/chronicle-application/src")
    if tracked_value == "not_checked":
        tracked = [
            str(path.relative_to(root))
            for path in (root / "crates/chronicle-application/src").rglob("*.rs")
        ]
    else:
        tracked = tracked_value.splitlines()
    groups = cfg.get("groups", {})
    owners: dict[str, list[str]] = {}
    unowned: list[str] = []
    for name in tracked:
        if not name.endswith(".rs"):
            continue
        matches_for_file = [
            group_name
            for group_name, group in groups.items()
            if "p2" in group.get("gates", [])
            and any(matches(name, pattern) for pattern in group.get("paths", []))
        ]
        if matches_for_file:
            owners[name] = sorted(matches_for_file)
        else:
            unowned.append(name)
    return {"files": len(owners) + len(unowned), "owners": owners, "unowned": sorted(unowned)}


def files_for_patterns(root: Path, patterns: list[str]) -> list[Path]:
    result: set[Path] = set()
    tracked = run(root, "git", "ls-files").splitlines()
    for name in tracked:
        if any(matches(name, pattern) for pattern in patterns):
            path = root / name
            if path.is_file():
                result.add(path)
    # Git-aware lookup above covers tracked files. Walk the checkout as well so
    # untracked acceptance-sensitive source is fingerprinted exactly as tested.
    ignored = {".git", "target", "graphify-out", ".venv", "node_modules"}
    for directory, directories, names in os.walk(root):
        directories[:] = [name for name in directories if name not in ignored]
        base = Path(directory)
        for name in names:
            path = base / name
            relative = str(path.relative_to(root))
            if any(matches(relative, pattern) for pattern in patterns):
                result.add(path)
    return sorted(result)


def digest(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def environment(root: Path) -> dict[str, Any]:
    os_release: dict[str, str] = {}
    capabilities: dict[str, str] = {}
    status = Path("/proc/self/status")
    if status.is_file():
        for line in status.read_text(encoding="utf-8", errors="replace").splitlines():
            if line.startswith(("CapPrm:", "CapEff:", "CapBnd:")):
                key, value = line.split(":", 1)
                capabilities[key] = value.strip()
    release = Path("/etc/os-release")
    if release.is_file():
        for line in release.read_text(encoding="utf-8", errors="replace").splitlines():
            if "=" in line:
                key, value = line.split("=", 1)
                os_release[key] = value.strip('"')
    return {
        "rustc": run(root, "rustc", "-Vv"),
        "architecture": platform.machine(),
        "kernel": platform.release(),
        "ubuntu": os_release.get("VERSION_ID", "not_checked"),
        "os": platform.system(),
        "target": os.environ.get("CARGO_BUILD_TARGET", "host"),
        "btf": Path("/sys/kernel/btf/vmlinux").is_file(),
        "cgroup_v2": Path("/sys/fs/cgroup/cgroup.controllers").is_file(),
        "capabilities": capabilities,
    }


def load_environment(root: Path, path: Path | None = None) -> dict[str, Any]:
    if path is None:
        return environment(root)
    try:
        value = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"invalid environment JSON: {path}") from exc
    if not isinstance(value, dict):
        raise ValueError("environment JSON must contain an object")
    return value


def fingerprint(
    root: Path,
    gate: str,
    cfg: dict[str, Any],
    environment_override: dict[str, Any] | None = None,
    *,
    profile: str | None = None,
    executor: str | None = None,
) -> dict[str, Any]:
    gate_cfg = cfg.get("gates", {}).get(gate)
    if not gate_cfg:
        raise ValueError(f"unknown gate: {gate}")
    groups = cfg.get("groups", {})
    patterns: list[str] = []
    for group_name in gate_cfg.get("source_groups", []):
        patterns.extend(groups.get(group_name, {}).get("paths", []))
    patterns.extend(gate_cfg.get("acceptance_paths", []))
    patterns.extend(gate_cfg.get("build_inputs", []))
    patterns.append("Cargo.lock")
    paths = files_for_patterns(root, sorted(set(patterns)))
    inputs = [
        {"path": str(path.relative_to(root)), "sha256": digest(path)} for path in paths
    ]
    payload = {
        "fingerprint_version": 2,
        "version": cfg.get("version", 1),
        "gate": gate,
        "profile": profile or gate,
        "executor": executor or "validation",
        "inputs": inputs,
        # Environment is compatibility metadata, not source identity. Keeping it
        # outside the hashed payload permits equivalent source trees to reuse only
        # when the executor separately proves environment compatibility.
        "environment": environment_override or environment(root),
        "validation_config_sha256": digest(root / "validation/groups.toml"),
        "checks": gate_cfg.get("checks", []),
    }
    fingerprint_payload = {key: value for key, value in payload.items() if key != "environment"}
    encoded = json.dumps(fingerprint_payload, sort_keys=True, separators=(",", ":")).encode()
    return {"fingerprint": hashlib.sha256(encoded).hexdigest(), **payload}


def acceptance_fingerprint(root: Path, cfg: dict[str, Any]) -> dict[str, Any]:
    """Fingerprint union of P1/P2 acceptance-sensitive source, independent of profile."""
    patterns: list[str] = []
    checks: set[str] = set()
    for gate in ("p1", "p2"):
        gate_cfg = cfg.get("gates", {}).get(gate, {})
        for group_name in gate_cfg.get("source_groups", []):
            patterns.extend(cfg.get("groups", {}).get(group_name, {}).get("paths", []))
        patterns.extend(gate_cfg.get("acceptance_paths", []))
        patterns.extend(gate_cfg.get("build_inputs", []))
        checks.update(gate_cfg.get("checks", []))
    patterns = [pattern for pattern in patterns if not pattern.startswith("docs/")]
    patterns.extend(
        [
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "crates/**",
            "ebpf/**",
            "scripts/acceptance.sh",
            "scripts/acceptance/**",
            "scripts/validation.py",
            "scripts/validate.sh",
            "validation/**",
        ]
    )
    paths = files_for_patterns(root, sorted(set(patterns)))
    inputs = [
        {"path": str(path.relative_to(root)), "sha256": digest(path)}
        for path in paths
    ]
    payload = {
        "fingerprint_version": 2,
        "inputs": inputs,
        "validation_config_sha256": digest(root / "validation/groups.toml"),
        "checks": sorted(checks),
    }
    encoded = json.dumps(payload, sort_keys=True, separators=(",", ":")).encode()
    return {"fingerprint": hashlib.sha256(encoded).hexdigest(), **payload}


def iso_now() -> str:
    return dt.datetime.now(dt.timezone.utc).replace(microsecond=0).isoformat()


def copy_failure_data(source: Path, dest: Path) -> list[str]:
    copied: list[str] = []
    report = source / "acceptance-report.json"
    if report.is_file():
        target = dest / "failure-summary.md"
        target.write_text(
            "# Validation failure\n\n```json\n"
            + report.read_text(encoding="utf-8", errors="replace")
            + "\n```\n",
            encoding="utf-8",
        )
        copied.append(str(target.relative_to(dest)))
    candidates = [
        "failed-test.log",
        "failure.log",
        "kernel-log.txt",
        "reproducer.sh",
        "commands.log",
        "build.log",
        "recorder-status.json",
        "recorder-status.stderr",
        "recorder-service-status.txt",
        "recorder-journal.log",
        "kernel-version.txt",
        "kernel-capabilities.json",
        "cgroup-information.txt",
        "btf-information.txt",
        "bpftool-programs.txt",
        "bpftool-links.txt",
        "wal-directory-listing.txt",
        "checkpoint-metadata.json",
        "process-list.txt",
        "disk-space.txt",
        "readiness-transitions.log",
    ]
    for name in candidates:
        path = source / name
        if path.is_file():
            target_name = name
            if (
                name in {"commands.log", "build.log"}
                and (dest / "failed-test.log").exists()
            ):
                continue
            if name in {"commands.log", "build.log"}:
                target_name = "failed-test.log"
            shutil.copy2(path, dest / target_name)
            copied.append(target_name)
    if not (dest / "failed-test.log").exists():
        logs = sorted(source.rglob("*.log"))
        if logs:
            shutil.copy2(logs[0], dest / "failed-test.log")
            copied.append("failed-test.log")
    paths_file = source / "failure-paths.txt"
    selected: list[Path] = []
    if paths_file.is_file():
        for line in paths_file.read_text(
            encoding="utf-8", errors="replace"
        ).splitlines():
            path = (source / line).resolve()
            try:
                path.relative_to(source.resolve())
            except ValueError:
                continue
            if path.is_file():
                selected.append(path)
    if selected:
        staging = dest / "relevant-wal-segments"
        for path in selected:
            relative = path.relative_to(source.resolve())
            target = staging / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)
        archive = dest / "relevant-wal-segments.tar.zst"
        zstd = shutil.which("zstd")
        if zstd:
            with tempfile.NamedTemporaryFile(suffix=".tar", delete=False) as handle:
                tar_path = Path(handle.name)
            try:
                with tarfile.open(tar_path, "w") as tar:
                    tar.add(staging, arcname="relevant-wal-segments")
                subprocess.run(
                    [zstd, "-q", "-f", str(tar_path), "-o", str(archive)], check=True
                )
                shutil.rmtree(staging)
                tar_path.unlink(missing_ok=True)
                copied.append(archive.name)
            except (OSError, subprocess.CalledProcessError):
                tar_path.unlink(missing_ok=True)
        if staging.exists():
            copied.append(str(staging.relative_to(dest)))
    return copied


def compact(args: argparse.Namespace) -> None:
    source, dest = Path(args.source), Path(args.dest)
    if args.artifact_mode == "no-artifact":
        return
    dest.mkdir(parents=True, exist_ok=True)
    status = args.status
    copied: list[str] = []
    if status != "passed":
        copied = copy_failure_data(source, dest)
    # Successful evidence stays compact locally. Full release artifacts belong in
    # an external evidence store, never in the repository workspace.
    elif args.artifact_mode == "release":
        copied = []
    run_environment = load_environment(source, getattr(args, "environment", None))
    artifact_policy = (
        "compact-success-external-full"
        if args.artifact_mode == "release"
        else "failure-first"
    )
    summary = {
        "version": 1,
        "gate": args.gate,
        "status": status,
        "fingerprint": args.fingerprint,
        "original_commit": args.commit,
        "checks": [check for check in args.checks.split(",") if check],
        "artifact_policy": artifact_policy,
    }
    (dest / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    (dest / "environment.json").write_text(
        json.dumps(run_environment, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    manifest = {
        "version": 1,
        "gate": args.gate,
        "status": status,
        "fingerprint": args.fingerprint,
        "original_commit": args.commit,
        "created_at": iso_now(),
        "environment": run_environment,
        "covered_checks": [check for check in args.checks.split(",") if check],
        "executed": status == "passed",
        "reused": False,
        "files": ["summary.json", "environment.json", "manifest.json", *copied],
        "artifact_policy": artifact_policy,
    }
    (dest / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    files = [
        path
        for path in sorted(dest.rglob("*"))
        if path.is_file() and path.name != "checksums.txt"
    ]
    (dest / "checksums.txt").write_text(
        "".join(f"{digest(path)}  {path.relative_to(dest)}\n" for path in files),
        encoding="utf-8",
    )


def reuse(args: argparse.Namespace) -> int:
    manifest_path = Path(args.evidence_root) / args.gate / "manifest.json"
    if not manifest_path.is_file():
        return 1
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        if (
            manifest.get("status") != "passed"
            or not isinstance(manifest.get("executed"), bool)
            or not manifest.get("executed")
            or manifest.get("fingerprint") != args.fingerprint
            or (
                getattr(args, "release", False)
                and getattr(args, "commit", None)
                and manifest.get("original_commit") != args.commit
            )
        ):
            return 1
        required_checks = {
            check for check in getattr(args, "checks", "").split(",") if check
        }
        covered_checks = set(manifest.get("covered_checks", []))
        if not required_checks.issubset(covered_checks):
            return 1
        checksums = manifest_path.parent / "checksums.txt"
        if not checksums.is_file():
            return 1
        root = manifest_path.parent.resolve()
        for line in checksums.read_text(encoding="utf-8").splitlines():
            expected, relative = line.split("  ", 1)
            candidate = Path(relative)
            if candidate.is_absolute() or ".." in candidate.parts:
                return 1
            path = (root / candidate).resolve()
            try:
                path.relative_to(root)
            except ValueError:
                return 1
            if not path.is_file() or digest(path) != expected:
                return 1
    except (OSError, ValueError, json.JSONDecodeError):
        return 1
    print(
        json.dumps(
            {
                "reused": True,
                **{
                    key: manifest.get(key)
                    for key in (
                        "fingerprint",
                        "original_commit",
                        "created_at",
                        "environment",
                        "covered_checks",
                    )
                },
            }
        )
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    select_parser = sub.add_parser("select")
    select_parser.add_argument("--root", type=Path, required=True)
    select_parser.add_argument("--changed-since")
    select_parser.add_argument("--paths", nargs="*")
    select_parser.add_argument("--config", type=Path)
    environment_parser = sub.add_parser("environment")
    environment_parser.add_argument("--root", type=Path, required=True)
    ownership_parser = sub.add_parser("ownership")
    ownership_parser.add_argument("--root", type=Path, required=True)
    ownership_parser.add_argument("--config", type=Path)
    fp_parser = sub.add_parser("fingerprint")
    fp_parser.add_argument("--root", type=Path, required=True)
    fp_parser.add_argument("--gate", required=True)
    fp_parser.add_argument("--config", type=Path)
    fp_parser.add_argument("--environment", type=Path)
    compact_parser = sub.add_parser("compact")
    compact_parser.add_argument("--source", required=True)
    compact_parser.add_argument("--dest", required=True)
    compact_parser.add_argument("--gate", required=True)
    compact_parser.add_argument("--status", required=True)
    compact_parser.add_argument("--fingerprint", required=True)
    compact_parser.add_argument("--commit", required=True)
    compact_parser.add_argument("--checks", default="")
    compact_parser.add_argument("--environment", type=Path)
    compact_parser.add_argument(
        "--artifact-mode",
        choices=("no-artifact", "artifact-on-failure", "release"),
        required=True,
    )
    reuse_parser = sub.add_parser("reuse")
    reuse_parser.add_argument("--evidence-root", required=True)
    reuse_parser.add_argument("--gate", required=True)
    reuse_parser.add_argument("--fingerprint", required=True)
    reuse_parser.add_argument("--checks", default="")
    reuse_parser.add_argument("--commit")
    reuse_parser.add_argument("--release", action="store_true")
    args = parser.parse_args()
    if args.command == "environment":
        print(json.dumps(environment(args.root), indent=2, sort_keys=True))
    elif args.command == "ownership":
        value = source_ownership(args.root, config(args.root, args.config))
        print(json.dumps(value, indent=2, sort_keys=True))
        if value["unowned"]:
            return 1
    elif args.command == "select":
        cfg = config(args.root, args.config)
        paths = (
            args.paths
            if args.paths is not None and args.paths
            else changed_paths(args.root, args.changed_since)
        )
        print(
            json.dumps(
                select(args.root, sorted(set(paths)), cfg), indent=2, sort_keys=True
            )
        )
    elif args.command == "fingerprint":
        print(
            json.dumps(
                fingerprint(
                    args.root,
                    args.gate,
                    config(args.root, args.config),
                    load_environment(args.root, args.environment),
                ),
                indent=2,
                sort_keys=True,
            )
        )
    elif args.command == "compact":
        compact(args)
    else:
        return reuse(args)
    return 0


if __name__ == "__main__":
    sys.exit(main())
