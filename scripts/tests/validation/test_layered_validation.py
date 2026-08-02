#!/usr/bin/env python3
import argparse
import contextlib
import io
import json
import shutil
import importlib.util
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


class LayeredValidationTests(unittest.TestCase):
    def setUp(self):
        self.temp = Path(tempfile.mkdtemp(prefix="chronicle-validation-"))
        (self.temp / "validation").mkdir()
        shutil.copy2(
            ROOT / "validation/groups.toml", self.temp / "validation/groups.toml"
        )
        (self.temp / "Cargo.lock").write_text("lock-v1\n")
        for path in (
            "docs/change.md",
            "unmapped/change.md",
            "crates/chronicle-etl/src/lib.rs",
            "crates/chronicle-wal/src/lib.rs",
            "ebpf/src/main.rs",
            "scripts/acceptance/p1-privileged.sh",
        ):
            target = self.temp / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(path + "\n")
        self.cfg = validation.config(self.temp)

    def tearDown(self):
        shutil.rmtree(self.temp)

    def test_unknown_path_selects_all_groups_fail_closed(self):
        result = validation.select(self.temp, ["unmapped/change.md"], self.cfg)
        self.assertEqual(result["unknown_paths"], ["unmapped/change.md"])
        self.assertEqual(result["selected"], sorted(self.cfg["groups"]))
        self.assertTrue(all(item["selected"] for item in result["decisions"].values()))

    def test_etl_does_not_select_ebpf(self):
        result = validation.select(
            self.temp, ["crates/chronicle-etl/src/lib.rs"], self.cfg
        )
        self.assertIn("etl", result["selected"])
        self.assertNotIn("ebpf", result["selected"])
        self.assertFalse(result["decisions"]["ebpf"]["selected"])

    def test_wal_selects_focused_group(self):
        result = validation.select(
            self.temp, ["crates/chronicle-wal/src/lib.rs"], self.cfg
        )
        self.assertIn("wal", result["selected"])
        self.assertNotIn("ebpf", result["selected"])

    def test_fingerprint_ignores_docs_and_invalidates_acceptance(self):
        first = validation.fingerprint(self.temp, "p1", self.cfg)["fingerprint"]
        (self.temp / "docs/change.md").write_text("unrelated\n")
        self.assertEqual(
            first, validation.fingerprint(self.temp, "p1", self.cfg)["fingerprint"]
        )
        (self.temp / "scripts/acceptance/p1-privileged.sh").write_text("changed\n")
        self.assertNotEqual(
            first, validation.fingerprint(self.temp, "p1", self.cfg)["fingerprint"]
        )

    def test_fingerprint_uses_supplied_environment(self):
        vm_environment = {
            "rustc": "rustc vm",
            "architecture": "aarch64",
            "kernel": "6.8.0",
            "ubuntu": "24.04",
            "os": "Linux",
            "target": "host",
            "btf": True,
            "cgroup_v2": True,
        }
        first = validation.fingerprint(self.temp, "p1", self.cfg, vm_environment)[
            "fingerprint"
        ]
        vm_environment["kernel"] = "6.8.1"
        self.assertNotEqual(
            first,
            validation.fingerprint(self.temp, "p1", self.cfg, vm_environment)[
                "fingerprint"
            ],
        )

    def test_no_artifact_does_not_create_output(self):
        source = self.temp / "source"
        dest = self.temp / "evidence"
        source.mkdir()
        validation.compact(
            argparse.Namespace(
                source=source,
                dest=dest,
                gate="p1",
                status="passed",
                fingerprint="fp",
                commit="sha",
                checks="P1",
                artifact_mode="no-artifact",
            )
        )
        self.assertFalse(dest.exists())

    def test_success_artifact_is_compact(self):
        source = self.temp / "source"
        dest = self.temp / "evidence"
        source.mkdir()
        (source / "acceptance-report.json").write_text('{"status":"passed"}\n')
        (source / "target").mkdir()
        (source / "target" / "large.bin").write_bytes(b"x" * 1000)
        environment_file = self.temp / "environment.json"
        environment_file.write_text('{"kernel": "vm-kernel"}\n')
        args = argparse.Namespace(
            source=source,
            dest=dest,
            gate="p1",
            status="passed",
            fingerprint="fp",
            commit="sha",
            checks="P1",
            environment=environment_file,
            artifact_mode="artifact-on-failure",
        )
        validation.compact(args)
        self.assertEqual(
            {path.name for path in dest.iterdir()},
            {"summary.json", "environment.json", "manifest.json", "checksums.txt"},
        )
        self.assertNotIn(
            "target", json.loads((dest / "manifest.json").read_text())["files"]
        )
        self.assertEqual(
            json.loads((dest / "environment.json").read_text())["kernel"],
            "vm-kernel",
        )

    def test_release_keeps_evidence_but_not_cache(self):
        source = self.temp / "source"
        dest = self.temp / "release"
        source.mkdir()
        (source / "acceptance-report.json").write_text('{"status":"passed"}\n')
        (source / "wal").mkdir()
        (source / "wal" / "segment.chwal").write_bytes(b"release")
        (source / "target").mkdir()
        (source / "target" / "cache").write_bytes(b"cache")
        validation.compact(
            argparse.Namespace(
                source=source,
                dest=dest,
                gate="p1",
                status="passed",
                fingerprint="fp",
                commit="sha",
                checks="P1",
                artifact_mode="release",
            )
        )
        self.assertTrue((dest / "wal" / "segment.chwal").is_file())
        self.assertFalse((dest / "target").exists())

    def test_checksum_paths_cannot_escape_evidence_root(self):
        evidence = self.temp / "evidence"
        evidence.mkdir()
        (self.temp / "outside").write_text("outside\n")
        (evidence / "p1").mkdir()
        (evidence / "p1" / "manifest.json").write_text('{"status":"passed"}\n')
        (evidence / "p1" / "checksums.txt").write_text("bad  ../outside\n")
        self.assertEqual(
            validation.reuse(
                argparse.Namespace(evidence_root=evidence, gate="p1", fingerprint="fp")
            ),
            1,
        )

    def test_matching_evidence_reuses_and_mismatch_does_not(self):
        source = self.temp / "source"
        evidence = self.temp / "evidence"
        source.mkdir()
        args = argparse.Namespace(
            source=source,
            dest=evidence / "p1",
            gate="p1",
            status="passed",
            fingerprint="fp",
            commit="sha",
            checks="P1",
            artifact_mode="artifact-on-failure",
        )
        validation.compact(args)
        reuse_args = argparse.Namespace(
            evidence_root=evidence, gate="p1", fingerprint="fp"
        )
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(validation.reuse(reuse_args), 0)
        self.assertEqual(
            validation.reuse(
                argparse.Namespace(
                    evidence_root=evidence, gate="p1", fingerprint="changed"
                )
            ),
            1,
        )

    def test_failure_artifact_contains_reproducer_inputs_only(self):
        source = self.temp / "source"
        dest = self.temp / "failure"
        source.mkdir()
        (source / "acceptance-report.json").write_text('{"status":"failed"}\n')
        (source / "reproducer.sh").write_text("#!/bin/sh\n")
        (source / "wal").mkdir()
        (source / "wal" / "segment.chwal").write_bytes(b"failure")
        (source / "failure-paths.txt").write_text("wal/segment.chwal\n")
        args = argparse.Namespace(
            source=source,
            dest=dest,
            gate="p2",
            status="failed",
            fingerprint="fp",
            commit="sha",
            checks="P2",
            artifact_mode="artifact-on-failure",
        )
        validation.compact(args)
        self.assertTrue((dest / "failure-summary.md").is_file())
        self.assertTrue((dest / "reproducer.sh").is_file())
        self.assertTrue((dest / "checksums.txt").is_file())


if __name__ == "__main__":
    unittest.main()
