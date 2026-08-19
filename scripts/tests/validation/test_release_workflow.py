#!/usr/bin/env python3
"""Portable semantic tests for Chronicle's release promotion workflow."""

from __future__ import annotations

import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
PROMPT_FILE = ROOT / ".github" / "prompts" / "release-notes.md"
RESOLVE_CONTEXT = ROOT / "scripts" / "release" / "resolve-release-context.sh"
MANAGE_STATE = ROOT / "scripts" / "release" / "manage-release-state.sh"
WRITE_MODELS = ROOT / "scripts" / "release" / "write-opencode-go-models-json.py"
PI_ACTION_SHA = "0698813906fb1f23425bd742510e3080d624840d"


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


class ReleasePromptTests(unittest.TestCase):
    def test_prompt_file_is_repository_managed_and_structured(self):
        self.assertTrue(PROMPT_FILE.is_file())
        prompt = PROMPT_FILE.read_text(encoding="utf-8")
        self.assertTrue(prompt.strip())
        self.assertNotIn("${", prompt)
        self.assertNotIn("```", prompt)
        for heading in (
            "## Highlights",
            "## Installation",
            "## What's included",
            "## Known limitations",
        ):
            self.assertIn(heading, prompt)
        self.assertIn("before tag promotion", prompt)

    def test_prompt_is_not_inlined(self):
        prompt = PROMPT_FILE.read_text(encoding="utf-8")
        workflow = workflow_text()
        self.assertIn(".github/prompts/release-notes.md", workflow)
        for marker in ("What to determine", "Output rules", "## Highlights"):
            self.assertNotIn(marker, workflow)
        self.assertIn("Cargo.toml", prompt)


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

    def test_release_notes_artifact_and_action_pin_are_preserved(self):
        workflow = workflow_text()
        self.assertIn("name: release-notes", workflow)
        self.assertIn("path: release-notes.md", workflow)
        self.assertIn("RELEASE_NOTES_RESPONSE", workflow)
        self.assertNotIn("CHRONICLE_NOTES_EOF", workflow)
        normalized = re.sub(r"\\\s*\n\s*", "", workflow)
        pins = re.findall(r"shaftoe/pi-coding-agent-action@([0-9a-fA-F]+)", normalized)
        self.assertEqual(pins, [PI_ACTION_SHA])

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


class OpenCodeGoProviderTests(unittest.TestCase):
    def _write_models(
        self, env: dict[str, str], home: Path
    ) -> subprocess.CompletedProcess[str]:
        full_env = dict(os.environ)
        full_env["HOME"] = str(home)
        full_env.update(env)
        return subprocess.run(
            [sys.executable, str(WRITE_MODELS)],
            capture_output=True,
            text=True,
            env=full_env,
        )

    def test_models_json_is_env_driven(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            env = {
                "OPENCODE_GO_BASE_URL": "https://example.invalid/v1",
                "RELEASE_NOTES_MODEL": "some-model",
                "OPENCODE_GO_API_KEY": "super-secret-value-never-inlined",
            }
            proc = self._write_models(env, home)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            config = json.loads(
                (home / ".pi" / "agent" / "models.json").read_text(encoding="utf-8")
            )
            provider = config["providers"]["opencode-go"]
            self.assertEqual(provider["baseUrl"], "https://example.invalid/v1")
            self.assertEqual(provider["api"], "openai-completions")
            self.assertTrue(provider["authHeader"])
            self.assertEqual(provider["models"], [{"id": "some-model"}])
            self.assertEqual(provider["apiKey"], "$OPENCODE_GO_API_KEY")
            self.assertNotIn(
                "super-secret-value-never-inlined",
                (home / ".pi" / "agent" / "models.json").read_text(encoding="utf-8"),
            )

    def test_models_json_fails_without_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = self._write_models({}, Path(tmp))
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("OPENCODE_GO_BASE_URL", proc.stderr)

    def test_workflow_keeps_provider_and_secret_boundaries(self):
        workflow = workflow_text()
        self.assertIn("scripts/release/write-opencode-go-models-json.py", workflow)
        self.assertIn("provider: opencode-go", workflow)
        self.assertIn("model: ${{ env.RELEASE_NOTES_MODEL }}", workflow)
        self.assertIn(
            "thinking_level: ${{ env.RELEASE_NOTES_THINKING_LEVEL }}", workflow
        )
        self.assertIn("load_builtin_extensions: false", workflow)
        self.assertEqual(workflow.count("OPENCODE_GO_BASE_URL:"), 1)
        self.assertEqual(workflow.count("RELEASE_NOTES_MODEL:"), 1)
        self.assertEqual(workflow.count("RELEASE_NOTES_THINKING_LEVEL:"), 1)
        self.assertEqual(workflow.count("secrets.OPENCODE_GO_API_KEY"), 1)
        self.assertNotIn('"$OPENCODE_GO_API_KEY"', workflow)


if __name__ == "__main__":
    unittest.main()
