#!/usr/bin/env python3
"""Resolve a deterministic release-note range from a frozen Git commit."""

from __future__ import annotations

import re
import subprocess
import sys
from dataclasses import dataclass
from functools import cmp_to_key

_VERSION_RE = re.compile(
    r"^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
    r"(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
_TAG_RE = re.compile(r"^v(?P<version>.*)$")


class ReleaseRangeError(RuntimeError):
    """Raised when release history cannot provide one safe boundary."""


@dataclass(frozen=True)
class SemanticVersion:
    major: int
    minor: int
    patch: int
    prerelease: tuple[str, ...] = ()


@dataclass(frozen=True)
class ReleaseTag:
    name: str
    version: SemanticVersion
    commit: str


def _git(*args: str) -> str:
    proc = subprocess.run(["git", *args], capture_output=True, text=True, check=False)
    if proc.returncode:
        detail = proc.stderr.strip() or proc.stdout.strip()
        raise ReleaseRangeError(f"git {' '.join(args)} failed: {detail}")
    return proc.stdout.strip()


def _ref_exists(tag: str) -> bool:
    return (
        subprocess.run(
            ["git", "show-ref", "--verify", "--quiet", f"refs/tags/{tag}"],
            check=False,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        ).returncode
        == 0
    )


def _tag_commit(tag: str) -> str:
    return _git("rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")


def _decimal(raw: str, label: str) -> int:
    try:
        return int(raw)
    except ValueError as exc:
        raise ReleaseRangeError(f"invalid numeric value for {label}: {raw}") from exc


def _parse_version(raw: str, label: str) -> SemanticVersion:
    match = _VERSION_RE.fullmatch(raw)
    if not match:
        raise ReleaseRangeError(f"invalid semantic version for {label}: {raw}")
    prerelease = tuple((match.group(4) or "").split(".")) if match.group(4) else ()
    for identifier in prerelease:
        if identifier.isdigit() and len(identifier) > 1 and identifier.startswith("0"):
            raise ReleaseRangeError(
                f"invalid semantic version for {label}: numeric prerelease "
                f"identifier has a leading zero: {raw}"
            )
    return SemanticVersion(
        _decimal(match.group(1), label),
        _decimal(match.group(2), label),
        _decimal(match.group(3), label),
        prerelease,
    )


def _parse_tag(tag: str) -> SemanticVersion:
    match = _TAG_RE.fullmatch(tag)
    if not match or not match.group("version"):
        raise ReleaseRangeError(f"invalid Chronicle release tag: {tag}")
    return _parse_version(match.group("version"), f"tag {tag}")


def _compare_versions(left: SemanticVersion, right: SemanticVersion) -> int:
    base_left = (left.major, left.minor, left.patch)
    base_right = (right.major, right.minor, right.patch)
    if base_left != base_right:
        return -1 if base_left < base_right else 1
    if not left.prerelease or not right.prerelease:
        if left.prerelease == right.prerelease:
            return 0
        return -1 if left.prerelease else 1
    for left_id, right_id in zip(left.prerelease, right.prerelease):
        if left_id == right_id:
            continue
        left_numeric = left_id.isdigit()
        right_numeric = right_id.isdigit()
        if left_numeric and right_numeric:
            left_number = _decimal(left_id, "prerelease")
            right_number = _decimal(right_id, "prerelease")
            return -1 if left_number < right_number else 1
        if left_numeric != right_numeric:
            return -1 if left_numeric else 1
        return -1 if left_id < right_id else 1
    if len(left.prerelease) == len(right.prerelease):
        return 0
    return -1 if len(left.prerelease) < len(right.prerelease) else 1


def resolve(version: str, tag: str, sha: str) -> list[str]:
    normalized_version = version[1:] if version.startswith("v") else version
    requested_version = _parse_version(normalized_version, "VERSION")
    expected_tag = f"v{normalized_version}"
    if tag != expected_tag:
        raise ReleaseRangeError(f"TAG does not match VERSION: {tag} != {expected_tag}")
    intended_version = _parse_tag(tag)
    if intended_version != requested_version:
        raise ReleaseRangeError(f"TAG does not match VERSION: {tag}")

    if _git("rev-parse", "--is-shallow-repository") in ("true",):
        raise ReleaseRangeError("release-note range requires full Git history")
    frozen_sha = _git("rev-parse", "--verify", f"{sha}^{{commit}}")

    if _ref_exists(tag):
        intended_sha = _tag_commit(tag)
        if intended_sha != frozen_sha:
            raise ReleaseRangeError(
                f"intended tag {tag} points to {intended_sha}, expected frozen SHA "
                f"{frozen_sha}"
            )

    reachable_tags = _git("tag", "--merged", frozen_sha, "--list", "v*").splitlines()
    candidates: list[ReleaseTag] = []
    for candidate_name in reachable_tags:
        candidate_version = _parse_tag(candidate_name)
        if candidate_name == tag:
            continue
        if _compare_versions(candidate_version, intended_version) >= 0:
            raise ReleaseRangeError(
                f"reachable release tag {candidate_name} is not older than intended "
                f"release {tag}"
            )
        candidates.append(
            ReleaseTag(candidate_name, candidate_version, _tag_commit(candidate_name))
        )

    previous: ReleaseTag | None = None
    if candidates:
        candidates.sort(
            key=cmp_to_key(lambda a, b: _compare_versions(a.version, b.version))
        )
        previous = candidates[-1]
        same_version = [
            candidate
            for candidate in candidates
            if candidate.version == previous.version
        ]
        if len(same_version) > 1:
            names = ", ".join(candidate.name for candidate in same_version)
            raise ReleaseRangeError(f"ambiguous release version {names}")

    previous_tag = previous.name if previous else ""
    previous_sha = previous.commit if previous else ""
    release_range = f"{previous_tag}..{frozen_sha}" if previous_tag else frozen_sha
    return [
        f"previous-tag={previous_tag}",
        f"previous-sha={previous_sha}",
        f"frozen-sha={frozen_sha}",
        f"range={release_range}",
    ]


def main(argv: list[str]) -> int:
    if len(argv) != 4:
        print(f"usage: {argv[0]} VERSION TAG SHA", file=sys.stderr)
        return 2
    try:
        print("\n".join(resolve(*argv[1:])))
    except (OSError, ReleaseRangeError) as exc:
        print(f"release range error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
