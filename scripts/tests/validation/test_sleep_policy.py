#!/usr/bin/env python3
"""Sleep-policy static check (task 6.3).

Rejects undocumented correctness sleeps and unbounded polling in product
black-box test files; fixture sleeper workloads (scripts/tests/**) are exempt
because they exercise the timeout/infra itself.

Policy:
- Python tests (tests/**/*.py): time.sleep(X) with X >= 0.5s is an
  undocumented correctness sleep (poll backoff is < 0.5s).
- Rust integration tests (crates/*/tests/*.rs): sleep(Duration::from_millis(N))
  with N >= 500 is a violation (poll intervals are < 500ms).
- Unbounded polling: bare `while True` in product Python tests outside
  tests/support/ (the bounded-wait helper home), unless the enclosing
  function names a deadline/timeout/attempts bound.
"""

import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
PY_SLEEP_MS = 500  # time.sleep seconds * 1000
RUST_SLEEP_MS = 500

PY_SLEEP_RE = re.compile(r"time\.sleep\s*\(\s*([0-9.]+)")
RUST_SLEEP_RE = re.compile(r"sleep\s*\(\s*Duration::from_millis\s*\(\s*(\d+)")
WHILE_TRUE_RE = re.compile(r"while\s+True\s*:")
BOUND_RE = re.compile(r"deadline|timeout|attempts|max_wait|max_wait_ms|elapsed")


def _seconds(value: str) -> float:
    return float(value)


def scan_python(path: Path, lines: list[str]) -> list[tuple[int, str]]:
    violations = []
    for i, line in enumerate(lines):
        for match in PY_SLEEP_RE.finditer(line):
            ms = _seconds(match.group(1)) * 1000
            if ms >= PY_SLEEP_MS:
                violations.append(
                    (i + 1, f"undocumented correctness sleep {match.group(1)}s")
                )
    return violations


def scan_rust(path: Path, lines: list[str]) -> list[tuple[int, str]]:
    violations = []
    for i, line in enumerate(lines):
        for match in RUST_SLEEP_RE.finditer(line):
            ms = int(match.group(1))
            if ms >= RUST_SLEEP_MS:
                violations.append((i + 1, f"undocumented correctness sleep {ms}ms"))
    return violations


def scan_unbounded_polling(path: Path, lines: list[str]) -> list[tuple[int, str]]:
    if "tests/support" in str(path):
        return []  # bounded-wait helper home
    violations = []
    for i, line in enumerate(lines):
        if not WHILE_TRUE_RE.search(line):
            continue
        # loop body until dedent (next line with indent <= while indent)
        indent = len(line) - len(line.lstrip())
        j = i + 1
        body = []
        while j < len(lines):
            if lines[j].strip() and len(lines[j]) - len(lines[j].lstrip()) <= indent:
                break
            body.append(lines[j])
            j += 1
        if not any(PY_SLEEP_RE.search(l) for l in body):
            continue  # protocol/server loop, not a synchronization poll
        # enclosing function body for bound tokens
        k = i
        while k > 0 and not re.match(r"^\s*(def|class) ", lines[k - 1]):
            k -= 1
        if not BOUND_RE.search("\n".join(lines[k:j])):
            violations.append(
                (i + 1, "unbounded polling (while True + sleep without deadline)")
            )
    return violations


def scan_file(path: Path) -> list[tuple[int, str]]:
    if "scripts/tests" in str(path):
        return []  # fixture sleeper workloads: exempt
    try:
        text = path.read_text()
    except OSError:
        return []
    lines = text.splitlines()
    if str(path).endswith(".py") and "tests/" in str(path):
        return scan_python(path, lines) + scan_unbounded_polling(path, lines)
    if str(path).endswith(".rs") and "/tests/" in str(path):
        return scan_rust(path, lines)
    return []


def scan_tree(root: Path) -> list[tuple[str, int, str]]:
    found = []
    targets = [p for p in root.rglob("*") if p.is_file() and p.suffix in (".py", ".rs")]
    for path in targets:
        for line_no, reason in scan_file(path):
            found.append((str(path.relative_to(root)), line_no, reason))
    return sorted(found)


class SleepPolicyScannerTests(unittest.TestCase):
    def test_rejects_correctness_sleep_python(self):
        v = scan_python(Path("tests/x.py"), ["import time", "time.sleep(5)"])
        self.assertEqual(len(v), 1)
        self.assertIn("5", v[0][1])

    def test_allows_poll_backoff_python(self):
        v = scan_python(Path("tests/x.py"), ["time.sleep(0.05)"])
        self.assertEqual(v, [])

    def test_rejects_correctness_sleep_rust(self):
        v = scan_rust(
            Path("crates/x/tests/y.rs"), ["thread::sleep(Duration::from_millis(2000));"]
        )
        self.assertEqual(len(v), 1)

    def test_allows_poll_interval_rust(self):
        v = scan_rust(
            Path("crates/x/tests/y.rs"), ["thread::sleep(Duration::from_millis(10));"]
        )
        self.assertEqual(v, [])

    def test_rejects_unbounded_while_true(self):
        v = scan_unbounded_polling(
            Path("tests/p.py"),
            ["def wait():", "    while True:", "        time.sleep(0.05)"],
        )
        self.assertEqual(len(v), 1)

    def test_allows_bounded_while_true(self):
        v = scan_unbounded_polling(
            Path("tests/p.py"),
            [
                "def wait():",
                "    deadline = time.monotonic() + 5",
                "    while True:",
                "        time.sleep(0.05)",
            ],
        )
        self.assertEqual(v, [])

    def test_allows_support_helper_home(self):
        v = scan_unbounded_polling(
            Path("tests/support/process.py"), ["while True:", "    time.sleep(0.05)"]
        )
        self.assertEqual(v, [])

    def test_fixture_workloads_exempt(self):
        v = scan_file(Path("scripts/tests/acceptance/test_runner.py"))
        self.assertEqual(v, [])

    def test_repo_tree_clean(self):
        violations = scan_tree(ROOT)
        self.assertEqual(
            violations,
            [],
            "sleep policy violations:\n"
            + "\n".join(f"{p}:{l}: {r}" for p, l, r in violations[:20]),
        )


if __name__ == "__main__":
    unittest.main()
