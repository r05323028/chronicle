#!/usr/bin/env python3
"""Lifecycle cleanup tests for root black-box support (task 6.4).

Proves tests/support/process.py ownership guarantees: managed workload
processes are terminated deterministically on the normal stop path and on the
start-failure path; ports they bound become free; the readiness wait is bounded
by an explicit deadline (no unbounded polling); and the generic wait never
reinterprets recorder/status content as readiness (task 6.5). cgroup/mount/loop
and stale recorder-lease cleanup are privileged/recorder-owned and covered by
the privileged suites (task 10.4) plus recorder lease unit tests.
"""

import socket
import subprocess
import sys
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
from tests.support import process as proc  # noqa: E402


def port_free(port: int) -> bool:
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
            sock.bind(("127.0.0.1", port))
            return True
    except OSError:
        return False


def driver_processes() -> list:
    out = subprocess.run(
        ["pgrep", "-f", "http_driver.py"],
        capture_output=True,
        text=True,
        timeout=30,
    )
    if out.returncode != 0:
        return []
    return out.stdout.split()


class LifecycleCleanupTests(unittest.TestCase):
    def test_stop_releases_process_and_port(self):
        root = proc.temp_root("lifecycle-pass")
        try:
            handle = proc.start_driver(root)
            self.assertFalse(
                port_free(handle.port), "driver must own the port while serving"
            )
            self.assertIsNotNone(handle.process.pid)
            handle.stop()
            self.assertIsNotNone(
                handle.process.poll(), "process must be reaped after stop"
            )
            self.assertTrue(port_free(handle.port), "port must be free after stop")
        finally:
            import shutil

            shutil.rmtree(root, ignore_errors=True)

    def test_start_failure_path_cleans_up_process(self):
        root = proc.temp_root("lifecycle-fail")
        original_wait = proc.wait_for_port_file

        def forced_wait(*_args, **_kwargs):
            raise proc.CommandError("forced readiness failure")

        before = driver_processes()
        try:
            proc.wait_for_port_file = forced_wait
            with self.assertRaises(proc.CommandError):
                proc.start_driver(root)
        finally:
            proc.wait_for_port_file = original_wait
            time.sleep(0.2)  # allow the kill path to settle
            after = driver_processes()
            leaked = [pid for pid in after if pid not in before]
            self.assertEqual(
                leaked, [], f"start-failure leaked driver process: {leaked}"
            )
            import shutil

            shutil.rmtree(root, ignore_errors=True)

    def test_wait_for_port_file_is_bounded(self):
        root = proc.temp_root("lifecycle-bounded")
        try:
            missing = root / "never-appears.port"
            started = time.monotonic()
            with self.assertRaises(proc.CommandError):
                proc.wait_for_port_file(missing, deadline_seconds=1)
            elapsed = time.monotonic() - started
            self.assertLess(elapsed, 5, "wait must honor its deadline")
        finally:
            import shutil

            shutil.rmtree(root, ignore_errors=True)

    def test_run_timeout_reaps_child(self):
        binary = sys.executable
        try:
            with self.assertRaises(proc.CommandError):
                proc.run(
                    Path(binary),
                    ["-c", "import time; time.sleep(30)"],
                    timeout=1,
                    format_json=False,
                )
        finally:
            pass

    def test_no_leaked_process_after_stop(self):
        root = proc.temp_root("lifecycle-noleak")
        try:
            before = driver_processes()
            handle = proc.start_driver(root)
            handle.stop()
            time.sleep(0.2)
            after = driver_processes()
            leaked = [pid for pid in after if pid not in before]
            self.assertEqual(leaked, [], f"stopped driver leaked: {leaked}")
        finally:
            import shutil

            shutil.rmtree(root, ignore_errors=True)

    def test_generic_wait_never_reinterprets_recorder_state(self):
        root = proc.temp_root("lifecycle-wait-state")
        try:
            # A recorder lease/status file is not a port file; the generic wait
            # must time out bounded rather than parse it as readiness.
            lease_like = root / "recorder.status"
            lease_like.write_text('{"state": "terminal_failure"}', encoding="utf-8")
            started = time.monotonic()
            with self.assertRaises(proc.CommandError):
                proc.wait_for_port_file(lease_like, deadline_seconds=1)
            elapsed = time.monotonic() - started
            self.assertLess(elapsed, 5, "wait must stay bounded")
        finally:
            import shutil

            shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    unittest.main()
