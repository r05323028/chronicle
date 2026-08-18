#!/usr/bin/env python3
"""Standard-library tests for the release publication workflow.

Protects meaningful release-workflow behavior without duplicating the workflow
YAML line-by-line:

- .github/prompts/release-notes.md exists and holds the prompt (not the
  workflow): non-empty, no GitHub Actions expressions, no code fence, not
  inlined in release.yml.
- workflow_dispatch (dry run) is supported and can never publish: gh release
  create (and other release-mutating gh commands) live only in the publish
  job, which is gated to a real tag push
  (github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')).
- Publication consumes the Pi-generated notes and prepared bundle:
  --notes-file release-notes.md, --verify-tag, never --generate-notes.
- Checksums are generated and verified in prepare-release before publication;
  publish depends on prepare-release.
- Version/tag context is resolved by scripts/release/resolve-release-context.sh:
  release mode requires v<workspace-version> == tag; dry-run mode derives the
  intended version/tag from the root Cargo.toml without any Git tag.
- OpenCode Go provider configuration is environment-variable-driven
  (scripts/release/write-opencode-go-models-json.py); the secret is never
  inlined.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
RELEASE_WORKFLOW = ROOT / ".github" / "workflows" / "release.yml"
PROMPT_FILE = ROOT / ".github" / "prompts" / "release-notes.md"
RESOLVE_CONTEXT = ROOT / "scripts" / "release" / "resolve-release-context.sh"
WRITE_MODELS = ROOT / "scripts" / "release" / "write-opencode-go-models-json.py"

PUBLISH_GATE = "github.event_name == 'push' && startsWith(github.ref, 'refs/tags/v')"
PI_ACTION_SHA = "0698813906fb1f23425bd742510e3080d624840d"


def workflow_text() -> str:
    return RELEASE_WORKFLOW.read_text(encoding="utf-8")


class WorkflowShape:
    """Minimal indentation scan of the release workflow.

    Captures, per job: job-level scalar keys (if/needs/runs-on/...), job-level
    env keys, job-level permission lines, and the concatenated text of all run
    steps. Property-based: the test asserts properties of this shape rather
    than duplicating the whole YAML.
    """

    def __init__(self, text: str) -> None:
        self.jobs: dict[str, dict] = {}
        job = None
        block = None  # "env" | "permissions" | "steps" | "outputs" | None
        current_step: list[str] = []
        run_lines: list[str] = []

        for raw in text.splitlines():
            if not raw.strip() or raw.lstrip().startswith("#"):
                continue
            indent = len(raw) - len(raw.lstrip())
            s = raw.strip()
            if indent == 2 and re.match(r"^[A-Za-z0-9_.-]+:$", s):
                if job is not None:
                    self.jobs[job]["run_text"] = "\n".join(run_lines)
                job = s[:-1]
                self.jobs[job] = {
                    "env": set(),
                    "permissions": set(),
                    "scalars": {},
                }  # permissions holds "key: value" lines
                block = None
                run_lines = []
            elif job is None:
                continue
            elif indent == 4 and s in ("env:", "permissions:", "steps:", "outputs:"):
                block = s[:-1]
            elif indent == 4 and ":" in s:
                block = None
                key, _, value = s.partition(":")
                self.jobs[job]["scalars"][key.strip()] = value.strip()
            elif indent == 6 and block == "env":
                self.jobs[job]["env"].add(s.split(":", 1)[0].strip())
            elif indent == 6 and block == "permissions":
                self.jobs[job]["permissions"].add(s)
            elif indent == 6 and s.startswith("- "):
                block = None
                current_step = [s]
            elif indent >= 8 and current_step:
                current_step.append(s)
            if current_step and any(line.startswith("run:") for line in current_step):
                run_lines.append(s)
        if job is not None:
            self.jobs[job]["run_text"] = "\n".join(run_lines)

    def gh_jobs(self) -> list[str]:
        return [
            name
            for name, cfg in self.jobs.items()
            if re.search(r"\bgh\s", cfg.get("run_text", ""))
        ]


class ReleasePromptTests(unittest.TestCase):
    def test_prompt_file_exists_and_nonempty(self):
        self.assertTrue(
            PROMPT_FILE.is_file(), "missing .github/prompts/release-notes.md"
        )
        self.assertGreater(len(PROMPT_FILE.read_text(encoding="utf-8").strip()), 0)

    def test_prompt_has_no_github_actions_expressions(self):
        self.assertNotIn("${", PROMPT_FILE.read_text(encoding="utf-8"))

    def test_prompt_has_no_code_fence(self):
        self.assertNotIn("```", PROMPT_FILE.read_text(encoding="utf-8"))

    def test_prompt_not_inlined_in_workflow(self):
        prompt = PROMPT_FILE.read_text(encoding="utf-8")
        wf = workflow_text()
        for marker in ("What to determine", "Output rules", "## Highlights"):
            self.assertIn(marker, prompt, f"prompt marker missing: {marker}")
            self.assertNotIn(
                marker, wf, f"prompt content inlined in release.yml: {marker}"
            )
        self.assertIn(".github/prompts/release-notes.md", wf)

    def test_prompt_has_required_structure_headings(self):
        prompt = PROMPT_FILE.read_text(encoding="utf-8")
        for heading in (
            "## Highlights",
            "## Installation",
            "## What's included",
            "## Known limitations",
        ):
            self.assertIn(heading, prompt)

    def test_prompt_handles_tagless_dry_run_context(self):
        prompt = PROMPT_FILE.read_text(encoding="utf-8")
        self.assertIn("Cargo.toml", prompt)
        self.assertIn("dry run", prompt)


class ReleasePublicationTests(unittest.TestCase):
    def test_no_generate_notes(self):
        self.assertNotIn("--generate-notes", workflow_text())

    def test_publication_consumes_release_notes(self):
        wf = workflow_text()
        self.assertIn("--notes-file release-notes.md", wf)
        self.assertIn("--verify-tag", wf)

    def test_release_notes_artifact_uploaded_and_downloaded(self):
        wf = workflow_text()
        self.assertIn("name: release-notes", wf)
        self.assertIn("path: release-notes.md", wf)

    def test_gh_actions_pin_is_immutable_stable(self):
        pins = re.findall(
            r"uses:\s*(?:>-\s*)?shaftoe/pi-coding-agent-action@([0-9a-fA-F]+)",
            workflow_text(),
        )
        self.assertTrue(pins, "pi-coding-agent-action not referenced")
        for pin in pins:
            self.assertRegex(
                pin, r"^[0-9a-fA-F]{40}$", f"non-immutable action reference: {pin}"
            )
        self.assertEqual(pins, [PI_ACTION_SHA])

    def test_publish_is_thin_and_depends_on_prepare_release(self):
        shape = WorkflowShape(workflow_text())
        self.assertEqual(
            shape.jobs["publish"]["scalars"].get("needs"),
            "[resolve-context, prepare-release]",
        )
        self.assertEqual(
            shape.jobs["prepare-release"]["scalars"].get("needs"),
            "[resolve-context, release, release-notes, privileged-acceptance]",
        )


class DryRunModeTests(unittest.TestCase):
    def test_workflow_dispatch_supported(self):
        self.assertIn("workflow_dispatch:", workflow_text())

    def test_publication_gated_to_real_tag_push(self):
        shape = WorkflowShape(workflow_text())
        self.assertEqual(shape.jobs["publish"]["scalars"].get("if"), PUBLISH_GATE)
        for name, cfg in shape.jobs.items():
            if name == "publish":
                self.assertIn("gh release create", cfg["run_text"])
            else:
                self.assertNotIn(
                    "gh release create", cfg["run_text"], f"{name} may not publish"
                )

    def test_no_release_mutating_gh_outside_publish(self):
        shape = WorkflowShape(workflow_text())
        for name, cfg in shape.jobs.items():
            if name == "publish":
                continue
            self.assertNotRegex(
                cfg["run_text"],
                r"gh release (create|upload|delete|edit|draft)",
                f"{name} mutates releases",
            )

    def test_prepare_release_checksums_before_publication(self):
        shape = WorkflowShape(workflow_text())
        prep = shape.jobs["prepare-release"]["run_text"]
        self.assertIn("sha256sum chronicle-*.tar.gz > SHA256SUMS", prep)
        # Verification must run inside the bundle directory: the manifest
        # holds bare filenames, so `sha256sum -c` from the workspace root
        # cannot find the archives.
        self.assertIn("cd prepared-release", prep)
        self.assertIn("sha256sum -c SHA256SUMS", prep)
        self.assertNotIn("sha256sum -c prepared-release/SHA256SUMS", prep)
        pub = shape.jobs["publish"]["run_text"]
        self.assertNotIn("sha256sum -c", pub)
        self.assertIn("SHA256SUMS", pub)  # publish attaches the manifest

    def test_checksum_generation_and_verification_succeed_in_bundle_dir(self):
        # Functional replica of the prepare-release checksum step: the
        # manifest must verify when checked inside the bundle directory.
        with tempfile.TemporaryDirectory() as tmp:
            bundle = Path(tmp) / "prepared-release"
            bundle.mkdir()
            for name in (
                "chronicle-0.1.0-x86_64-unknown-linux-gnu.tar.gz",
                "chronicle-0.1.0-aarch64-unknown-linux-gnu.tar.gz",
            ):
                (bundle / name).write_bytes(b"archive-content")
            script = (
                "set -euo pipefail\n"
                "(\n"
                "  cd prepared-release\n"
                "  sha256sum chronicle-*.tar.gz > SHA256SUMS\n"
                "  sha256sum -c SHA256SUMS\n"
                ")\n"
            )
            proc = subprocess.run(
                ["bash", "-c", script],
                cwd=tmp,
                capture_output=True,
                text=True,
            )
            self.assertEqual(proc.returncode, 0, proc.stderr)
            manifest = (bundle / "SHA256SUMS").read_text(encoding="utf-8")
            self.assertIn("x86_64-unknown-linux-gnu.tar.gz", manifest)
            self.assertIn("aarch64-unknown-linux-gnu.tar.gz", manifest)

    def test_prepare_release_asserts_expected_archives(self):
        shape = WorkflowShape(workflow_text())
        prep = shape.jobs["prepare-release"]["run_text"]
        self.assertIn("x86_64-unknown-linux-gnu", prep)
        self.assertIn("aarch64-unknown-linux-gnu", prep)
        self.assertIn("chronicle-${VERSION}-${target}.tar.gz", prep)
        self.assertIn("release-notes.md", prep)

    def test_prepared_release_bundle_uploaded(self):
        wf = workflow_text()
        self.assertIn("name: prepared-release", wf)
        self.assertIn("path: prepared-release", wf)

    def test_dry_run_summary_present(self):
        shape = WorkflowShape(workflow_text())
        self.assertIn("GITHUB_STEP_SUMMARY", shape.jobs["prepare-release"]["run_text"])

    def test_build_matrix_produces_versioned_archives(self):
        shape = WorkflowShape(workflow_text())
        release_run = shape.jobs["release"]["run_text"]
        self.assertIn("dist/chronicle-${VERSION}-${TARGET}.tar.gz", release_run)
        self.assertIn("cargo build -p chronicle-cli --release --locked", release_run)
        # Live capture is included automatically on Linux targets: the build
        # must not require (or mention) an implementation feature flag.
        self.assertNotIn("--features", release_run)
        self.assertNotIn("linux-ebpf", release_run)

    def test_binary_validation_tolerates_doctor_environment_exit(self):
        # doctor exits nonzero when the runner lacks kernel/privilege
        # prerequisites (BTF, cgroup v2, CAP_BPF). The release step must
        # parse the JSON report instead of dying on check_output so the
        # embedded-capture-object probes can gate the artifact.
        shape = WorkflowShape(workflow_text())
        validate_run = shape.jobs["release"]["run_text"]
        self.assertIn("subprocess.run(", validate_run)
        self.assertNotIn("check_output", validate_run)
        self.assertIn("capture.object", validate_run)
        self.assertIn("capture.programs", validate_run)


class GhContextTests(unittest.TestCase):
    def test_every_gh_job_has_explicit_auth_and_repo(self):
        shape = WorkflowShape(workflow_text())
        gh_jobs = shape.gh_jobs()
        self.assertTrue(gh_jobs, "expected at least one gh-invoking job")
        for name in gh_jobs:
            cfg = shape.jobs[name]
            self.assertIn("GH_TOKEN", cfg["env"], f"{name} missing GH_TOKEN")
            self.assertIn("GH_REPO", cfg["env"], f"{name} missing GH_REPO")

    def test_write_permission_only_on_publish(self):
        wf = workflow_text()
        shape = WorkflowShape(wf)
        write_jobs = [
            name
            for name, cfg in shape.jobs.items()
            if "contents: write" in cfg["permissions"]
        ]
        self.assertEqual(
            write_jobs, ["publish"], f"unexpected contents: write: {write_jobs}"
        )
        self.assertEqual(wf.count("contents: write"), 1)

    def test_preparation_jobs_are_read_only(self):
        shape = WorkflowShape(workflow_text())
        for name in ("resolve-context", "release", "release-notes", "prepare-release"):
            self.assertIn(name, shape.jobs)
            self.assertNotIn(
                "contents: write", shape.jobs[name]["permissions"], f"{name} has write"
            )


class ReleaseContextTests(unittest.TestCase):
    def _run(
        self, mode: str, manifest: Path, tag: str | None = None
    ) -> subprocess.CompletedProcess:
        cmd = [str(RESOLVE_CONTEXT), mode, str(manifest)]
        if tag is not None:
            cmd.append(tag)
        return subprocess.run(cmd, capture_output=True, text=True)

    def test_release_matching_tag_passes(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text(
                '[workspace.package]\nversion = "0.1.0"\n', encoding="utf-8"
            )
            proc = self._run("release", manifest, "v0.1.0")
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("mode=release", proc.stdout)
            self.assertIn("version=0.1.0", proc.stdout)
            self.assertIn("tag=v0.1.0", proc.stdout)
            self.assertIn("is-release=true", proc.stdout)

    def test_release_mismatched_tag_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('version = "0.1.0"\n', encoding="utf-8")
            proc = self._run("release", manifest, "v0.1.1")
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("0.1.1", proc.stderr)
            self.assertIn("0.1.0", proc.stderr)

    def test_release_mode_requires_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('version = "0.1.0"\n', encoding="utf-8")
            proc = self._run("release", manifest)
            self.assertNotEqual(proc.returncode, 0)

    def test_dry_run_resolves_version_and_intended_tag(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('version = "0.1.0"\n', encoding="utf-8")
            proc = self._run("dry-run", manifest)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            self.assertIn("mode=dry-run", proc.stdout)
            self.assertIn("version=0.1.0", proc.stdout)
            self.assertIn("tag=v0.1.0", proc.stdout)
            self.assertIn("is-release=false", proc.stdout)

    def test_dry_run_malformed_version_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('version = "not-a-version"\n', encoding="utf-8")
            proc = self._run("dry-run", manifest)
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("malformed", proc.stderr)

    def test_dry_run_missing_version_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            manifest = Path(tmp) / "Cargo.toml"
            manifest.write_text('[workspace]\nresolver = "2"\n', encoding="utf-8")
            proc = self._run("dry-run", manifest)
            self.assertNotEqual(proc.returncode, 0)

    def test_missing_manifest_fails(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = self._run("dry-run", Path(tmp) / "nope.toml")
            self.assertNotEqual(proc.returncode, 0)

    def test_workflow_uses_script_for_context_resolution(self):
        wf = workflow_text()
        self.assertIn("scripts/release/resolve-release-context.sh", wf)
        self.assertNotIn("validate-tag.sh", wf)  # superseded, single script


class OpenCodeGoProviderTests(unittest.TestCase):
    def _write_models(
        self, env: dict[str, str], home: Path
    ) -> subprocess.CompletedProcess:
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
            rendered = (home / ".pi" / "agent" / "models.json").read_text(
                encoding="utf-8"
            )
            self.assertNotIn("super-secret-value-never-inlined", rendered)

    def test_models_json_fails_without_env(self):
        with tempfile.TemporaryDirectory() as tmp:
            proc = self._write_models({}, Path(tmp))
            self.assertNotEqual(proc.returncode, 0)
            self.assertIn("OPENCODE_GO_BASE_URL", proc.stderr)

    def test_workflow_uses_script_and_provider_name(self):
        wf = workflow_text()
        self.assertIn("scripts/release/write-opencode-go-models-json.py", wf)
        self.assertIn("provider: opencode-go", wf)
        self.assertIn("model: ${{ env.RELEASE_NOTES_MODEL }}", wf)
        self.assertIn("thinking_level: ${{ env.RELEASE_NOTES_THINKING_LEVEL }}", wf)
        self.assertIn("load_builtin_extensions: false", wf)

    def test_workflow_level_env_holds_non_secret_values_only(self):
        wf = workflow_text()
        self.assertEqual(wf.count("OPENCODE_GO_BASE_URL:"), 1)
        self.assertEqual(wf.count("RELEASE_NOTES_MODEL:"), 1)
        self.assertEqual(wf.count("RELEASE_NOTES_THINKING_LEVEL:"), 1)
        self.assertIn("https://opencode.ai/zen/go/v1", wf)
        self.assertIn("deepseek-v4-flash", wf)
        self.assertEqual(wf.count("secrets.OPENCODE_GO_API_KEY"), 1)
        writer = WRITE_MODELS.read_text(encoding="utf-8")
        self.assertIn('"$OPENCODE_GO_API_KEY"', writer)
        self.assertNotIn('"$OPENCODE_GO_API_KEY"', wf)


if __name__ == "__main__":
    unittest.main()
