#!/usr/bin/env python3
"""Validate Chronicle pull-request titles against Conventional Commits."""

from __future__ import annotations

import os
import re
import sys

ALLOWED_TYPES = (
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
)
TITLE_FORMAT = "<type>(<optional-scope>)<optional-!>: <description>"
EXAMPLES = (
    "feat(record): improve session correlation",
    "fix(replay): preserve deterministic response ordering",
    "docs: clarify Linux installation requirements",
    "feat(cli)!: replace the legacy replay invocation",
)
_TYPE_PATTERN = "|".join(ALLOWED_TYPES)
_TITLE_PATTERN = re.compile(
    rf"^(?:{_TYPE_PATTERN})(?:\([^\s()]+\))?(?:!)?: \S(?:.*\S)?$"
)


def is_valid_title(title: str) -> bool:
    """Return whether title matches Chronicle's PR-title contract."""
    return (
        "\n" not in title
        and "\r" not in title
        and bool(_TITLE_PATTERN.fullmatch(title))
    )


def main() -> int:
    title = os.environ.get("PR_TITLE")
    if title is None:
        print("PR_TITLE environment variable is required", file=sys.stderr)
        return 2
    if is_valid_title(title):
        return 0

    print(f"Invalid pull request title: {title!r}", file=sys.stderr)
    print(f"Required format: {TITLE_FORMAT}", file=sys.stderr)
    print(f"Allowed types: {', '.join(ALLOWED_TYPES)}", file=sys.stderr)
    print("Examples:", file=sys.stderr)
    for example in EXAMPLES:
        print(f"  {example}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
