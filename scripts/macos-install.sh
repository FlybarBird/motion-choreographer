#!/usr/bin/env bash
# Build Motion Choreographer for this Mac (Apple Silicon or Intel) and install it for this user.
#
#   scripts/macos-install.sh               build and install, or install over an older version
#   scripts/macos-install.sh --uninstall   remove the app and the tool links; the show data stays
#
# The app was called PTZ Programmer before. An install under that name is the same app (same
# bundle id, data folder and tools): installing removes it, and --uninstall removes either name.
#
# Installs:
#   ~/Applications/Motion Choreographer.app    the desktop app
#   ~/.local/bin/ptz-server, ptz-probe, ptz-bringup, ptz-panel   links into the bundle
#
# Environment (all optional):
#   PTZ_TARGET       Rust target triple       (default: this Mac's, aarch64- or x86_64-apple-darwin)
#   PTZ_APP_DEST     where the .app goes      (default ~/Applications)
#   PTZ_BIN_DEST     where the tool links go  (default ~/.local/bin)
#   PTZ_SKIP_BUILD   1 = use binaries already in target/<triple>/release
#   PTZ_SKIP_CHECKS  1 = do not check for Xcode tools, the macOS platform, the Rust version
#                        or a server still running from the installed app
#   PTZ_SKIP_SIGN    1 = do not run codesign
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
case "$(uname -m)" in
  x86_64) HOST_TARGET="x86_64-apple-darwin" ;;
  *) HOST_TARGET="aarch64-apple-darwin" ;;
esac
TARGET="${PTZ_TARGET:-$HOST_TARGET}"
APP_DEST="${PTZ_APP_DEST:-$HOME/Applications}"
BIN_DEST="${PTZ_BIN_DEST:-$HOME/.local/bin}"
APP_NAME="Motion Choreographer"
OLD_APP_NAME="PTZ Programmer"
BUNDLE_ID="com.ptzprogrammer.app"
TOOLS=(ptz-server ptz-probe ptz-bringup ptz-panel)
ICON="$ROOT/brand/icon/macos/AppIcon.icns"

say() { printf '==> %s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

INSTALLED="$APP_DEST/$APP_NAME.app"
OLD_INSTALLED="$APP_DEST/$OLD_APP_NAME.app"

# A server started from the installed app keeps running from the old files after they are replaced.
running_from_install() {
  pgrep -fl 'ptz-(server|app)' 2>/dev/null | grep -F -e "$INSTALLED/" -e "$OLD_INSTALLED/" || true
}

if [ "${1:-}" = "--uninstall" ]; then
  if [ "${PTZ_SKIP_CHECKS:-0}" != "1" ] && [ -n "$(running_from_install)" ]; then
    fail "$APP_NAME is still running from $APP_DEST. Quit the app and stop ptz-server, then run this again"
  fi
  if [ -d "$OLD_INSTALLED" ]; then
    rm -rf "${OLD_INSTALLED:?}"
    say "Removed $OLD_INSTALLED, the app under its old name"
  fi
  rm -rf "${INSTALLED:?}"
  for bin in "${TOOLS[@]}"; do
    if [ -L "$BIN_DEST/$bin" ]; then rm -f "$BIN_DEST/$bin"; fi
  done
  say "Removed $INSTALLED and the links in $BIN_DEST"
  say "Kept: the show data (~/PTZProgrammer unless you chose another folder) and the app's settings"
  exit 0
fi
[ $# -eq 0 ] || fail "unknown option $1. Use --uninstall, or no option to install"

# 1. Tools.
if [ "${PTZ_SKIP_CHECKS:-0}" != "1" ]; then
  [ "$(uname -s)" = "Darwin" ] || fail "this script installs on macOS. On Linux use: make run-server and make run-app"
  if ! xcode-select -p >/dev/null 2>&1; then
    fail "the Xcode command line tools are missing. Install them with: xcode-select --install"
  fi
fi
if ! command -v cargo >/dev/null 2>&1; then
  fail "Rust is missing. Install it with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
fi

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
[ -n "$VERSION" ] || fail "could not read the version from Cargo.toml"

if [ "${PTZ_SKIP_CHECKS:-0}" != "1" ]; then
  NEED="$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
  HAVE="$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9]*\.[0-9]*\).*/\1/p')"
  if [ -n "$NEED" ] && [ -n "$HAVE" ]; then
    if [ "$(printf '%s\n%s\n' "$NEED" "$HAVE" | sort -t. -k1,1n -k2,2n | head -n 1)" != "$NEED" ]; then
      fail "Rust $HAVE is too old; this needs $NEED or newer. Update it with: rustup update stable"
    fi
  fi
  if [ -n "$(running_from_install)" ]; then
    fail "$APP_NAME is still running from $APP_DEST. Quit the app and stop ptz-server (the show data is kept), then run this again"
  fi
fi

# 2. Build.
RELEASE_DIR="$ROOT/target/$TARGET/release"
if [ "${PTZ_SKIP_BUILD:-0}" != "1" ]; then
  if command -v rustup >/dev/null 2>&1; then
    say "Making sure the $TARGET target is installed"
    rustup target add "$TARGET"
  fi
  say "Building version $VERSION for $TARGET (the first build takes several minutes)"
  (cd "$ROOT" && cargo build --release --locked --target "$TARGET")
fi
for bin in ptz-app "${TOOLS[@]}"; do
  [ -x "$RELEASE_DIR/$bin" ] || fail "missing $RELEASE_DIR/$bin. The build did not produce it"
done
[ -f "$ICON" ] || fail "missing $ICON, the app icon from the brand kit"

# 3. Assemble the bundle.
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
BUNDLE="$STAGE/$APP_NAME.app"
mkdir -p "$BUNDLE/Contents/MacOS" "$BUNDLE/Contents/Resources"
cp "$RELEASE_DIR/ptz-app" "$BUNDLE/Contents/MacOS/ptz-app"
cp "$ICON" "$BUNDLE/Contents/Resources/AppIcon.icns"
for bin in "${TOOLS[@]}"; do
  cp "$RELEASE_DIR/$bin" "$BUNDLE/Contents/MacOS/$bin"
done

# The licences of what is built into the app travel with it.
mkdir -p "$BUNDLE/Contents/Resources/licenses"
cp "$ROOT/LICENSE" "$ROOT/THIRD-PARTY-NOTICES.md" "$BUNDLE/Contents/Resources/licenses/"
cp "$ROOT"/crates/app/assets/fonts/*.txt "$BUNDLE/Contents/Resources/licenses/"

cat >"$BUNDLE/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$BUNDLE_ID</string>
  <key>CFBundleExecutable</key>
  <string>ptz-app</string>
  <key>CFBundleIconFile</key>
  <string>AppIcon</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$VERSION</string>
  <key>CFBundleVersion</key>
  <string>$VERSION</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSMicrophoneUsageDescription</key>
  <string>$APP_NAME reads LTC timecode from an audio input.</string>
  <key>NSLocalNetworkUsageDescription</key>
  <string>$APP_NAME talks to the show server, the camera and the pedestal on the local network.</string>
</dict>
</plist>
PLIST

# 4. Sign. Ad-hoc is enough to run on the Mac that built it.
if [ "${PTZ_SKIP_SIGN:-0}" != "1" ]; then
  say "Signing (ad hoc)"
  codesign --force --deep --sign - "$BUNDLE"
fi

# 5. Install.
mkdir -p "$APP_DEST" "$BIN_DEST"
rm -rf "${INSTALLED:?}"
if [ -d "$OLD_INSTALLED" ]; then
  rm -rf "${OLD_INSTALLED:?}"
  say "Removed $OLD_INSTALLED, the app under its old name"
fi
cp -R "$BUNDLE" "$APP_DEST/"
for bin in "${TOOLS[@]}"; do
  ln -sf "$INSTALLED/Contents/MacOS/$bin" "$BIN_DEST/$bin"
done

# 6. What to do next.
say "Installed $INSTALLED"
say "Linked ${TOOLS[*]} into $BIN_DEST"
# shellcheck disable=SC2016  # $PATH is meant to reach the user's shell file unexpanded
case ":$PATH:" in
  *":$BIN_DEST:"*) ;;
  *) printf '\n%s is not on your PATH. Add it with:\n  echo '\''export PATH="%s:$PATH"'\'' >> ~/.zshrc && exec zsh\n' "$BIN_DEST" "$BIN_DEST" ;;
esac
cat <<NEXT

Next:
  ptz-server                 start the show server (data in ~/PTZProgrammer, simulator rig)
  ptz-probe                  check every transport action against that server
  ptz-panel --panel ADDRESS  run the show from a SKAARHOJ panel at that address
  open "$INSTALLED"
                             or use "Start a server on this computer" in the app's connect screen
NEXT
