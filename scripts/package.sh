#!/usr/bin/env bash
# Build Motion Choreographer for this machine and write two installers into dist/:
# one for the desktop app, which offers the show server as an option, and one
# for the show server alone.
#
#   scripts/package.sh
#
# On a Mac (for the kind of Mac it is run on; an Apple Silicon Mac also builds the Intel
# installers, with PTZ_TARGET=x86_64-apple-darwin):
#   dist/Motion-Choreographer-<version>-macos-<arch>.pkg          the app. "Show server and tools" is a
#                                                                 checkbox in the installer, ticked to start with
#   dist/Motion-Choreographer-Server-<version>-macos-<arch>.pkg   the show server and tools only
#   They install /Applications/Motion Choreographer.app and /usr/local/ptz-programmer.
#   The app installer removes an older /Applications/PTZ Programmer.app, the same app
#   under its old name.
#
# On Linux:
#   dist/motion-choreographer-<version>-linux-<arch>.run          the app. Asks whether to install the server too
#   dist/motion-choreographer-server-<version>-linux-<arch>.run   the show server and tools only
#   They install under ~/.local, or under --prefix. Run one with --help for its options.
#
# Environment (all optional):
#   PTZ_TARGET          Rust target triple (default: this machine's)
#   PTZ_DIST            where the installers are written (default dist)
#   PTZ_SKIP_BUILD      1 = use binaries already in target/<triple>/release
#   PTZ_APP_SIGN        codesign identity for the Mac app and tools (default "-", ad hoc)
#   PTZ_INSTALLER_SIGN  "Developer ID Installer: ..." identity for the .pkg files (default: unsigned)
#   PTZ_SKIP_SIGN       1 = do not run codesign
#   PTZ_PACKAGE_OS      macos or linux, to override what this machine is (for the tests)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_NAME="Motion Choreographer"
# The app was called this before; its bundle id, data folder and tool names are unchanged.
OLD_APP_NAME="PTZ Programmer"
APP_ID="com.ptzprogrammer.app"
SERVER_ID="com.ptzprogrammer.server"
MAC_SERVER_HOME="/usr/local/ptz-programmer"
TOOLS=(ptz-server ptz-probe ptz-bringup ptz-panel)
MAC_ICON="$ROOT/brand/icon/macos/AppIcon.icns"
LINUX_ICON="$ROOT/brand/icon/png/app-icon-256.png"

say() { printf '==> %s\n' "$*"; }
fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

case "${PTZ_PACKAGE_OS:-$(uname -s)}" in
  Darwin | macos) OS=macos ;;
  Linux | linux) OS=linux ;;
  *) fail "installers are made for macOS and Linux, not $(uname -s)" ;;
esac
MACHINE="$(uname -m)"
case "$OS-$MACHINE" in
  macos-x86_64) HOST_TARGET="x86_64-apple-darwin" ;;
  macos-*) HOST_TARGET="aarch64-apple-darwin" ;;
  linux-aarch64 | linux-arm64) HOST_TARGET="aarch64-unknown-linux-gnu" ;;
  *) HOST_TARGET="x86_64-unknown-linux-gnu" ;;
esac
TARGET="${PTZ_TARGET:-$HOST_TARGET}"
case "$TARGET" in
  aarch64-*) ARCH=arm64 ;;
  x86_64-*) ARCH=x86_64 ;;
  *) ARCH="$MACHINE" ;;
esac
DIST="${PTZ_DIST:-$ROOT/dist}"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
[ -n "$VERSION" ] || fail "could not read the version from Cargo.toml"

# 1. Build.
RELEASE_DIR="$ROOT/target/$TARGET/release"
if [ "${PTZ_SKIP_BUILD:-0}" != "1" ]; then
  if [ "$OS" = macos ] && ! xcode-select -p >/dev/null 2>&1; then
    fail "the Xcode command line tools are missing. Install them with: xcode-select --install"
  fi
  command -v cargo >/dev/null 2>&1 ||
    fail "Rust is missing. Install it with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  NEED="$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -n 1)"
  HAVE="$(rustc --version 2>/dev/null | sed -n 's/^rustc \([0-9]*\.[0-9]*\).*/\1/p')"
  if [ -n "$NEED" ] && [ -n "$HAVE" ] &&
    [ "$(printf '%s\n%s\n' "$NEED" "$HAVE" | sort -t. -k1,1n -k2,2n | head -n 1)" != "$NEED" ]; then
    fail "Rust $HAVE is too old; this needs $NEED or newer. Update it with: rustup update stable"
  fi
  if command -v rustup >/dev/null 2>&1; then
    rustup target add "$TARGET"
  fi
  say "Building version $VERSION for $TARGET (the first build takes several minutes)"
  (cd "$ROOT" && cargo build --release --locked --target "$TARGET")
fi
for bin in ptz-app "${TOOLS[@]}"; do
  [ -x "$RELEASE_DIR/$bin" ] || fail "missing $RELEASE_DIR/$bin. The build did not produce it"
done
for icon in "$MAC_ICON" "$LINUX_ICON"; do
  [ -f "$icon" ] || fail "missing $icon, the app icon from the brand kit"
done

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$DIST"

licenses_into() {
  mkdir -p "$1"
  cp "$ROOT/LICENSE" "$ROOT/THIRD-PARTY-NOTICES.md" "$1/"
  cp "$ROOT"/crates/app/assets/fonts/*.txt "$1/"
}

# ---------------------------------------------------------------- macOS

sign() {
  if [ "${PTZ_SKIP_SIGN:-0}" != "1" ]; then
    codesign --force --sign "${PTZ_APP_SIGN:--}" "$@"
  fi
}

# distribution <file> <title> <choice>...   where a choice is id|package id|title|description|required
distribution() {
  local out="$1" title="$2" hosts
  shift 2
  # An Intel build also runs on Apple Silicon; an Apple Silicon build runs nowhere else.
  if [ "$ARCH" = arm64 ]; then hosts="arm64"; else hosts="x86_64,arm64"; fi
  {
    printf '<?xml version="1.0" encoding="utf-8"?>\n<installer-gui-script minSpecVersion="2">\n'
    printf '  <title>%s</title>\n' "$title"
    printf '  <options customize="always" require-scripts="false" hostArchitectures="%s"/>\n' "$hosts"
    printf '  <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>\n'
    printf '  <volume-check><allowed-os-versions><os-version min="12.0"/></allowed-os-versions></volume-check>\n'
    printf '  <choices-outline>\n'
    local choice
    for choice in "$@"; do printf '    <line choice="%s"/>\n' "${choice%%|*}"; done
    printf '  </choices-outline>\n'
    local id pkg name text required
    for choice in "$@"; do
      IFS='|' read -r id pkg name text required <<<"$choice"
      # "selected" is an expression the Installer keeps applying, so it is not put on a choice the
      # user may clear: that one only starts ticked.
      if [ "$required" = required ]; then
        printf '  <choice id="%s" title="%s" description="%s" start_selected="true" enabled="false">\n' "$id" "$name" "$text"
      else
        printf '  <choice id="%s" title="%s" description="%s" start_selected="true">\n' "$id" "$name" "$text"
      fi
      printf '    <pkg-ref id="%s"/>\n  </choice>\n' "$pkg"
      printf '  <pkg-ref id="%s" version="%s">%s.pkg</pkg-ref>\n' "$pkg" "$VERSION" "$id"
    done
    printf '</installer-gui-script>\n'
  } >"$out"
}

# product <out> <distribution> <part>...   Build the installer, then open it again and check its parts are in it.
product() {
  local out="$1" dist="$2" part check
  shift 2
  if [ -n "${PTZ_INSTALLER_SIGN:-}" ]; then
    productbuild --distribution "$dist" --package-path "$STAGE/pkgs" --sign "$PTZ_INSTALLER_SIGN" "$out"
  else
    productbuild --distribution "$dist" --package-path "$STAGE/pkgs" "$out"
  fi
  [ -s "$out" ] || fail "productbuild wrote nothing to $out"
  check="$STAGE/check-$(basename "$out" .pkg)"
  pkgutil --expand "$out" "$check" || fail "$out cannot be opened again. It is not a usable installer"
  [ -f "$check/Distribution" ] || fail "$out holds no list of what to install"
  for part in "$@"; do
    [ -e "$check/$part.pkg" ] || fail "$out is missing its $part part"
  done
}

package_macos() {
  command -v pkgbuild >/dev/null 2>&1 || fail "pkgbuild is missing. It comes with macOS: run this on a Mac"
  command -v productbuild >/dev/null 2>&1 || fail "productbuild is missing. It comes with macOS: run this on a Mac"
  command -v pkgutil >/dev/null 2>&1 || fail "pkgutil is missing. It comes with macOS: run this on a Mac"
  mkdir -p "$STAGE/pkgs"

  # The app, without the server: the server is its own choice.
  local bundle="$STAGE/app-root/$APP_NAME.app"
  mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
  cp "$RELEASE_DIR/ptz-app" "$bundle/Contents/MacOS/ptz-app"
  cp "$MAC_ICON" "$bundle/Contents/Resources/AppIcon.icns"
  licenses_into "$bundle/Contents/Resources/licenses"
  cat >"$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$APP_ID</string>
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
  sign "$bundle"

  # Without this the Installer puts the app wherever it finds an older copy, not in /Applications,
  # and refuses to install over a copy with the same or a newer version number.
  cat >"$STAGE/app-component.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<array>
  <dict>
    <key>BundleHasStrictIdentifier</key>
    <true/>
    <key>BundleIsRelocatable</key>
    <false/>
    <key>BundleIsVersionChecked</key>
    <false/>
    <key>BundleOverwriteAction</key>
    <string>upgrade</string>
    <key>RootRelativeBundlePath</key>
    <string>$APP_NAME.app</string>
  </dict>
</array>
</plist>
PLIST
  # The app under its old name is the same app: it goes when this one is installed.
  mkdir -p "$STAGE/app-scripts"
  cat >"$STAGE/app-scripts/postinstall" <<SCRIPT
#!/bin/sh
old="/Applications/$OLD_APP_NAME.app"
if [ -d "\$old" ] && [ ! -L "\$old" ] && [ "\$(defaults read "\$old/Contents/Info" CFBundleIdentifier 2>/dev/null)" = "$APP_ID" ]; then
  rm -rf "\$old"
  # A copy that updated itself under the old name reopens itself from there: the name leads to the new app.
  ln -s "/Applications/$APP_NAME.app" "\$old"
fi
exit 0
SCRIPT
  chmod +x "$STAGE/app-scripts/postinstall"
  pkgbuild --root "$STAGE/app-root" --component-plist "$STAGE/app-component.plist" --scripts "$STAGE/app-scripts" \
    --identifier "$APP_ID" --version "$VERSION" --install-location /Applications "$STAGE/pkgs/app.pkg"

  # The server and tools in a folder of their own, put on PATH the way macOS provides for.
  local home="$STAGE/server-root"
  mkdir -p "$home/bin" "$STAGE/server-scripts"
  for bin in "${TOOLS[@]}"; do
    cp "$RELEASE_DIR/$bin" "$home/bin/$bin"
    sign "$home/bin/$bin"
  done
  licenses_into "$home/licenses"
  cat >"$STAGE/server-scripts/postinstall" <<SCRIPT
#!/bin/sh
mkdir -p /etc/paths.d
echo "$MAC_SERVER_HOME/bin" > /etc/paths.d/ptz-programmer
exit 0
SCRIPT
  chmod +x "$STAGE/server-scripts/postinstall"
  pkgbuild --root "$home" --scripts "$STAGE/server-scripts" \
    --identifier "$SERVER_ID" --version "$VERSION" --install-location "$MAC_SERVER_HOME" "$STAGE/pkgs/server.pkg"

  local app_choice="app|$APP_ID|$APP_NAME|The desktop app: timeline, monitors and transport.|required"
  local server_text="ptz-server, ptz-probe, ptz-bringup and ptz-panel (for a SKAARHOJ control panel), in $MAC_SERVER_HOME. Needed on the computer that runs the show. Clear it on a station that only connects to a server elsewhere."
  local server_choice="server|$SERVER_ID|Show server and tools|$server_text|optional"

  local app_out="$DIST/Motion-Choreographer-$VERSION-macos-$ARCH.pkg"
  local server_out="$DIST/Motion-Choreographer-Server-$VERSION-macos-$ARCH.pkg"
  distribution "$STAGE/app-distribution.xml" "$APP_NAME $VERSION" "$app_choice" "$server_choice"
  product "$app_out" "$STAGE/app-distribution.xml" app server
  distribution "$STAGE/server-distribution.xml" "$APP_NAME show server $VERSION" \
    "server|$SERVER_ID|Show server and tools|$server_text|required"
  product "$server_out" "$STAGE/server-distribution.xml" server

  say "Wrote $app_out"
  say "Wrote $server_out"
  if [ -z "${PTZ_INSTALLER_SIGN:-}" ]; then
    cat <<NOTE

These installers are not signed with a Developer ID. On another Mac, open one
with a right-click and Open, or run:
  sudo installer -pkg "$app_out" -target /
NOTE
  fi
}

# ---------------------------------------------------------------- Linux

# run_file <out> <kind> <payload folder>   A shell installer with its files appended as a tar.gz.
run_file() {
  local out="$1" kind="$2" payload="$3"
  sed -e "s/@KIND@/$kind/" -e "s/@VERSION@/$VERSION/" -e "s/@ARCH@/$ARCH/" "$ROOT/scripts/linux-installer.sh" >"$out"
  tar -C "$payload" -czf - . >>"$out"
  chmod +x "$out"
  say "Wrote $out"
}

package_linux() {
  local app="$STAGE/app-payload" server="$STAGE/server-payload"
  mkdir -p "$app/app/bin" "$app/app/share" "$app/server/bin" "$server/server/bin"
  cp "$RELEASE_DIR/ptz-app" "$app/app/bin/"
  cp "$LINUX_ICON" "$app/app/share/app-icon-256.png"
  for bin in "${TOOLS[@]}"; do
    cp "$RELEASE_DIR/$bin" "$app/server/bin/"
    cp "$RELEASE_DIR/$bin" "$server/server/bin/"
  done
  licenses_into "$app/licenses"
  licenses_into "$server/licenses"
  run_file "$DIST/motion-choreographer-$VERSION-linux-$ARCH.run" app "$app"
  run_file "$DIST/motion-choreographer-server-$VERSION-linux-$ARCH.run" server "$server"
}

"package_$OS"
