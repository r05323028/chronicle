#!/usr/bin/env python3

import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

DRIVER = Path(__file__).with_name("http_driver.py")


class HttpAcceptanceDriverTest(unittest.TestCase):
    def test_workload_has_sequential_chunked_and_second_connection(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            port_file = root / "port"
            requests = root / "requests.jsonl"
            server = subprocess.Popen(
                [
                    sys.executable,
                    str(DRIVER),
                    "serve",
                    "--port-file",
                    str(port_file),
                    "--requests",
                    str(requests),
                ]
            )
            try:
                deadline = time.monotonic() + 5
                while not port_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(port_file.exists())
                subprocess.run(
                    [
                        sys.executable,
                        str(DRIVER),
                        "workload",
                        "--origin",
                        f"http://127.0.0.1:{port_file.read_text()}",
                    ],
                    check=True,
                )
                recorded = [
                    json.loads(line) for line in requests.read_text().splitlines()
                ]
                self.assertEqual(
                    [(item["method"], item["target"]) for item in recorded],
                    [
                        ("GET", "/content-length"),
                        ("GET", "/chunked"),
                        ("POST", "/echo"),
                    ],
                )
            finally:
                server.terminate()
                server.wait(timeout=5)


if __name__ == "__main__":
    unittest.main()
