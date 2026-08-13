#!/usr/bin/env python3
"""Privileged preflight probe (privileged-preflight-schema.toml v1).

Classifies environment support for privileged Chronicle tests before any
product assertion runs. This script contains NO product assertions (no
WAL/ETL/CLI/replay logic): it only classifies the environment.

Outcomes and exit codes per schema [exit_mapping]:
  supported_pass=0  unsupported_environment=78  not_checked=77
  infrastructure_error=79  usage_or_contract=2

Rootless test seam: --probe-results JSON injects per-probe status so
runner tests can drive every outcome without root or a VM.
"""

import argparse
import json
import os
import platform
import shutil
import sys
import tempfile
from pathlib import Path

SCHEMA_VERSION = 1
EXIT = {
    "supported_pass": 0,
    "product_failure": 1,
    "usage_or_contract": 2,
    "not_checked": 77,
    "unsupported_environment": 78,
    "infrastructure_error": 79,
    "timeout": 124,
}

# CAP_NET_ADMIN=12, CAP_SYS_ADMIN=21, CAP_BPF=39 (Linux capability bits)
REQUIRED_CAPS = (("CAP_NET_ADMIN", 12), ("CAP_SYS_ADMIN", 21), ("CAP_BPF", 39))


def _os_release() -> dict:
    try:
        data = Path("/etc/os-release").read_text()
    except OSError:
        return {}
    out = {}
    for line in data.splitlines():
        if "=" in line and not line.startswith("#"):
            k, _, v = line.partition("=")
            out[k.strip()] = v.strip().strip('"')
    return out


def _effective_caps():
    try:
        for line in Path("/proc/self/status").read_text().splitlines():
            if line.startswith("CapEff:"):
                return line.split(":", 1)[1].strip()
    except OSError:
        return None
    return None


def _caps_ok(hexcap) -> bool:
    try:
        value = int(hexcap, 16)
    except (TypeError, ValueError):
        return False
    return all(bool(value & (1 << bit)) for _, bit in REQUIRED_CAPS)


PROBES = {}


def probe(name):
    def wrap(fn):
        PROBES[name] = fn
        return fn

    return wrap


@probe("os")
def _os():
    return "supported" if platform.system() == "Linux" else "unsupported"


@probe("architecture")
def _arch():
    return "supported" if platform.machine() in ("x86_64", "aarch64") else "unsupported"


@probe("ubuntu_version")
def _ubuntu():
    rel = _os_release()
    if rel.get("ID") != "ubuntu":
        return "unsupported"
    return "supported" if rel.get("VERSION_ID") == "24.04" else "unsupported"


@probe("kernel_version")
def _kernel():
    try:
        parts = platform.release().split("-", 1)[0].split(".")
        major, minor = int(parts[0]), int(parts[1])
    except (IndexError, ValueError):
        return "unsupported"
    return "supported" if (major, minor) >= (6, 8) else "unsupported"


@probe("privileges")
def _privileges():
    if os.geteuid() == 0:
        return "supported"
    caps = _effective_caps()
    if caps is None:
        return "not_checked"
    return "supported" if _caps_ok(caps) else "unsupported"


@probe("cgroup_v2")
def _cgroup_v2():
    try:
        ok = Path("/sys/fs/cgroup/cgroup.controllers").is_file()
    except OSError:
        return "not_checked"
    return "supported" if ok else "unsupported"


@probe("btf")
def _btf():
    try:
        ok = Path("/sys/kernel/btf/vmlinux").is_file()
    except OSError:
        return "not_checked"
    return "supported" if ok else "unsupported"


@probe("bpffs")
def _bpffs():
    try:
        mounts = Path("/proc/mounts").read_text()
    except OSError:
        return "not_checked"
    ok = any(
        parts[1] == "/sys/fs/bpf" and parts[2] == "bpf"
        for parts in (line.split() for line in mounts.splitlines())
    )
    return "supported" if ok else "unsupported"


@probe("hooks_tooling")
def _tooling():
    missing = [
        tool
        for tool in ("cargo", "python3", "bpf-linker", "bpftool", "mountpoint", "find")
        if shutil.which(tool) is None
    ]
    return "supported" if not missing else "unsupported"


@probe("executor_readiness")
def _executor():
    # Bare-host self-check: process spawn and temp write must work.
    try:
        fd, name = tempfile.mkstemp(prefix="chronicle-preflight-")
        os.close(fd)
        os.unlink(name)
        return "supported"
    except OSError:
        return "not_checked"


MESSAGES = {
    "os": (
        "privileged Chronicle tests require Linux",
        "run on a supported Linux host or Multipass Ubuntu VM",
    ),
    "architecture": (
        "architecture is not supported",
        "use an aarch64/x86_64 supported image",
    ),
    "ubuntu_version": (
        "supported Ubuntu release not detected",
        "use Ubuntu 24.04 image",
    ),
    "kernel_version": (
        "kernel too old for supported capture",
        "update kernel or use supported image",
    ),
    "privileges": (
        "insufficient capabilities",
        "grant CAP_NET_ADMIN/CAP_SYS_ADMIN/CAP_BPF or run as privileged service",
    ),
    "cgroup_v2": (
        "cgroup v2 unified hierarchy missing",
        "boot with cgroup v2 (systemd.unified_cgroup_hierarchy)",
    ),
    "btf": (
        "kernel BTF unavailable",
        "install linux-image with BTF or enable CONFIG_DEBUG_INFO_BTF",
    ),
    "bpffs": ("bpffs not mounted", "mount -t bpf bpf /sys/fs/bpf"),
    "hooks_tooling": (
        "required tooling missing",
        "install cargo, python3, bpf-linker, bpftool, mountpoint, find",
    ),
    "executor_readiness": (
        "executor or VM not ready",
        "check Multipass/VM state and re-run prepare",
    ),
}


VALID_STATUSES = {"supported", "unsupported", "not_checked", "infrastructure_error"}


def run_probes(injected: dict | None) -> list:
    results = []
    for probe_id in PROBES:
        if injected is not None:
            status = injected.get(probe_id, "not_checked")
        else:
            try:
                status = PROBES[probe_id]()
            except (
                OSError,
                ValueError,
                TypeError,
            ):  # probe crashed -> environment cannot be proven
                status = "infrastructure_error"
        message, remediation = MESSAGES[probe_id]
        entry = {"id": probe_id, "status": status, "message": message}
        if status == "unsupported":
            entry["remediation"] = remediation
        results.append(entry)
    return results


def classify(results: list) -> str:
    if any(r["status"] == "unsupported" for r in results):
        return "unsupported_environment"
    if any(r["status"] == "not_checked" for r in results):
        return "not_checked"
    if any(r["status"] == "infrastructure_error" for r in results):
        return "infrastructure_error"
    return "supported"


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(
        prog="preflight", description=(__doc__ or "").splitlines()[0]
    )
    parser.add_argument(
        "--probe-results",
        metavar="JSON",
        help="inject per-probe status for rootless runner tests",
    )
    parser.add_argument(
        "--tests",
        metavar="ID[,ID...]",
        help="selected privileged test IDs (recorded only; no assertions run here)",
    )
    args = parser.parse_args(argv)

    injected = None
    if args.probe_results:
        try:
            injected = json.loads(args.probe_results)
        except json.JSONDecodeError:
            print(
                json.dumps(
                    {
                        "schema_version": SCHEMA_VERSION,
                        "error": "invalid --probe-results JSON",
                    }
                ),
                file=sys.stderr,
            )
            return EXIT["usage_or_contract"]
        if not isinstance(injected, dict):
            print(
                json.dumps(
                    {
                        "schema_version": SCHEMA_VERSION,
                        "error": "--probe-results must be a JSON object",
                    }
                ),
                file=sys.stderr,
            )
            return EXIT["usage_or_contract"]
        unknown = set(injected) - set(PROBES)
        if unknown:
            print(
                json.dumps(
                    {
                        "schema_version": SCHEMA_VERSION,
                        "error": f"unknown probe ids: {sorted(unknown)}",
                    }
                ),
                file=sys.stderr,
            )
            return EXIT["usage_or_contract"]
        invalid = sorted(
            pid for pid, status in injected.items() if status not in VALID_STATUSES
        )
        if invalid:
            print(
                json.dumps(
                    {
                        "schema_version": SCHEMA_VERSION,
                        "error": f"invalid probe statuses: {invalid}",
                    }
                ),
                file=sys.stderr,
            )
            return EXIT["usage_or_contract"]

    results = run_probes(injected)
    outcome = classify(results)
    OUTCOME_EXIT = {
        "supported": EXIT["supported_pass"],
        "unsupported_environment": EXIT["unsupported_environment"],
        "not_checked": EXIT["not_checked"],
        "infrastructure_error": EXIT["infrastructure_error"],
    }
    exit_code = OUTCOME_EXIT[outcome]
    selected = args.tests.split(",") if args.tests else []
    report = {
        "schema_version": SCHEMA_VERSION,
        "outcome": outcome,
        "exit_code": exit_code,
        "probes": results,
        "selected_tests": selected,
    }
    print(json.dumps(report))
    return exit_code


if __name__ == "__main__":
    sys.exit(main())
