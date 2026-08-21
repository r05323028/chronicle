#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
readonly GIT_CLIFF_VERSION=2.13.1
readonly GIT_CLIFF_SHA512=e716cce3a07dda41b1e370d6afbd7a59eb3d4739509fb7856aeec8da2be28c0396584e29e106141c1a1c535c1827dbc1f60417524f5cfb1da9e11f700bd00f30
readonly GIT_CLIFF_ARCHIVE="git-cliff-${GIT_CLIFF_VERSION}-x86_64-unknown-linux-gnu.tar.gz"
readonly GIT_CLIFF_URL="https://github.com/orhun/git-cliff/releases/download/v${GIT_CLIFF_VERSION}/${GIT_CLIFF_ARCHIVE}"

if [[ $# -gt 1 ]]; then
    printf 'usage: %s [bin-dir]\n' "$0" >&2
    exit 2
fi

BIN_DIR="${1:-${GIT_CLIFF_BIN_DIR:-$ROOT/target/release-tools/bin}}"

for command in curl sha512sum tar install; do
    if ! command -v "$command" >/dev/null 2>&1; then
        printf 'required command not found: %s\n' "$command" >&2
        exit 1
    fi
done

case "$GIT_CLIFF_URL" in
https://*) ;;
*)
    printf 'git-cliff download URL must use HTTPS\n' >&2
    exit 1
    ;;
esac

workdir="$(mktemp -d "${TMPDIR:-/tmp}/chronicle-git-cliff.XXXXXX")"
trap 'rm -rf "$workdir"' EXIT
archive_path="$workdir/$GIT_CLIFF_ARCHIVE"

curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --output "$archive_path" "$GIT_CLIFF_URL"
printf '%s  %s\n' "$GIT_CLIFF_SHA512" "$archive_path" | sha512sum --check --status -

mkdir -p "$workdir/extracted" "$BIN_DIR"
tar --extract --gzip --file "$archive_path" \
    --directory "$workdir/extracted" --strip-components=1
install -m 0755 "$workdir/extracted/git-cliff" "$BIN_DIR/git-cliff"

installed_version="$("$BIN_DIR/git-cliff" --version)"
if [[ "$installed_version" != "git-cliff $GIT_CLIFF_VERSION" ]]; then
    printf 'unexpected git-cliff version: %s\n' "$installed_version" >&2
    exit 1
fi
printf '%s installed at %s\n' "$installed_version" "$BIN_DIR/git-cliff"
