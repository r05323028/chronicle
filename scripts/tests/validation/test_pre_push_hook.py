#!/usr/bin/env python3
"""Tests for the act-based pre-push validation hook (developer workflow).

Verifies: the hook is repository-managed and executable, it invokes act with a
job that exists in the GitHub Actions workflow, and missing dependencies
produce actionable errors instead of silently skipping validation.
"""

import os
import shutil
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/pre-push-validation.sh"
CONFIG = ROOT / ".pre-commit-config.yaml"
WORKFLOW = ROOT / ".github/workflows/ci.yml"
CONTRIBUTING = ROOT / "CONTRIBUTING.md"
AGENTS = ROOT / "AGENTS.md"


def run_script(env_extra: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env.pop("CHRONICLE_PRE_PUSH_JOB", None)
    env.pop("CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS", None)
    env.update(env_extra or {})
    return subprocess.run(
        [str(SCRIPT)], env=env, capture_output=True, text=True, timeout=60
    )


def path_with_only(*tools: str) -> str:
    """A PATH containing only the named real tools (deterministic anywhere)."""
    directory = Path(tempfile.mkdtemp(prefix="prepush-path-"))
    for tool in tools:
        real = shutil.which(tool)
        if real:
            (directory / tool).symlink_to(real)
    return str(directory)


class PrePushHookTests(unittest.TestCase):
    def test_hook_script_is_tracked_and_executable(self):
        self.assertTrue(SCRIPT.is_file(), "hook script must be tracked")
        self.assertTrue(SCRIPT.stat().st_mode & stat.S_IXUSR, "hook must be executable")

    def test_config_declares_push_stage_hook(self):
        text = CONFIG.read_text()
        self.assertIn("pre-push-act", text)
        self.assertIn("stages: [push]", text)
        self.assertIn("scripts/pre-push-validation.sh", text)
        self.assertIn("language: system", text)

    def test_script_invokes_act_with_referenced_job(self):
        text = SCRIPT.read_text()
        self.assertIn("act -j", text)
        self.assertIn("CHRONICLE_PRE_PUSH_JOB:-checks", text)
        self.assertIn("ubuntu-latest=$IMAGE", text)  # Rust-capable act image
        self.assertIn("catthehacker/ubuntu:rust-latest", text)
        # The default job must exist in the workflow that is the source of truth.
        self.assertIn("checks:", WORKFLOW.read_text())

    def test_missing_act_produces_actionable_error(self):
        result = run_script({"PATH": path_with_only("bash", "dirname", "grep", "tr")})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires 'act'", result.stderr)
        self.assertIn("https://github.com/nektos/act", result.stderr)
        self.assertIn("Push aborted", result.stderr)

    def test_missing_docker_produces_actionable_error(self):
        # act present as a stub; docker absent -> docker branch fires.
        with tempfile.TemporaryDirectory(prefix="prepush-act-") as directory:
            stub = Path(directory) / "act"
            stub.write_text("#!/bin/sh\nexit 0\n")
            stub.chmod(0o755)
            base = path_with_only("bash", "dirname", "grep", "tr")
            result = run_script({"PATH": f"{directory}:{base}"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("requires 'docker'", result.stderr)
        self.assertIn("Push aborted", result.stderr)

    def test_unknown_job_override_fails_actionably(self):
        result = run_script({"CHRONICLE_PRE_PUSH_JOB": "no-such-job"})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no-such-job", result.stderr)
        self.assertIn("not found in", result.stderr)
        self.assertIn("Push aborted", result.stderr)

    def test_docs_match_implementation(self):
        contributing = CONTRIBUTING.read_text()
        self.assertIn("Pre-push validation with act", contributing)
        self.assertIn("act -j checks", contributing)
        self.assertIn("CHRONICLE_PRE_PUSH_TIMEOUT_SECONDS", contributing)
        agents = AGENTS.read_text()
        self.assertIn("pre-push-validation.sh", agents)
        self.assertIn("install-pre-push-hook.sh", agents)


if __name__ == "__main__":
    unittest.main()
