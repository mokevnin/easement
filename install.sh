#!/bin/sh
# Installs easement from a GitHub release: picks the archive for this OS and CPU, checks it against
# the release's sha256.sum and copies the binary into a directory.
#
#   curl -fsSL https://raw.githubusercontent.com/mokevnin/easement/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/mokevnin/easement/main/install.sh | sh -s -- -b ~/.local/bin -v 0.1.0
#
# -b <dir>  install into <dir> (default /usr/local/bin, or $BINDIR)
# -v <ver>  install this version, such as 0.1.0 (default: the latest release, or $VERSION)
set -eu

repo="mokevnin/easement"
bindir="${BINDIR:-/usr/local/bin}"
version="${VERSION:-latest}"
# Overridden by the tests of this script.
base_url="${EASEMENT_BASE_URL:-https://github.com/$repo/releases}"

usage() {
  sed -n '/^# -b/,/^# -v/p' "$0" | sed 's/^# //' >&2
  exit 2
}

while getopts "b:v:h" opt; do
  case "$opt" in
    b) bindir="$OPTARG" ;;
    v) version="$OPTARG" ;;
    *) usage ;;
  esac
done

fail() {
  echo "install.sh: $*" >&2
  exit 1
}

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "unsupported OS $(uname -s); see https://github.com/$repo/releases" ;;
esac

case "$(uname -m)" in
  x86_64 | amd64) arch="x86_64" ;;
  aarch64 | arm64) arch="aarch64" ;;
  *) fail "unsupported CPU $(uname -m)" ;;
esac

if [ "$version" = "latest" ]; then
  download="$base_url/latest/download"
else
  download="$base_url/download/v${version#v}"
fi

target="$arch-$os"
archive="easement-$target.tar.xz"

if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
  fail "neither sha256sum nor shasum is available"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $archive ($version)"
curl -fsSL -o "$tmp/$archive" "$download/$archive" || fail "cannot download $download/$archive"
curl -fsSL -o "$tmp/sha256.sum" "$download/sha256.sum" || fail "cannot download $download/sha256.sum"

# Lines look like "<hash> *<file>"; the star marks a binary file.
expected="$(awk -v f="$archive" '{ n = $2; sub(/^\*/, "", n); if (n == f) print $1 }' "$tmp/sha256.sum")"
[ -n "$expected" ] || fail "$archive is not listed in sha256.sum"
[ "$expected" = "$(sha256 "$tmp/$archive")" ] || fail "checksum mismatch for $archive"

tar -xJf "$tmp/$archive" -C "$tmp" || fail "cannot unpack $archive (is xz installed?)"
binary="$tmp/easement-$target/easement"
[ -f "$binary" ] || fail "easement-$target/easement is missing from $archive"

mkdir -p "$bindir" 2>/dev/null || true
if [ -w "$bindir" ]; then
  install -m 0755 "$binary" "$bindir/easement"
else
  echo "$bindir is not writable, using sudo"
  sudo install -d "$bindir"
  sudo install -m 0755 "$binary" "$bindir/easement"
fi

echo "Installed easement to $bindir/easement"
case ":$PATH:" in
  *":$bindir:"*) ;;
  *) echo "Add $bindir to your PATH to run it" ;;
esac
