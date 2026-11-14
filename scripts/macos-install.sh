#!/usr/bin/env bash
# Build PTZ Director for Apple Silicon, ad-hoc sign the app, install to ~/Applications,
# and package the server binary + launch script.
#
# Run this ON an aarch64 Mac (native build). Cross-compile from Linux is not supported here.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

ARCH="$(uname -m)"
if [[ "$ARCH" != "arm64" ]]; then
  echo "warning: expected Apple Silicon (arm64); got $ARCH" >&2
fi

TARGET="aarch64-apple-darwin"
rustup target add "$TARGET" >/dev/null

echo "==> Building release binaries ($TARGET)"
cargo build --workspace --release --target "$TARGET"

BIN_DIR="$ROOT/target/$TARGET/release"
CLIENT="$BIN_DIR/ptz-director"
SERVER="$BIN_DIR/ptz-server"
test -x "$CLIENT"
test -x "$SERVER"

DIST="$ROOT/dist/macos"
APP="$DIST/PTZ Director.app"
APP_MACOS="$APP/Contents/MacOS"
APP_RES="$APP/Contents/Resources"
mkdir -p "$APP_MACOS" "$APP_RES" "$DIST/server"

echo "==> Assembling PTZ Director.app"
cat > "$APP/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>PTZ Director</string>
  <key>CFBundleDisplayName</key><string>PTZ Director</string>
  <key>CFBundleIdentifier</key><string>com.flybar.ptz-director</string>
  <key>CFBundleVersion</key><string>0.2.0</string>
  <key>CFBundleShortVersionString</key><string>0.2.0</string>
  <key>CFBundleExecutable</key><string>ptz-director</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST

cp "$CLIENT" "$APP_MACOS/ptz-director"
chmod +x "$APP_MACOS/ptz-director"

echo "==> Ad-hoc signing app"
codesign --force --deep --sign - "$APP"

INSTALL_DIR="$HOME/Applications"
mkdir -p "$INSTALL_DIR"
DEST="$INSTALL_DIR/PTZ Director.app"
if [[ -d "$DEST" ]]; then
  BACKUP="$INSTALL_DIR/PTZ Director.app.backup-$(date +%Y%m%d-%H%M%S)"
  echo "==> Backing up existing install to $BACKUP"
  mv "$DEST" "$BACKUP"
fi
echo "==> Installing to $DEST"
rm -rf "$DEST"
cp -R "$APP" "$DEST"

echo "==> Packaging server"
cp "$SERVER" "$DIST/server/ptz-server"
chmod +x "$DIST/server/ptz-server"
cat > "$DIST/server/start-ptz-server.sh" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"
DATA="${PTZ_DATA_DIR:-$HOME/Library/Application Support/ptz-director}"
mkdir -p "$DATA"
exec "$DIR/ptz-server" --listen 127.0.0.1:8787 --data-dir "$DATA"
SH
chmod +x "$DIST/server/start-ptz-server.sh"

# Also place a convenience copy next to the app install
SERVER_INSTALL="$INSTALL_DIR/PTZ Director Server"
mkdir -p "$SERVER_INSTALL"
cp "$DIST/server/ptz-server" "$SERVER_INSTALL/"
cp "$DIST/server/start-ptz-server.sh" "$SERVER_INSTALL/"

echo
echo "Installed:"
echo "  App:    $DEST"
echo "  Server: $SERVER_INSTALL/start-ptz-server.sh"
echo
echo "Launch server first, then open PTZ Director."
echo "Default backend URL: http://127.0.0.1:8787"
EOF