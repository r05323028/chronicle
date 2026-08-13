#!/usr/bin/env python3
"""Rootless preflight outcome tests (tasks 7.2/7.3).

Drives scripts/privileged/preflight.py through every outcome state via the
--probe-results injection seam plus one real host run, and proves the
no-product-assertion contract.
"""

import json
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PREFLIGHT = ROOT / "scripts" / "privileged" / "preflight.py"

PROBES = [
    "os",
    "architecture",
    "ubuntu_version",
    "kernel_version",
    "privileges",
    "cgroup_v2",
    "btf",
    "bpffs",
    "hooks_tooling",
    "executor_readiness",
]

PRODUCT_KEYS = {
    "wal",
    "session",
    "etl",
    "checkpoint",
    "replay",
    "recording_id",
    "counters",
}


def run(*args):
    return subprocess.run(
        [sys.executable, str(PREFLIGHT), *args],
        capture_output=True,
        text=True,
        timeout=60,
    )


def injected(statuses: dict) -> str:
    payload = {pid: statuses.get(pid, "supported") for pid in PROBES}
    return json.dumps(payload)


class PreflightOutcomeTests(unittest.TestCase):
    def test_real_host_run_never_claims_product_failure(self):
        r = run()
        report = json.loads(r.stdout)
        self.assertNotIn("product_failure", report["outcome"])
        self.assertIn(
            report["outcome"],
            {
                "supported",
                "unsupported_environment",
                "not_checked",
                "infrastructure_error",
            },
        )
        self.assertEqual(len(report["probes"]), len(PROBES))
        expected = {
            "supported": 0,
            "unsupported_environment": 78,
            "not_checked": 77,
            "infrastructure_error": 79,
        }[report["outcome"]]
        self.assertEqual(r.returncode, expected)
        if report["outcome"] == "unsupported_environment":
            self.assertTrue(any("remediation" in p for p in report["probes"]))

    def test_all_supported_injection_exits_zero(self):
        r = run("--probe-results", injected({}))
        report = json.loads(r.stdout)
        self.assertEqual(r.returncode, 0)
        self.assertEqual(report["outcome"], "supported")
        self.assertTrue(all(p["status"] == "supported" for p in report["probes"]))

    def test_unsupported_injection_exits_78_with_remediation(self):
        r = run("--probe-results", injected({"btf": "unsupported"}))
        report = json.loads(r.stdout)
        self.assertEqual(r.returncode, 78)
        self.assertEqual(report["outcome"], "unsupported_environment")
        btf = next(p for p in report["probes"] if p["id"] == "btf")
        self.assertEqual(btf["status"], "unsupported")
        self.assertIn("remediation", btf)

    def test_not_checked_injection_exits_77(self):
        r = run("--probe-results", injected({"privileges": "not_checked"}))
        report = json.loads(r.stdout)
        self.assertEqual(r.returncode, 77)
        self.assertEqual(report["outcome"], "not_checked")

    def test_infrastructure_error_injection_exits_79(self):
        r = run(
            "--probe-results", injected({"executor_readiness": "infrastructure_error"})
        )
        report = json.loads(r.stdout)
        self.assertEqual(r.returncode, 79)
        self.assertEqual(report["outcome"], "infrastructure_error")

    def test_unsupported_precedes_not_checked(self):
        r = run(
            "--probe-results",
            injected({"os": "unsupported", "privileges": "not_checked"}),
        )
        report = json.loads(r.stdout)
        self.assertEqual(report["outcome"], "unsupported_environment")
        self.assertEqual(r.returncode, 78)

    def test_selected_tests_recorded_not_run(self):
        r = run(
            "--probe-results",
            injected({}),
            "--tests",
            "acceptance:checkpoint-kill-restart,e2e:privileged-capture-pipeline",
        )
        report = json.loads(r.stdout)
        self.assertEqual(
            report["selected_tests"],
            ["acceptance:checkpoint-kill-restart", "e2e:privileged-capture-pipeline"],
        )
        self.assertEqual(report["outcome"], "supported")


class PreflightContractTests(unittest.TestCase):
    def test_invalid_probe_results_json_exits_2(self):
        r = run("--probe-results", "{not json")
        self.assertEqual(r.returncode, 2)

    def test_unknown_probe_id_exits_2(self):
        r = run("--probe-results", json.dumps({"wal": "supported"}))
        self.assertEqual(r.returncode, 2)

    def test_non_dict_probe_results_exits_2(self):
        r = run("--probe-results", json.dumps(["supported"]))
        self.assertEqual(r.returncode, 2)

    def test_invalid_probe_status_exits_2(self):
        r = run("--probe-results", injected({"os": "garbage"}))
        self.assertEqual(r.returncode, 2)

    def test_report_has_no_product_assertions(self):
        r = run("--probe-results", injected({}))
        report = json.loads(r.stdout)
        flat = set(report.keys()) | {p["id"] for p in report["probes"]}
        self.assertTrue(
            flat.isdisjoint(PRODUCT_KEYS),
            f"preflight leaked product vocabulary: {flat & PRODUCT_KEYS}",
        )
        self.assertEqual(report["schema_version"], 1)


if __name__ == "__main__":
    unittest.main()
