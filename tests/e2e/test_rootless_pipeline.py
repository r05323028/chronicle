#!/usr/bin/env python3
"""Rootless end-to-end: complete Chronicle system path through the binary.

synthetic capture fixture
        -> record (WAL + ETL + canonical publication)
        -> inspect (canonical consumable, identity continuity)
        -> replay dry-run (plan consumable)
        -> replay execute (operations execute, verification succeeds)

Asserts only cross-stage invariants: identity/correlation continuity,
operation survival, stage consumability, and successful verification. No
exhaustive WAL/ETL/protocol/replay matrices (those stay in lower layers).

Run: python3 tests/e2e/test_rootless_pipeline.py
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


class RootlessPipelineTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("e2e")
        self.driver = None

    def tearDown(self):
        if self.driver is not None:
            self.driver.stop()
        shutil.rmtree(self.root, ignore_errors=True)

    def test_synthetic_capture_to_replay_verification(self):
        # Stage 1: synthetic capture -> WAL + canonical publication.
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        segments = list((self.root / "wal" / session / "segments").glob("*.chwal"))
        self.assertTrue(segments, "capture must persist WAL segments")
        session_json = self.root / "sessions" / session / "session.json"
        self.assertTrue(session_json.is_file(), "ETL must publish canonical session")

        # Stage 2: canonical output is consumable by inspect with identity
        # continuity across record -> inspect.
        inspection = process.inspect_session(self.binary, session, self.root)
        self.assertEqual(inspection["session_id"], session)
        self.assertEqual(inspection["replayability"], "fully_replayable")
        self.assertIs(inspection["integrity_valid"], True)
        operations = [
            op
            for connection in inspection["connections"]
            for op in connection["operations"]
        ]
        self.assertGreaterEqual(len(operations), 1)

        # Stage 3: replay plan is consumable (dry-run default).
        dry = process.replay(self.binary, session, self.root, "http://127.0.0.1:9")
        self.assertEqual(dry.returncode, 0)
        self.assertEqual(
            json.loads(dry.stdout.decode())["result"]["outcome"], "dry_run"
        )

        # Stage 4: replay executes the recorded operations and verifies them.
        self.driver = process.start_driver(self.root)
        executed = process.replay(
            self.binary,
            session,
            self.root,
            self.driver.target,
            execute=True,
            allow_read=True,
        )
        self.assertEqual(executed.returncode, 0)
        report = json.loads(executed.stdout.decode())
        completed = [
            op for op in report["result"]["operations"] if op["state"] == "completed"
        ]
        self.assertTrue(completed, "replay must execute recorded operations")
        self.assertTrue(
            all(op["verification"] == "passed" for op in completed),
            f"verification must succeed: {report}",
        )
        # Operation survival: recorded operations all reached replay.
        self.assertEqual(len(completed), len(operations))


if __name__ == "__main__":
    unittest.main()
