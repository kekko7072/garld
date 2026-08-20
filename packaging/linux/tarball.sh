#!/usr/bin/env bash
# Builds a self-contained Linux tarball into dist/.
#
#   packaging/linux/tarball.sh
#
# Contains both binaries, the desktop entry, icons, and an install script that
# copies them into ~/.local (no root needed) or /usr/local with --system.
set -euo pipefail

cd "$(dirname "$0")/../.."

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
ARCH="$(uname -m)"
NAME="garld-$VERSION-linux-$ARCH"
STAGE="dist/$NAME"

echo "==> building garld $VERSION"
cargo build --release

echo "==> staging $STAGE"
rm -rf "$STAGE"
mkdir -p "$STAGE/bin" "$STAGE/share/applications" "$STAGE/share/icons"
cp target/release/garld target/release/garld-gui "$STAGE/bin/"
cp packaging/linux/garld.desktop "$STAGE/share/applications/"
cp packaging/icon/garld-*.png "$STAGE/share/icons/"
cp README.md LICENSE "$STAGE/"

cat > "$STAGE/install.sh" <<'INNER'
#!/usr/bin/env sh
# Installs garld for the current user, or system-wide with --system.
set -eu
cd "$(dirname "$0")"

PREFIX="$HOME/.local"
[ "${1:-}" = "--system" ] && PREFIX="/usr/local"

mkdir -p "$PREFIX/bin" "$PREFIX/share/applications" \
         "$PREFIX/share/icons/hicolor/256x256/apps" \
         "$PREFIX/share/icons/hicolor/128x128/apps"

install -m 755 bin/garld bin/garld-gui "$PREFIX/bin/"
install -m 644 share/applications/garld.desktop "$PREFIX/share/applications/"
install -m 644 share/icons/garld-256.png "$PREFIX/share/icons/hicolor/256x256/apps/garld.png"
install -m 644 share/icons/garld-128.png "$PREFIX/share/icons/hicolor/128x128/apps/garld.png"

command -v update-desktop-database >/dev/null 2>&1 \
  && update-desktop-database "$PREFIX/share/applications" 2>/dev/null || true

echo "installed to $PREFIX"
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "note: $PREFIX/bin is not on your PATH" ;;
esac
INNER
chmod +x "$STAGE/install.sh"

echo "==> packing"
tar -czf "dist/$NAME.tar.gz" -C dist "$NAME"
rm -rf "$STAGE"
echo "==> dist/$NAME.tar.gz"
