#!/usr/bin/env python3
"""Documentation-command contract: the README's public command forms are real.

Black-box and rootless. Executes the documented public surface against the
built binary -- version, help, doctor, list, record preflight -- and asserts
that README relative links resolve. Never matches README prose; it executes
commands and asserts contracts. Live-capture command forms that require
privileged Linux are proven by the privileged quick-start acceptance
scenario, not here.

Run: python3 tests/smoke/test_documented_commands.py
"""

from __future__ import annotations

import importlib.util
import json
import re
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

PUBLIC_COMMANDS = ("record", "replay", "list", "inspect", "doctor")


def workspace_version() -> str:
    manifest = (ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r"^version = \"([^\"]+)\"", manifest, re.MULTILINE)
    assert match is not None, "workspace version missing from Cargo.toml"
    return match.group(1)


class DocumentedCommandsTests(unittest.TestCase):
    @staticmethod
    def parse_json(raw: bytes, context: str) -> dict:
        try:
            return json.loads(raw.decode())
        except (ValueError, UnicodeDecodeError) as exc:
            raise AssertionError(
                f"{context}: malformed JSON output: {raw!r:.200}"
            ) from exc

    def setUp(self):
        self.binary = process.find_binary()
        self.root = process.temp_root("doc-commands")

    def tearDown(self):
        shutil.rmtree(self.root, ignore_errors=True)

    def test_version_reports_workspace_version(self):
        result = process.run(self.binary, ["--version"], format_json=False, timeout=15)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            result.stdout.decode().strip(), f"chronicle {workspace_version()}"
        )

    def test_help_lists_public_commands_and_hides_internal_mechanics(self):
        result = process.run(self.binary, ["--help"], format_json=False, timeout=15)
        self.assertEqual(result.returncode, 0)
        text = result.stdout.decode()
        for command in PUBLIC_COMMANDS:
            self.assertIn(command, text)
        self.assertNotIn("--wal-dir", text)
        self.assertNotIn("--segment-bytes", text)

    def test_record_help_hides_legacy_flags(self):
        result = process.run(
            self.binary, ["record", "--help"], format_json=False, timeout=15
        )
        self.assertEqual(result.returncode, 0)
        text = result.stdout.decode()
        # --root appears only as prose explaining that --data-dir is not a
        # legacy alias; it must not be an actual flag or subcommand here.
        for legacy in ("--source", "--wal-dir", "--segment-bytes"):
            self.assertNotIn(legacy, text)
        self.assertNotIn("Usage: chronicle record --root", text)

    def test_doctor_is_non_destructive_with_actionable_remediation(self):
        data_dir = self.root / "doctor-data"
        result = process.run(self.binary, ["doctor"], data_dir=data_dir)
        # Doctor exits 0 when every required probe is supported, 4 when a
        # required probe is unsupported or not_checked (as on builds without
        # live capture). Either way the report is one atomic JSON object.
        self.assertIn(result.returncode, (0, 4))
        payload = self.parse_json(result.stdout, "doctor")
        self.assertEqual(payload["version"], 1)
        probes = payload["probes"]
        self.assertIsInstance(probes, list)
        self.assertTrue(probes, "doctor must report at least one probe")
        capture = next(
            (probe for probe in probes if probe["code"] == "capture.platform"), None
        )
        self.assertIsNotNone(capture, "doctor must report capture.platform")
        assert capture is not None
        self.assertTrue(capture["required"])
        if capture["status"] == "unsupported":
            self.assertTrue(
                capture["remediation"],
                "unsupported required probes need actionable remediation",
            )
        for probe in probes:
            if probe["required"] and probe["status"] == "unsupported":
                self.assertTrue(probe["remediation"])
        self.assertFalse(
            data_dir.exists(),
            "doctor must not create the data directory (non-destructive)",
        )

    def test_public_record_preflight_fails_before_mutation(self):
        data_dir = self.root / "record-data"
        marker = self.root / "target-started"
        result = process.run(
            self.binary,
            ["record", "--name", "checkout", "--", "/usr/bin/touch", str(marker)],
            data_dir=data_dir,
        )
        self.assertEqual(result.returncode, 4)
        self.assertEqual(result.stdout, b"")
        payload = self.parse_json(result.stderr, "record preflight")
        self.assertEqual(payload["code"], 4)
        self.assertFalse(marker.exists(), "record must fail before starting the target")
        self.assertFalse((data_dir / "catalog.json").exists())
        self.assertFalse((data_dir / "recordings").exists())
        self.assertFalse((data_dir / "sessions").exists())

    def test_list_empty_contract(self):
        payload = process.run_json(
            self.binary, ["list"], data_dir=self.root / "list-data"
        )
        self.assertEqual(payload["version"], 1)
        self.assertEqual(payload["recordings"], [])

    def test_readme_relative_links_resolve(self):
        readme = (ROOT / "README.md").read_text(encoding="utf-8")
        links = re.findall(r"\[[^]]*\]\(([^)]+)\)", readme)
        relative = [
            link
            for link in links
            if not link.startswith(("#", "http://", "https://", "mailto:"))
        ]
        self.assertTrue(relative, "README must contain relative links")
        for link in relative:
            target = (ROOT / link.lstrip("./")).resolve()
            self.assertTrue(
                target.is_file(),
                f"README link does not resolve: {link!r} -> {target}",
            )


if __name__ == "__main__":
    unittest.main()
