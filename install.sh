#!/bin/sh
# install.sh - install the Chronicle release binary for this host.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
#     | CHRONICLE_VERSION=v0.1.0 sh
#   curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
#     | CHRONICLE_INSTALL_DIR=/some/path sh
#
# Environment:
#   CHRONICLE_VERSION     pin a release version (leading "v" optional); default:
#                         latest stable GitHub Release
#   CHRONICLE_INSTALL_DIR destination directory; default: $HOME/.local/bin
#   CHRONICLE_REPO        owner/repo, default r05323028/chronicle
#   CHRONICLE_API_URL     override releases/latest API base (tests)
#   CHRONICLE_BASE_URL    override release download base (tests)
#   CHRONICLE_UNAME_OS    override uname -s (tests)
#   CHRONICLE_UNAME_MACHINE  override uname -m (tests)
#
# POSIX sh only: no bashisms. Requires: sh, curl, tar, and a SHA-256 utility
# (sha256sum or shasum -a 256).

set -eu

# --- diagnostics -----------------------------------------------------------

die() {
	printf 'chronicle-install: %s\n' "$1" >&2
	exit 1
}

# --- preflight dependencies ------------------------------------------------

command -v curl >/dev/null 2>&1 || die 'curl is required (not found on PATH)'
command -v tar >/dev/null 2>&1 || die 'tar is required (not found on PATH)'
command -v awk >/dev/null 2>&1 || die 'awk is required (not found on PATH)'
command -v mktemp >/dev/null 2>&1 || die 'mktemp is required (not found on PATH)'

if command -v sha256sum >/dev/null 2>&1; then
	sha256_of() { sha256sum "$1"; }
elif command -v shasum >/dev/null 2>&1; then
	sha256_of() { shasum -a 256 "$1"; }
else
	die 'no SHA-256 utility found (need sha256sum or shasum -a 256)'
fi

# --- configuration ---------------------------------------------------------

repo=${CHRONICLE_REPO:-r05323028/chronicle}
case "$repo" in
*/*) ;;
*) die "CHRONICLE_REPO must be owner/repo, got: $repo" ;;
esac

api_url=${CHRONICLE_API_URL:-https://api.github.com/repos/$repo/releases/latest}
base_url=${CHRONICLE_BASE_URL:-https://github.com/$repo/releases/download}

home=${HOME:-}
[ -n "$home" ] || die 'HOME is not set; cannot determine default install directory'
install_dir=${CHRONICLE_INSTALL_DIR:-$home/.local/bin}

# --- platform detection ----------------------------------------------------

os=${CHRONICLE_UNAME_OS:-$(uname -s)}
machine=${CHRONICLE_UNAME_MACHINE:-$(uname -m)}

case "$os" in
Linux) ;;
*) die "unsupported operating system: $os (supported: Linux)" ;;
esac

case "$machine" in
x86_64 | amd64) target=x86_64-unknown-linux-gnu ;;
aarch64 | arm64) target=aarch64-unknown-linux-gnu ;;
*) die "unsupported architecture: $machine (supported: x86_64, aarch64/arm64)" ;;
esac

# --- version resolution ----------------------------------------------------

if [ -n "${CHRONICLE_VERSION:-}" ]; then
	version=$CHRONICLE_VERSION
	case "$version" in
	v*) version=${version#v} ;;
	esac
	case "$version" in
	*/* | *..*) die "invalid CHRONICLE_VERSION: $CHRONICLE_VERSION" ;;
	esac
	[ -n "$version" ] || die "invalid CHRONICLE_VERSION: $CHRONICLE_VERSION"
	tag=v$version
else
	tag=$(
		curl -fsSL "$api_url" 2>/dev/null |
			grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' |
			head -n 1 |
			cut -d '"' -f 4
	) || true
	[ -n "$tag" ] || die "could not resolve latest release from $api_url"
	version=${tag#v}
fi

# --- asset resolution and download ------------------------------------------

asset=chronicle-${version}-${target}.tar.gz
archive_url=$base_url/$tag/$asset
checksums_url=$base_url/$tag/SHA256SUMS

_tmpdir=$(mktemp -d "${TMPDIR:-/tmp}/chronicle-install.XXXXXX") || die 'could not create temporary directory'
_tmp_bin=
cleanup() {
	rm -rf "$_tmpdir"
	[ -z "$_tmp_bin" ] || rm -f "$_tmp_bin"
}
trap cleanup EXIT HUP INT TERM

printf 'chronicle-install: installing %s (%s) into %s\n' "$asset" "$tag" "$install_dir"
printf 'chronicle-install: downloading %s\n' "$archive_url"
curl -fsSL -o "$_tmpdir/$asset" "$archive_url" ||
	die "failed to download $archive_url"
printf 'chronicle-install: downloading %s\n' "$checksums_url"
curl -fsSL -o "$_tmpdir/SHA256SUMS" "$checksums_url" ||
	die "failed to download checksums $checksums_url"

# --- integrity verification -------------------------------------------------

expected=$(
	awk -v name="$asset" '$2 == name || $2 == "*" name { print $1; exit }' \
		"$_tmpdir/SHA256SUMS"
) || true
[ -n "$expected" ] || die "no SHA-256 checksum found for $asset in $checksums_url"

actual=$(sha256_of "$_tmpdir/$asset" | awk '{ print $1 }') ||
	die "could not compute SHA-256 of the downloaded archive"
[ "$expected" = "$actual" ] ||
	die "checksum mismatch for $asset (expected $expected, got $actual); aborting"

printf 'chronicle-install: checksum verified (%s)\n' "$expected"

# --- extraction -------------------------------------------------------------

tar -xzf "$_tmpdir/$asset" -C "$_tmpdir" ||
	die "failed to extract $asset"
[ -f "$_tmpdir/chronicle" ] ||
	die "$asset does not contain a 'chronicle' binary"

# --- installation (atomic replace only after full verification) -------------

if [ ! -d "$install_dir" ]; then
	mkdir -p "$install_dir" || die "could not create install directory: $install_dir"
fi
[ -w "$install_dir" ] || die "install directory not writable: $install_dir"

_tmp_bin=$install_dir/.chronicle.tmp.$$
cp "$_tmpdir/chronicle" "$_tmp_bin" || die "could not copy binary into $install_dir"
chmod +x "$_tmp_bin" || die "could not make binary executable"
mv -f "$_tmp_bin" "$install_dir/chronicle" || die "could not move binary into $install_dir"
_tmp_bin=

printf 'chronicle-install: installed %s to %s/chronicle\n' "$version" "$install_dir"

# --- PATH guidance (no shell-config edits) ----------------------------------

case ":$PATH:" in
*":$install_dir:"*) ;;
*)
	printf 'chronicle-install: %s is not on your PATH.\n' "$install_dir" >&2
	printf 'chronicle-install: add it with: export PATH="%s:$PATH"\n' "$install_dir" >&2
	;;
esac

printf 'chronicle-install: done. Run "chronicle --version" and "chronicle doctor" to verify.\n'
