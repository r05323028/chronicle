#!/usr/bin/env python3
"""Acceptance: the ETL command fail-closed contract through the CLI.

The internal ETL surface canonicalizes a production WAL; the CLI-only fixture
path cannot produce a metadata-bearing WAL (recording identity is written by
the real recorder), so this suite proves the ETL command's rootless fail-closed
contracts: missing metadata and identity mismatch are rejected before any
processing. The exhaustive ETL idempotence/corruption matrix is owned by
chronicle-etl integration tests (migration task 3.3).

Run: python3 tests/acceptance/test_etl.py
"""

from __future__ import annotations

import importlib.util
import json
import shutil
import unittest
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_spec = importlib.util.spec_from_file_location(
    "chronicle_test_support", ROOT / "tests/support/process.py"
)
assert _spec is not None and _spec.loader is not None
process = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(process)


class EtlAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("accept-etl")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def fixture_wal(self) -> Path:
        session = process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)
        return self.root / "wal" / session

    def etl_failure(self, wal: Path, output: Path) -> dict:
        result = process.run(
            self.binary,
            ["internal", "etl", "--wal-dir", str(wal), "--output", str(output)],
            timeout=15,
        )
        self.assertEqual(result.returncode, 3)
        self.assertEqual(result.stdout, b"")
        return json.loads(result.stderr.decode())

    def test_etl_rejects_wal_without_recording_metadata(self):
        wal = self.fixture_wal()
        payload = self.etl_failure(wal, self.root / "out")
        self.assertEqual(payload["code"], 3)
        self.assertIn("recording metadata", payload["message"])

    def test_etl_rejects_identity_mismatch(self):
        wal = self.fixture_wal()
        process.write_recording_metadata(wal, str(uuid.uuid4()))
        payload = self.etl_failure(wal, self.root / "out")
        self.assertEqual(payload["code"], 3)
        self.assertIn("identity mismatch", payload["message"])


if __name__ == "__main__":
    unittest.main()
