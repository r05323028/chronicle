#!/usr/bin/env python3
import copy
import importlib.util
import json
import os
import shutil
import subprocess
import tempfile
import time
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
        self.definition = report.load_scenarios(
            ROOT / "scripts/acceptance/scenarios.toml"
        )
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
            "identity_error": None,
        }

    def tearDown(self):
        shutil.rmtree(self.temp, ignore_errors=True)

    def _read_report(self, path):
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            self.fail(f"cannot read report {path}: {exc}")

    def test_recorder_includes_live_capture(self):
        live_capture = set(report.profile_scenarios(self.definition, "live-capture"))
        recorder = set(report.profile_scenarios(self.definition, "recorder"))
        self.assertTrue(live_capture < recorder)

    def test_dispatcher_honors_configured_order(self):
        sandbox = self.temp / "dispatcher"
        (sandbox / "scripts/acceptance/lib/scenarios/live-capture").mkdir(parents=True)
        shutil.copy2(
            ROOT / "scripts/acceptance/lib/scenario-dispatch.sh",
            sandbox / "dispatcher.sh",
        )
        scenarios = ["capture-basic", "wal-recovery", "replay", "resource-cleanup"]
        for scenario in scenarios:
            function = f"scenario_live_capture_{scenario.replace('-', '_')}"
            (
                sandbox
                / "scripts/acceptance/lib/scenarios/live-capture"
                / f"{scenario}.sh"
            ).write_text(f"{function}() {{ printf '%s\\n' '{scenario}'; }}\n")
        command = sandbox / "run-dispatcher.sh"
        command.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"ROOT={sandbox}\n"
            f"source {sandbox}/dispatcher.sh\n"
            "die() { exit 1; }\n"
            "CHRONICLE_ACCEPTANCE_SCENARIOS='replay,capture-basic,wal-recovery,resource-cleanup'\n"
            "run_scenario_plan live-capture\n"
        )
        output = subprocess.check_output(["bash", str(command)], text=True, timeout=10)
        actual = [
            line for line in output.splitlines() if not line.startswith("[scenario]")
        ]
        self.assertEqual(
            actual, ["replay", "capture-basic", "wal-recovery", "resource-cleanup"]
        )
        command.write_text(
            command.read_text().replace(
                "replay,capture-basic,wal-recovery,resource-cleanup",
                "replay,replay,capture-basic,wal-recovery,resource-cleanup",
            )
        )
        self.assertNotEqual(
            subprocess.run(["bash", str(command)], check=False, timeout=10).returncode,
            0,
        )

    def test_dispatcher_bounds_scenarios_and_runs_cleanup(self):
        sandbox = self.temp / "scenario-timeout"
        scenario_root = sandbox / "scripts/acceptance/lib/scenarios/live-capture"
        scenario_root.mkdir(parents=True)
        shutil.copy2(
            ROOT / "scripts/acceptance/lib/scenario-dispatch.sh",
            sandbox / "dispatcher.sh",
        )
        (scenario_root / "hang.sh").write_text(
            "scenario_live_capture_hang() { case $SCENARIO_TEST_MODE in "
            "pass) return 0 ;; fail) return 9 ;; "
            'hang) sh -c \'trap "" TERM; echo $$ >"$1"; '
            'while :; do sleep 1; done\' sh "$ARTIFACT_ROOT/child.pid" ;; '
            "esac; }\n"
        )
        command = sandbox / "run.sh"
        artifact = sandbox / "artifacts"
        command.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"ROOT={sandbox}\n"
            f"TIMEOUT={ROOT / 'scripts/run-with-timeout.sh'}\n"
            f"ARTIFACT_ROOT={artifact}\n"
            'mkdir -p "$ARTIFACT_ROOT"\n'
            "cleanup() { if [[ -f \"$ARTIFACT_ROOT/scenario-timeout.json\" ]]; then sleep 2; fi; "
            "printf cleaned >\"$ARTIFACT_ROOT/cleanup.txt\"; "
            "printf report >\"$ARTIFACT_ROOT/cleanup-report.txt\"; }\n"
            "trap cleanup EXIT\n"
            f"source {sandbox}/dispatcher.sh\n"
            "die() { exit 1; }\n"
            "CHRONICLE_ACCEPTANCE_SCENARIOS=hang\n"
            "run_scenario_plan live-capture\n"
        )
        wrapper = ROOT / "scripts/run-with-timeout.sh"
        for mode, expected in (("pass", 0), ("fail", 9)):
            shutil.rmtree(artifact, ignore_errors=True)
            env = {**os.environ, "SCENARIO_TEST_MODE": mode}
            result = subprocess.run(
                [str(wrapper), "10", "bash", str(command)],
                env=env,
                check=False,
                timeout=15,
            )
            self.assertEqual(result.returncode, expected)
            self.assertTrue((artifact / "cleanup.txt").is_file())
        shutil.rmtree(artifact, ignore_errors=True)
        env = {
            **os.environ,
            "SCENARIO_TEST_MODE": "hang",
            "CHRONICLE_ACCEPTANCE_SCENARIO_TIMEOUT_SECONDS": "1",
            "CHRONICLE_ACCEPTANCE_CLEANUP_GRACE_SECONDS": "6",
            "CHRONICLE_TIMEOUT_GRACE_SECONDS": "1",
        }
        started = time.monotonic()
        result = subprocess.run(
            [str(wrapper), "10", "bash", str(command)],
            env=env,
            check=False,
            timeout=15,
        )
        self.assertEqual(result.returncode, 124)
        self.assertLess(time.monotonic() - started, 8)
        self.assertTrue((artifact / "cleanup.txt").is_file())
        self.assertTrue((artifact / "cleanup-report.txt").is_file())
        timeout = self._read_report(artifact / "scenario-timeout.json")
        self.assertEqual(timeout["scenario"], "hang")
        self.assertEqual(timeout["timeout_layer"], "scenario")
        self.assertEqual(timeout["phase"], "scenario:live-capture/hang")
        child = int((artifact / "child.pid").read_text())
        for _ in range(30):
            try:
                os.kill(child, 0)
            except ProcessLookupError:
                break
            time.sleep(0.1)
        else:
            self.fail(f"scenario child {child} survived TERM/KILL escalation")

    def test_dispatch_order_and_implementations_are_complete(self):
        self.assertEqual(
            self.definition["scenarios"]["capture-basic"]["timeout_seconds"], 900
        )
        self.assertEqual(
            self.definition["scenarios"]["quota-pressure"]["timeout_seconds"], 600
        )
        self.assertEqual(
            self.definition["scenarios"]["checkpoint-kill-restart"]["timeout_seconds"],
            300,
        )
        scenario_root = ROOT / "scripts/acceptance/lib/scenarios"
        for profile in ("live-capture", "recorder"):
            selected = report.profile_scenarios(self.definition, profile)
            ordered = report.dispatch_scenarios(self.definition, profile)
            self.assertEqual(set(selected), set(ordered))
            self.assertEqual(len(selected), len(ordered))
            self.assertTrue(
                all(
                    (scenario_root / "shared" / f"{scenario}.sh").is_file()
                    or (scenario_root / profile / f"{scenario}.sh").is_file()
                    for scenario in selected
                )
            )
        # Single-owner convention: every scenario has exactly one implementation
        # (shared XOR profile), and the owner matches profile membership.
        live_capture_selected = report.profile_scenarios(
            self.definition, "live-capture"
        )
        recorder_selected = report.profile_scenarios(self.definition, "recorder")
        for scenario in sorted(set(live_capture_selected) | set(recorder_selected)):
            shared = scenario_root / "shared" / f"{scenario}.sh"
            live_capture_impl = scenario_root / "live-capture" / f"{scenario}.sh"
            recorder_impl = scenario_root / "recorder" / f"{scenario}.sh"
            # One owner per scenario: a shared implementation XOR profile
            # implementations matching profile membership.
            self.assertEqual(
                shared.is_file()
                + (live_capture_impl.is_file() or recorder_impl.is_file()),
                1,
                f"scenario {scenario} must have one implementation owner",
            )
            if shared.is_file():
                self.assertFalse(live_capture_impl.is_file())
                self.assertFalse(recorder_impl.is_file())
            else:
                self.assertEqual(
                    live_capture_impl.is_file(), scenario in live_capture_selected
                )
                self.assertEqual(recorder_impl.is_file(), scenario in recorder_selected)
        live_capture = report.dispatch_scenarios(self.definition, "live-capture")
        recorder = report.dispatch_scenarios(self.definition, "recorder")
        self.assertEqual(
            [scenario for scenario in recorder if scenario in live_capture],
            live_capture,
        )
        dispatcher = (ROOT / "scripts/acceptance/lib/scenario-dispatch.sh").read_text()
        self.assertNotIn("profile-live-capture.sh", dispatcher)
        self.assertNotIn("profile-recorder.sh", dispatcher)
        self.assertNotIn("extensions", dispatcher)
        multipass = (ROOT / "scripts/acceptance/lib/multipass.sh").read_text()
        self.assertNotIn("run_remote live-capture", multipass.split("else", 1)[1])
        self.assertIn('status = value.get("status", "not_checked")', multipass)
        self.assertIn("GUEST_GATE_TIMEOUT", multipass)
        self.assertIn("MULTIPASS_REMOTE_TIMEOUT < HOST_PROFILE_TIMEOUT", multipass)
        self.assertIn("scripts/run-with-timeout.sh", multipass)
        self.assertIn("gate-timeout.json", multipass)
        self.assertIn("/proc/sys/kernel/random/boot_id", multipass)
        self.assertIn("reboot-boot-ids.json", multipass)
        self.assertIn("GUEST_GATE_TIMEOUT + ACCEPTANCE_CLEANUP_GRACE", multipass)
        self.assertNotIn(
            "live_capture_artifact_root",
            (ROOT / "scripts/acceptance/runner.py").read_text(),
        )

    def test_dispatcher_rejects_missing_implementation(self):
        sandbox = self.temp / "missing-impl"
        (sandbox / "scripts/acceptance/lib/scenarios/live-capture").mkdir(parents=True)
        (sandbox / "scripts/acceptance/lib/scenarios/shared").mkdir(parents=True)
        shutil.copy2(
            ROOT / "scripts/acceptance/lib/scenario-dispatch.sh",
            sandbox / "dispatcher.sh",
        )
        (
            sandbox
            / "scripts/acceptance/lib/scenarios/live-capture"
            / "capture-basic.sh"
        ).write_text(
            "scenario_live_capture_capture_basic() { printf 'ran-capture-basic\n'; }\n"
        )
        command = sandbox / "run-dispatcher.sh"
        command.write_text(
            "#!/usr/bin/env bash\n"
            "set -euo pipefail\n"
            f"ROOT={sandbox}\n"
            f"source {sandbox}/dispatcher.sh\n"
            "die() { printf 'DIE: %s\n' \"$*\" >&2; exit 1; }\n"
            "CHRONICLE_ACCEPTANCE_SCENARIOS=capture-basic\n"
            "run_scenario_plan live-capture\n"
        )
        output = subprocess.check_output(["bash", str(command)], text=True, timeout=10)
        self.assertIn("ran-capture-basic", output)
        # A selected scenario with no shared and no profile implementation must
        # fail loudly before running anything.
        command.write_text(
            command.read_text().replace(
                "CHRONICLE_ACCEPTANCE_SCENARIOS=capture-basic",
                "CHRONICLE_ACCEPTANCE_SCENARIOS=ghost",
            )
        )
        result = subprocess.run(
            ["bash", str(command)],
            check=False,
            capture_output=True,
            text=True,
            timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not implemented", result.stderr)

    def test_report_load_scenarios_rejects_missing_implementation(self):
        sandbox = self.temp / "report-scenarios"
        root = sandbox / "scripts/acceptance"
        (root / "lib/scenarios/shared").mkdir(parents=True)
        (root / "lib/scenarios/live-capture").mkdir(parents=True)
        (root / "lib/scenarios/recorder").mkdir(parents=True)
        config = root / "scenarios.toml"
        config.write_text(
            "schema_version = 1\n"
            "[profiles.live-capture]\n"
            'scenarios = ["present", "ghost"]\n'
            "[profiles.recorder]\n"
            'scenarios = ["present", "ghost"]\n'
            "[scenarios.present]\n"
            "timeout_seconds = 300\n"
            "[scenarios.ghost]\n"
            "timeout_seconds = 300\n"
        )
        (root / "lib/scenarios/shared/present.sh").write_text("")
        with self.assertRaises(ValueError):
            report.load_scenarios(config)
        # A shared implementation resolves without any profile extension file.
        (root / "lib/scenarios/shared/ghost.sh").write_text("")
        loaded = report.load_scenarios(config)
        self.assertEqual(
            loaded["profiles"]["live-capture"]["scenarios"], ["present", "ghost"]
        )

    def test_recorder_profile_owns_failure_cleanup_resources(self):
        profile = (ROOT / "scripts/acceptance/lib/profile-recorder.sh").read_text()
        for contract in (
            'QUOTA_UNIT=""',
            'QUOTA_CGROUP=""',
            'QUOTA_MOUNT=""',
            'stop_managed_unit "$QUOTA_UNIT"',
            'stop_managed_unit "$UNIT"',
            "--kill-who=all",
            'stop_process "$UPSTREAM_PID"',
            'mountpoint -q "$QUOTA_MOUNT"',
            'losetup -j "$QUOTA_IMAGE"',
            'CURRENT_PHASE="cleanup_failed"',
            "active-units.txt",
        ):
            self.assertIn(contract, profile)
        reboot = (
            ROOT / "scripts/acceptance/lib/scenarios/recorder/reboot-recovery.sh"
        ).read_text()
        self.assertIn("pre-reboot-handoff.txt", reboot)
        self.assertIn('wait_for_unit_inactive "$UNIT"', reboot)

    def test_all_deduplicates_to_recorder_superset(self):
        with mock.patch.object(runner, "run_profile", return_value=0) as run:
            self.assertEqual(runner.main(["--profile", "all", "--no-reuse"]), 0)
        run.assert_called_once()
        self.assertEqual(run.call_args.args[1], "recorder")

    def test_parser_selects_executor_and_no_reuse(self):
        args = runner.parser().parse_args(
            ["--profile", "recorder", "--executor", "multipass", "--no-reuse"]
        )
        self.assertEqual(args.executor, "multipass")
        self.assertTrue(args.no_reuse)
        self.assertEqual(args.gate_timeout_seconds, 3600)
        with self.assertRaises(SystemExit):
            runner.parser().parse_args(
                ["--profile", "live-capture", "--gate-timeout-seconds", "0"]
            )

    def test_gate_command_has_outer_timeout_and_evidence(self):
        evidence = self.temp / "gate-timeout.json"
        phase = self.temp / "current-phase.txt"
        phase.write_text("checkpoint_crash_matrix\n")
        diagnostic = self.temp / "process-list.txt"
        diagnostic.write_text("pid state\n")
        env = {
            **os.environ,
            "CHRONICLE_TIMEOUT_EVIDENCE_FILE": str(evidence),
            "CHRONICLE_TIMEOUT_PHASE_FILE": str(phase),
            "CHRONICLE_TIMEOUT_DIAGNOSTICS": "process-list.txt:missing.log",
            "CHRONICLE_TIMEOUT_LAYER": "acceptance_gate",
            "CHRONICLE_TIMEOUT_NAME": "recorder",
        }
        command = runner.bounded_profile_command(["sh", "-c", "sleep 30"], 1)
        result = subprocess.run(command, env=env, check=False, timeout=5)
        self.assertEqual(result.returncode, 124)
        value = self._read_report(evidence)
        self.assertEqual(value["timeout_layer"], "acceptance_gate")
        self.assertEqual(value["configured_timeout_seconds"], 1)
        self.assertEqual(value["phase"], "checkpoint_crash_matrix")
        self.assertEqual(value["diagnostics"], ["process-list.txt"])

    def test_runner_timeout_finishes_central_report_and_manifest(self):
        sandbox = self.temp / "runner-timeout"
        profile = sandbox / "scripts/acceptance/lib/profile-live-capture.sh"
        profile.parent.mkdir(parents=True)
        profile.write_text("#!/usr/bin/env bash\nexec sleep 30\n")
        wrapper = sandbox / "scripts/run-with-timeout.sh"
        wrapper.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "scripts/run-with-timeout.sh", wrapper)
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "local",
                "--no-reuse",
                "--gate-timeout-seconds",
                "1",
                "--evidence-root",
                str(self.temp / "timeout-evidence"),
            ]
        )
        with (
            mock.patch.object(runner, "ROOT", sandbox),
            mock.patch.object(
                runner,
                "source_fingerprint",
                return_value=("fp-a", {"fingerprint": "fp-a"}),
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(report, "source_provenance", return_value=self.source),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 124
            )
        report_path = next(
            (self.temp / "timeout-evidence").glob(
                "live-capture/fp-a/*/acceptance-report.json"
            )
        )
        value = self._read_report(report_path)
        self.assertEqual(value["status"], "failed")
        self.assertEqual(value["timeouts"][0]["timeout_layer"], "acceptance_gate")
        self.assertTrue((report_path.parent / "artifact-manifest.sha256").is_file())
        self.assertTrue(report.verify_manifest(report_path.parent))
        entrypoint = (ROOT / "scripts/acceptance.sh").read_text()
        self.assertNotIn("run-with-timeout.sh", entrypoint)
        self.assertIn("acceptance/runner.py", entrypoint)

    def test_timeout_records_fail_report_and_reuse(self):
        runtime = self.temp / "runtime"
        runtime.mkdir()
        (runtime / "scenario-timeout.json").write_text(
            json.dumps({"timeout_layer": "scenario", "scenario": "hang"})
        )
        timeouts = report.timeout_records(runtime)
        self.assertEqual(timeouts[0]["scenario"], "hang")
        value = report.make_report(
            run_id="timeout",
            profile="live-capture",
            executor="local",
            selected=["hang"],
            completed=["hang"],
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=124,
            timeouts=timeouts,
        )
        self.assertEqual(value["status"], "failed")
        root = self.temp / "timeout-evidence"
        root.mkdir()
        report.write_report(root / "acceptance-report.json", value)
        report.write_manifest(root)
        self.assertFalse(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=["hang"],
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=root,
            )
        )

    def test_scenario_state_preserves_pass_fail_and_not_checked(self):
        selected = ["capture-basic", "wal-recovery", "replay", "resource-cleanup"]
        runtime = self.temp / "scenario-state"
        runtime.mkdir()
        (runtime / "scenario-state.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "profile": "live-capture",
                    "selected": selected,
                    "current": "wal-recovery",
                    "completed": ["capture-basic"],
                    "failed": "wal-recovery",
                    "statuses": {
                        "capture-basic": "passed",
                        "wal-recovery": "failed",
                        "replay": "not_checked",
                        "resource-cleanup": "not_checked",
                    },
                }
            )
        )
        self.assertEqual(
            report.load_scenario_state(runtime, selected, "live-capture"),
            (["capture-basic"], ["wal-recovery"], ["replay", "resource-cleanup"]),
        )

    def test_scenario_state_merges_pre_and_post_reboot_ledgers(self):
        selected = ["capture-basic", "wal-recovery", "replay", "resource-cleanup"]
        runtime = self.temp / "scenario-state-pre-post"
        runtime.mkdir()
        (runtime / "pre-reboot").mkdir()
        (runtime / "post-reboot").mkdir()
        (runtime / "pre-reboot" / "scenario-state.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "profile": "recorder",
                    "selected": selected,
                    "current": "wal-recovery",
                    "completed": ["capture-basic"],
                    "failed": None,
                    "statuses": {
                        "capture-basic": "passed",
                        "wal-recovery": "not_checked",
                        "replay": "not_checked",
                        "resource-cleanup": "not_checked",
                    },
                }
            )
        )
        (runtime / "post-reboot" / "scenario-state.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "profile": "recorder",
                    "selected": selected,
                    "current": None,
                    "completed": ["wal-recovery", "replay", "resource-cleanup"],
                    "failed": None,
                    "statuses": {
                        "capture-basic": "not_checked",
                        "wal-recovery": "passed",
                        "replay": "passed",
                        "resource-cleanup": "passed",
                    },
                }
            )
        )
        # Lexicographic rglob order must not matter: the union of both phase
        # ledgers attributes every executed scenario, not just the last file.
        self.assertEqual(
            report.load_scenario_state(runtime, selected, "recorder"),
            (selected, [], []),
        )

    def test_scenario_state_timeout_attributes_only_current(self):
        selected = ["capture-basic", "wal-recovery", "replay"]
        runtime = self.temp / "scenario-timeout-state"
        runtime.mkdir()
        (runtime / "scenario-state.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "profile": "live-capture",
                    "selected": selected,
                    "current": "wal-recovery",
                    "completed": ["capture-basic"],
                    "failed": "wal-recovery",
                    "statuses": {
                        "capture-basic": "passed",
                        "wal-recovery": "failed",
                        "replay": "not_checked",
                    },
                }
            )
        )
        (runtime / "scenario-timeout.json").write_text(
            json.dumps(
                {
                    "timeout_layer": "scenario",
                    "scenario": "wal-recovery",
                    "configured_timeout_seconds": 1,
                }
            )
        )
        completed, failed, not_checked = report.load_scenario_state(
            runtime, selected, "live-capture"
        )
        # Scenario timeout classification remains execution-ledger based even
        # when a legacy profile report has coarse failed status.
        legacy_status, _, legacy_failed, _ = report.normalize_legacy_report(
            self.definition,
            "live-capture",
            {"status": "failed", "checks": {}},
            report.profile_scenarios(self.definition, "live-capture"),
            124,
        )
        self.assertEqual(legacy_status, "failed")
        self.assertEqual(legacy_failed, [])
        value = report.make_report(
            run_id="scenario-timeout",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=completed,
            failed=failed,
            skipped=[],
            not_checked=not_checked,
            phases=[{"name": "run", "status": "failed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=124,
            timeouts=report.timeout_records(runtime),
        )
        self.assertEqual(value["completed_scenarios"], ["capture-basic"])
        self.assertEqual(value["failed_scenarios"], ["wal-recovery"])
        self.assertEqual(value["not_checked_scenarios"], ["replay"])
        self.assertEqual(value["status"], "failed")

    def test_scenario_state_rejects_malformed_ledger(self):
        runtime = self.temp / "malformed-state"
        runtime.mkdir()
        (runtime / "scenario-state.json").write_text("{not-json")
        with self.assertRaises(ValueError):
            report.load_scenario_state(runtime, ["capture-basic"], "live-capture")

    def test_scenarios_use_explicit_wait_convention(self):
        scenario_root = ROOT / "scripts/acceptance/lib/scenarios"
        allowed_fixture_sleep = "sleep 600"
        for path in scenario_root.rglob("*.sh"):
            for number, line in enumerate(path.read_text().splitlines(), 1):
                stripped = line.strip()
                if stripped.startswith("sleep "):
                    self.assertIn(
                        allowed_fixture_sleep,
                        stripped,
                        f"undocumented synchronization sleep at {path}:{number}",
                    )
        wait_text = (ROOT / "scripts/acceptance/lib/wait.sh").read_text()
        self.assertEqual(wait_text.count("while :; do"), 1)
        self.assertIn("SECONDS >= deadline", wait_text)
        self.assertLess(
            wait_text.index("SECONDS >= deadline"),
            wait_text.index('sleep "$interval"'),
        )
        readiness = (ROOT / "scripts/acceptance/recorder-readiness.sh").read_text()
        self.assertIn("while ((SECONDS < deadline)); do", readiness)
        self.assertIn("systemd_active_epoch", readiness)
        self.assertIn("recorder_metadata_epoch", readiness)

    def test_destructive_scenarios_have_postcondition_barriers(self):
        root = ROOT / "scripts/acceptance/lib/scenarios/recorder"
        checkpoint = (root / "checkpoint-kill-restart.sh").read_text()
        self.assertIn('wait_for_path_absent "$CHECKPOINT_PAUSE_FILE"', checkpoint)
        self.assertIn('wait_for_path_absent "$CHECKPOINT_READY_FILE"', checkpoint)
        quota = (root / "quota-pressure.sh").read_text()
        self.assertIn("quota mount to disappear", quota)
        self.assertIn("quota loop device to detach", quota)
        retention = (root / "retention-interruption.sh").read_text()
        self.assertIn('wait_for_path_absent "$CLEANUP_PAUSE_FILE"', retention)
        self.assertIn('wait_for_path_absent "$CLEANUP_READY_FILE"', retention)
        reboot = (root / "reboot-recovery.sh").read_text()
        self.assertIn('wait_for_unit_inactive "$UNIT"', reboot)

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

    def test_source_mutation_during_release_run_fails(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "local",
                "--release",
                "--no-reuse",
                "--evidence-root",
                str(self.temp / "mutation-evidence"),
            ]
        )
        source_end = {
            **self.source,
            "commit_sha": "commit-b",
            "tree_sha": "tree-b",
        }
        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                return_value=("fp-a", {"fingerprint": "fp-a"}),
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(
                runner.subprocess, "run", return_value=mock.Mock(returncode=0)
            ),
            mock.patch.object(
                runner.report,
                "normalize_legacy_report",
                return_value=("passed", selected, [], []),
            ),
            mock.patch.object(
                runner.report,
                "source_provenance",
                side_effect=[self.source, source_end],
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 1
            )
        report_path = next(
            (self.temp / "mutation-evidence").glob(
                "live-capture/fp-a/*/acceptance-report.json"
            )
        )
        value = self._read_report(report_path)
        self.assertEqual(value["status"], "failed")
        self.assertFalse(value["release_eligible"])
        self.assertFalse(value["source"]["stable"])

    def test_fingerprint_mutation_during_fresh_release_run_fails(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "local",
                "--release",
                "--no-reuse",
                "--evidence-root",
                str(self.temp / "fingerprint-mutation-evidence"),
            ]
        )
        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                side_effect=[
                    ("fp-a", {"fingerprint": "fp-a"}),
                    ("fp-b", {"fingerprint": "fp-b"}),
                ],
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(
                runner.subprocess, "run", return_value=mock.Mock(returncode=0)
            ),
            mock.patch.object(
                runner.report,
                "normalize_legacy_report",
                return_value=("passed", selected, [], []),
            ),
            mock.patch.object(
                runner.report,
                "source_provenance",
                side_effect=[self.source, self.source],
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 1
            )
        report_path = next(
            (self.temp / "fingerprint-mutation-evidence").glob(
                "live-capture/fp-a/*/acceptance-report.json"
            )
        )
        value = self._read_report(report_path)
        self.assertEqual(value["status"], "failed")
        self.assertFalse(value["release_eligible"])
        self.assertFalse(value["source"]["stable"])

    def test_report_completeness_and_release_identity(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        value = report.make_report(
            run_id="run-a",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=0,
            source_stable=True,
        )
        self.assertEqual(value["status"], "passed")
        self.assertTrue(value["release_eligible"])
        self.assertEqual(value["not_checked_scenarios"], [])
        value = report.make_report(
            run_id="run-no-reuse",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=0,
            reuse_requested=False,
        )
        self.assertFalse(value["reuse"]["requested"])

    def test_report_records_preflight_outcome(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        preflight = {
            "schema_version": 1,
            "outcome": "supported",
            "exit_code": 0,
            "selected_tests": selected,
        }
        value = report.make_report(
            run_id="preflight-a",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=0,
            source_stable=True,
            preflight=preflight,
        )
        self.assertEqual(value["preflight"]["outcome"], "supported")
        value = report.make_report(
            run_id="preflight-unsupported",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=[],
            failed=[],
            skipped=[],
            not_checked=selected,
            phases=[{"name": "run", "status": "not_checked"}],
            source=self.source,
            environment_value=self.environment,
            return_code=77,
            preflight={"schema_version": 1, "outcome": "unsupported_environment"},
        )
        self.assertEqual(value["status"], "not_checked")
        self.assertEqual(value["preflight"]["outcome"], "unsupported_environment")

    def _write_evidence(
        self, profile, selected, completed, environment=None, source=None
    ):
        root = self.temp / "evidence" / profile / "fp-a" / "run-a"
        root.mkdir(parents=True)
        value = report.make_report(
            run_id="run-a",
            profile=profile,
            executor="local",
            selected=selected,
            completed=completed,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=source or self.source,
            environment_value=environment or self.environment,
            return_code=0,
            source_stable=True,
        )
        runner.write_evidence(root, value)
        return root

    def test_recorder_evidence_satisfies_live_capture_but_not_reverse(self):
        live_capture = report.profile_scenarios(self.definition, "live-capture")
        recorder = report.profile_scenarios(self.definition, "recorder")
        root = self._write_evidence("recorder", recorder, recorder)
        candidate = self._read_report(root / "acceptance-report.json")
        self.assertTrue(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=live_capture,
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=root,
            )
        )
        root = self._write_evidence("live-capture", live_capture, live_capture)
        candidate = self._read_report(root / "acceptance-report.json")
        self.assertFalse(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=recorder,
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=root,
            )
        )

    def test_dirty_historical_evidence_reuses_across_commit_sha(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        evidence_source = {**self.source, "working_tree_dirty": True}
        root = self._write_evidence(
            "live-capture", selected, selected, source=evidence_source
        )
        candidate = self._read_report(root / "acceptance-report.json")
        self.assertFalse(candidate["release_eligible"])
        current_source = {
            **self.source,
            "commit_sha": "commit-b",
            "tree_sha": "tree-b",
        }
        self.assertTrue(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=selected,
                environment_value=self.environment,
                release=True,
                source=current_source,
                root=root,
            )
        )
        self.assertTrue(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=selected,
                environment_value=self.environment,
                release=False,
                source={**current_source, "working_tree_dirty": True},
                root=root,
            )
        )

    def test_release_run_reuses_cross_sha_evidence_and_detects_mutation(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        evidence_source = {**self.source, "working_tree_dirty": True}
        self._write_evidence("live-capture", selected, selected, source=evidence_source)
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "local",
                "--release",
                "--evidence-root",
                str(self.temp / "evidence"),
            ]
        )
        current = {
            **self.source,
            "commit_sha": "commit-b",
            "tree_sha": "tree-b",
        }
        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                side_effect=[
                    ("fp-a", {"fingerprint": "fp-a"}),
                    ("fp-a", {"fingerprint": "fp-a"}),
                ],
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(
                runner.report, "source_provenance", side_effect=[current, current]
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 0
            )
        receipt = next(
            value
            for path in (self.temp / "evidence").glob(
                "live-capture/fp-a/*/acceptance-report.json"
            )
            if (value := self._read_report(path))["reuse"]["reused"]
        )
        self.assertTrue(receipt["reuse"]["reused"])
        self.assertTrue(receipt["release_eligible"])
        self.assertEqual(receipt["source"]["commit_sha"], "commit-b")
        self.assertEqual(receipt["source"]["tree_sha"], "tree-b")
        self.assertEqual(
            receipt["reuse"]["from"],
            str((self.temp / "evidence" / "live-capture" / "fp-a" / "run-a").resolve()),
        )
        self.assertTrue(receipt["reuse"]["origin"]["manifest_sha256"])

        mutated = {**current, "commit_sha": "commit-c", "tree_sha": "tree-c"}
        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                side_effect=[
                    ("fp-a", {"fingerprint": "fp-a"}),
                    ("fp-a", {"fingerprint": "fp-a"}),
                ],
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(
                runner.report,
                "source_provenance",
                side_effect=[current, mutated],
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 2
            )

        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                side_effect=[
                    ("fp-a", {"fingerprint": "fp-a"}),
                    ("fp-b", {"fingerprint": "fp-b"}),
                ],
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ),
            mock.patch.object(
                runner.report,
                "source_provenance",
                side_effect=[current, current],
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 2
            )

    def test_release_run_rejects_dirty_current_source_before_reuse(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        self._write_evidence("live-capture", selected, selected)
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "local",
                "--release",
                "--evidence-root",
                str(self.temp / "evidence"),
            ]
        )
        dirty_current = {
            **self.source,
            "commit_sha": "commit-b",
            "tree_sha": "tree-b",
            "working_tree_dirty": True,
        }
        with (
            mock.patch.object(
                runner,
                "source_fingerprint",
                return_value=("fp-a", {"fingerprint": "fp-a"}),
            ),
            mock.patch.object(
                runner, "collect_environment", return_value=self.environment
            ) as collect_environment,
            mock.patch.object(
                runner.report, "source_provenance", return_value=dirty_current
            ),
        ):
            self.assertEqual(
                runner.run_profile(args, "live-capture", self.definition), 2
            )
        collect_environment.assert_not_called()
        reports = [
            self._read_report(path)
            for path in (self.temp / "evidence").glob(
                "live-capture/fp-a/*/acceptance-report.json"
            )
        ]
        self.assertEqual(len(reports), 1)
        self.assertFalse(any(value["reuse"]["reused"] for value in reports))

    def test_empty_phase_evidence_cannot_pass_or_reuse(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        value = report.make_report(
            run_id="empty-phases",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[],
            source={**self.source, "identity_error": None},
            environment_value=self.environment,
            return_code=0,
        )
        self.assertFalse(value["release_eligible"])
        root = self.temp / "empty-phases"
        root.mkdir()
        report.write_report(root / "acceptance-report.json", value)
        report.write_manifest(root)
        self.assertFalse(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=selected,
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=root,
            )
        )

    def test_environment_mismatch_and_manifest_corruption_reject_reuse(self):
        live_capture = report.profile_scenarios(self.definition, "live-capture")
        root = self._write_evidence("live-capture", live_capture, live_capture)
        candidate = self._read_report(root / "acceptance-report.json")
        changed = {**self.environment, "kernel": "6.8.1"}
        self.assertFalse(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=live_capture,
                environment_value=changed,
                release=False,
                source=self.source,
                root=root,
            )
        )
        (root / "gate-timeout.json").write_text(
            json.dumps({"timeout_layer": "acceptance_gate"})
        )
        report.write_manifest(root)
        self.assertFalse(
            report.report_is_compatible(
                candidate,
                fingerprint="fp-a",
                required=live_capture,
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=root,
            )
        )
        (root / "acceptance-report.json").write_text("corrupt\n")
        self.assertFalse(report.verify_manifest(root))

    def test_incompatible_or_incomplete_evidence_rejects_reuse(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        base = report.make_report(
            run_id="invalid",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=0,
        )
        base["artifact_manifest"] = {
            "file": "artifact-manifest.sha256",
            "complete": True,
        }
        cases = {
            "schema": lambda value: value.update(schema_version=999),
            "compatibility": lambda value: value.update(compatibility_version=999),
            "fingerprint": lambda value: value["source"].update(fingerprint="fp-b"),
            "executor-environment": lambda value: value["environment"].update(
                executor="multipass"
            ),
            "scenario-set": lambda value: value.update(selected_scenarios=[]),
            "incomplete": lambda value: value.update(completed_scenarios=[]),
            "partially-incomplete": lambda value: value.update(
                selected_scenarios=[*selected, "new-required"]
            ),
            "failed": lambda value: value.update(failed_scenarios=[selected[0]]),
            "not-checked": lambda value: value.update(
                not_checked_scenarios=[selected[0]]
            ),
            "phase": lambda value: value.update(
                phases=[{"name": "run", "status": "failed"}]
            ),
            "timeout": lambda value: value.update(
                timeouts=[{"timeout_layer": "scenario"}]
            ),
            "manifest-incomplete": lambda value: value.update(
                artifact_manifest={
                    "file": "artifact-manifest.sha256",
                    "complete": False,
                }
            ),
            "missing-profile": lambda value: value.pop("profile"),
            "missing-exit": lambda value: value.pop("exit_code"),
            "missing-created-at": lambda value: value.pop("created_at"),
        }
        for name, mutate in cases.items():
            with self.subTest(name=name):
                value = copy.deepcopy(base)
                mutate(value)
                root = self.temp / "invalid" / name
                root.mkdir(parents=True)
                report.write_report(root / "acceptance-report.json", value)
                report.write_manifest(root)
                self.assertTrue(report.verify_manifest(root))
                self.assertFalse(
                    report.report_is_compatible(
                        value,
                        fingerprint="fp-a",
                        required=selected,
                        environment_value=self.environment,
                        release=False,
                        source=self.source,
                        root=root,
                    )
                )

    def test_malformed_timeout_and_guest_evidence_fail_closed(self):
        runtime = self.temp / "malformed-timeout"
        runtime.mkdir()
        (runtime / "scenario-timeout.json").write_text("not-json\n")
        self.assertEqual(
            report.timeout_records(runtime),
            [
                {
                    "timeout_layer": "invalid",
                    "evidence_file": "scenario-timeout.json",
                }
            ],
        )

        selected = report.profile_scenarios(self.definition, "live-capture")
        root = self.temp / "malformed-guest"
        root.mkdir()
        value = report.make_report(
            run_id="malformed-guest",
            profile="live-capture",
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
            guest_source={"records": [None]},
        )
        runner.write_evidence(root, value)
        self.assertFalse(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=selected,
                environment_value={**self.environment, "executor": "multipass"},
                release=False,
                source=self.source,
                root=root,
            )
        )

        root = self.temp / "mismatched-guest"
        root.mkdir()
        value = report.make_report(
            run_id="mismatched-guest",
            profile="live-capture",
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
            guest_source={
                "records": [
                    {
                        "run_id": "mismatched-guest",
                        "fingerprint": "fp-a",
                        "commit_sha": "other-commit",
                        "tree_sha": "other-tree",
                        "working_tree_dirty": False,
                    }
                ]
            },
        )
        runner.write_evidence(root, value)
        self.assertFalse(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=selected,
                environment_value={**self.environment, "executor": "multipass"},
                release=False,
                source=self.source,
                root=root,
            )
        )

    def test_reuse_receipt_records_recorder_to_live_capture_source(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        args = runner.parser().parse_args(
            [
                "--profile",
                "live-capture",
                "--executor",
                "multipass",
                "--no-reuse",
                "--evidence-root",
                str(self.temp / "receipts"),
            ]
        )
        candidate = self.temp / "candidate"
        candidate.mkdir()
        candidate_value = report.make_report(
            run_id="source",
            profile="recorder",
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
            guest_source={
                "records": [
                    {
                        "run_id": "source",
                        "fingerprint": "fp-a",
                        "commit_sha": "commit-a",
                        "tree_sha": "tree-a",
                        "working_tree_dirty": False,
                    }
                ]
            },
        )
        runner.write_evidence(candidate, candidate_value)
        (candidate / "payload.txt").write_text("immutable\n")
        report.write_manifest(candidate)
        receipt_status = runner.write_reuse_receipt(
            args,
            "live-capture",
            selected,
            "fp-a",
            self.source,
            {**self.environment, "executor": "multipass"},
            candidate,
            candidate_value,
        )
        self.assertEqual(receipt_status, 0)
        receipt = next(
            (self.temp / "receipts").glob("live-capture/fp-a/*/acceptance-report.json")
        )
        value = self._read_report(receipt)
        self.assertTrue(value["reuse"]["reused"])
        self.assertEqual(value["reuse"]["from"], str(candidate))
        self.assertTrue(value["reuse"]["origin"]["manifest_sha256"])
        self.assertTrue(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=selected,
                environment_value={**self.environment, "executor": "multipass"},
                release=False,
                source=self.source,
                root=receipt.parent,
            )
        )
        (candidate / "payload.txt").write_text("tampered\n")
        self.assertFalse(
            report.report_is_compatible(
                value,
                fingerprint="fp-a",
                required=selected,
                environment_value={**self.environment, "executor": "multipass"},
                release=False,
                source=self.source,
                root=receipt.parent,
            )
        )

    def test_reuse_chain_binds_fingerprint_environment_and_scenarios(self):
        selected = report.profile_scenarios(self.definition, "live-capture")
        origin = self.temp / "chain-origin"
        origin.mkdir()
        origin_value = report.make_report(
            run_id="origin",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source={**self.source, "fingerprint": "fp-b"},
            environment_value={**self.environment, "kernel": "different"},
            return_code=0,
        )
        runner.write_evidence(origin, origin_value)

        receipt_root = self.temp / "chain-receipt"
        receipt_root.mkdir()
        receipt_value = report.make_report(
            run_id="receipt",
            profile="live-capture",
            executor="local",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "reuse", "status": "passed"}],
            source=self.source,
            environment_value=self.environment,
            return_code=0,
            reused=True,
            reused_from=str(origin),
        )
        receipt_value["reuse"]["origin"] = {
            "root": str(origin),
            "manifest": "artifact-manifest.sha256",
            "manifest_sha256": report.sha256(origin / "artifact-manifest.sha256"),
        }
        runner.write_evidence(receipt_root, receipt_value)
        self.assertFalse(
            report.report_is_compatible(
                receipt_value,
                fingerprint="fp-a",
                required=selected,
                environment_value=self.environment,
                release=False,
                source=self.source,
                root=receipt_root,
            )
        )

    def test_unknown_source_identity_cannot_be_release_eligible(self):
        source = report.source_provenance(self.temp, "fp-a")
        self.assertTrue(source["identity_error"])
        value = report.make_report(
            run_id="unknown-source",
            profile="live-capture",
            executor="local",
            selected=["capture-basic"],
            completed=["capture-basic"],
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[{"name": "run", "status": "passed"}],
            source=source,
            environment_value=self.environment,
            return_code=0,
        )
        self.assertFalse(value["release_eligible"])

    def test_guest_identity_mismatch_rejects_resume(self):
        runtime = self.temp / "runtime"
        runtime.mkdir()
        (runtime / "guest-source.json").write_text(
            json.dumps(
                {"run_id": "wrong", "fingerprint": "fp-a", "working_tree_dirty": False}
            )
        )
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", False)
        self.assertFalse(valid)
        (runtime / "guest-source.json").write_text(
            json.dumps(
                {
                    "run_id": "run-a",
                    "fingerprint": "fp-a",
                    "commit_sha": "not_checked",
                    "tree_sha": "tree-a",
                    "working_tree_dirty": False,
                }
            )
        )
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", False)
        self.assertFalse(valid)
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", True)
        self.assertFalse(valid)
        (runtime / "guest-source.json").write_text(
            json.dumps(
                {
                    "run_id": "run-a",
                    "fingerprint": "fp-a",
                    "commit_sha": "other-commit",
                    "tree_sha": "other-tree",
                    "working_tree_dirty": False,
                }
            )
        )
        _, valid = runner.guest_identity(runtime, "run-a", "fp-a", True, self.source)
        self.assertFalse(valid)

    def test_snapshot_inputs_exclude_docs(self):
        cfg = validation.config(ROOT)
        inputs = validation.acceptance_fingerprint(ROOT, cfg)["inputs"]
        self.assertTrue(inputs)
        self.assertTrue(all(not item["path"].startswith("docs/") for item in inputs))
        # docs/ is git-tracked; the snapshot must include it so the guest-side
        # release clean-tree check does not see spurious deletions.
        snapshot = (ROOT / "scripts/acceptance/lib/multipass.sh").read_text()
        self.assertNotIn("--exclude='./docs'", snapshot)
        self.assertIn("--exclude='./target'", snapshot)
        self.assertIn("--exclude='./graphify-out'", snapshot)
        self.assertIn("--exclude='./.codegraph'", snapshot)
        self.assertIn("--exclude='./.pi'", snapshot)

    def test_legacy_profile_failure_does_not_fail_every_scenario(self):
        live_capture = report.profile_scenarios(self.definition, "live-capture")
        status, completed, failed, not_checked = report.normalize_legacy_report(
            self.definition,
            "live-capture",
            {"status": "failed", "checks": {}},
            live_capture,
            1,
        )
        self.assertEqual(status, "failed")
        self.assertEqual(completed, [])
        self.assertEqual(failed, [])
        self.assertEqual(set(not_checked), set(live_capture))

    def test_reboot_phases_are_single_run(self):
        selected = report.profile_scenarios(self.definition, "recorder")
        value = report.make_report(
            run_id="run-reboot",
            profile="recorder",
            executor="multipass",
            selected=selected,
            completed=selected,
            failed=[],
            skipped=[],
            not_checked=[],
            phases=[
                {"name": "pre_reboot", "status": "passed"},
                {"name": "post_reboot", "status": "passed"},
            ],
            source=self.source,
            environment_value={**self.environment, "executor": "multipass"},
            return_code=0,
            source_stable=True,
        )
        self.assertEqual(value["run_id"], "run-reboot")
        self.assertEqual(
            [phase["name"] for phase in value["phases"]], ["pre_reboot", "post_reboot"]
        )
        reboot = (
            ROOT / "scripts/acceptance/lib/scenarios/recorder/reboot-recovery.sh"
        ).read_text()
        self.assertIn("incremental-etl-checkpoint-v2.json", reboot)
        self.assertIn("committed(post)", reboot)
        self.assertIn("epoch(post)", reboot)

    def test_profiles_share_acceptance_fingerprint(self):
        self.assertEqual(
            runner.source_fingerprint("live-capture", "local")[0],
            runner.source_fingerprint("recorder", "multipass")[0],
        )

    def test_openspec_archive_does_not_change_fingerprint_but_scenarios_do(self):
        (self.temp / "validation").mkdir()
        shutil.copy2(
            ROOT / "validation/groups.toml", self.temp / "validation/groups.toml"
        )
        (self.temp / "Cargo.lock").write_text("lock\n")
        scenario = self.temp / "scripts/acceptance/scenarios.toml"
        scenario.parent.mkdir(parents=True)
        scenario.write_text("scenario contract v1\n")
        cfg = validation.config(self.temp)
        first = validation.acceptance_fingerprint(self.temp, cfg)["fingerprint"]
        archive = self.temp / "openspec/changes/archive/change/spec.md"
        archive.parent.mkdir(parents=True)
        archive.write_text("archive only\n")
        cache = self.temp / "scripts/acceptance/__pycache__/runner.pyc"
        cache.parent.mkdir(parents=True)
        cache.write_bytes(b"generated cache")
        (self.temp / "scripts/acceptance/.DS_Store").write_bytes(b"finder cache")
        self.assertEqual(
            first,
            validation.acceptance_fingerprint(self.temp, cfg)["fingerprint"],
        )
        scenario.write_text("scenario contract v2\n")
        self.assertNotEqual(
            first,
            validation.acceptance_fingerprint(self.temp, cfg)["fingerprint"],
        )

    def test_fingerprint_ignores_docs_but_changes_source(self):
        (self.temp / "validation").mkdir()
        shutil.copy2(
            ROOT / "validation/groups.toml", self.temp / "validation/groups.toml"
        )
        (self.temp / "Cargo.lock").write_text("lock\n")
        for name in (
            "scripts/acceptance.sh",
            "scripts/acceptance/runner.py",
            "scripts/run-with-timeout.sh",
            "crates/chronicle-etl/src/lib.rs",
        ):
            path = self.temp / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(name + "\n")
        cfg = validation.config(self.temp)
        first = validation.fingerprint(
            self.temp, "live-capture", cfg, profile="live-capture", executor="local"
        )["fingerprint"]
        (self.temp / "docs").mkdir()
        (self.temp / "docs/change.md").write_text("docs only\n")
        self.assertEqual(
            first,
            validation.fingerprint(
                self.temp, "live-capture", cfg, profile="live-capture", executor="local"
            )["fingerprint"],
        )
        (self.temp / "scripts/run-with-timeout.sh").write_text(
            "timeout contract changed\n"
        )
        self.assertNotEqual(
            first,
            validation.fingerprint(
                self.temp, "live-capture", cfg, profile="live-capture", executor="local"
            )["fingerprint"],
        )

    def test_obsolete_wrappers_absent_and_unreferenced(self):
        removed = (
            "scripts/acceptance/live-capture-privileged.sh",
            "scripts/acceptance/recorder-privileged.sh",
            "scripts/acceptance/live-capture-multipass.sh",
            "scripts/acceptance/recorder-multipass.sh",
            "scripts/tests/acceptance/test-live-capture-privileged-runner.sh",
            "scripts/tests/acceptance/test-recorder-privileged-runner.sh",
        )
        for path in removed:
            self.assertFalse(
                (ROOT / path).exists(), f"removed wrapper still present: {path}"
            )
        # No living executable/documentation surface may reference the removed
        # basenames. OpenSpec planning snapshots (openspec/) are historical
        # records, not callers, so they are excluded from the scan.
        scan_prefixes = (".github/", "scripts/", "tests/", "docs/", "validation/")
        scan_roots = ("AGENTS.md", "README.md", "CONTRIBUTING.md")
        tracked = subprocess.run(
            ["git", "-C", str(ROOT), "ls-files"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.splitlines()
        basenames = tuple(path.rsplit("/", 1)[1] for path in removed)
        for file in tracked:
            if file == "scripts/tests/acceptance/test_runner.py":
                # This test intentionally names the removed wrappers.
                continue
            if not (file.startswith(scan_prefixes) or file in scan_roots):
                continue
            path = ROOT / file
            if not path.is_file():
                continue
            try:
                text = path.read_text(errors="ignore")
            except OSError:
                continue
            for basename in basenames:
                self.assertNotIn(
                    basename, text, f"tracked file references removed wrapper: {file}"
                )


if __name__ == "__main__":
    unittest.main()
