#!/usr/bin/env python3
"""Smoke tests: shipped executables and basic entry points are alive.

Black-box, minimal, bounded. No full workflows, no arbitrary correctness
sleeps, no daemon lifecycle. Covers help, usage errors, empty list, invalid
fixture/config errors, unsupported-build record failure, and one minimal
fixture invocation per public surface (record -> inspect/replay liveness).

Run: python3 tests/smoke/test_smoke.py
"""

from __future__ import annotations

import importlib.util
import json
import shutil
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "chronicle_test_support", ROOT / "tests/support/process.py"
)
assert _spec is not None and _spec.loader is not None
process = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(process)


class SmokeTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("smoke")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_help_is_alive(self):
        result = process.run(self.binary, ["--help"], format_json=False, timeout=15)
        self.assertEqual(result.returncode, 0)
        text = result.stdout.decode()
        for command in ("record", "replay", "list", "inspect", "doctor"):
            self.assertIn(command, text)

    def test_usage_error_is_sane(self):
        result = process.run(
            self.binary, ["--bogus-flag"], format_json=False, timeout=15
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Usage", result.stderr.decode())

    def test_empty_list_is_alive(self):
        result = process.run(self.binary, ["list"], data_dir=self.root, timeout=15)
        self.assertEqual(result.returncode, 0)
        value = json.loads(result.stdout.decode())
        self.assertEqual(value["version"], 1)
        self.assertEqual(value["recordings"], [])

    def test_missing_fixture_returns_sane_error(self):
        missing = self.root / "missing.json"
        result = process.run(
            self.binary,
            [
                "record",
                "--source",
                "fixture",
                "--input",
                str(missing),
                "--root",
                str(self.root),
            ],
            timeout=15,
        )
        self.assertEqual(result.returncode, 3)
        payload = json.loads(result.stderr.decode())
        self.assertEqual(payload["code"], 3)

    def test_unsupported_build_record_fails_fast(self):
        marker = self.root / "target-started"
        process.require_rootless_record_failure(self.binary, self.root, marker)

    def test_record_liveness_minimal_fixture(self):
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        self.assertTrue(session)
        self.assertTrue((self.root / "sessions" / session / "session.json").is_file())
        self.assertTrue(
            list((self.root / "wal" / session / "segments").glob("*.chwal"))
        )

    def test_inspect_liveness_minimal_recording(self):
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        inspection = process.inspect_session(self.binary, session, self.root)
        self.assertEqual(inspection["version"], 1)
        self.assertEqual(inspection["replayability"], "fully_replayable")

    def test_replay_liveness_minimal_fixture(self):
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        result = process.replay(self.binary, session, self.root, "http://127.0.0.1:9")
        self.assertEqual(result.returncode, 0)
        report = json.loads(result.stdout.decode())
        self.assertEqual(report["result"]["outcome"], "dry_run")


if __name__ == "__main__":
    unittest.main()
