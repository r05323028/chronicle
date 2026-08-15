#!/usr/bin/env python3
"""Standard-library tests for the Chronicle workspace architecture checker."""

import importlib.util
import shutil
import sys
import tempfile
import tomllib
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


class ArchitectureBoundaryTests(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-architecture-"))
        (self.temp / "validation").mkdir()
        shutil.copy2(
            ROOT / "validation/architecture.toml",
            self.temp / "validation/architecture.toml",
        )
        self.policy = self.temp / "validation/architecture.toml"

    def tearDown(self):
        shutil.rmtree(self.temp)

    def write_workspace(self, crates: dict[str, dict]) -> None:
        """Write a minimal Cargo workspace: name -> dependency table dict.

        Each dependency entry is a dict with keys: package (target name),
        rename (local name, optional), kind (normal/dev/build), optional,
        target (cfg condition string).
        """
        (self.temp / "Cargo.toml").write_text(
            '[workspace]\nresolver = "2"\nmembers = ["crates/*"]\n'
        )
        for name, deps in crates.items():
            crate_dir = self.temp / "crates" / name.removeprefix("chronicle-")
            crate_dir.mkdir(parents=True, exist_ok=True)
            normal: list[str] = []
            dev: list[str] = []
            build: list[str] = []
            target_tables: dict[str, list[str]] = {}
            for dep in deps:
                target = dep.get("target")
                body = f"{{ path = \"../{dep['package'].removeprefix('chronicle-')}\""
                if dep.get("optional"):
                    body += ", optional = true"
                body += " }"
                if dep.get("rename"):
                    body = (
                        '{ path = "../'
                        + dep["package"].removeprefix("chronicle-")
                        + '", package = "'
                        + dep["package"]
                        + '" }'
                    )
                    line = f"{dep['rename']} = {body}"
                else:
                    line = f"{dep['package']} = {body}"
                kind = dep.get("kind", "normal")
                if target:
                    target_tables.setdefault(target, []).append(line)
                elif kind == "normal":
                    normal.append(line)
                elif kind == "dev":
                    dev.append(line)
                else:
                    build.append(line)
            manifest = (
                f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n\n'
            )
            if normal:
                manifest += "[dependencies]\n" + "\n".join(normal) + "\n\n"
            if dev:
                manifest += "[dev-dependencies]\n" + "\n".join(dev) + "\n\n"
            if build:
                manifest += "[build-dependencies]\n" + "\n".join(build) + "\n\n"
            for condition, lines in sorted(target_tables.items()):
                manifest += (
                    f"[target.'{condition}'.dependencies]\n" + "\n".join(lines) + "\n\n"
                )
            (crate_dir / "Cargo.toml").write_text(manifest)
            (crate_dir / "src").mkdir(exist_ok=True)
            (crate_dir / "src" / "lib.rs").write_text("pub fn ping() {}\n")

    def issues_for(self, crates: dict[str, dict]) -> list[str]:
        self.write_workspace(crates)
        return validation.architecture_check(self.temp, self.policy)["issues"]

    def test_accepted_graph(self):
        crates = {
            "chronicle-common": {},
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-capture": [{"package": "chronicle-common"}],
        }
        self.assertEqual(self.issues_for(crates), [])

    def test_forbidden_normal_session_to_wal(self):
        crates = {
            "chronicle-common": {},
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-wal": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
                {"package": "chronicle-wal"},
            ],
        }
        issues = self.issues_for(crates)
        self.assertTrue(any("session-to-wal" in issue for issue in issues))
        self.assertTrue(
            any(
                "chronicle-session -> chronicle-wal [normal]" in issue
                for issue in issues
            )
        )

    def test_forbidden_dev_edge(self):
        crates = {
            "chronicle-common": {},
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-wal": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
                {"package": "chronicle-wal", "kind": "dev"},
            ],
        }
        issues = self.issues_for(crates)
        self.assertTrue(any("session-to-wal" in issue for issue in issues))
        self.assertTrue(any("[dev]" in issue for issue in issues))

    def test_forbidden_build_edge(self):
        crates = {
            "chronicle-common": {},
            "chronicle-protocol": [{"package": "chronicle-common"}],
            "chronicle-protocol-builtins": [
                {"package": "chronicle-protocol"},
                {"package": "chronicle-common"},
            ],
            "chronicle-session": [{"package": "chronicle-common"}],
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-replay": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-protocol"},
                {"package": "chronicle-session", "kind": "build"},
            ],
        }
        issues = self.issues_for(crates)
        self.assertTrue(any("[build]" in issue for issue in issues))

    def test_target_specific_optional_forbidden_edge_detected(self):
        # A forbidden edge declared only for Linux must be discovered even on
        # a non-Linux host because policy checks declarations, not resolution.
        crates = {
            "chronicle-common": {},
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-wal": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-protocol": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-session"},
            ],
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-protocol-builtins": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-protocol"},
                {
                    "package": "chronicle-wal",
                    "optional": True,
                    "target": 'cfg(target_os = "linux")',
                },
            ],
        }
        issues = self.issues_for(crates)
        self.assertTrue(
            any(
                "chronicle-protocol-builtins -> chronicle-wal" in issue
                for issue in issues
            )
        )
        self.assertTrue(any("[optional]" in issue for issue in issues))
        self.assertTrue(any("[cfg(target_os" in issue for issue in issues))

    def test_renamed_package_identity(self):
        # Renaming the local dependency does not change package identity; an
        # allowed rename of a permitted target passes, and a forbidden target
        # fails regardless of the local name.
        crates = {
            "chronicle-common": {},
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common", "rename": "shared"},
            ],
        }
        self.assertEqual(self.issues_for(crates), [])
        crates["chronicle-session"].append(
            {"package": "chronicle-wal", "rename": "storage_wire"}
        )
        crates["chronicle-wal"] = [
            {"package": "chronicle-capture"},
            {"package": "chronicle-common"},
        ]
        issues = self.issues_for(crates)
        self.assertTrue(
            any("chronicle-session -> chronicle-wal" in issue for issue in issues)
        )

    def test_dependency_on_cli_rejected(self):
        crates = {
            "chronicle-common": {},
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-session": [{"package": "chronicle-common"}],
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-wal": [{"package": "chronicle-common"}],
            "chronicle-application": [{"package": "chronicle-common"}],
            "chronicle-cli": [
                {"package": "chronicle-application"},
                {"package": "chronicle-common"},
            ],
            "chronicle-etl": [{"package": "chronicle-common"}],
        }
        issues = self.issues_for(crates)
        # cli -> common is forbidden (cli non-application edge).
        self.assertTrue(any("cli non-application" in issue for issue in issues))

    def test_cli_dependency_on_application_only_accepted(self):
        crates = {
            "chronicle-common": {},
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-session": [{"package": "chronicle-common"}],
            "chronicle-application": [
                {"package": "chronicle-common"},
                {"package": "chronicle-canonical"},
                {"package": "chronicle-capture"},
                {"package": "chronicle-session"},
            ],
            "chronicle-cli": [{"package": "chronicle-application"}],
        }
        self.assertEqual(self.issues_for(crates), [])

    def test_unknown_policy_member(self):
        crates = {
            "chronicle-common": {},
            "chronicle-newcrate": [{"package": "chronicle-common"}],
        }
        issues = self.issues_for(crates)
        self.assertTrue(
            any("no architecture policy member" in issue for issue in issues)
        )

    def test_cycle_detected(self):
        crates = {
            "chronicle-common": {},
            "chronicle-capture": [{"package": "chronicle-common"}],
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
            ],
            "chronicle-canonical": [{"package": "chronicle-common"}],
            "chronicle-protocol": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-session"},
            ],
            "chronicle-protocol-builtins": [
                {"package": "chronicle-protocol"},
                {"package": "chronicle-common"},
            ],
            "chronicle-replay": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-protocol"},
            ],
            "chronicle-etl": [
                {"package": "chronicle-canonical"},
                {"package": "chronicle-common"},
                {"package": "chronicle-protocol"},
                {"package": "chronicle-session"},
            ],
            "chronicle-application": [{"package": "chronicle-etl"}],
            # cycle: application -> etl -> ... -> application via common? no -
            # introduce an explicit cycle through allowed edges is impossible;
            # use a forbidden self-cycle via dev to exercise cycle detection.
            "chronicle-session": [
                {"package": "chronicle-capture"},
                {"package": "chronicle-common"},
                {"package": "chronicle-session", "kind": "dev"},
            ],
        }
        issues = self.issues_for(crates)
        self.assertTrue(any("dependency cycle" in issue for issue in issues))

    def test_missing_policy_allowlist_table(self):
        # A member without a dev allowlist table must be reported. Write a
        # minimal valid policy that omits the [dev] table for chronicle-common.
        policy = (
            "version = 1\n"
            "\n"
            "[members]\n"
            'members = ["chronicle-common"]\n'
            "\n"
            "[normal]\n"
            '"chronicle-common" = []\n'
            "\n"
            "[build]\n"
            '"chronicle-common" = []\n'
        )
        self.policy.write_text(policy)
        crates = {"chronicle-common": {}}
        self.write_workspace(crates)
        issues = validation.architecture_check(self.temp, self.policy)["issues"]
        self.assertTrue(any("missing dev allowlist" in issue for issue in issues))


class SemanticBoundaryTests(unittest.TestCase):
    """Semantic/API boundary checks (S1/S2/policy self-consistency)."""

    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-semantic-"))
        with (ROOT / "validation/architecture.toml").open("rb") as handle:
            self.policy = tomllib.load(handle)["semantic"]
        # Defining crates mirror the real repo so policy self-consistency
        # (every forbidden symbol resolves to a forbidden source crate) passes.
        self.write(
            "crates/chronicle-replay/src/lib.rs",
            "pub struct LoopbackReplayOptions;\n"
            "pub enum ReplayOutcome { Completed }\n"
            "pub enum Replayability { FullyReplayable }\n"
            "pub enum TimingMode { Asap }\n"
            "pub enum OperationExecutionState { NotAttempted }\n"
            "pub enum ReplayError { PreflightDenied }\n",
        )
        self.write(
            "crates/chronicle-protocol/src/lib.rs",
            "pub enum TransportErrorCategory { Timeout }\n"
            "pub enum ProtocolError { Transport { category: TransportErrorCategory } }\n",
        )

    def tearDown(self):
        shutil.rmtree(self.temp)

    def write(self, rel: str, text: str) -> None:
        path = self.temp / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def issues(self) -> list[str]:
        return validation.semantic_check(self.temp, self.policy)

    def test_application_owned_result_type_passes(self):
        self.write(
            "crates/chronicle-application/src/replay_inspect.rs",
            "pub enum ReplayStatus { Completed }\n"
            "pub struct ReplaySessionResult { pub outcome: ReplayStatus }\n",
        )
        self.write(
            "crates/chronicle-cli/src/main.rs",
            "use chronicle_application::ReplaySessionResult;\n",
        )
        self.assertEqual(self.issues(), [])

    def test_common_primitives_re_export_passes(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_common::{RecordingId, SessionId, Timestamp};\n",
        )
        self.assertEqual(self.issues(), [])

    def test_internal_application_module_uses_lower_layer_without_re_export(self):
        self.write(
            "crates/chronicle-application/src/command_replay.rs",
            "use chronicle_replay::LoopbackReplayOptions;\n",
        )
        self.assertEqual(self.issues(), [])

    def test_re_export_of_forbidden_symbol_fails(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_replay::ReplayOutcome;\n",
        )
        issues = self.issues()
        self.assertTrue(any("semantic-boundary" in issue for issue in issues))
        self.assertTrue(any("lib.rs" in issue for issue in issues))

    def test_multiline_re_export_block_reported_once(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_replay::{\n    ReplayOutcome,\n    Replayability,\n};\n",
        )
        issues = self.issues()
        self.assertEqual(
            len([issue for issue in issues if "semantic-boundary" in issue]), 1
        )

    def test_bare_crate_re_export_fails(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_replay;\n",
        )
        self.assertTrue(any("semantic-boundary" in issue for issue in self.issues()))

    def test_glob_re_export_fails(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_replay::*;\n",
        )
        self.assertTrue(any("semantic-boundary" in issue for issue in self.issues()))

    def test_cli_vocabulary_import_fails(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub enum ReplayOutcome { Completed }\n"
            "pub struct LoopbackReplayOptions;\n",
        )
        self.write(
            "crates/chronicle-cli/src/main.rs",
            "use chronicle_application::{ReplayOutcome, LoopbackReplayOptions};\n",
        )
        issues = self.issues()
        self.assertEqual(
            len([issue for issue in issues if "semantic-boundary" in issue]), 2
        )
        self.assertTrue(any("main.rs" in issue for issue in issues))

    def test_comment_mention_passes(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub enum ReplayOutcome { Completed }\n",
        )
        self.write(
            "crates/chronicle-cli/src/main.rs",
            "// ReplayOutcome must not be used here\n"
            "use chronicle_application::ReplaySessionResult;\n",
        )
        self.assertEqual(self.issues(), [])

    def test_identifier_containing_forbidden_name_passes(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub enum ReplayOutcome { Completed }\n"
            "pub enum Replayability { FullyReplayable }\n",
        )
        self.write(
            "crates/chronicle-cli/src/main.rs",
            "use chronicle_application::{ReplayabilityStatus, ReplayStatus};\n",
        )
        self.assertEqual(self.issues(), [])

    def test_violation_message_has_four_elements(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub use chronicle_replay::ReplayOutcome;\n",
        )
        message = next(issue for issue in self.issues() if "semantic-boundary" in issue)
        for element in ("semantic-boundary", "location:", "rationale:", "remediation:"):
            self.assertIn(element, message)

    def test_policy_self_consistency_unresolvable_symbol_fails(self):
        self.write(
            "crates/chronicle-application/src/lib.rs",
            "pub enum SomeOtherSymbol { X }\n",
        )
        policy = {**self.policy, "forbidden_outer_vocabulary": ["GhostSymbol"]}
        issues = validation.semantic_check(self.temp, policy)
        self.assertTrue(any("does not resolve" in issue for issue in issues))

    def test_allowlist_entry_requires_crate_scope(self):
        policy = {**self.policy, "allowed_re_exports": ["ReplayOutcome"]}
        issues = validation.semantic_check(self.temp, policy)
        self.assertTrue(any("must be crate::Symbol" in issue for issue in issues))


if __name__ == "__main__":
    unittest.main()
