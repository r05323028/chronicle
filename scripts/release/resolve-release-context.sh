#!/usr/bin/env bash
# Resolve release context (mode, version, tag) for the release workflow.
#
# Single testable source of truth for version/tag handling:
#   - release mode: the pushed Git tag must equal v<workspace-version> (from the
#     root Cargo.toml), otherwise exit 1 (mismatch fails before any build).
#   - dry-run mode: no Git tag exists; the intended release version is the
#     workspace version and the intended tag is v<version>. No tag is created.
#
# Usage: resolve-release-context.sh <release|dry-run> <path-to-Cargo.toml> [tag]
#
# Prints KEY=VALUE lines (safe to append to $GITHUB_OUTPUT):
#   mode=<release|dry-run>
#   version=<workspace version>
#   tag=<intended release tag>
#   is-release=<true|false>
set -euo pipefail

mode=${1:?usage: resolve-release-context.sh <release|dry-run> <path-to-Cargo.toml> [tag]}
manifest=${2:?usage: resolve-release-context.sh <release|dry-run> <path-to-Cargo.toml> [tag]}
tag=${3:-}

[[ $mode == release || $mode == dry-run ]] || {
	printf 'mode must be release or dry-run\n' >&2
	exit 2
}
[[ -f "$manifest" ]] || {
	printf 'Cargo.toml not found: %s\n' "$manifest" >&2
	exit 1
}

version=$(grep -m1 '^version' "$manifest" | sed -E 's/version *= *"([^"]+)".*/\1/')
[[ -n "$version" ]] || {
	printf 'could not derive version from %s\n' "$manifest" >&2
	exit 1
}
# Basic semver sanity: dotted numeric triple, optional -prerelease/+build.
if [[ ! $version =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]]; then
	printf 'malformed workspace version "%s" in %s\n' "$version" "$manifest" >&2
	exit 1
fi

if [[ $mode == release ]]; then
	[[ -n "$tag" ]] || {
		printf 'release mode requires a tag\n' >&2
		exit 1
	}
	normalized=${tag#v}
	if [[ "$normalized" != "$version" ]]; then
		printf 'tag "%s" does not match workspace version "%s" (Cargo.toml)\n' \
			"$tag" "$version" >&2
		exit 1
	fi
	printf 'mode=release\nversion=%s\ntag=%s\nis-release=true\n' "$version" "$tag"
else
	printf 'mode=dry-run\nversion=%s\ntag=v%s\nis-release=false\n' "$version" "$version"
fi
