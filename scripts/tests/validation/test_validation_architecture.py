#!/usr/bin/env python3
"""Standard-library tests for the responsibility-based test architecture.

Covers the classified test catalog and gate/selector semantics (live-capture/recorder as
selectors, not categories; privilege as an environment dimension; no-coverage-
loss obligations), static prevention of portable Cargo/repository commands
embedded in privileged orchestration, and dev-edge architecture checks for
shared test support.
"""

from __future__ import annotations

import importlib.util
import re
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
_spec = importlib.util.spec_from_file_location(
    "chronicle_validation", ROOT / "scripts/validation.py"
)
assert _spec is not None and _spec.loader is not None
validation = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = validation
_spec.loader.exec_module(validation)

MINIMAL_CATALOG = """version = 1
layers = ["unit", "integration", "smoke", "acceptance", "e2e"]
environments = ["portable", "privileged"]
selectors = ["live-capture", "recorder", "release"]

[required.live-capture]
tests = ["unit:a"]

[required.recorder]
tests = ["unit:a", "integration:b"]

[required.release]
tests = ["unit:a", "integration:b"]

[[test]]
id = "unit:a"
layer = "unit"
environment = "portable"
owner = "chronicle-a"
command = "cargo test -p chronicle-a"
status = "existing"
gates = ["live-capture", "recorder", "release"]

[[test]]
id = "integration:b"
layer = "integration"
environment = "portable"
owner = "chronicle-b"
command = "cargo test -p chronicle-b"
status = "existing"
gates = ["recorder", "release"]

[scenario_tests]
s1 = ["unit:a"]

[legacy_check_tests]
c1 = ["unit:a"]
"""

MINIMAL_SCENARIOS = """[scenarios.s1]
capabilities = []
legacy_live_capture_checks = ["c1"]
legacy_recorder_checks = ["c1"]
"""

# Embedded-command detection: an optional `if`/`then`, an optional
# `run_compat_command <name>` prefix, and leading VAR=... assignments are
# stripped before matching a cargo command.
EMBEDDED_CARGO_COMMAND = re.compile(
    r"^(?:(?:if|then)\s+)?(?:run_compat_command\s+\S+\s+)?"
    r"(?:[A-Za-z_][A-Za-z0-9_]*=\S+\s+)*cargo\b"
)


def portable_repo_commands(
    directory: Path, allowlist: set[str]
) -> list[tuple[str, int, str]]:
    """Return non-allowlisted portable Cargo/repository commands in shell files."""
    findings: list[tuple[str, int, str]] = []
    for path in sorted(directory.rglob("*.sh")):
        for number, line in enumerate(path.read_text(errors="replace").splitlines(), 1):
            stripped = line.strip()
            if not stripped or stripped.startswith(("#", "echo", "printf")):
                continue
            if EMBEDDED_CARGO_COMMAND.match(stripped) and stripped not in allowlist:
                findings.append(
                    (path.relative_to(directory).as_posix(), number, stripped)
                )
    return findings


# Privileged scenario orchestration may invoke cargo only for genuinely
# privileged work: building release/eBPF artifacts and executing the
# `--ignored` privileged test suites (production-object kernel acceptance,
# signal handling). Deny-by-default: any other embedded cargo invocation fails
# the guard below until it is added here with a privileged justification.
PRIVILEGED_EMBEDDED_CARGO_ALLOWLIST = {
    "cargo build --release --locked",
    'CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" cargo build --release -p chronicle-cli --locked >>"$ARTIFACT_ROOT/build.log" 2>&1',
    'CHRONICLE_EBPF_TARGET_DIR="$EBPF_TARGET_DIR" cargo build -p chronicle-cli --locked',
    'cargo +nightly build -Z build-std=core --manifest-path "$ROOT/ebpf/Cargo.toml" --target bpfel-unknown-none --release --locked',
    'cargo test -p chronicle-capture-ebpf --test privileged_kernel --locked -- --ignored --nocapture >"$ARTIFACT_ROOT/privileged-kernel.log" 2>&1',
    'cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture >"$ARTIFACT_ROOT/signal-tests.log" 2>&1',
    "if run_compat_command signal cargo test -p chronicle-cli --all-features --locked --test privileged_signal -- --ignored --nocapture; then",
    "run_compat_command kernel-acceptance cargo test -p chronicle-capture-ebpf --test privileged_kernel --locked -- --ignored --nocapture || set_check privileged_kernel failed",
}


class CatalogSemanticsTests(unittest.TestCase):
    """Task 2.2: selectors, orthogonality, and no-coverage-loss obligations."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-catalog-"))

    def tearDown(self):
        shutil.rmtree(self.temp)

    def check(self, catalog_text: str, scenarios_text: str = MINIMAL_SCENARIOS) -> dict:
        catalog_path = self.temp / "catalog.toml"
        scenarios_path = self.temp / "scenarios.toml"
        catalog_path.write_text(catalog_text, encoding="utf-8")
        scenarios_path.write_text(scenarios_text, encoding="utf-8")
        return validation.catalog_check(self.temp, catalog_path, scenarios_path)

    def issues(self, catalog_text: str) -> list[str]:
        return self.check(catalog_text)["issues"]

    def test_minimal_catalog_valid(self):
        self.assertEqual(self.issues(MINIMAL_CATALOG), [])

    def test_duplicate_id_fails(self):
        broken = (
            MINIMAL_CATALOG.replace(
                'gates = ["live-capture", "recorder", "release"]\n',
                'gates = ["live-capture", "recorder", "release"]\n',
                1,
            )
            + '\n[[test]]\nid = "unit:a"\nlayer = "unit"\nenvironment = "portable"\nowner = "x"\ncommand = "cargo test"\nstatus = "existing"\ngates = ["live-capture"]\n'
        )
        self.assertIn("duplicate test id: unit:a", self.issues(broken))

    def test_unknown_required_test_fails(self):
        broken = MINIMAL_CATALOG.replace(
            'tests = ["unit:a"]', 'tests = ["unit:a", "unit:missing"]', 1
        )
        self.assertIn(
            "selector live-capture: required test unit:missing is undefined",
            self.issues(broken),
        )

    def test_missing_layer_fails(self):
        broken = MINIMAL_CATALOG.replace(
            'id = "unit:a"\nlayer = "unit"', 'id = "unit:a"\n'
        )
        issues = self.issues(broken)
        self.assertTrue(any("test unit:a: missing layer" in i for i in issues))

    def test_invalid_environment_fails(self):
        broken = MINIMAL_CATALOG.replace(
            'environment = "portable"', 'environment = "hyper"', 1
        )
        self.assertTrue(any("invalid environment" in i for i in self.issues(broken)))

    def test_undefined_command_fails(self):
        broken = MINIMAL_CATALOG.replace('command = "cargo test -p chronicle-a"\n', "")
        self.assertTrue(
            any("test unit:a: missing command" in i for i in self.issues(broken))
        )

    def test_selector_membership_inconsistent_fails(self):
        broken = MINIMAL_CATALOG.replace(
            'gates = ["live-capture", "recorder", "release"]',
            'gates = ["recorder", "release"]',
            1,
        )
        self.assertIn(
            "selector live-capture: required test unit:a does not declare gate live-capture",
            self.issues(broken),
        )

    def test_p2_not_superset_of_p1_fails(self):
        broken = MINIMAL_CATALOG.replace(
            'tests = ["unit:a", "integration:b"]', 'tests = ["integration:b"]', 1
        )
        self.assertIn(
            "selector live-capture required coverage missing from superset: ['unit:a']",
            self.issues(broken),
        )

    def test_uncovered_scenario_fails_with_scenario_id(self):
        broken = MINIMAL_CATALOG.replace('s1 = ["unit:a"]', "")
        self.assertIn(
            "scenario(s) without classified coverage: ['s1']",
            self.issues(broken),
        )

    def test_unmapped_legacy_check_fails_with_check_id(self):
        broken = MINIMAL_CATALOG.replace('c1 = ["unit:a"]', "")
        self.assertIn(
            "legacy check(s) without classified coverage: ['c1']",
            self.issues(broken),
        )

    def test_unknown_scenario_reference_fails(self):
        broken = MINIMAL_CATALOG.replace('s1 = ["unit:a"]', 's2 = ["unit:a"]')
        self.assertIn(
            "scenario_tests references unknown scenario 's2'", self.issues(broken)
        )

    def test_privilege_is_environment_dimension_not_layer(self):
        catalog = MINIMAL_CATALOG.replace(
            'id = "unit:a"\nlayer = "unit"\nenvironment = "portable"',
            'id = "unit:a"\nlayer = "acceptance"\nenvironment = "privileged"',
        )
        self.assertEqual(self.issues(catalog), [])

    def test_gate_is_not_a_layer(self):
        broken = MINIMAL_CATALOG.replace(
            'id = "unit:a"\nlayer = "unit"', 'id = "unit:a"\nlayer = "live-capture"'
        )
        self.assertTrue(any("invalid layer" in i for i in self.issues(broken)))

    def test_real_catalog_and_scenarios_valid(self):
        value = validation.catalog_check(ROOT)
        self.assertEqual(value["issues"], [])
        self.assertEqual(value["scenarios"], 14)
        self.assertGreater(value["recorder_required"], value["live_capture_required"])

    def test_planned_required_is_reported_not_silent(self):
        catalog = MINIMAL_CATALOG.replace(
            'status = "existing"\ngates = ["live-capture", "recorder", "release"]\n',
            'status = "planned"\ngates = ["live-capture", "recorder", "release"]\n',
            1,
        )
        value = self.check(catalog)
        self.assertEqual(value["issues"], [])
        self.assertIn("unit:a", value["planned_required"])

    def test_required_planned_test_deletion_still_fails(self):
        catalog = MINIMAL_CATALOG.replace(
            'status = "existing"\ngates = ["live-capture", "recorder", "release"]\n',
            'status = "planned"\ngates = ["live-capture", "recorder", "release"]\n',
            1,
        ).replace('[[test]]\nid = "unit:a"\n', '[[test]]\nid = "unit:renamed"\n', 1)
        self.assertIn(
            "selector live-capture: required test unit:a is undefined",
            self.issues(catalog),
        )


class OrchestrationEmbeddingTests(unittest.TestCase):
    """No portable Cargo/repository commands inside privileged orchestration
    beyond the documented privileged-only allowlist."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-orchestration-"))

    def tearDown(self):
        shutil.rmtree(self.temp)

    def test_documented_privileged_commands_are_not_flagged(self):
        findings = portable_repo_commands(
            ROOT / "scripts/acceptance/lib/scenarios",
            PRIVILEGED_EMBEDDED_CARGO_ALLOWLIST,
        )
        self.assertEqual(findings, [])

    def test_scanner_matches_privileged_allowlist(self):
        found = set()
        for path in sorted((ROOT / "scripts/acceptance/lib/scenarios").rglob("*.sh")):
            for line in path.read_text(errors="replace").splitlines():
                stripped = line.strip()
                if not stripped or stripped.startswith(("#", "echo", "printf")):
                    continue
                if EMBEDDED_CARGO_COMMAND.match(stripped):
                    found.add(stripped)
        self.assertEqual(found, PRIVILEGED_EMBEDDED_CARGO_ALLOWLIST)

    def test_new_embedded_portable_command_fails(self):
        scenario = self.temp / "s.sh"
        scenario.write_text(
            "#!/usr/bin/env bash\n\nphase 1 'x'\ncargo test -p chronicle-wal\n",
            encoding="utf-8",
        )
        findings = portable_repo_commands(self.temp, set())
        self.assertEqual(len(findings), 1)
        self.assertEqual(findings[0][2], "cargo test -p chronicle-wal")

    def test_allowlisted_command_is_not_flagged(self):
        scenario = self.temp / "s.sh"
        scenario.write_text(
            "#!/usr/bin/env bash\ncargo test -p chronicle-wal\n",
            encoding="utf-8",
        )
        allowlist = {"cargo test -p chronicle-wal"}
        self.assertEqual(portable_repo_commands(self.temp, allowlist), [])

    def test_fixture_repo_check_in_orchestrator_fails(self):
        runner = self.temp / "orchestrator.sh"
        runner.write_text(
            "#!/usr/bin/env bash\ncargo check --workspace --all-targets --locked\n",
            encoding="utf-8",
        )
        findings = portable_repo_commands(self.temp, set())
        self.assertEqual(len(findings), 1)
        self.assertIn("orchestrator.sh", findings[0][0])


class TestSupportArchitectureTests(unittest.TestCase):
    """Task 2.4: shared test support cannot bypass workspace dependency policy."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-testsupport-"))
        (self.temp / "validation").mkdir()
        shutil.copy2(
            ROOT / "validation/architecture.toml",
            self.temp / "validation/architecture.toml",
        )
        self.policy = self.temp / "validation/architecture.toml"

    def tearDown(self):
        shutil.rmtree(self.temp)

    def write_workspace(self, crates: dict[str, dict]) -> None:
        (self.temp / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n',
            encoding="utf-8",
        )
        for name, deps in crates.items():
            crate_dir = self.temp / "crates" / name.removeprefix("chronicle-")
            crate_dir.mkdir(parents=True, exist_ok=True)
            lines = [f'[package]\nname = "{name}"\nversion = "0.0.0"\n']
            for kind in ("dependencies", "dev-dependencies"):
                selected = deps.get(kind)
                if not selected:
                    continue
                lines.append(f"[{kind}]\n")
                for package, target in selected.items():
                    target_name = target or package
                    lines.append(
                        f"{target_name} = {{ path = \"../{package.removeprefix('chronicle-')}\" }}\n"
                    )
            (crate_dir / "Cargo.toml").write_text("".join(lines), encoding="utf-8")
            (crate_dir / "src").mkdir()
            (crate_dir / "src/lib.rs").write_text("pub fn x() {}\n", encoding="utf-8")

    def issues_for(self, crates: dict[str, dict]) -> list[str]:
        self.write_workspace(crates)
        return validation.architecture_check(self.temp, self.policy)["issues"]

    def test_cli_dev_dependency_on_lower_crate_rejected(self):
        issues = self.issues_for(
            {
                "chronicle-common": {"dependencies": {}},
                "chronicle-wal": {
                    "dependencies": {
                        "chronicle-capture": None,
                        "chronicle-common": None,
                    }
                },
                "chronicle-capture": {"dependencies": {"chronicle-common": None}},
                "chronicle-application": {
                    "dependencies": {"chronicle-wal": None, "chronicle-common": None}
                },
                "chronicle-cli": {
                    "dependencies": {"chronicle-application": None},
                    "dev-dependencies": {"chronicle-wal": None},
                },
            }
        )
        self.assertTrue(
            any(
                "forbidden cli non-application edge: chronicle-cli -> chronicle-wal [dev]"
                in issue
                for issue in issues
            )
        )

    def test_session_dev_dependency_on_wal_rejected(self):
        issues = self.issues_for(
            {
                "chronicle-common": {"dependencies": {}},
                "chronicle-capture": {"dependencies": {"chronicle-common": None}},
                "chronicle-wal": {
                    "dependencies": {
                        "chronicle-capture": None,
                        "chronicle-common": None,
                    }
                },
                "chronicle-session": {
                    "dependencies": {
                        "chronicle-capture": None,
                        "chronicle-common": None,
                    },
                    "dev-dependencies": {"chronicle-wal": None},
                },
            }
        )
        self.assertTrue(any("session-to-wal coupling" in issue for issue in issues))

    def test_real_workspace_architecture_passes(self):
        value = validation.architecture_check(
            ROOT, ROOT / "validation/architecture.toml"
        )
        self.assertEqual(value["issues"], [])
        self.assertGreater(value["normal_edges"], 0)


if __name__ == "__main__":
    unittest.main()
