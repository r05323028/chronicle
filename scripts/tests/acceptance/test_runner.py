#!/usr/bin/env python3
import importlib.util
import json
import shutil
import subprocess
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

    def test_dispatcher_honors_configured_order(self):
        sandbox = self.temp / "dispatcher"
        (sandbox / "scripts/acceptance/lib/scenarios/p1").mkdir(parents=True)
        shutil.copy2(ROOT / "scripts/acceptance/lib/scenario-dispatch.sh", sandbox / "dispatcher.sh")
        scenarios = ["capture-basic", "wal-recovery", "replay", "resource-cleanup"]
        for scenario in scenarios:
            function = f"scenario_p1_{scenario.replace('-', '_')}"
            (sandbox / "scripts/acceptance/lib/scenarios/p1" / f"{scenario}.sh").write_text(
                f"{function}() {{ printf '%s\\n' '{scenario}'; }}\n"
            )
        command = sandbox / "run-dispatcher.sh"
        command.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"ROOT={sandbox}\n"
            f"source {sandbox}/dispatcher.sh\n"
            "die() { exit 1; }\n"
            "CHRONICLE_ACCEPTANCE_SCENARIOS='replay,capture-basic,wal-recovery,resource-cleanup'\n"
            "run_scenario_plan p1\n"
        )
        output = subprocess.check_output(["bash", str(command)], text=True)
        actual = [line for line in output.splitlines() if not line.startswith("[scenario]")]
        self.assertEqual(actual, ["replay", "capture-basic", "wal-recovery", "resource-cleanup"])
        command.write_text(command.read_text().replace(
            "replay,capture-basic,wal-recovery,resource-cleanup",
            "replay,replay,capture-basic,wal-recovery,resource-cleanup",
        ))
        self.assertNotEqual(subprocess.run(["bash", str(command)], check=False).returncode, 0)

    def test_dispatch_order_and_implementations_are_complete(self):
        for profile in ("p1", "p2"):
            selected = report.profile_scenarios(self.definition, profile)
            ordered = report.dispatch_scenarios(self.definition, profile)
            self.assertEqual(set(selected), set(ordered))
            self.assertEqual(len(selected), len(ordered))
            scenario_root = ROOT / "scripts/acceptance/lib/scenarios"
            self.assertTrue(all(
                (scenario_root / "shared" / f"{scenario}.sh").is_file()
                and (scenario_root / "extensions" / profile / f"{scenario}.sh").is_file()
                or (scenario_root / profile / f"{scenario}.sh").is_file()
                for scenario in selected
            ))
        for scenario in report.profile_scenarios(self.definition, "p1"):
            self.assertTrue((scenario_root / "shared" / f"{scenario}.sh").is_file())
            self.assertFalse((scenario_root / "p1" / f"{scenario}.sh").exists())
            self.assertFalse((scenario_root / "p2" / f"{scenario}.sh").exists())
        p1 = report.dispatch_scenarios(self.definition, "p1")
        p2 = report.dispatch_scenarios(self.definition, "p2")
        self.assertEqual([scenario for scenario in p2 if scenario in p1], p1)
        dispatcher = (ROOT / "scripts/acceptance/lib/scenario-dispatch.sh").read_text()
        self.assertNotIn("profile-p1.sh", dispatcher)
        self.assertNotIn("profile-p2.sh", dispatcher)
        multipass = (ROOT / "scripts/acceptance/lib/multipass.sh").read_text()
        self.assertNotIn("run_remote p1", multipass.split("else", 1)[1])
        self.assertIn('status = value.get("status", "not_checked")', multipass)
        self.assertNotIn("p1_artifact_root", (ROOT / "scripts/acceptance/runner.py").read_text())

    def test_all_deduplicates_to_p2_superset(self):
        with mock.patch.object(runner, "run_profile", return_value=0) as run:
            self.assertEqual(runner.main(["--profile", "all", "--no-reuse"]), 0)
        run.assert_called_once()
        self.assertEqual(run.call_args.args[1], "p2")

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
        value = report.make_report(
            run_id="run-no-reuse", profile="p1", executor="local", selected=selected,
            completed=selected, failed=[], skipped=[], not_checked=[],
            phases=[{"name": "run", "status": "passed"}], source=self.source,
            environment_value=self.environment, return_code=0, reuse_requested=False,
        )
        self.assertFalse(value["reuse"]["requested"])

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

    def test_empty_phase_evidence_cannot_pass_or_reuse(self):
        selected = report.profile_scenarios(self.definition, "p1")
        value = report.make_report(
            run_id="empty-phases", profile="p1", executor="local", selected=selected,
            completed=selected, failed=[], skipped=[], not_checked=[], phases=[],
            source={**self.source, "identity_error": None}, environment_value=self.environment,
            return_code=0,
        )
        self.assertFalse(value["release_eligible"])
        root = self.temp / "empty-phases"
        root.mkdir()
        report.write_report(root / "acceptance-report.json", value)
        report.write_manifest(root)
        self.assertFalse(report.report_is_compatible(
            value, fingerprint="fp-a", required=selected, environment_value=self.environment,
            release=False, source=self.source, root=root,
        ))

    def test_environment_mismatch_and_manifest_corruption_reject_reuse(self):
        p1 = report.profile_scenarios(self.definition, "p1")
        root = self._write_evidence("p1", p1, p1)
        candidate = self._read_report(root / "acceptance-report.json")
        changed = {**self.environment, "kernel": "6.8.1"}
        self.assertFalse(report.report_is_compatible(candidate, fingerprint="fp-a", required=p1, environment_value=changed, release=False, source=self.source, root=root))
        (root / "acceptance-report.json").write_text("corrupt\n")
        self.assertFalse(report.verify_manifest(root))

    def test_reuse_receipt_records_p2_to_p1_source(self):
        selected = report.profile_scenarios(self.definition, "p1")
        args = runner.parser().parse_args(["--profile", "p1", "--executor", "multipass", "--no-reuse", "--evidence-root", str(self.temp / "receipts")])
        candidate = self.temp / "candidate"
        candidate.mkdir()
        candidate_value = report.make_report(
            run_id="source",
            profile="p2",
            executor="multipass",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value={**self.environment, "executor": "multipass"},
            return_code=0,
            guest_source={"records": [{"run_id": "source", "fingerprint": "fp-a"}]},
        )
        report.write_report(candidate / "acceptance-report.json", candidate_value)
        (candidate / "payload.txt").write_text("immutable\n")
        report.write_manifest(candidate)
        receipt_status = runner.write_reuse_receipt(
            args,
            "p1",
            selected,
            "fp-a",
            self.source,
            {**self.environment, "executor": "multipass"},
            candidate,
            candidate_value,
        )
        self.assertEqual(receipt_status, 0)
        receipt = next((self.temp / "receipts").glob("p1/fp-a/*/acceptance-report.json"))
        value = self._read_report(receipt)
        self.assertTrue(value["reuse"]["reused"])
        self.assertEqual(value["reuse"]["from"], str(candidate))
        self.assertTrue(value["reuse"]["origin"]["manifest_sha256"])
        self.assertTrue(report.report_is_compatible(
            value,
            fingerprint="fp-a",
            required=selected,
            environment_value={**self.environment, "executor": "multipass"},
            release=False,
            source=self.source,
            root=receipt.parent,
        ))
        (candidate / "payload.txt").write_text("tampered\n")
        self.assertFalse(report.report_is_compatible(
            value,
            fingerprint="fp-a",
            required=selected,
            environment_value={**self.environment, "executor": "multipass"},
            release=False,
            source=self.source,
            root=receipt.parent,
        ))

    def test_unknown_source_identity_cannot_be_release_eligible(self):
        source = report.source_provenance(self.temp, "fp-a")
        self.assertTrue(source["identity_error"])
        value = report.make_report(
            run_id="unknown-source", profile="p1", executor="local", selected=["capture-basic"],
            completed=["capture-basic"], failed=[], skipped=[], not_checked=[],
            phases=[{"name": "run", "status": "passed"}], source=source,
            environment_value=self.environment, return_code=0,
        )
        self.assertFalse(value["release_eligible"])

    def test_guest_identity_mismatch_rejects_resume(self):
        runtime = self.temp / "runtime"
        runtime.mkdir()
        (runtime / "guest-source.json").write_text(json.dumps({"run_id": "wrong", "fingerprint": "fp-a", "working_tree_dirty": False}))
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", False)
        self.assertFalse(valid)
        (runtime / "guest-source.json").write_text(json.dumps({
            "run_id": "run-a", "fingerprint": "fp-a", "commit_sha": "not_checked",
            "tree_sha": "tree-a", "working_tree_dirty": False,
        }))
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", True)
        self.assertFalse(valid)

    def test_snapshot_inputs_exclude_docs(self):
        cfg = validation.config(ROOT)
        inputs = validation.acceptance_fingerprint(ROOT, cfg)["inputs"]
        self.assertTrue(inputs)
        self.assertTrue(all(not item["path"].startswith("docs/") for item in inputs))
        self.assertIn("--exclude='./docs'", (ROOT / "scripts/acceptance/lib/multipass.sh").read_text())

    def test_p2_common_coverage_requires_p1_pass(self):
        p1 = report.profile_scenarios(self.definition, "p1")
        status, completed, failed, not_checked = report.normalize_legacy_report(
            self.definition, "p1", {"status": "failed", "checks": {}}, p1, 1
        )
        self.assertEqual(status, "failed")
        self.assertEqual(completed, [])
        self.assertEqual(set(failed), set(p1))
        self.assertEqual(not_checked, [])

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
        reboot = (ROOT / "scripts/acceptance/lib/scenarios/p2/reboot-recovery.sh").read_text()
        self.assertIn("incremental-etl-checkpoint.json", reboot)
        self.assertIn("committed(post)", reboot)
        self.assertIn("epoch(post)", reboot)

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
