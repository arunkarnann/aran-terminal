#!/usr/bin/env bash
# Build "Aran Terminal.app" from the native crate, sign it, and package .zip + .dmg.
#
#   scripts/bundle-macos.sh              # host architecture
#   UNIVERSAL=1 scripts/bundle-macos.sh  # arm64 + x86_64 (needs both rustup targets)
#
# Signing: set APPLE_SIGNING_IDENTITY (e.g. "Aran Terminal Signing", see
# conductor/SIGNING.md) for a stable signature; otherwise the app is ad-hoc signed.
set -euo pipefail
cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
APP_NAME="Aran Terminal"
OUT=target/bundle
APP="$OUT/$APP_NAME.app"

if [[ "${UNIVERSAL:-0}" == "1" ]]; then
  cargo build --release --target aarch64-apple-darwin
  cargo build --release --target x86_64-apple-darwin
  BIN="$OUT/aran-universal"
  mkdir -p "$OUT"
  lipo -create -output "$BIN" \
    target/aarch64-apple-darwin/release/aran target/x86_64-apple-darwin/release/aran
  ARCH=universal
else
  cargo build --release
  BIN=target/release/aran
  ARCH=$(uname -m)
fi

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN" "$APP/Contents/MacOS/aran"
cp ../conductor/src-tauri/icons/icon.icns "$APP/Contents/Resources/icon.icns"
sed "s/__VERSION__/$VERSION/g" macos/Info.plist > "$APP/Contents/Info.plist"

IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
codesign --force --deep --options runtime --timestamp=none \
  --entitlements macos/Entitlements.plist --sign "$IDENTITY" "$APP"
codesign --verify --verbose=2 "$APP"

BASE="Aran-Terminal_${VERSION}_${ARCH}"
rm -f "$OUT/$BASE.zip" "$OUT/$BASE.dmg"
ditto -c -k --keepParent "$APP" "$OUT/$BASE.zip"

# DMG with an /Applications shortcut for drag-to-install.
STAGE=$(mktemp -d)
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
# hdiutil is occasionally "Resource busy" on CI runners; retry a few times.
for attempt in 1 2 3; do
  if hdiutil create -volname "$APP_NAME" -srcfolder "$STAGE" -ov -format UDZO "$OUT/$BASE.dmg" >/dev/null; then
    break
  fi
  [[ $attempt == 3 ]] && exit 1
  sleep 5
done
rm -rf "$STAGE"

echo "Built:"
ls -lh "$OUT/$BASE.zip" "$OUT/$BASE.dmg"
