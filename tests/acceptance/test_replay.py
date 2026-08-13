#!/usr/bin/env python3
"""Acceptance: the replay feature contract through the public CLI.

Black-box: dry-run default, execution gating (--allow-read), verification
against a loopback target, and mismatch exit. Uses the shared HTTP driver as
the replay target with condition-based readiness.

Run: python3 tests/acceptance/test_replay.py
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


class ReplayAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("accept-replay")
        self.driver = None

    def tearDown(self):
        if self.driver is not None:
            self.driver.stop()
        shutil.rmtree(self.root, ignore_errors=True)

    def record(self) -> str:
        return process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)

    def test_replay_defaults_to_dry_run(self):
        session = self.record()
        result = process.replay(self.binary, session, self.root, "http://127.0.0.1:9")
        self.assertEqual(result.returncode, 0)
        report = json.loads(result.stdout.decode())
        self.assertEqual(report["result"]["outcome"], "dry_run")

    def test_replay_execute_requires_read_authorization(self):
        session = self.record()
        result = process.replay(
            self.binary, session, self.root, "http://127.0.0.1:9", execute=True
        )
        self.assertEqual(result.returncode, 4)
        self.assertIn(b"Hint", result.stderr)

    def test_replay_execute_verifies_against_loopback_target(self):
        session = self.record()
        self.driver = process.start_driver(self.root)
        result = process.replay(
            self.binary,
            session,
            self.root,
            self.driver.target,
            execute=True,
            allow_read=True,
        )
        self.assertEqual(result.returncode, 0)
        report = json.loads(result.stdout.decode())
        self.assertEqual(report["version"], 1)
        completed = [
            op for op in report["result"]["operations"] if op["state"] == "completed"
        ]
        self.assertTrue(completed)
        self.assertTrue(
            all(op["verification"] == "passed" for op in completed),
            f"verification failed: {report}",
        )

    def test_replay_mismatch_returns_dedicated_exit(self):
        session = self.record()
        self.driver = process.start_driver(self.root, mismatch=True)
        result = process.replay(
            self.binary,
            session,
            self.root,
            self.driver.target,
            execute=True,
            allow_read=True,
        )
        self.assertEqual(result.returncode, 6)


if __name__ == "__main__":
    unittest.main()
