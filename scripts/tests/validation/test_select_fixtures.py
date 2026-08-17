#!/usr/bin/env python3
"""Selection fixture matrix (task 9.1).

Proves scripts/validation.py select() picks the expected smallest valid set
per changed-path category and treats unknown paths conservatively (full
validation). Also verifies the catalog-aware report carries functional layer,
environment, owner, and no-coverage-loss markers.
"""

import importlib.util
import pathlib
import tomllib
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
_SPEC = importlib.util.spec_from_file_location(
    "validation_select_mod", ROOT / "scripts" / "validation.py"
)
assert _SPEC is not None and _SPEC.loader is not None
VALIDATION = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(VALIDATION)

CONFIG = tomllib.loads((ROOT / "validation" / "groups.toml").read_text())
ALL_GROUPS = sorted(CONFIG["groups"])

FIXTURES = {
    "docs_only": (["docs/architecture/overview.md"], ["cli_docs"]),
    "etl_only": (["crates/chronicle-etl/src/publication.rs"], ["etl", "portable"]),
    "wal_only": (["crates/chronicle-wal/src/lib.rs"], ["portable", "wal"]),
    "ebpf": (["ebpf/main.bpf.c"], ["ebpf"]),
    "cli": (["crates/chronicle-cli/src/main.rs"], ["cli_docs", "portable"]),
    "helper": (["tests/support/process.py"], ["portable"]),
    "acceptance": (
        ["scripts/acceptance/lib/scenarios/recorder/quota-pressure.sh"],
        ["acceptance", "build_tooling"],
    ),
    "e2e": (["tests/e2e/test_rootless_pipeline.py"], ["portable"]),
    "privileged": (
        ["scripts/acceptance/lib/scenarios/live-capture/capture-basic.sh"],
        ["acceptance", "build_tooling"],
    ),
    "unknown": (["mystery/path.txt"], ALL_GROUPS),
}


class SelectFixtureTests(unittest.TestCase):
    def test_each_fixture_selects_expected_smallest_set(self):
        for name, (paths, expected) in FIXTURES.items():
            with self.subTest(fixture=name):
                result = VALIDATION.select(ROOT, paths, CONFIG)
                self.assertEqual(result["selected"], expected, name)
                if name == "unknown":
                    self.assertEqual(result["unknown_paths"], paths)
                else:
                    self.assertEqual(result["unknown_paths"], [], name)

    def test_conservative_unknown_is_full_validation(self):
        result = VALIDATION.select(ROOT, ["mystery/path.txt"], CONFIG)
        self.assertEqual(result["selected"], ALL_GROUPS)
        for group in ALL_GROUPS:
            self.assertIn(
                "conservative full validation", result["decisions"][group]["reason"]
            )

    def test_ebpf_only_does_not_pull_portable(self):
        result = VALIDATION.select(ROOT, ["ebpf/main.bpf.c"], CONFIG)
        self.assertNotIn("portable", result["selected"])

    def test_report_carries_layer_environment_owner_and_coverage(self):
        result = VALIDATION.select(ROOT, ["docs/architecture/overview.md"], CONFIG)
        self.assertIn("layers_covered", result)
        self.assertIn("environments_covered", result)
        self.assertIn("owners_covered", result)
        self.assertIn("coverage", result)
        self.assertIn("catalog obligations verified", result["coverage"])
        cli_docs = result["decisions"]["cli_docs"]
        self.assertTrue(cli_docs["tests"], "selected group must list catalog tests")
        sample = cli_docs["tests"][0]
        for key in ("id", "layer", "environment", "owner"):
            self.assertIn(key, sample, key)

    def test_unknown_fixture_reports_full_coverage(self):
        result = VALIDATION.select(ROOT, ["mystery/path.txt"], CONFIG)
        self.assertEqual(
            set(result["environments_covered"]), {"portable", "privileged"}
        )
        self.assertIn("unit", result["layers_covered"])
        self.assertIn("acceptance", result["layers_covered"])


if __name__ == "__main__":
    unittest.main()
