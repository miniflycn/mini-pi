#!/usr/bin/env bash
set -euo pipefail

VERSION="${1:-0.1.0}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

# cargo-bundle and create-dmg write large staging images under $TMPDIR;
# /var/folders can be space-constrained on the sealed system volume, so force
# them to use a project-local temp directory.
export TMPDIR="$ROOT/target/build-tmp"
mkdir -p "$TMPDIR"

PACKAGE="$ROOT/target/package/mini-pi"
TOOLS="$ROOT/tools"
BUNDLE_DIR="$ROOT/target/release/bundle/osx"
APP_NAME="Mini Pi"
APP_BUNDLE="$BUNDLE_DIR/$APP_NAME.app"

mkdir -p "$TOOLS"

if ! command -v cargo-bundle &>/dev/null; then
  echo "Installing cargo-bundle..."
  cargo install cargo-bundle
fi

if ! command -v create-dmg &>/dev/null; then
  echo "Installing create-dmg..."
  if command -v brew &>/dev/null; then
    brew install create-dmg
  else
    echo "create-dmg is required for a styled DMG. Install it from https://github.com/create-dmg/create-dmg" >&2
    exit 1
  fi
fi

echo "Building release app bundle..."
# Only build the .app bundle here. cargo-bundle defaults to building both
# osx and dmg on macOS, which (1) bundles the app twice and (2) often fails
# its internal DMG step with a too-small staging image. We build the styled
# DMG ourselves below with create-dmg.
cargo bundle --release --format osx

ARCH="$(uname -m)"
DMG_OUT="$ROOT/target/mini-pi-$VERSION-$ARCH.dmg"

BUN_VERSION="1.2.22"
if [ "$ARCH" = "arm64" ]; then
  BUN_ZIP="bun-darwin-aarch64.zip"
else
  BUN_ZIP="bun-darwin-x64.zip"
fi
BUN_URL="https://github.com/oven-sh/bun/releases/download/bun-v${BUN_VERSION}/${BUN_ZIP}"
BUN_DIR="$TOOLS/bun-${BUN_VERSION}-${ARCH}"

if [ ! -f "$TOOLS/$BUN_ZIP" ]; then
  echo "Downloading Bun v${BUN_VERSION} for ${ARCH}..."
  curl -L -o "$TOOLS/$BUN_ZIP" "$BUN_URL"
fi
if [ ! -d "$BUN_DIR" ]; then
  mkdir -p "$BUN_DIR"
  unzip -o "$TOOLS/$BUN_ZIP" -d "$BUN_DIR"
fi
BUN_BIN="$BUN_DIR/${BUN_ZIP%.zip}/bun"

MINI_PI_DIR="$HOME/.mini-pi"
BUN_CACHE_DIR="$MINI_PI_DIR/bun-cache"
mkdir -p "$BUN_CACHE_DIR/install-cache"

echo "Bundling pi-bridge with --target bun..."
(cd "$PACKAGE/pi-bridge" && BUN_INSTALL="$BUN_CACHE_DIR" BUN_INSTALL_CACHE_DIR="$BUN_CACHE_DIR/install-cache" "$BUN_BIN" build --target bun src/index.ts --outfile pi-bridge.js)

echo "Injecting Bun runtime and pi-bridge bundle into app bundle..."
rm -rf "$APP_BUNDLE/Contents/Resources/pi-bridge"
rm -f "$APP_BUNDLE/Contents/Resources/pi-bridge.js"
cp "$PACKAGE/pi-bridge/pi-bridge.js" "$APP_BUNDLE/Contents/Resources/pi-bridge.js"
cp "$BUN_BIN" "$APP_BUNDLE/Contents/Resources/bun"
chmod +x "$APP_BUNDLE/Contents/Resources/bun"

DMG_TMP="$ROOT/target/mini-pi-dmg"
rm -rf "$DMG_TMP"
mkdir -p "$DMG_TMP"
cp -R "$APP_BUNDLE" "$DMG_TMP/"

# Remove any existing DMG so hdiutil/create-dmg don't complain.
rm -f "$DMG_OUT"

if command -v create-dmg &>/dev/null; then
  echo "Creating DMG with create-dmg..."
  create-dmg \
    --volname "$APP_NAME" \
    --background "$ROOT/scripts/installer/dmg-background.png" \
    --window-size 560 400 \
    --icon-size 100 \
    --icon "$APP_NAME.app" 140 200 \
    --app-drop-link 420 200 \
    "$DMG_OUT" \
    "$DMG_TMP"
else
  echo "create-dmg not found; falling back to hdiutil..."
  ln -sf /Applications "$DMG_TMP/Applications"
  hdiutil create -srcfolder "$DMG_TMP" -volname "$APP_NAME" -fs HFS+ -format UDZO "$DMG_OUT"
fi

echo "Installer created: $DMG_OUT"
