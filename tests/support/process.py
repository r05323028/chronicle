#!/usr/bin/env python3
"""Shared black-box process support for root test suites.

Owned by root-black-box-support (task 6.2). Provides bounded process execution,
JSON parsing, fixture recording, and condition-based readiness. Every helper
here is deterministic: no arbitrary correctness sleeps; the only wait is a
bounded condition (port-file appearance) with an explicit deadline.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE_BASIC = REPO_ROOT / "fixtures/http/basic-session.json"
DEFAULT_TIMEOUT_SECONDS = 45


def find_binary() -> Path:
    """Resolve the chronicle binary: CHRONICLE_BIN env or the debug build."""
    override = os.environ.get("CHRONICLE_BIN")
    if override:
        return Path(override)
    candidate = REPO_ROOT / "target/debug/chronicle"
    if not candidate.is_file():
        raise RuntimeError(
            f"chronicle binary not found at {candidate}; "
            "build it with: cargo build -p chronicle-cli"
        )
    return candidate


class CommandError(RuntimeError):
    """A bounded chronicle invocation failed or timed out."""


def run(
    binary: Path,
    arguments: list[str],
    *,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    stdin: bytes | None = None,
    data_dir: Path | None = None,
    format_json: bool = True,
) -> subprocess.CompletedProcess[bytes]:
    """Run the binary with a hard deadline; never retries and never sleeps."""
    argv = [str(binary)]
    if data_dir is not None:
        argv += ["--data-dir", str(data_dir)]
    if format_json:
        argv += ["--format", "json"]
    argv += arguments
    try:
        return subprocess.run(
            argv,
            input=stdin,
            capture_output=True,
            timeout=timeout,
            check=False,
        )
    except subprocess.TimeoutExpired as exc:
        raise CommandError(
            f"command timed out after {timeout}s: {' '.join(argv)}"
        ) from exc


def run_json(
    binary: Path,
    arguments: list[str],
    *,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
    data_dir: Path | None = None,
) -> dict:
    """Run and parse JSON stdout; fails on nonzero exit or malformed JSON."""
    result = run(binary, arguments, timeout=timeout, data_dir=data_dir)
    if result.returncode != 0:
        raise CommandError(
            f"exit {result.returncode}: {' '.join(arguments)} stdout={result.stdout!r:.200} stderr={result.stderr!r:.300}"
        )
    if not result.stdout:
        raise CommandError(f"empty stdout for: {' '.join(arguments)}")
    try:
        return json.loads(result.stdout.decode())
    except (ValueError, UnicodeDecodeError) as exc:
        raise CommandError(
            f"malformed JSON stdout for {' '.join(arguments)}: {result.stdout!r:.200}"
        ) from exc


def wait_for_port_file(path: Path, deadline_seconds: int = 10) -> int:
    """Condition-based wait: return the port once the server wrote its port file.

    Polls with a bounded deadline; raises on expiry. This is the readiness
    contract for spawned workload servers (no arbitrary correctness sleep).
    """
    deadline = time.monotonic() + deadline_seconds
    while time.monotonic() < deadline:
        try:
            text = path.read_text(encoding="utf-8").strip()
        except (OSError, UnicodeDecodeError):
            text = ""
        if text.isdigit():
            return int(text)
        time.sleep(0.05)
    raise CommandError(f"port file {path} did not appear within {deadline_seconds}s")


def record_fixture(
    binary: Path, fixture: Path, root: Path, *, timeout: int = DEFAULT_TIMEOUT_SECONDS
) -> str:
    """Record a deterministic fixture; returns the session id."""
    result = run(
        binary,
        ["internal", "record-fixture", "--input", str(fixture), "--root", str(root)],
        timeout=timeout,
        format_json=False,
    )
    if result.returncode != 0:
        raise CommandError(
            f"record-fixture exit {result.returncode}: stderr={result.stderr!r:.300}"
        )
    for line in result.stdout.decode().splitlines():
        if line.startswith("session_id: "):
            return line[len("session_id: ") :].strip()
    raise CommandError(f"record-fixture printed no session id: {result.stdout!r:.200}")


def etl(
    binary: Path, wal_dir: Path, output: Path, *, timeout: int = DEFAULT_TIMEOUT_SECONDS
) -> dict:
    """Canonicalize a WAL into a fresh output root via the internal ETL surface."""
    return run_json(
        binary,
        ["internal", "etl", "--wal-dir", str(wal_dir), "--output", str(output)],
        timeout=timeout,
    )


def inspect_session(
    binary: Path,
    session_id: str,
    root: Path,
    *,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
) -> dict:
    """Inspect a session through the public inspect command (--root form)."""
    return run_json(
        binary,
        ["inspect", session_id, "--root", str(root)],
        timeout=timeout,
        data_dir=root,
    )


def replay(
    binary: Path,
    session_id: str,
    root: Path,
    target: str,
    *,
    execute: bool = False,
    allow_read: bool = False,
    timeout: int = DEFAULT_TIMEOUT_SECONDS,
) -> subprocess.CompletedProcess[bytes]:
    """Replay a session against a target through the public replay command."""
    arguments = [
        "replay",
        session_id,
        "--root",
        str(root),
        "--target",
        target,
        "--allow-host",
        "127.0.0.1",
    ]
    if allow_read:
        arguments.append("--allow-read")
    if execute:
        arguments.append("--execute")
    return run(binary, arguments, timeout=timeout, data_dir=root)


def temp_root(prefix: str) -> Path:
    """Fresh temporary test root that callers must clean up."""
    return Path(tempfile.mkdtemp(prefix=f"chronicle-{prefix}-"))


def require_rootless_record_failure(binary: Path, root: Path, marker: Path) -> None:
    """Public record fails fast and never touches the target when live capture
    cannot operate: on Linux the live-capture binary fails host preflight
    (exit 3, rootless host lacks eBPF privileges); elsewhere the portable
    build rejects live capture (exit 4). Proves the record preflight
    contract on every supported platform."""
    result = run(
        binary,
        ["record", "--", "/usr/bin/touch", str(marker)],
        data_dir=root,
    )
    expected = 3 if sys.platform.startswith("linux") else 4
    if result.returncode != expected:
        raise CommandError(
            f"record expected exit {expected}, got {result.returncode}: {result.stderr!r:.300}"
        )
    if result.stdout:
        raise CommandError(
            f"record failure must have empty stdout: {result.stdout!r:.100}"
        )
    try:
        payload = json.loads(result.stderr.decode())
    except (ValueError, UnicodeDecodeError) as exc:
        raise CommandError(
            f"record failure stderr must be JSON: {result.stderr!r:.300}"
        ) from exc
    if payload.get("code") != expected:
        raise CommandError(f"record failure JSON code must be {expected}: {payload}")
    if marker.exists():
        raise CommandError("record must fail before touching the target")


DRIVER = REPO_ROOT / "tests/support/http_driver.py"


class DriverHandle:
    """Owned workload/replay server process with deterministic shutdown."""

    def __init__(self, process, port: int, root: Path):
        self.process = process
        self.port = port
        self.root = root

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)

    @property
    def target(self) -> str:
        return f"http://127.0.0.1:{self.port}"


def start_driver(
    root: Path, *, mismatch: bool = False, deadline_seconds: int = 10
) -> DriverHandle:
    """Spawn the HTTP workload/replay driver and wait for its port file."""
    if not DRIVER.is_file():
        raise CommandError(f"driver not found at {DRIVER}")
    port_file = root / "driver.port"
    requests = root / "driver-requests.jsonl"
    arguments = [
        sys.executable,
        str(DRIVER),
        "serve",
        "--port-file",
        str(port_file),
        "--requests",
        str(requests),
    ]
    if mismatch:
        arguments.append("--mismatch")
    process = subprocess.Popen(
        arguments, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    try:
        port = wait_for_port_file(port_file, deadline_seconds=deadline_seconds)
    except CommandError:
        process.kill()
        process.wait(timeout=5)
        raise
    return DriverHandle(process, port, root)


def write_recording_metadata(wal_dir: Path, recording_id: str) -> None:
    """Write deterministic recording.json into a fixture WAL directory.

    Used to exercise ETL fail-closed identity validation rootlessly. The full
    counter shape matches what validate_recording_metadata requires.
    """
    zero = {"records": 0, "bytes": 0}
    one = {"records": 1, "bytes": 1}
    metadata = {
        "version": 1,
        "recording_id": recording_id,
        "status": "completed",
        "shutdown_reason": "source_completed",
        "counters": {
            "received": one,
            "accepted_into_queue": one,
            "written_not_committed": one,
            "committed": one,
            "discarded_from_queue_due_to_wal_limit": zero,
            "kernel_or_backend_dropped": zero,
            "rejected_after_stop": zero,
            "rejected_due_to_quota": zero,
            "etl_checkpointed": zero,
        },
    }
    (wal_dir / "recording.json").write_text(json.dumps(metadata), encoding="utf-8")


if __name__ == "__main__":
    binary = find_binary()
    print(f"binary: {binary}")
    root = temp_root("selfcheck")
    try:
        session = record_fixture(binary, FIXTURE_BASIC, root)
        inspection = inspect_session(binary, session, root)
        assert inspection["version"] == 1
        print(
            f"self-check OK: session={session[:8]} ops={len(inspection['connections'][0]['operations'])}"
        )
    finally:
        import shutil

        shutil.rmtree(root, ignore_errors=True)
