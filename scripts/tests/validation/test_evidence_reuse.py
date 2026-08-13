#!/usr/bin/env python3
"""Evidence reuse policy tests (task 9.4).

Proves scripts/validation.py compact/reuse contract: timeout, unsupported,
infrastructure, failed, and not_checked evidence is never reusable, and the
manifest executed flag is true only for passed evidence. Reuse additionally
requires matching gate, fingerprint, environment, covered checks, and intact
checksums (verified by the reuse command itself).
"""

import json
import pathlib
import subprocess
import sys
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
VALIDATION = ROOT / "scripts" / "validation.py"

NON_PASS = [
    "failed",
    "not_checked",
    "unsupported_environment",
    "infrastructure_error",
    "timeout",
]


def compact(dest: pathlib.Path, status: str) -> pathlib.Path:
    source = dest / "workdir"
    source.mkdir(parents=True)
    (source / "artifact.txt").write_text("evidence", encoding="utf-8")
    gate_root = dest / "p1"
    gate_root.mkdir(parents=True)
    run = subprocess.run(
        [
            sys.executable,
            str(VALIDATION),
            "compact",
            "--source",
            str(source),
            "--dest",
            str(gate_root),
            "--gate",
            "p1",
            "--status",
            status,
            "--fingerprint",
            "fp-test",
            "--commit",
            "abc123",
            "--checks",
            "p1",
            "--artifact-mode",
            "artifact-on-failure",
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    assert run.returncode == 0, run.stderr
    return gate_root / "manifest.json"


def reuse(dest: pathlib.Path) -> int:
    run = subprocess.run(
        [
            sys.executable,
            str(VALIDATION),
            "reuse",
            "--evidence-root",
            str(dest),
            "--gate",
            "p1",
            "--fingerprint",
            "fp-test",
            "--checks",
            "p1",
        ],
        capture_output=True,
        text=True,
        timeout=120,
    )
    return run.returncode


class EvidenceReusePolicyTests(unittest.TestCase):
    def test_non_pass_statuses_are_never_reused(self):
        for status in NON_PASS:
            with self.subTest(status=status), tempfile.TemporaryDirectory() as tmp:
                dest = pathlib.Path(tmp)
                manifest = compact(dest, status)
                data = json.loads(manifest.read_text(encoding="utf-8"))
                self.assertFalse(data["executed"], f"{status} must not be executed")
                self.assertEqual(
                    reuse(dest), 1, f"{status} evidence must not be reused"
                )

    def test_passed_manifest_reports_executed(self):
        with tempfile.TemporaryDirectory() as tmp:
            dest = pathlib.Path(tmp)
            manifest = compact(dest, "passed")
            data = json.loads(manifest.read_text(encoding="utf-8"))
            self.assertTrue(data["executed"])
            self.assertFalse(data["reused"])

    def test_checksums_present_for_every_manifest(self):
        with tempfile.TemporaryDirectory() as tmp:
            dest = pathlib.Path(tmp)
            manifest = compact(dest, "failed")
            checksums = manifest.parent / "checksums.txt"
            self.assertTrue(checksums.is_file())
            listed = checksums.read_text(encoding="utf-8").splitlines()
            self.assertTrue(listed, "checksums must list manifest and friends")


if __name__ == "__main__":
    unittest.main()
