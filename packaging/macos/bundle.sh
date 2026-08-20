#!/usr/bin/env bash
# Builds garld.app, and optionally a .dmg, into dist/.
#
#   packaging/macos/bundle.sh              # .app for the host architecture
#   packaging/macos/bundle.sh --universal  # arm64 + x86_64 in one binary
#   packaging/macos/bundle.sh --dmg        # also produce a disk image
#
# The bundle's executable is garld-gui, which is built for the windowing
# subsystem, so double-clicking opens the dashboard. The garld CLI is included
# alongside it inside the bundle.
set -euo pipefail

cd "$(dirname "$0")/../.."

UNIVERSAL=0
MAKE_DMG=0
for arg in "$@"; do
  case "$arg" in
    --universal) UNIVERSAL=1 ;;
    --dmg) MAKE_DMG=1 ;;
    -h|--help) sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $arg" >&2; exit 2 ;;
  esac
done

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "this script builds a macOS bundle and must run on macOS" >&2
  exit 1
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
APP="dist/garld.app"
IDENTIFIER="io.github.kekko7072.garld"

echo "==> building garld $VERSION"
if [[ "$UNIVERSAL" == 1 ]]; then
  for target in aarch64-apple-darwin x86_64-apple-darwin; do
    rustup target add "$target" >/dev/null 2>&1 || true
    cargo build --release --target "$target"
  done
else
  cargo build --release
fi

echo "==> generating icons"
python3 packaging/icon/make_icons.py packaging/icon/out >/dev/null

echo "==> assembling $APP"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

place() { # place <binary-name>
  local name="$1"
  if [[ "$UNIVERSAL" == 1 ]]; then
    lipo -create \
      "target/aarch64-apple-darwin/release/$name" \
      "target/x86_64-apple-darwin/release/$name" \
      -output "$APP/Contents/MacOS/$name"
  else
    cp "target/release/$name" "$APP/Contents/MacOS/$name"
  fi
}
place garld-gui
place garld

cp packaging/icon/out/garld.icns "$APP/Contents/Resources/garld.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>                 <string>garld</string>
    <key>CFBundleDisplayName</key>          <string>garld</string>
    <key>CFBundleIdentifier</key>           <string>$IDENTIFIER</string>
    <key>CFBundleExecutable</key>           <string>garld-gui</string>
    <key>CFBundleIconFile</key>             <string>garld</string>
    <key>CFBundlePackageType</key>          <string>APPL</string>
    <key>CFBundleShortVersionString</key>   <string>$VERSION</string>
    <key>CFBundleVersion</key>              <string>$VERSION</string>
    <key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
    <key>LSMinimumSystemVersion</key>       <string>11.0</string>
    <key>LSApplicationCategoryType</key>    <string>public.app-category.developer-tools</string>
    <key>NSHighResolutionCapable</key>      <true/>
</dict>
</plist>
PLIST

printf 'APPL????' > "$APP/Contents/PkgInfo"

# Ad-hoc signature. Not a Developer ID, so Gatekeeper still asks the user to
# confirm the first launch, but the bundle gets a stable identity and the
# hardened-runtime complaints go away.
if command -v codesign >/dev/null; then
  echo "==> ad-hoc signing"
  codesign --force --deep --sign - "$APP" >/dev/null 2>&1 \
    || echo "    (ad-hoc signing failed; the bundle still runs)"
fi

echo "==> $APP"
if [[ "$UNIVERSAL" == 1 ]]; then
  lipo -info "$APP/Contents/MacOS/garld-gui" | sed 's/^/    /'
fi

if [[ "$MAKE_DMG" == 1 ]]; then
  DMG="dist/garld-$VERSION-macos.dmg"
  echo "==> building $DMG"
  rm -f "$DMG"
  STAGE="$(mktemp -d)"
  cp -R "$APP" "$STAGE/"
  ln -s /Applications "$STAGE/Applications"
  hdiutil create -quiet -volname "garld $VERSION" -srcfolder "$STAGE" \
    -ov -format UDZO "$DMG"
  rm -rf "$STAGE"
  echo "==> $DMG"
fi
