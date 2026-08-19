#!/usr/bin/env bash
# Resolve release context from the root workspace package version.
#
# Usage: resolve-release-context.sh <release|prepare> <Cargo.toml> <version>
# Prints KEY=VALUE lines suitable for $GITHUB_OUTPUT.
set -euo pipefail

mode=${1:?usage: resolve-release-context.sh <release|prepare> <Cargo.toml> <version>}
manifest=${2:?usage: resolve-release-context.sh <release|prepare> <Cargo.toml> <version>}
requested=${3:?usage: resolve-release-context.sh <release|prepare> <Cargo.toml> <version>}

[[ $mode == release || $mode == prepare ]] || {
    printf 'mode must be release or prepare\n' >&2
    exit 2
}
[[ -f $manifest ]] || {
    printf 'Cargo.toml not found: %s\n' "$manifest" >&2
    exit 1
}

python3 - "$manifest" "$mode" "$requested" <<'PY'
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path

manifest = Path(sys.argv[1])
mode = sys.argv[2]
raw_requested = sys.argv[3]

try:
    document = tomllib.loads(manifest.read_text(encoding="utf-8"))
except (OSError, tomllib.TOMLDecodeError) as exc:
    print(f"could not parse {manifest}: {exc}", file=sys.stderr)
    raise SystemExit(1)

workspace_package = document.get("workspace", {}).get("package", {})
workspace_version = workspace_package.get("version")
if not isinstance(workspace_version, str) or not workspace_version:
    print(
        f"could not derive [workspace.package].version from {manifest}",
        file=sys.stderr,
    )
    raise SystemExit(1)


def semver_parts(value: str) -> tuple[str, str | None] | None:
    """Validate SemVer 2.0.0, intentionally excluding build metadata."""
    if "+" in value:
        return None
    core, separator, prerelease = value.partition("-")
    numbers = core.split(".")
    if len(numbers) != 3:
        return None
    if any(
        not re.fullmatch(r"(?:0|[1-9][0-9]*)", number) for number in numbers
    ):
        return None
    if separator:
        identifiers = prerelease.split(".")
        if not identifiers or any(
            not re.fullmatch(r"[0-9A-Za-z-]+", identifier)
            or (identifier.isdigit() and len(identifier) > 1 and identifier[0] == "0")
            for identifier in identifiers
        ):
            return None
    return value, prerelease if separator else None

workspace = semver_parts(workspace_version)
if workspace is None:
    print(
        f'malformed workspace version "{workspace_version}" in {manifest}; '
        "release versions must be SemVer without build metadata",
        file=sys.stderr,
    )
    raise SystemExit(1)

requested_version = raw_requested[1:] if raw_requested.startswith("v") else raw_requested
requested_parts = semver_parts(requested_version)
if requested_parts is None:
    print(
        f'malformed requested release version "{raw_requested}"; '
        "expected SemVer without build metadata",
        file=sys.stderr,
    )
    raise SystemExit(1)

if requested_version != workspace_version:
    print(
        f'requested version "{requested_version}" does not match workspace version '
        f'"{workspace_version}" (Cargo.toml)',
        file=sys.stderr,
    )
    raise SystemExit(1)

print(f"mode={mode}")
print(f"version={workspace_version}")
print(f"tag=v{workspace_version}")
print(f"is-release={'true' if mode == 'release' else 'false'}")
print(f"is-prerelease={'true' if requested_parts[1] is not None else 'false'}")
PY
