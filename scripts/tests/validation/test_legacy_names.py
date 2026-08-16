#!/usr/bin/env python3
"""Tests for the legacy migration-baggage guard in validation.py."""

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_spec = importlib.util.spec_from_file_location(
    "chronicle_validation", ROOT / "scripts/validation.py"
)
assert _spec is not None and _spec.loader is not None
validation = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(validation)


class LegacyNamesCheckTests(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-legacy-names-"))
        (self.temp / "crates").mkdir(parents=True)

    def tearDown(self):
        shutil.rmtree(self.temp)

    def issues_for(self, content: str) -> list[str]:
        path = self.temp / "crates" / "chronicle-x" / "src" / "lib.rs"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        return validation.legacy_names_check(self.temp)["issues"]

    def test_clean_crate_has_no_issues(self):
        self.assertEqual(
            self.issues_for("pub struct EpochCatalog { pub version: u16 }\n"), []
        )

    def test_removed_legacy_type_is_rejected(self):
        issues = self.issues_for("pub struct EpochCatalogV1 { pub version: u16 }\n")
        self.assertEqual(len(issues), 1)
        self.assertIn("EpochCatalogV1", issues[0])

    def test_removed_adapter_names_are_rejected(self):
        for name in [
            "RolloverTransitionV1",
            "IncrementalEtlCheckpointV1",
            "ContinuationDependencyV2",
            "load_transition_v2",
            "record_live_ebpf",
            "from_legacy",
        ]:
            self.assertEqual(len(self.issues_for(f"fn {name}() {{}}\n")), 1, name)

    def test_intentional_wal_boundary_adapter_is_allowed(self):
        # from_legacy_uuid is the intentional WAL v1 identity bridge; the
        # guard must match whole identifiers only.
        self.assertEqual(self.issues_for("fn from_legacy_uuid() {}\n"), [])


if __name__ == "__main__":
    unittest.main()
