#!/usr/bin/env bash
# Renders the Homebrew formula and cask for a released version.
#
#   packaging/homebrew/sync-tap.sh 0.1.0 [/path/to/homebrew-garld]
#
# Downloads the published source tarball and disk image, computes their
# checksums, and writes the filled-in files. With a tap checkout as the second
# argument it writes them straight into it, ready to commit; otherwise they land
# in dist/homebrew for inspection.
set -euo pipefail

cd "$(dirname "$0")/../.."

VERSION="${1:-}"
TAP="${2:-}"
if [[ -z "$VERSION" ]]; then
  echo "usage: $0 <version> [tap-checkout]" >&2
  exit 2
fi

REPO="kekko7072/garld"
SRC_URL="https://github.com/$REPO/archive/refs/tags/v$VERSION.tar.gz"
DMG_URL="https://github.com/$REPO/releases/download/v$VERSION/garld-$VERSION-macos.dmg"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

checksum() { # checksum <url>
  local url="$1" file="$work/$(basename "$1")"
  if ! curl -fsSL --retry 3 -o "$file" "$url"; then
    echo "could not download $url" >&2
    echo "  (is the v$VERSION release published, with its assets uploaded?)" >&2
    exit 1
  fi
  shasum -a 256 "$file" | cut -d' ' -f1
}

echo "==> checksumming the source tarball"
SRC_SHA="$(checksum "$SRC_URL")"
echo "    $SRC_SHA"

echo "==> checksumming the disk image"
DMG_SHA="$(checksum "$DMG_URL")"
echo "    $DMG_SHA"

if [[ -n "$TAP" ]]; then
  out_formula="$TAP/Formula"
  out_cask="$TAP/Casks"
else
  out_formula="dist/homebrew/Formula"
  out_cask="dist/homebrew/Casks"
fi
mkdir -p "$out_formula" "$out_cask"

sed -e "s|vVERSION|v$VERSION|g" -e "s|SHA256|$SRC_SHA|g" \
  packaging/homebrew/Formula/garld.rb > "$out_formula/garld.rb"
sed -e "s|VERSION|$VERSION|g" -e "s|SHA256_DMG|$DMG_SHA|g" \
  packaging/homebrew/Casks/garld.rb > "$out_cask/garld.rb"

echo "==> wrote $out_formula/garld.rb"
echo "==> wrote $out_cask/garld.rb"

if [[ -n "$TAP" ]]; then
  echo
  echo "Next, in $TAP:"
  echo "    git add Formula/garld.rb Casks/garld.rb"
  echo "    git commit -m 'garld $VERSION'"
  echo "    git push"
fi
