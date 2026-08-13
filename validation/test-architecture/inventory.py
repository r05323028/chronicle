#!/usr/bin/env python3
"""Generate validation/test-architecture/baseline.json from the current checkout.

Snapshots every significant test/assertion surface so the responsibility-based
migration can record a before-state. Re-run after repository changes to refresh:

    python3 validation/test-architecture/inventory.py > validation/test-architecture/baseline.json

Standard-library only; deterministic ordering.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
import tomllib
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]

SIGNAL_SUBSTRINGS = {
    "subprocess": ("Command::new", "std::process", "tokio::process"),
    "network": ("TcpListener", "TcpStream", "UdpSocket", "127.0.0.1", "localhost"),
    "sleep": ("thread::sleep", "tokio::time::sleep"),
    "privilege": ("bpffs", "BTF", "cgroup", "ebpf", "eBPF", "bpftool", "CapEff"),
    "tempfs": ("tempdir", "TempDir", "NamedTempFile"),
    "process_signal": ("::kill", "signal(", "sudo", "systemctl"),
}


def rust_inventory() -> dict:
    files = []
    for path in sorted((ROOT / "crates").rglob("*.rs")):
        text = path.read_text(errors="replace")
        cfg_modules = len(re.findall(r"#\s*\[cfg\(test\)\]", text))
        test_attrs = len(re.findall(r"#\s*\[(?:tokio::)?test(?:\\([^]]*\\))?\]", text))
        in_tests_dir = "/tests/" in path.as_posix()
        if not (cfg_modules or test_attrs or in_tests_dir):
            continue
        signals = sorted(
            name
            for name, needles in SIGNAL_SUBSTRINGS.items()
            if any(needle in text for needle in needles)
        )
        names = re.findall(
            r"#\s*\[(?:tokio::)?test(?:\\([^]]*\\))?\][\s\S]{0,200}?fn\s+([A-Za-z0-9_]+)",
            text,
        )
        files.append(
            {
                "path": path.relative_to(ROOT).as_posix(),
                "kind": "integration" if in_tests_dir else "unit_colocated",
                "cfg_test_modules": cfg_modules,
                "test_attributes": test_attrs,
                "test_names": names,
                "signals": signals,
                "lines": text.count("\n") + 1,
            }
        )
    return {
        "files": files,
        "file_count": len(files),
        "colocated_count": sum(1 for f in files if f["kind"] == "unit_colocated"),
        "integration_count": sum(1 for f in files if f["kind"] == "integration"),
        "test_attribute_count": sum(f["test_attributes"] for f in files),
    }


def tree_files(directory: str, skip: set[str]) -> list[dict]:
    out = []
    for path in sorted((ROOT / directory).rglob("*")):
        if not path.is_file():
            continue
        if any(part in skip for part in path.parts):
            continue
        out.append(
            {
                "path": path.relative_to(ROOT).as_posix(),
                "lines": len(path.read_text(errors="replace").splitlines()),
            }
        )
    return out


def acceptance_inventory() -> dict:
    scenarios_path = ROOT / "scripts/acceptance/scenarios.toml"
    with scenarios_path.open("rb") as handle:
        definition = tomllib.load(handle)
    scenarios = []
    for scenario_id, cfg in sorted(definition.get("scenarios", {}).items()):
        scenarios.append(
            {
                "id": scenario_id,
                "timeout_seconds": cfg.get("timeout_seconds"),
                "capabilities": sorted(cfg.get("capabilities", [])),
                "phases": sorted(cfg.get("phases", [])),
                "legacy_p1_checks": sorted(cfg.get("legacy_p1_checks", [])),
                "legacy_p2_checks": sorted(cfg.get("legacy_p2_checks", [])),
            }
        )
    embedded = []
    for path in sorted((ROOT / "scripts/acceptance/lib/scenarios").rglob("*.sh")):
        command_position = re.compile(
            r"^(?:(?:if|then)\s+)?(?:run_compat_command\s+\S+\s+)?"
            r"(?:[A-Za-z_][A-Za-z0-9_]*=\S+\s+)*cargo\b"
        )
        wrapped = re.compile(r"\brun_compat_command\s+\S+\s+cargo\b")
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if command_position.match(stripped) or wrapped.search(stripped):
                embedded.append(
                    {
                        "file": path.relative_to(ROOT).as_posix(),
                        "line": number,
                        "command": re.sub(r"\s+", " ", stripped),
                    }
                )
    embedded.sort(key=lambda item: (item["file"], item["line"]))
    profiles = {}
    for profile in ("p1", "p2"):
        profiles[profile] = {
            "scenarios": definition.get("profiles", {})
            .get(profile, {})
            .get("scenarios", []),
            "execution_order": definition.get("profiles", {})
            .get(profile, {})
            .get("execution_order", []),
        }
    return {
        "scenario_definitions": scenarios,
        "scenario_id_count": len(scenarios),
        "profiles": profiles,
        "embedded_cargo_repo_commands": embedded,
        "embedded_command_count": len(embedded),
    }


def ci_inventory() -> dict:
    out = []
    workflow = ROOT / ".github/workflows/ci.yml"
    if not workflow.is_file():
        return {"workflow": None, "invocations": []}
    text = workflow.read_text(errors="replace")
    for index, line in enumerate(text.splitlines(), 1):
        stripped = line.strip()
        if re.search(
            r"validate|accept|cargo|pytest|multipass|gate|release|smoke|e2e",
            stripped,
            re.I,
        ):
            out.append({"line": index, "text": stripped[:200]})
    return {"workflow": workflow.relative_to(ROOT).as_posix(), "invocations": out}


def groups_inventory() -> dict:
    with (ROOT / "validation/groups.toml").open("rb") as handle:
        cfg = tomllib.load(handle)
    return {
        "groups": {
            name: {
                "paths": group.get("paths", []),
                "commands": group.get("commands", []),
                "gates": group.get("gates", []),
            }
            for name, group in cfg.get("groups", {}).items()
        },
        "gates": {
            name: {
                "source_groups": gate.get("source_groups", []),
                "checks": gate.get("checks", []),
            }
            for name, gate in cfg.get("gates", {}).items()
        },
    }


def evidence_fields() -> list[str]:
    fields = set()
    for path in (ROOT / "scripts/acceptance").rglob("*.py"):
        fields.update(re.findall(r"\"([a-z_]+)\":", path.read_text(errors="replace")))
    return sorted(fields)


def checkout() -> str:
    try:
        return subprocess.check_output(
            ["git", "-C", str(ROOT), "rev-parse", "--short", "HEAD"],
            text=True,
            timeout=30,
            stderr=subprocess.DEVNULL,
        ).strip()
    except (OSError, subprocess.SubprocessError):
        return "not_checked"


def main() -> int:
    baseline = {
        "version": 1,
        "captured_at_utc": datetime.now(timezone.utc).isoformat(),
        "commit": checkout(),
        "rust": rust_inventory(),
        "root_tests": tree_files("tests", {".ruff_cache", "__pycache__"}),
        "script_tests": tree_files("scripts/tests", {"__pycache__"}),
        "acceptance": acceptance_inventory(),
        "ci": ci_inventory(),
        "validation_groups": groups_inventory(),
        "evidence_report_fields": evidence_fields(),
    }
    print(json.dumps(baseline, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main())
