#!/usr/bin/env python3

from __future__ import annotations

import os
import subprocess
import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts" / "validate_pr_title.py"
WORKFLOW = ROOT / ".github" / "workflows" / "ci.yml"


class PullRequestTitleTests(unittest.TestCase):
    def run_validator(self, title: str) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env["PR_TITLE"] = title
        return subprocess.run(
            [sys.executable, str(SCRIPT)],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_valid_titles(self) -> None:
        for title in (
            "feat: add session filtering",
            "feat(record): add session filtering",
            "fix(replay): preserve ordering",
            "perf(wal): reduce checkpoint amplification",
            "docs: update installation guide",
            "refactor(etl): separate storage concerns",
            "test(replay): add corrupted-session regression coverage",
            "ci: validate pull request titles",
            "feat(cli)!: remove legacy syntax",
            "revert: restore previous behavior",
        ):
            with self.subTest(title=title):
                result = self.run_validator(title)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_invalid_titles_show_actionable_diagnostics(self) -> None:
        for title in (
            "Update stuff",
            "Fix bug",
            "WIP",
            "fix tests",
            "feat add session filtering",
            "feat:",
        ):
            with self.subTest(title=title):
                result = self.run_validator(title)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(title, result.stderr)
                self.assertIn(
                    "<type>(<optional-scope>)<optional-!>: <description>", result.stderr
                )
                self.assertIn(
                    "feat(record): improve session correlation", result.stderr
                )

    def test_ci_runs_stable_pr_title_job_on_title_and_sync_events(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("  push:\n", workflow)
        self.assertIn(
            "  pull_request:\n"
            "    types:\n"
            "      - opened\n"
            "      - edited\n"
            "      - synchronize\n"
            "      - reopened\n",
            workflow,
        )
        start = workflow.index("  pr-title:\n")
        end = workflow.index("  checks:\n", start)
        job = workflow[start:end]
        self.assertIn("    name: PR title\n", job)
        self.assertIn("    if: github.event_name == 'pull_request'\n", job)
        self.assertIn("PR_TITLE: ${{ github.event.pull_request.title }}", job)
        self.assertIn("run: python3 scripts/validate_pr_title.py", job)

    def test_pr_title_passes_untrusted_title_through_environment(self) -> None:
        workflow = WORKFLOW.read_text(encoding="utf-8")
        start = workflow.index("  pr-title:\n")
        end = workflow.index("  checks:\n", start)
        job = workflow[start:end]
        self.assertIn(
            "        env:\n          PR_TITLE: ${{ github.event.pull_request.title }}",
            job,
        )
        self.assertNotIn("run: ${{ github.event.pull_request.title }}", job)
        self.assertIn("persist-credentials: false", job)


if __name__ == "__main__":
    unittest.main()
