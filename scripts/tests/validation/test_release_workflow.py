#!/usr/bin/env python3
"""Portable semantic tests for Chronicle's release promotion workflow."""

from __future__ import annotations

import os
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import tomllib

ROOT = Path(__file__).resolve().parents[3]
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
CLIFF_CONFIG = ROOT / "cliff.toml"
RELEASE_RANGE = ROOT / "scripts" / "release" / "resolve-release-range.py"
RESOLVE_CONTEXT = ROOT / "scripts" / "release" / "resolve-release-context.sh"
MANAGE_STATE = ROOT / "scripts" / "release" / "manage-release-state.sh"
GIT_CLIFF = shutil.which("git-cliff")


def workflow_text() -> str:
    return RELEASE_WORKFLOW.read_text(encoding="utf-8")


class WorkflowShape:
    """Small indentation-based view of job properties and run commands."""

    def __init__(self, text: str) -> None:
        self.jobs: dict[str, dict] = {}
        job: str | None = None
        block: str | None = None
        current_step: list[str] = []
        run_lines: list[str] = []

        def finish() -> None:
            if job is not None:
                self.jobs[job]["run_text"] = "\n".join(run_lines)

        for raw in text.splitlines():
            if not raw.strip() or raw.lstrip().startswith("#"):
                continue
            indent = len(raw) - len(raw.lstrip())
            stripped = raw.strip()
            if indent == 2 and re.match(r"^[A-Za-z0-9_.-]+:$", stripped):
                finish()
                job = stripped[:-1]
                self.jobs[job] = {
                    "env": set(),
                    "permissions": set(),
                    "scalars": {},
                    "run_text": "",
                }
                block = None
                current_step = []
                run_lines = []
            elif job is None:
                continue
            elif indent == 4 and stripped in (
                "env:",
                "permissions:",
                "steps:",
                "outputs:",
            ):
                block = stripped[:-1]
            elif indent == 4 and ":" in stripped:
                block = None
                key, _, value = stripped.partition(":")
                self.jobs[job]["scalars"][key.strip()] = value.strip()
            elif indent == 6 and block == "env":
                self.jobs[job]["env"].add(stripped.split(":", 1)[0].strip())
            elif indent == 6 and block == "permissions":
                self.jobs[job]["permissions"].add(stripped)
            elif indent == 6 and stripped.startswith("- "):
                block = None
                current_step = [stripped]
            elif indent >= 8 and current_step:
                current_step.append(stripped)
            if current_step and any(line.startswith("run:") for line in current_step):
                run_lines.append(stripped)
        finish()

    def gh_jobs(self) -> list[str]:
        return [
            name
            for name, config in self.jobs.items()
            if re.search(r"\bgh\s|manage-release-state\.sh", config["run_text"])
        ]


class WorkflowContractTests(unittest.TestCase):
    def test_dispatch_replaces_pushed_tag_entrypoint(self):
        workflow = workflow_text()
        self.assertIn("workflow_dispatch:", workflow)
        self.assertNotRegex(workflow, r"(?m)^\s+push:")
        self.assertNotIn("on:\n  push:", workflow)
        self.assertIn("type: choice", workflow)
        self.assertIn("- prepare", workflow)
        self.assertIn("- release", workflow)
        self.assertIn("version:", workflow)
        self.assertIn("required: true", workflow)

    def test_dispatch_is_main_only_and_concurrent_runs_are_not_cancelled(self):
        workflow = workflow_text()
        self.assertIn("WORKFLOW_REF", workflow)
        self.assertIn("refs/heads/main", workflow)
        self.assertIn("WORKFLOW_EVENT", workflow)
        self.assertIn("group: chronicle-release", workflow)
        self.assertIn("cancel-in-progress: false", workflow)

    def test_frozen_sha_is_verified_and_propagated(self):
        workflow = workflow_text()
        self.assertIn("ref: ${{ github.sha }}", workflow)
        self.assertEqual(
            workflow.count("ref: ${{ github.sha }}"),
            workflow.count("uses: actions/checkout@"),
        )
        self.assertNotIn("ref: ${{ needs.resolve-context.outputs.sha }}", workflow)
        self.assertIn("frozen_sha=$(git rev-parse HEAD)", workflow)
        self.assertIn("sha=$WORKFLOW_SHA", workflow)
        self.assertIn("sha: ${{ steps.resolve.outputs.sha }}", workflow)
        self.assertNotIn("ref: main", workflow)

    def test_canonical_release_gate_precedes_build_and_tag(self):
        workflow = workflow_text()
        shape = WorkflowShape(workflow)
        self.assertIn("release-qualification", shape.jobs)
        self.assertIn(
            "./scripts/validate.sh release",
            shape.jobs["release-qualification"]["run_text"],
        )
        self.assertEqual(workflow.count("./scripts/validate.sh release"), 1)
        self.assertEqual(
            shape.jobs["release"]["scalars"]["needs"],
            "[resolve-context, release-qualification]",
        )
        self.assertEqual(
            shape.jobs["prepare-release"]["scalars"]["needs"],
            "[resolve-context, release-qualification, release, release-notes]",
        )
        self.assertEqual(
            shape.jobs["create-tag"]["scalars"]["needs"],
            "[resolve-context, release-qualification, prepare-release]",
        )
        self.assertLess(
            workflow.index("./scripts/validate.sh release"),
            workflow.index("\n  release:\n"),
        )
        self.assertLess(
            workflow.index("sha256sum -c SHA256SUMS"),
            workflow.index("\n  create-tag:\n"),
        )
        self.assertNotIn("privileged-acceptance:", workflow)
        self.assertNotIn("--profile all", workflow)

    def test_release_targets_and_preparation_contract_remain(self):
        shape = WorkflowShape(workflow_text())
        build = shape.jobs["release"]["run_text"]
        prepare = shape.jobs["prepare-release"]["run_text"]
        self.assertIn("cargo build -p chronicle-cli --release --locked", build)
        self.assertIn("capture.object", build)
        self.assertIn("capture.programs", build)
        self.assertIn("dist/chronicle-${VERSION}-${TARGET}.tar.gz", build)
        self.assertIn("x86_64-unknown-linux-gnu", prepare)
        self.assertIn("aarch64-unknown-linux-gnu", prepare)
        self.assertIn("release-notes.md", prepare)
        self.assertIn("SHA256SUMS", prepare)
        self.assertIn("name: prepared-release", workflow_text())

    def test_tag_boundary_targets_frozen_sha_without_force_update(self):
        workflow = workflow_text()
        state = MANAGE_STATE.read_text(encoding="utf-8")
        shape = WorkflowShape(workflow)
        self.assertEqual(
            shape.jobs["create-tag"]["scalars"]["if"],
            "needs.resolve-context.outputs.mode == 'release'",
        )
        self.assertIn(
            "manage-release-state.sh ensure-tag", shape.jobs["create-tag"]["run_text"]
        )
        self.assertIn("SHA: ${{ needs.resolve-context.outputs.sha }}", workflow)
        tag_start = workflow.index("\n  create-tag:\n")
        publish_start = workflow.index("\n  publish:\n")
        tag_job = workflow[tag_start:publish_start]
        self.assertIn("ref: ${{ github.sha }}", tag_job)
        self.assertIn("refs/tags/$tag", state)
        self.assertIn('"sha=$sha"', state)
        self.assertIn("--method POST", state)
        self.assertNotIn("--force", state)
        self.assertNotIn("git tag -f", state)
        self.assertNotIn("--method PATCH", state)

    def test_prepare_mode_cannot_reach_mutation_jobs(self):
        shape = WorkflowShape(workflow_text())
        for name in ("create-tag", "publish"):
            self.assertEqual(
                shape.jobs[name]["scalars"]["if"],
                "needs.resolve-context.outputs.mode == 'release'",
            )
        self.assertNotIn("gh release create", shape.jobs["prepare-release"]["run_text"])
        self.assertNotIn(
            "manage-release-state.sh ensure-tag",
            shape.jobs["prepare-release"]["run_text"],
        )

    def test_publication_is_draft_first_and_fail_closed(self):
        workflow = workflow_text()
        shape = WorkflowShape(workflow)
        publish = shape.jobs["publish"]["run_text"]
        self.assertEqual(
            shape.jobs["publish"]["scalars"]["needs"],
            "[resolve-context, create-tag, prepare-release]",
        )
        self.assertIn("gh release create", publish)
        self.assertIn("--draft", publish)
        self.assertIn("--verify-tag", publish)
        self.assertIn("gh release upload", publish)
        self.assertIn("--clobber", publish)
        self.assertIn("gh release edit", publish)
        self.assertIn("isDraft", publish)
        self.assertIn("assets", publish)
        self.assertIn("--method PATCH", publish)
        self.assertIn("-F draft=false", publish)
        self.assertNotIn("--generate-notes", workflow)
        self.assertIn("--notes-file release-notes.md", publish)
        self.assertLess(publish.index("gh release upload"), publish.index("assets"))
        self.assertLess(publish.index("assets"), publish.index("-F draft=false"))

    def test_expected_assets_and_prerelease_state_are_verified(self):
        workflow = workflow_text()
        publish = WorkflowShape(workflow).jobs["publish"]["run_text"]
        for asset in (
            "chronicle-${VERSION}-x86_64-unknown-linux-gnu.tar.gz",
            "chronicle-${VERSION}-aarch64-unknown-linux-gnu.tar.gz",
            "SHA256SUMS",
        ):
            self.assertIn(asset, publish)
        self.assertIn("is-prerelease", workflow)
        self.assertIn("--prerelease", publish)
        self.assertIn("isPrerelease", publish)
        self.assertIn("IS_PRERELEASE", workflow)

    def test_release_notes_artifact_and_git_cliff_contract(self):
        workflow = workflow_text()
        start = workflow.index("\n  release-notes:\n")
        end = workflow.index("\n  prepare-release:\n")
        notes_job = workflow[start:end]
        self.assertIn("name: release-notes", workflow)
        self.assertIn("path: release-notes.md", workflow)
        self.assertIn("fetch-depth: 0", notes_job)
        self.assertIn("ref: ${{ github.sha }}", notes_job)
        self.assertIn("GIT_CLIFF_VERSION: 2.13.1", notes_job)
        self.assertIn("GIT_CLIFF_SHA512:", notes_job)
        self.assertIn("GITHUB_TOKEN: ${{ github.token }}", notes_job)
        self.assertIn("GITHUB_REPO: ${{ github.repository }}", notes_job)
        self.assertIn('--config cliff.toml --github-repo "$GITHUB_REPO"', notes_job)
        self.assertIn('grep -q "^## What\'s Changed$" release-notes.md', notes_job)
        self.assertNotIn("--offline", notes_job)
        self.assertIn("scripts/release/resolve-release-range.py", notes_job)
        self.assertIn(
            "VERSION: ${{ needs.resolve-context.outputs.version }}", notes_job
        )
        self.assertIn("TAG: ${{ needs.resolve-context.outputs.tag }}", notes_job)
        self.assertIn("SHA: ${{ needs.resolve-context.outputs.sha }}", notes_job)
        self.assertIn("--output release-notes.md", notes_job)
        self.assertIn("--notes-file release-notes.md", workflow)
        self.assertNotIn("--generate-notes", workflow)
        for obsolete in (
            "pi-coding-agent-action",
            "OPENCODE_GO",
            "OPENCODE_GO_API_KEY",
            "RELEASE_NOTES_MODEL",
            "RELEASE_NOTES_THINKING_LEVEL",
            ".github/prompts/release-notes.md",
        ):
            self.assertNotIn(obsolete, workflow)

    def test_gh_jobs_have_explicit_context_and_only_mutators_write(self):
        shape = WorkflowShape(workflow_text())
        self.assertEqual(
            sorted(shape.gh_jobs()),
            ["create-tag", "publish", "resolve-context"],
        )
        for name in shape.gh_jobs():
            self.assertIn("GH_TOKEN", shape.jobs[name]["env"])
            self.assertIn("GH_REPO", shape.jobs[name]["env"])
        write_jobs = [
            name
            for name, config in shape.jobs.items()
            if "contents: write" in config["permissions"]
        ]
        self.assertEqual(sorted(write_jobs), ["create-tag", "publish"])
        self.assertEqual(workflow_text().count("contents: write"), 2)

    def test_binary_validation_tolerates_doctor_environment_exit(self):
        validate_run = WorkflowShape(workflow_text()).jobs["release"]["run_text"]
        self.assertIn("subprocess.run(", validate_run)
        self.assertNotIn("check_output", validate_run)
        self.assertIn("capture.object", validate_run)
        self.assertIn("capture.programs", validate_run)


class ReleaseContextTests(unittest.TestCase):
    def _run(
        self, mode: str, manifest: Path, version: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [str(RESOLVE_CONTEXT), mode, str(manifest), version],
            capture_output=True,
            text=True,
        )

    @staticmethod
    def _manifest(directory: str, version: str) -> Path:
        manifest = Path(directory) / "Cargo.toml"
        manifest.write_text(
            f'[workspace.package]\nversion = "{version}"\n', encoding="utf-8"
        )
        return manifest

    def test_stable_version_matches_and_leading_v_normalizes(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = self._manifest(tmp, "0.2.0")
            for requested in ("0.2.0", "v0.2.0"):
                proc = self._run("release", manifest, requested)
                self.assertEqual(proc.returncode, 0, proc.stderr)
                self.assertIn("version=0.2.0", proc.stdout)
                self.assertIn("tag=v0.2.0", proc.stdout)
                self.assertIn("is-prerelease=false", proc.stdout)

    def test_prerelease_version_matches_and_sets_flag(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = self._manifest(tmp, "0.2.0-rc.1")
            proc = self._run("release", manifest, "0.2.0-rc.1")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("tag=v0.2.0-rc.1", proc.stdout)
            self.assertIn("is-prerelease=true", proc.stdout)

    def test_prepare_mode_resolves_same_context_without_release(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = self._manifest(tmp, "0.2.0")
            proc = self._run("prepare", manifest, "0.2.0")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("mode=prepare", proc.stdout)
            self.assertIn("is-release=false", proc.stdout)

    def test_mismatch_malformed_and_build_metadata_fail(self):
        cases = (
            ("0.2.1", "requested version"),
            ("0.2", "malformed"),
            ("0.2.0+build.1", "malformed"),
            ("0.2.0-01", "malformed"),
        )
        for version, expected in cases:
            with self.subTest(version=version), tempfile.TemporaryDirectory() as tmp:
                manifest = self._manifest(tmp, "0.2.0")
                proc = self._run("release", manifest, version)
                self.assertNotEqual(proc.returncode, 0)
                self.assertIn(expected, proc.stderr)

    def test_workspace_build_metadata_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = self._manifest(tmp, "0.2.0+build.1")
            proc = self._run("release", manifest, "0.2.0+build.1")
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("build metadata", proc.stderr)

    def test_missing_workspace_version_or_manifest_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('[workspace]\nresolver = "2"\n', encoding="utf-8")
            proc = self._run("prepare", manifest, "0.2.0")
            self.assertNotEqual(proc.returncode, 0)
            missing = self._run("prepare", Path(tmp) / "missing.toml", "0.2.0")
            self.assertNotEqual(missing.returncode, 0)

    def test_workflow_uses_requested_version_resolver_and_docs_snapshot(self):
        workflow = workflow_text()
        self.assertIn("REQUESTED_VERSION", workflow)
        self.assertIn("scripts/release/resolve-release-context.sh", workflow)
        self.assertIn("website/scripts/verify-release-docs.mjs", workflow)


class ReleaseStateTests(unittest.TestCase):
    @staticmethod
    def _fake_gh(directory: Path) -> Path:
        fake = directory / "gh"
        fake.write_text(
            """#!/usr/bin/env python3
import os
import pathlib
import sys

args = sys.argv[1:]
endpoint = next((value for value in args if value.startswith('repos/')), '')
state = os.environ['FAKE_STATE']
expected = os.environ['EXPECTED_SHA']
created = pathlib.Path(os.environ.get('FAKE_CREATED', ''))

if '--method' in args and 'POST' in args:
    created.write_text('created', encoding='utf-8')
    print('{}')
    raise SystemExit(0)
if '/git/ref/tags/' in endpoint:
    if state == 'missing' and not created.exists():
        print('gh: Not Found (HTTP 404)', file=sys.stderr)
        raise SystemExit(1)
    print('different' if state == 'different' else expected)
    raise SystemExit(0)
if '/releases/tags/' in endpoint:
    if state == 'draft':
        print('true')
    elif state == 'published':
        print('false')
    else:
        print('gh: Not Found (HTTP 404)', file=sys.stderr)
        raise SystemExit(1)
    raise SystemExit(0)
print('unexpected fake gh request', args, file=sys.stderr)
raise SystemExit(2)
""",
            encoding="utf-8",
        )
        fake.chmod(fake.stat().st_mode | stat.S_IXUSR)
        return fake

    def _run_state(self, command: str, state: str) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as tmp:
            directory = Path(tmp)
            self._fake_gh(directory)
            created = directory / "created"
            env = dict(os.environ)
            env.update(
                {
                    "PATH": f"{directory}{os.pathsep}{env['PATH']}",
                    "GH_REPO": "example/chronicle",
                    "GH_TOKEN": "test-token",
                    "FAKE_STATE": state,
                    "EXPECTED_SHA": "abc123",
                    "FAKE_CREATED": str(created),
                }
            )
            proc = subprocess.run(
                [str(MANAGE_STATE), command, "v0.2.0", "abc123"],
                capture_output=True,
                text=True,
                env=env,
            )
            return proc

    def test_same_sha_tag_and_draft_release_are_recoverable(self):
        proc = self._run_state("inspect", "draft")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("tag-exists=true", proc.stdout)
        self.assertIn("release-state=draft", proc.stdout)

    def test_different_sha_tag_fails(self):
        proc = self._run_state("inspect", "different")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("expected frozen SHA", proc.stderr)

    def test_published_release_fails_without_overwrite(self):
        proc = self._run_state("inspect", "published")
        self.assertNotEqual(proc.returncode, 0)
        self.assertIn("refusing overwrite", proc.stderr)

    def test_missing_tag_is_created_once_and_verified(self):
        proc = self._run_state("ensure-tag", "missing")
        self.assertEqual(proc.returncode, 0, proc.stderr)
        self.assertIn("tag=v0.2.0", proc.stdout)
        self.assertIn("sha=abc123", proc.stdout)


class CliffConfigTests(unittest.TestCase):
    def test_cliff_config_is_repository_managed_and_strict(self):
        self.assertTrue(CLIFF_CONFIG.is_file())
        raw = CLIFF_CONFIG.read_text(encoding="utf-8")
        config = tomllib.loads(raw)
        git = config["git"]
        self.assertTrue(git["conventional_commits"])
        self.assertTrue(git["require_conventional"])
        self.assertTrue(git["fail_on_unmatched_commit"])
        self.assertTrue(git["protect_breaking_commits"])
        parsers = git["commit_parsers"]
        groups = {parser["group"] for parser in parsers if "group" in parser}
        self.assertEqual(
            groups,
            {
                "<!-- 0 -->Breaking Changes",
                "<!-- 1 -->Features",
                "<!-- 2 -->Bug Fixes",
                "<!-- 3 -->Performance",
                "<!-- 4 -->Reverts",
            },
        )
        self.assertTrue(any(parser.get("field") == "raw_message" for parser in parsers))
        self.assertTrue(
            any(
                parser.get("skip") and "docs" in parser["message"] for parser in parsers
            )
        )
        self.assertIn('owner = "r05323028"', raw)
        self.assertIn('repo = "chronicle"', raw)
        self.assertIn("commit.remote.username", raw)
        self.assertIn("commit.remote.pr_number", raw)
        self.assertIn("by @{{ commit.remote.username }}", raw)
        self.assertIn("in #{{ commit.remote.pr_number }}", raw)
        self.assertIn("github.contributors", raw)
        self.assertIn("## New Contributors", raw)
        self.assertIn('filter(attribute="is_first_time", value=true)', raw)
        self.assertNotIn("commit.remote.pr_title", raw)
        self.assertNotIn("remote.pr_labels", raw)
        for commit_type in (
            "feat",
            "fix",
            "perf",
            "refactor",
            "docs",
            "test",
            "build",
            "ci",
            "chore",
            "revert",
        ):
            self.assertIn(commit_type, raw)


class GitRepositoryMixin:
    @staticmethod
    def _git(repo: Path, *args: str) -> str:
        proc = subprocess.run(
            ["git", *args], cwd=repo, capture_output=True, text=True, check=True
        )
        return proc.stdout.strip()

    def _init_repo(self, repo: Path) -> None:
        self._git(repo, "init", "-q")
        self._git(repo, "config", "user.email", "test@example.invalid")
        self._git(repo, "config", "user.name", "Chronicle Test")

    def _commit(self, repo: Path, message: str) -> str:
        self._git(repo, "commit", "--allow-empty", "-qm", message)
        return self._git(repo, "rev-parse", "HEAD")

    def _resolve(
        self, repo: Path, version: str, tag: str, sha: str
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(RELEASE_RANGE), version, tag, sha],
            cwd=repo,
            capture_output=True,
            text=True,
        )

    @staticmethod
    def _values(proc: subprocess.CompletedProcess[str]) -> dict[str, str]:
        return dict(line.split("=", 1) for line in proc.stdout.splitlines())


class ReleaseRangeTests(GitRepositoryMixin, unittest.TestCase):
    def test_intended_tag_exclusion_keeps_range_identical(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.0")
            self._commit(repo, "feat(record): support correlation")
            candidate = self._commit(repo, "fix(replay): preserve ordering")

            absent = self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            self.assertEqual(absent.returncode, 0, absent.stderr)
            absent_values = self._values(absent)
            self.assertEqual(absent_values["previous-tag"], "v0.1.0")
            self.assertEqual(absent_values["range"], f"v0.1.0..{candidate}")

            self._git(repo, "tag", "v0.1.1", candidate)
            present = self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            self.assertEqual(present.returncode, 0, present.stderr)
            self.assertEqual(present.stdout, absent.stdout)

    def test_prerelease_semver_selects_previous_prerelease(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.0")
            self._commit(repo, "feat: prepare release candidate")
            self._git(repo, "tag", "v0.2.0-rc.1")
            candidate = self._commit(repo, "fix: stabilize release candidate")
            proc = self._resolve(repo, "0.2.0-rc.2", "v0.2.0-rc.2", candidate)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertEqual(self._values(proc)["previous-tag"], "v0.2.0-rc.1")

    def test_initial_release_uses_full_history_range(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            candidate = self._commit(repo, "feat: initial release")
            proc = self._resolve(repo, "0.1.0", "v0.1.0", candidate)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            values = self._values(proc)
            self.assertEqual(values["previous-tag"], "")
            self.assertEqual(values["range"], candidate)

    def test_different_existing_intended_tag_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            old = self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.1", old)
            candidate = self._commit(repo, "feat: candidate")
            proc = self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("expected frozen SHA", proc.stderr)


@unittest.skipUnless(GIT_CLIFF, "git-cliff is not installed; fixture test skipped")
class GitCliffFixtureTests(GitRepositoryMixin, unittest.TestCase):
    def _render(self, repo: Path, tag: str, release_range: str, output: Path):
        cliff = GIT_CLIFF or "git-cliff"
        return subprocess.run(
            [
                cliff,
                "--offline",
                "--config",
                str(CLIFF_CONFIG),
                "--github-repo",
                "r05323028/chronicle",
                "--tag",
                tag,
                "--output",
                str(output),
                release_range,
            ],
            cwd=repo,
            capture_output=True,
            text=True,
        )

    def test_groups_breaking_and_retry_output_deterministically(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            output = Path(tmp) / "release-notes.md"
            retry_output = Path(tmp) / "release-notes-retry.md"
            self._init_repo(repo)
            self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.0")
            for message in (
                "feat(record): support correlation",
                "test(record): add correlation fixtures",
                "docs(record): document correlation semantics",
                "fix(replay): preserve ordering",
                "ci(release): improve publication gate",
                "refactor(wal): split checkpoint writer",
                "refactor(etl)!: reorganize internals",
                "perf(wal): reduce checkpoint writes",
                "build: update packaging",
                "chore: update metadata",
                "feat(cli)!: remove legacy replay syntax",
                "revert: restore replay guard",
            ):
                candidate = self._commit(repo, message)

            first_range = self._values(
                self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            )["range"]
            first = self._render(repo, "v0.1.1", first_range, output)
            self.assertEqual(first.returncode, 0, first.stderr)
            notes = output.read_text(encoding="utf-8")
            self.assertIn("## What's Changed", notes)
            self.assertNotIn("## Highlights", notes)
            self.assertNotIn("Release v0.1.1.", notes)
            for heading in (
                "### Breaking Changes",
                "### Features",
                "### Bug Fixes",
                "### Performance",
                "### Reverts",
            ):
                self.assertIn(heading, notes)
            heading_positions = [
                notes.index(heading)
                for heading in (
                    "### Breaking Changes",
                    "### Features",
                    "### Bug Fixes",
                    "### Performance",
                    "### Reverts",
                )
            ]
            self.assertEqual(heading_positions, sorted(heading_positions))
            self.assertIn("- **record:** Support correlation", notes)
            self.assertIn("- **replay:** Preserve ordering", notes)
            self.assertIn("- **wal:** Reduce checkpoint writes", notes)
            self.assertIn("- **cli:** Remove legacy replay syntax", notes)
            self.assertIn("- **etl:** Reorganize internals", notes)
            self.assertIn("- Restore replay guard", notes)
            for excluded in (
                "correlation fixtures",
                "document correlation semantics",
                "improve publication gate",
                "split checkpoint writer",
                "update packaging",
                "update metadata",
            ):
                self.assertNotIn(excluded, notes)
            self.assertNotIn("[**breaking**]", notes)
            self.assertNotIn("initial", notes.lower())
            for prefix in ("feat(", "fix(", "perf(", "revert:"):
                self.assertNotIn(prefix, notes)

            self._git(repo, "tag", "v0.1.1", candidate)
            retry_range = self._values(
                self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            )["range"]
            self.assertEqual(retry_range, first_range)
            retry = self._render(repo, "v0.1.1", retry_range, retry_output)
            self.assertEqual(retry.returncode, 0, retry.stderr)
            self.assertEqual(
                output.read_bytes(),
                retry_output.read_bytes(),
                "same frozen history and config must render identical notes",
            )

    def test_breaking_footer_remains_visible(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.0")
            self._git(
                repo,
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "feat(cli): remove legacy syntax",
                "-m",
                "BREAKING CHANGE: legacy syntax removed",
            )
            candidate = self._git(repo, "rev-parse", "HEAD")
            release_range = self._values(
                self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            )["range"]
            output = Path(tmp) / "breaking-footer-notes.md"
            proc = self._render(repo, "v0.1.1", release_range, output)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            notes = output.read_text(encoding="utf-8")
            self.assertIn("### Breaking Changes", notes)
            self.assertIn("Remove legacy syntax", notes)

    def test_revert_remains_visible(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._init_repo(repo)
            self._commit(repo, "chore: initial")
            self._git(repo, "tag", "v0.1.0")
            candidate = self._commit(repo, "revert: restore ordering")
            release_range = self._values(
                self._resolve(repo, "0.1.1", "v0.1.1", candidate)
            )["range"]
            output = Path(tmp) / "revert-notes.md"
            proc = self._render(repo, "v0.1.1", release_range, output)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            notes = output.read_text(encoding="utf-8")
            self.assertIn("### Reverts", notes)
            self.assertIn("Restore ordering", notes)

    def test_invalid_commits_fail_in_release_range(self):
        for invalid in (
            "Update stuff",
            "fix tests",
            "random message",
            "random: unsupported type",
        ):
            with self.subTest(message=invalid), tempfile.TemporaryDirectory() as tmp:
                repo = Path(tmp)
                self._init_repo(repo)
                self._commit(repo, "chore: initial")
                self._git(repo, "tag", "v0.1.0")
                candidate = self._commit(repo, invalid)
                release_range = self._values(
                    self._resolve(repo, "0.1.1", "v0.1.1", candidate)
                )["range"]
                proc = self._render(
                    repo, "v0.1.1", release_range, Path(tmp) / "notes.md"
                )
                self.assertNotEqual(proc.returncode, 0)
                self.assertRegex(proc.stderr, "conventional|matched|Unmatched")


if __name__ == "__main__":
    unittest.main()
