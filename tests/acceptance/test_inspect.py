#!/usr/bin/env python3
"""Acceptance: the inspect feature contract through the public CLI.

Black-box: only the binary and JSON output; asserts user-visible information
(endpoints, operations, replayability, integrity) without payload leakage and
without reaching into crate internals.

Run: python3 tests/acceptance/test_inspect.py
"""

from __future__ import annotations

import importlib.util
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


class InspectAcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("accept-inspect")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def record(self) -> str:
        return process.record_fixture(self.binary, process.FIXTURE_BASIC, self.root)

    def test_inspect_displays_expected_user_visible_information(self):
        session = self.record()
        value = process.inspect_session(self.binary, session, self.root)
        self.assertEqual(value["version"], 1)
        self.assertEqual(value["session_id"], session)
        self.assertEqual(value["replayability"], "fully_replayable")
        self.assertIs(value["integrity_valid"], True)
        self.assertIs(value["complete"], True)
        connection = value["connections"][0]
        self.assertEqual(connection["client"]["host"], "192.0.2.10")
        self.assertEqual(connection["client"]["port"], 41000)
        self.assertEqual(connection["server"]["host"], "192.0.2.20")
        self.assertEqual(connection["server"]["port"], 8080)
        operation = connection["operations"][0]
        self.assertEqual(operation["method"], "GET")
        self.assertEqual(operation["target"], "/hello")
        self.assertEqual(operation["response_status"], "200")

    def test_inspect_does_not_leak_payload_body(self):
        session = self.record()
        result = process.run(
            self.binary,
            ["inspect", session],
            data_dir=self.root,
        )
        self.assertEqual(result.returncode, 0)
        self.assertNotIn('"OK"', result.stdout.decode())


if __name__ == "__main__":
    unittest.main()
