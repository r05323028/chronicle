#!/usr/bin/env python3
import importlib.util
import json
import shutil
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module

report = load("acceptance_report", ROOT / "scripts/acceptance/report.py")
validation = load("chronicle_validation_runner", ROOT / "scripts/validation.py")
runner = load("acceptance_runner", ROOT / "scripts/acceptance/runner.py")


class AcceptanceRunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-acceptance-"))
        self.definition = report.load_scenarios(ROOT / "scripts/acceptance/scenarios.toml")
        self.environment = {
            "executor": "local",
            "os": "Linux",
            "ubuntu": "24.04",
            "architecture": "aarch64",
            "kernel": "6.8.0",
            "rustc": "rustc test",
            "target": "host",
            "btf": True,
            "cgroup_v2": True,
            "capabilities": {"CapEff": "0001"},
        }
        self.source = {
            "commit_sha": "commit-a",
            "tree_sha": "tree-a",
            "fingerprint": "fp-a",
            "working_tree_dirty": False,
        }

    def tearDown(self):
        shutil.rmtree(self.temp, ignore_errors=True)

    def _read_report(self, path):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            self.fail(f"cannot read report {path}: {exc}")

    def test_p2_includes_p1(self):
        p1 = set(report.profile_scenarios(self.definition, "p1"))
        p2 = set(report.profile_scenarios(self.definition, "p2"))
        self.assertTrue(p1 < p2)

    def test_parser_selects_executor_and_no_reuse(self):
        args = runner.parser().parse_args(["--profile", "p2", "--executor", "multipass", "--no-reuse"])
        self.assertEqual(args.executor, "multipass")
        self.assertTrue(args.no_reuse)

    def test_multipass_environment_timeout_does_not_fallback_to_host(self):
        with mock.patch.object(
            runner.subprocess,
            "check_output",
            side_effect=runner.subprocess.TimeoutExpired("multipass", 60),
        ):
            self.assertIsNone(runner.multipass_environment("chronicle-ubuntu"))
        with mock.patch.object(runner, "multipass_environment", return_value=None):
            with self.assertRaises(RuntimeError):
                runner.collect_environment("multipass", "chronicle-ubuntu")

    def test_dirty_source_allowed_development_rejected_release(self):
        dirty = {**self.source, "working_tree_dirty": True}
        self.assertEqual(runner.release_source_errors(dirty), ["working tree is dirty"])
        self.assertEqual(runner.release_source_errors(self.source), [])

    def test_report_completeness_and_release_identity(self):
        selected = report.profile_scenarios(self.definition, "p1")
        value = report.make_report(
            run_id="run-a", profile="p1", executor="local", selected=selected,
            completed=selected, failed=[], skipped=[], not_checked=[],
            phases=[{"name": "run", "status": "passed"}], source=self.source,
            environment_value=self.environment, return_code=0, source_stable=True,
        )
        self.assertEqual(value["status"], "passed")
        self.assertTrue(value["release_eligible"])
        self.assertEqual(value["not_checked_scenarios"], [])

    def _write_evidence(self, profile, selected, completed, environment=None, source=None):
        root = self.temp / "evidence" / profile / "fp-a" / "run-a"
        root.mkdir(parents=True)
        value = report.make_report(
            run_id="run-a", profile=profile, executor="local", selected=selected,
            completed=completed, failed=[], skipped=[], not_checked=[],
            phases=[{"name": "run", "status": "passed"}], source=source or self.source,
            environment_value=environment or self.environment, return_code=0, source_stable=True,
        )
        report.write_report(root / "acceptance-report.json", value)
        report.write_manifest(root)
        return root

    def test_p2_evidence_satisfies_p1_but_p1_does_not_satisfy_p2(self):
        p1 = report.profile_scenarios(self.definition, "p1")
        p2 = report.profile_scenarios(self.definition, "p2")
        root = self._write_evidence("p2", p2, p2)
        candidate = self._read_report(root / "acceptance-report.json")
        self.assertTrue(report.report_is_compatible(candidate, fingerprint="fp-a", required=p1, environment_value=self.environment, release=False, source=self.source, root=root))
        root = self._write_evidence("p1", p1, p1)
        candidate = self._read_report(root / "acceptance-report.json")
        self.assertFalse(report.report_is_compatible(candidate, fingerprint="fp-a", required=p2, environment_value=self.environment, release=False, source=self.source, root=root))

    def test_environment_mismatch_and_manifest_corruption_reject_reuse(self):
        p1 = report.profile_scenarios(self.definition, "p1")
        root = self._write_evidence("p1", p1, p1)
        candidate = self._read_report(root / "acceptance-report.json")
        changed = {**self.environment, "kernel": "6.8.1"}
        self.assertFalse(report.report_is_compatible(candidate, fingerprint="fp-a", required=p1, environment_value=changed, release=False, source=self.source, root=root))
        (root / "acceptance-report.json").write_text("corrupt\n")
        self.assertFalse(report.verify_manifest(root))

    def test_reboot_phases_are_single_run(self):
        selected = report.profile_scenarios(self.definition, "p2")
        value = report.make_report(
            run_id="run-reboot", profile="p2", executor="multipass", selected=selected,
            completed=selected, failed=[], skipped=[], not_checked=[],
            phases=[{"name": "pre_reboot", "status": "passed"}, {"name": "post_reboot", "status": "passed"}],
            source=self.source, environment_value={**self.environment, "executor": "multipass"},
            return_code=0, source_stable=True,
        )
        self.assertEqual(value["run_id"], "run-reboot")
        self.assertEqual([phase["name"] for phase in value["phases"]], ["pre_reboot", "post_reboot"])

    def test_profiles_share_acceptance_fingerprint(self):
        self.assertEqual(
            runner.source_fingerprint("p1", "local")[0],
            runner.source_fingerprint("p2", "multipass")[0],
        )

    def test_fingerprint_ignores_docs_but_changes_source(self):
        (self.temp / "validation").mkdir()
        shutil.copy2(ROOT / "validation/groups.toml", self.temp / "validation/groups.toml")
        (self.temp / "Cargo.lock").write_text("lock\n")
        for name in ("scripts/acceptance.sh", "scripts/acceptance/runner.py", "crates/chronicle-etl/src/lib.rs"):
            path = self.temp / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name + "\n")
        cfg = validation.config(self.temp)
        first = validation.fingerprint(self.temp, "p1", cfg, profile="p1", executor="local")["fingerprint"]
        (self.temp / "docs").mkdir()
        (self.temp / "docs/change.md").write_text("docs only\n")
        self.assertEqual(first, validation.fingerprint(self.temp, "p1", cfg, profile="p1", executor="local")["fingerprint"])
        (self.temp / "scripts/acceptance/runner.py").write_text("source changed\n")
        self.assertNotEqual(first, validation.fingerprint(self.temp, "p1", cfg, profile="p1", executor="local")["fingerprint"])

    def test_compatibility_wrappers_delegate(self):
        for name, profile, executor in (
            ("p1-privileged.sh", "p1", "local"),
            ("p2-privileged.sh", "p2", "local"),
            ("p1-multipass.sh", "p1", "multipass"),
            ("p2-multipass.sh", "p2", "multipass"),
        ):
            text = (ROOT / "scripts/acceptance" / name).read_text()
            self.assertIn("DEPRECATED", text)
            self.assertIn(f"--profile {profile}", text)
            self.assertIn(f"--executor {executor}", text)


if __name__ == "__main__":
    unittest.main()
