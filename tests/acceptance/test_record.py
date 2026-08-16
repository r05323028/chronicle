#!/usr/bin/env python3
"""Acceptance: the record feature contract through the public CLI.

Black-box: only the binary, filesystem/storage output, and exit codes.
Deterministic fixture input stands in for capture on rootless hosts; the
unsupported-build failure contract is also proven here (real capture is
privileged on supported Linux).

Run: python3 tests/acceptance/test_record.py
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


class RecordAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("accept-record")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_fixture_record_produces_published_session_and_wal(self):
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        session_dir = self.root / "sessions" / session
        self.assertTrue((session_dir / "session.json").is_file())
        self.assertTrue((session_dir / "manifest.json").is_file())
        segments = list(
            (self.root / "recordings" / session / "segments").glob("*.chwal")
        )
        self.assertTrue(segments, "fixture record must persist WAL segments")

    def test_malformed_fixture_returns_safe_error(self):
        malformed = self.root / "malformed.json"
        malformed.write_text("{}", encoding="utf-8")
        result = process.run(
            self.binary,
            [
                "internal",
                "record-fixture",
                "--input",
                str(malformed),
                "--root",
                str(self.root),
            ],
            timeout=15,
        )
        self.assertEqual(result.returncode, 3)
        payload = json.loads(result.stderr.decode())
        self.assertEqual(payload["code"], 3)
        self.assertTrue(payload["message"])

    def test_public_record_fails_before_target_on_unsupported_build(self):
        marker = self.root / "target-started"
        process.require_rootless_record_failure(self.binary, self.root, marker)


if __name__ == "__main__":
    unittest.main()
